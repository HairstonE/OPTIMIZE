//! [yours — P0, src/common/common.md Step 4–5] Lowering: IR → DataFusion LogicalPlan.
//!
//! ── CONTRACT ──────────────────────────────────────────────────────────────
//! Mechanical translation via `datafusion::logical_expr::LogicalPlanBuilder`.
//! No decisions — any cleverness found here during review gets evicted into
//! an optimizer.
//!
//! The one design point: every scan is registered under its rel's QUALIFIER
//! (alias-or-table-name), so IR ColRefs translate to DataFusion `Column`s
//! by (qualifier, column-name) — pure name lookup through Ir::rels, zero
//! index arithmetic at this boundary. Self-joins (q18: customer AS c1/c2)
//! resolve unambiguously because qualifiers are unique per query.
//! From P4, lowering emits physical plans instead; this logical path stays
//! for P0–P3.
//! ──────────────────────────────────────────────────────────────────────────

use crate::catalog;
use crate::ir::{ArithOp, CmpOp, ColRef, Expr, Ir, Plan};
use anyhow::{Context, Result};
use datafusion::common::{Column, DFSchema, TableReference};
use datafusion::datasource::provider_as_source;
use datafusion::logical_expr::{
    EmptyRelation, Expr as DfExpr, JoinType, LogicalPlan, LogicalPlanBuilder, SortExpr,
    TableSource,
};
use datafusion::prelude::{lit, SessionContext};
use std::sync::Arc;

pub async fn lower(ir: &Ir, ctx: &SessionContext) -> Result<LogicalPlan> {
    let sources = table_sources(ir, ctx).await?;
    Ok(lower_node(&ir.root, ir, &sources)?.build()?)
}

/// Fetch table sources up front (the only async part), so the tree walk
/// is plain synchronous recursion. pub(crate): p4's lowering v2 reuses
/// this plus lower_node/df_expr/df_col for everything but joins.
pub(crate) async fn table_sources(
    ir: &Ir,
    ctx: &SessionContext,
) -> Result<Vec<Arc<dyn TableSource>>> {
    let mut sources: Vec<Arc<dyn TableSource>> = Vec::with_capacity(ir.rels.len());
    for rel in &ir.rels {
        let provider = ctx
            .table_provider(rel.table.as_str())
            .await
            .with_context(|| format!("table '{}' not registered", rel.table))?;
        sources.push(provider_as_source(provider));
    }
    Ok(sources)
}

pub(crate) fn lower_node(
    plan: &Plan,
    ir: &Ir,
    sources: &[Arc<dyn TableSource>],
) -> Result<LogicalPlanBuilder> {
    Ok(match plan {
        Plan::Scan { rel } => scan_node(*rel, ir, sources)?,
        Plan::EmptyScan { rels } => {
            // Zero rows over the same qualified schema the rels' real scans
            // would expose — built from actual scan nodes so it can never
            // drift from the Scan arm's.
            let mut schema: Option<DFSchema> = None;
            for rel in rels {
                let s = scan_node(*rel, ir, sources)?.build()?.schema().as_ref().clone();
                schema = Some(match schema {
                    None => s,
                    Some(acc) => acc.join(&s)?,
                });
            }
            let schema = schema.context("EmptyScan with empty rel set")?;
            LogicalPlanBuilder::from(LogicalPlan::EmptyRelation(EmptyRelation {
                produce_one_row: false,
                schema: Arc::new(schema),
            }))
        }
        Plan::Filter { input, predicate } => {
            lower_node(input, ir, sources)?.filter(df_expr(predicate, ir))?
        }
        Plan::Project { input, exprs } => {
            let exprs: Vec<DfExpr> = exprs.iter().map(|e| df_expr(e, ir)).collect();
            lower_node(input, ir, sources)?.project(exprs)?
        }
        Plan::Join {
            left,
            right,
            on,
            filter,
        } => {
            let l = lower_node(left, ir, sources)?;
            let r = lower_node(right, ir, sources)?.build()?;
            let mut preds: Vec<DfExpr> = on
                .iter()
                .map(|(a, b)| df_col(*a, ir).eq(df_col(*b, ir)))
                .collect();
            if let Some(f) = filter {
                preds.push(df_expr(f, ir));
            }
            if preds.is_empty() {
                l.cross_join(r)?
            } else {
                l.join_on(r, JoinType::Inner, preds)?
            }
        }
        Plan::Sort { input, keys } => {
            let keys: Vec<SortExpr> = keys
                .iter()
                // asc = !desc; nulls_first mirrors Postgres (irrelevant
                // here: the corpus has no NULLs by construction).
                .map(|(e, desc)| SortExpr::new(df_expr(e, ir), !*desc, *desc))
                .collect();
            lower_node(input, ir, sources)?.sort(keys)?
        }
        Plan::Limit { input, n } => lower_node(input, ir, sources)?.limit(0, Some(*n as usize))?,
    })
}

/// The scan for one rel, registered under its qualifier — shared by the
/// Scan and EmptyScan arms so their schemas cannot drift apart.
fn scan_node(rel: usize, ir: &Ir, sources: &[Arc<dyn TableSource>]) -> Result<LogicalPlanBuilder> {
    Ok(LogicalPlanBuilder::scan(
        TableReference::bare(ir.rels[rel].qualifier.clone()),
        sources[rel].clone(),
        None,
    )?)
}

/// ColRef → qualified DataFusion column, by name lookup through Ir::rels.
pub(crate) fn df_col(c: ColRef, ir: &Ir) -> DfExpr {
    let rel = &ir.rels[c.rel];
    let name = catalog::columns_for(&rel.table)[c.col].0;
    DfExpr::Column(Column::new(Some(rel.qualifier.clone()), name))
}

pub(crate) fn df_expr(e: &Expr, ir: &Ir) -> DfExpr {
    match e {
        Expr::Col(c) => df_col(*c, ir),
        Expr::Int(n) => lit(*n),
        Expr::Str(s) => lit(s.clone()),
        Expr::Bool(b) => lit(*b),
        Expr::Float(bits) => lit(f64::from_bits(*bits)),
        Expr::Cmp { op, l, r } => {
            let (l, r) = (df_expr(l, ir), df_expr(r, ir));
            match op {
                CmpOp::Eq => l.eq(r),
                CmpOp::Ne => l.not_eq(r),
                CmpOp::Lt => l.lt(r),
                CmpOp::Le => l.lt_eq(r),
                CmpOp::Gt => l.gt(r),
                CmpOp::Ge => l.gt_eq(r),
            }
        }
        Expr::And(cs) => fold_binary(cs, ir, DfExpr::and),
        Expr::Or(cs) => fold_binary(cs, ir, DfExpr::or),
        Expr::Not(inner) => DfExpr::Not(Box::new(df_expr(inner, ir))),
        Expr::Arith { op, l, r } => {
            let (l, r) = (df_expr(l, ir), df_expr(r, ir));
            match op {
                ArithOp::Add => l + r,
                ArithOp::Sub => l - r,
                ArithOp::Mul => l * r,
                ArithOp::Div => l / r,
            }
        }
    }
}

/// n-ary IR conjunction/disjunction → DataFusion's binary chain.
/// Empty lists can't be produced by the binder; guard anyway.
fn fold_binary(cs: &[Expr], ir: &Ir, op: fn(DfExpr, DfExpr) -> DfExpr) -> DfExpr {
    let mut it = cs.iter().map(|e| df_expr(e, ir));
    match it.next() {
        Some(first) => it.fold(first, op),
        None => lit(true),
    }
}
