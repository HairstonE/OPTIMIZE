//! [scaffold] Everything the differential needs: corpus loading, the
//! subject pipeline (parse → bind → optimize → lower → lobotomized
//! execute), the DataFusion oracle, result normalization, and diffing.
//!
//! Result comparison convention (shared with the DuckDB oracle in
//! tests/common/mod.rs):
//!   - every value becomes a String; floats via `fmt_f64` (fixed 6
//!     decimals, -0 folded into 0) so both engines format identically
//!   - rows are compared as an ordered list when the query has ORDER BY,
//!     as a sorted multiset otherwise
//!   - corpus rule backing the ordered case: every ORDER BY ends in a
//!     unique tiebreaker, so "same order" is well-defined across engines

use crate::error::NotYetImplemented;
use crate::{bind, catalog, lower, opt, session, stats};
use anyhow::{bail, Context, Result};
use datafusion::arrow::array::{Array, Float64Array};
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::arrow::util::display::array_value_to_string;
use datafusion::logical_expr::LogicalPlan;
use datafusion::prelude::SessionContext;
use datafusion::sql::sqlparser::{dialect::GenericDialect, parser::Parser};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

// ── corpus ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct QueryFile {
    pub name: String, // "q07"
    pub sql: String,
    /// 0 = executable even unoptimized; 1 = needs pushdown to be feasible.
    /// See opt::execution_tier. From queries/corpus.toml.
    pub tier: u8,
}

impl QueryFile {
    pub fn has_order_by(&self) -> bool {
        self.sql.to_uppercase().contains("ORDER BY")
    }
}

#[derive(Debug, Deserialize)]
struct QueryMeta {
    tier: u8,
}

pub fn queries_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join("queries")
}

pub fn corpus() -> Result<Vec<QueryFile>> {
    let manifest_path = queries_dir().join("corpus.toml");
    let manifest: BTreeMap<String, QueryMeta> = toml::from_str(
        &std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {}", manifest_path.display()))?,
    )?;

    let mut out = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(queries_dir())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let sql = std::fs::read_to_string(&path)?.trim().to_string();
        let tier = manifest
            .get(&name)
            .map(|m| m.tier)
            // unlisted queries default to tier 1: they won't run through
            // identity, which is the safe direction to fail in
            .unwrap_or(1);
        out.push(QueryFile { name, sql, tier });
    }
    if out.is_empty() {
        bail!("no queries found in {}", queries_dir().display());
    }
    Ok(out)
}

// ── normalization & diffing ───────────────────────────────────────────────

pub type Rows = Vec<Vec<String>>;

/// One float formatting for both engines. Fixed six decimals; -0 → 0.
pub fn fmt_f64(v: f64) -> String {
    let v = if v == 0.0 { 0.0 } else { v };
    format!("{v:.6}")
}

pub fn batches_to_rows(batches: &[RecordBatch]) -> Result<Rows> {
    let mut rows = Vec::new();
    for batch in batches {
        for r in 0..batch.num_rows() {
            let mut row = Vec::with_capacity(batch.num_columns());
            for c in 0..batch.num_columns() {
                let col = batch.column(c);
                if col.is_null(r) {
                    row.push("NULL".to_string());
                    continue;
                }
                let s = match col.data_type() {
                    DataType::Float64 => {
                        let a = col.as_any().downcast_ref::<Float64Array>().unwrap();
                        fmt_f64(a.value(r))
                    }
                    _ => array_value_to_string(col, r)?,
                };
                row.push(s);
            }
            rows.push(row);
        }
    }
    Ok(rows)
}

/// Sorted-multiset semantics unless the query carries ORDER BY.
pub fn normalize(mut rows: Rows, ordered: bool) -> Rows {
    if !ordered {
        rows.sort();
    }
    rows
}

/// None = identical. Some(report) = human-readable first-differences.
pub fn diff_report(label: &str, oracle: &Rows, subject: &Rows) -> Option<String> {
    if oracle == subject {
        return None;
    }
    let mut out = format!(
        "{label}: MISMATCH — oracle {} rows, subject {} rows\n",
        oracle.len(),
        subject.len()
    );
    let shown = oracle
        .iter()
        .zip(subject.iter())
        .enumerate()
        .filter(|(_, (o, s))| o != s)
        .take(5)
        .map(|(i, (o, s))| format!("  row {i}:\n    oracle:  {o:?}\n    subject: {s:?}"))
        .collect::<Vec<_>>();
    if shown.is_empty() {
        out.push_str("  (rows agree on common prefix; lengths differ)\n");
        if oracle.len() > subject.len() {
            out.push_str(&format!("  first missing row: {:?}\n", oracle[subject.len()]));
        } else {
            out.push_str(&format!("  first extra row:   {:?}\n", subject[oracle.len()]));
        }
    } else {
        out.push_str(&shown.join("\n"));
        out.push('\n');
    }
    Some(out)
}

// ── pipelines ─────────────────────────────────────────────────────────────

pub fn parse_single(sql: &str) -> Result<datafusion::sql::sqlparser::ast::Statement> {
    let mut stmts = Parser::parse_sql(&GenericDialect {}, sql).context("SQL parse error")?;
    if stmts.len() != 1 {
        bail!("expected exactly one statement, got {}", stmts.len());
    }
    Ok(stmts.remove(0))
}

pub async fn ready_subject_ctx() -> Result<SessionContext> {
    let ctx = session::lobotomized_ctx();
    catalog::register_all(&ctx).await?;
    Ok(ctx)
}

pub async fn ready_oracle_ctx() -> Result<SessionContext> {
    let ctx = session::oracle_ctx();
    catalog::register_all(&ctx).await?;
    Ok(ctx)
}

/// Oracle #1: stock DataFusion, its own parser/binder/optimizer end-to-end.
pub async fn oracle_datafusion(sql: &str) -> Result<Rows> {
    let ctx = ready_oracle_ctx().await?;
    let batches = ctx.sql(sql).await?.collect().await?;
    batches_to_rows(&batches)
}

/// parse → bind → optimize → render. No execution, no ctx — this is what
/// the EXPLAIN snapshot gate calls, and it works for every query at every
/// tier (rendering a hopeless plan is free). `key` is a project id ("p1")
/// or an optimizer name ("heuristic").
pub fn explain_ir(key: &str, sql: &str) -> Result<String> {
    let stmt = parse_single(sql)?;
    let cat = catalog::catalog();
    let ir = bind::bind(stmt, &cat)?;
    let entry = opt::find(key).with_context(|| format!("unknown project/optimizer '{key}'"))?;
    let octx = opt::OptContext {
        stats: stats::load_default()?,
    };
    // From P4, EXPLAIN shows the PHYSICAL plan — operator choice and
    // enforcer placement are the project (COURSE.md changelog 2026-09-20).
    if entry.project == "p4" {
        let (winner, _cost, _ir) = crate::p4::optimize_physical(ir, &octx)?;
        return Ok(winner.to_string());
    }
    let ir = entry.optimizer.optimize(ir, &octx)?;
    Ok(ir.to_string())
}

/// The full subject pipeline on an already-registered lobotomized ctx.
/// Returns (post-optimization IR rendering, lowered logical plan).
pub async fn subject_plan(
    ctx: &SessionContext,
    key: &str,
    sql: &str,
) -> Result<(String, LogicalPlan)> {
    let stmt = parse_single(sql)?;
    let cat = catalog::catalog();
    let ir = bind::bind(stmt, &cat)?;
    let entry = opt::find(key).with_context(|| format!("unknown project/optimizer '{key}'"))?;
    let octx = opt::OptContext {
        stats: stats::load_default()?,
    };
    // P4's side door (its notes.md decision 8): the search returns a
    // physical winner; lowering v2 rebuilds the DataFusion plan in the
    // winner's exact shape (join order, build side, explicit Sort).
    if entry.project == "p4" {
        let (winner, _cost, ir) = crate::p4::optimize_physical(ir, &octx)?;
        let rendered = winner.to_string();
        let plan = crate::p4::lower_phys(&winner, &ir, ctx).await?;
        return Ok((rendered, plan));
    }
    let ir = entry.optimizer.optimize(ir, &octx)?;
    let rendered = ir.to_string();
    let plan = lower::lower(&ir, ctx).await?;
    Ok((rendered, plan))
}

/// Subject pipeline through execution, normalized rows out.
pub async fn subject_rows(key: &str, sql: &str) -> Result<Rows> {
    let ctx = ready_subject_ctx().await?;
    let (_, plan) = subject_plan(&ctx, key, sql).await?;
    let batches = ctx.execute_logical_plan(plan).await?.collect().await?;
    batches_to_rows(&batches)
}

// ── skip-vs-require ───────────────────────────────────────────────────────

/// Gates set OPTIMIZE_REQUIRE to a comma-separated list of project ids
/// ("p0" for gate0, "p0,p1" for gate1, ... or "all"). For a required
/// project, NotYetImplemented stops being a SKIP and becomes a failure —
/// a gate can never pass on stubs, while later projects' stubs stay
/// silent. Unset/empty = nothing required (plain `cargo test`).
pub fn is_required(project: &str) -> bool {
    match std::env::var("OPTIMIZE_REQUIRE") {
        Ok(v) if v.trim() == "all" => true,
        Ok(v) => v.split(',').any(|p| p.trim() == project),
        Err(_) => false,
    }
}

/// Convenience used by every harness test: Ok(Some(v)) = ran, Ok(None) =
/// skipped (not yet implemented + not required), Err = real failure.
pub fn skip_or_fail<T>(result: Result<T>, what: &str, required: bool) -> Result<Option<T>> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(e) if crate::error::is_todo(&e) && !required => {
            eprintln!("SKIP {what}: {e}");
            Ok(None)
        }
        Err(e) => Err(e.context(format!("in {what}"))),
    }
}

/// Placeholder marker other tools can use to detect the todo state.
pub fn todo_err(what: &'static str) -> anyhow::Error {
    NotYetImplemented(what).into()
}
