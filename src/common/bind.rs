//! [yours — P0, src/common/common.md Step 4–5] The binder: sqlparser AST → IR.
//!
//! ── CONTRACT ──────────────────────────────────────────────────────────────
//! Scope (the corpus defines the contract; the binder meets the corpus,
//! nothing more):
//!   - SELECT list: named columns, qualified (t.col / alias.col) and
//!     unqualified, and `*`
//!   - FROM: comma-joins, `INNER JOIN … ON`, optional table aliases (`AS`)
//!   - WHERE, ORDER BY (may reference non-projected columns — Sort sits
//!     BELOW the final Project, so the full FROM scope is visible to it),
//!     LIMIT
//!   - Expressions: literals (int, float, string), comparisons
//!     (= <> < <= > >=), AND/OR/NOT, arithmetic (+ - *; / only on floats)
//!
//! Everything else is a clean Err(Unsupported(...)) — use
//! `crate::error::unsupported("GROUP BY")`. Never panic on user SQL.
//!
//! Resolution: names → positional refs against `Catalog`. Comma-join
//! `WHERE a.x = b.y` conjuncts are NOT classified here — emit a filter over
//! a cross-join-shaped tree and let optimizers discover join predicates.
//! Explicit `INNER JOIN … ON` binds to Join { on } directly. That asymmetry
//! is the point: P1's pushdown pass gets real work on day one, and the P0
//! EXPLAIN snapshots show the difference between the two FROM spellings.
//!
//! Tree shape, bottom-up: Scan(s)/Join(s) → Filter? → Sort? → Limit? →
//! Project — Project at the VERY top, so every clause below it binds
//! against the full FROM scope and only the final Project defines the
//! output (ORDER BY on a non-projected column, q25, works for free).
//! ──────────────────────────────────────────────────────────────────────────

use crate::catalog::Catalog;
use crate::error::unsupported;
use crate::ir::{ArithOp, CmpOp, ColRef, Expr, Ir, Plan, RelInfo};
use anyhow::{bail, Result};
use datafusion::sql::sqlparser::ast as sql;

pub fn bind(stmt: sql::Statement, catalog: &Catalog) -> Result<Ir> {
    let query = match stmt {
        sql::Statement::Query(q) => *q,
        _ => return Err(unsupported("only SELECT queries are supported")),
    };
    if query.with.is_some() {
        return Err(unsupported("WITH (CTEs)"));
    }
    if query.fetch.is_some() || !query.locks.is_empty() {
        return Err(unsupported("FETCH / FOR UPDATE"));
    }

    let select = match *query.body {
        sql::SetExpr::Select(s) => *s,
        _ => return Err(unsupported("set operations (UNION/EXCEPT/INTERSECT) or VALUES")),
    };
    if select.distinct.is_some() {
        return Err(unsupported("DISTINCT"));
    }
    if select.top.is_some() {
        return Err(unsupported("TOP"));
    }
    if select.having.is_some() {
        return Err(unsupported("HAVING"));
    }
    match &select.group_by {
        sql::GroupByExpr::Expressions(exprs, mods) if exprs.is_empty() && mods.is_empty() => {}
        _ => return Err(unsupported("GROUP BY")),
    }

    // ── FROM: rels + the join-shaped bottom of the tree ──────────────────
    let mut rels: Vec<RelInfo> = Vec::new();
    let mut plan: Option<Plan> = None;
    for twj in select.from {
        // Base relation of this comma-group, then any explicit JOINs
        // chained onto it. `group_rels` tracks which rels are inside this
        // group so ON predicates can be oriented (left side vs right side).
        let base = push_rel(&mut rels, catalog, twj.relation)?;
        let mut group_rels = vec![base];
        let mut group = Plan::Scan { rel: base };

        for j in twj.joins {
            let constraint = match j.join_operator {
                sql::JoinOperator::Inner(c) | sql::JoinOperator::Join(c) => c,
                sql::JoinOperator::CrossJoin(sql::JoinConstraint::None) => {
                    sql::JoinConstraint::None
                }
                _ => return Err(unsupported("only INNER JOIN is supported")),
            };
            let right = push_rel(&mut rels, catalog, j.relation)?;
            let right_plan = Plan::Scan { rel: right };
            let (on, filter) = match constraint {
                sql::JoinConstraint::On(e) => {
                    let bound = bind_expr(&e, &rels, catalog)?;
                    split_on(bound, &group_rels, right)?
                }
                sql::JoinConstraint::None => (vec![], None),
                _ => return Err(unsupported("USING / NATURAL join")),
            };
            group_rels.push(right);
            group = Plan::Join {
                left: Box::new(group),
                right: Box::new(right_plan),
                on,
                filter,
            };
        }

        // Comma between FROM items = cross-join shape (on=[]); WHERE stays
        // whole on top. Optimizers discover the join predicates — not us.
        plan = Some(match plan {
            None => group,
            Some(acc) => Plan::Join {
                left: Box::new(acc),
                right: Box::new(group),
                on: vec![],
                filter: None,
            },
        });
    }
    let mut plan = match plan {
        Some(p) => p,
        None => return Err(unsupported("SELECT without FROM")),
    };

    // ── WHERE: bound whole, never classified ─────────────────────────────
    if let Some(sel) = select.selection {
        plan = Plan::Filter {
            input: Box::new(plan),
            predicate: bind_expr(&sel, &rels, catalog)?,
        };
    }

    // ── ORDER BY (below Project: full scope visible) ─────────────────────
    if let Some(ob) = query.order_by {
        let exprs = match ob.kind {
            sql::OrderByKind::Expressions(v) => v,
            sql::OrderByKind::All(_) => return Err(unsupported("ORDER BY ALL")),
        };
        let mut keys = Vec::with_capacity(exprs.len());
        for obe in &exprs {
            if obe.with_fill.is_some() || obe.options.nulls_first.is_some() {
                return Err(unsupported("ORDER BY WITH FILL / NULLS FIRST"));
            }
            let desc = obe.options.asc == Some(false);
            keys.push((bind_expr(&obe.expr, &rels, catalog)?, desc));
        }
        if !keys.is_empty() {
            plan = Plan::Sort {
                input: Box::new(plan),
                keys,
            };
        }
    }

    // ── LIMIT ────────────────────────────────────────────────────────────
    if let Some(lc) = query.limit_clause {
        let limit = match lc {
            sql::LimitClause::LimitOffset {
                limit,
                offset: None,
                limit_by,
            } if limit_by.is_empty() => limit,
            _ => return Err(unsupported("OFFSET / LIMIT BY")),
        };
        if let Some(e) = limit {
            let n = match bind_expr(&e, &rels, catalog)? {
                Expr::Int(n) if n >= 0 => n as u64,
                _ => return Err(unsupported("non-integer LIMIT")),
            };
            plan = Plan::Limit {
                input: Box::new(plan),
                n,
            };
        }
    }

    // ── SELECT list: Project at the very top ─────────────────────────────
    let mut exprs = Vec::new();
    for item in select.projection {
        match item {
            sql::SelectItem::UnnamedExpr(e) => exprs.push(bind_expr(&e, &rels, catalog)?),
            sql::SelectItem::Wildcard(_) => {
                for (ri, rel) in rels.iter().enumerate() {
                    let table = catalog
                        .table(&rel.table)
                        .expect("rel bound against catalog");
                    for ci in 0..table.columns.len() {
                        exprs.push(Expr::Col(ColRef { rel: ri, col: ci }));
                    }
                }
            }
            sql::SelectItem::QualifiedWildcard(..) => {
                return Err(unsupported("qualified wildcard (t.*)"))
            }
            sql::SelectItem::ExprWithAlias { .. } | sql::SelectItem::ExprWithAliases { .. } => {
                return Err(unsupported("SELECT-list aliases"))
            }
        }
    }
    if exprs.is_empty() {
        return Err(unsupported("empty SELECT list"));
    }
    Ok(Ir {
        root: Plan::Project {
            input: Box::new(plan),
            exprs,
        },
        rels,
    })
}

/// Resolve one FROM item into a RelInfo; returns its rel index. The
/// qualifier is the alias if present, else the table name — and SQL says an
/// alias REPLACES the table name for the query's scope, so resolution
/// matches qualifiers only. Qualifiers must be unique (SQL guarantees it;
/// we enforce it so qualifier-based resolution stays sound).
fn push_rel(rels: &mut Vec<RelInfo>, catalog: &Catalog, factor: sql::TableFactor) -> Result<usize> {
    let (name, alias) = match factor {
        sql::TableFactor::Table {
            name,
            alias,
            args: None,
            ..
        } => (name, alias),
        sql::TableFactor::Derived { .. } => return Err(unsupported("subquery in FROM")),
        _ => return Err(unsupported("non-table FROM item")),
    };
    let table = match name.0.as_slice() {
        [sql::ObjectNamePart::Identifier(id)] => id.value.clone(),
        _ => return Err(unsupported("qualified table name (schema.table)")),
    };
    if catalog.table(&table).is_none() {
        bail!("unknown table '{table}'");
    }
    let qualifier = match alias {
        Some(a) if a.columns.is_empty() => a.name.value,
        Some(_) => return Err(unsupported("table alias with column list")),
        None => table.clone(),
    };
    if rels.iter().any(|r| r.qualifier == qualifier) {
        bail!("duplicate table name/alias '{qualifier}' in FROM (add an alias)");
    }
    rels.push(RelInfo { table, qualifier });
    Ok(rels.len() - 1)
}

/// Decompose a bound ON predicate for Join { left=group_rels, right }:
/// equi-conjuncts linking the two sides become (left_col, right_col) `on`
/// pairs; anything else stays as the join's residual filter.
fn split_on(
    bound: Expr,
    group_rels: &[usize],
    right: usize,
) -> Result<(Vec<(ColRef, ColRef)>, Option<Expr>)> {
    let conjuncts = match bound {
        Expr::And(cs) => cs,
        other => vec![other],
    };
    let mut on = Vec::new();
    let mut residual = Vec::new();
    for c in conjuncts {
        match &c {
            Expr::Cmp { op: CmpOp::Eq, l, r } => match (l.as_ref(), r.as_ref()) {
                (Expr::Col(a), Expr::Col(b)) => {
                    let (a, b) = (*a, *b);
                    if group_rels.contains(&a.rel) && b.rel == right {
                        on.push((a, b));
                    } else if group_rels.contains(&b.rel) && a.rel == right {
                        on.push((b, a));
                    } else if a.rel == right && b.rel == right {
                        residual.push(c);
                    } else {
                        return Err(unsupported(
                            "ON predicate referencing tables outside the join",
                        ));
                    }
                }
                _ => residual.push(c),
            },
            _ => residual.push(c),
        }
    }
    let filter = match residual.len() {
        0 => None,
        1 => Some(residual.remove(0)),
        _ => Some(Expr::And(residual)),
    };
    Ok((on, filter))
}

/// sqlparser expression → bound IR expression. Every arm we don't handle is
/// a clean Unsupported carrying sqlparser's own rendering of the node.
fn bind_expr(e: &sql::Expr, rels: &[RelInfo], catalog: &Catalog) -> Result<Expr> {
    Ok(match e {
        sql::Expr::Identifier(id) => Expr::Col(resolve(None, &id.value, rels, catalog)?),
        sql::Expr::CompoundIdentifier(ids) => match ids.as_slice() {
            [q, c] => Expr::Col(resolve(Some(&q.value), &c.value, rels, catalog)?),
            _ => return Err(unsupported("3+-part column name")),
        },
        sql::Expr::Value(v) => bind_literal(&v.value)?,
        sql::Expr::Nested(inner) => bind_expr(inner, rels, catalog)?,
        sql::Expr::UnaryOp {
            op: sql::UnaryOperator::Not,
            expr,
        } => Expr::Not(Box::new(bind_expr(expr, rels, catalog)?)),
        // Negative literals arrive as unary minus over a value (q18's
        // c_acctbal < -900.0): fold into the literal, reject on non-literals.
        sql::Expr::UnaryOp {
            op: sql::UnaryOperator::Minus,
            expr,
        } => match bind_expr(expr, rels, catalog)? {
            Expr::Int(n) => Expr::Int(-n),
            Expr::Float(bits) => float_expr(-f64::from_bits(bits)),
            _ => return Err(unsupported("unary minus on a non-literal")),
        },
        sql::Expr::BinaryOp { left, op, right } => {
            let l = bind_expr(left, rels, catalog)?;
            let r = bind_expr(right, rels, catalog)?;
            use sql::BinaryOperator as B;
            let cmp = |op: CmpOp, l: Expr, r: Expr| Expr::Cmp {
                op,
                l: Box::new(l),
                r: Box::new(r),
            };
            let arith = |op: ArithOp, l: Expr, r: Expr| Expr::Arith {
                op,
                l: Box::new(l),
                r: Box::new(r),
            };
            match op {
                // Flatten AND/OR chains on construction: the parser is
                // left-associative, so `a AND b AND c` arrives nested; the
                // IR is n-ary and keeps TEXTUAL order (canonical sorting is
                // canonicalize's job, not the binder's).
                B::And => match l {
                    Expr::And(mut v) => {
                        v.push(r);
                        Expr::And(v)
                    }
                    _ => Expr::And(vec![l, r]),
                },
                B::Or => match l {
                    Expr::Or(mut v) => {
                        v.push(r);
                        Expr::Or(v)
                    }
                    _ => Expr::Or(vec![l, r]),
                },
                B::Eq => cmp(CmpOp::Eq, l, r),
                B::NotEq => cmp(CmpOp::Ne, l, r),
                B::Lt => cmp(CmpOp::Lt, l, r),
                B::LtEq => cmp(CmpOp::Le, l, r),
                B::Gt => cmp(CmpOp::Gt, l, r),
                B::GtEq => cmp(CmpOp::Ge, l, r),
                B::Plus => arith(ArithOp::Add, l, r),
                B::Minus => arith(ArithOp::Sub, l, r),
                B::Multiply => arith(ArithOp::Mul, l, r),
                B::Divide => arith(ArithOp::Div, l, r),
                other => return Err(unsupported(format!("operator {other}"))),
            }
        }
        other => return Err(unsupported(format!("expression: {other}"))),
    })
}

fn bind_literal(v: &sql::Value) -> Result<Expr> {
    Ok(match v {
        sql::Value::Number(s, _) => match s.parse::<i64>() {
            Ok(n) => Expr::Int(n),
            Err(_) => match s.parse::<f64>() {
                Ok(f) => float_expr(f),
                Err(_) => return Err(unsupported(format!("numeric literal {s}"))),
            },
        },
        sql::Value::SingleQuotedString(s) => Expr::Str(s.clone()),
        sql::Value::Boolean(b) => Expr::Bool(*b),
        other => return Err(unsupported(format!("literal {other}"))),
    })
}

/// Float literal with -0.0 folded to 0.0 so structural equality can't
/// distinguish two spellings of zero (mirrors harness::fmt_f64).
fn float_expr(f: f64) -> Expr {
    let f = if f == 0.0 { 0.0 } else { f };
    Expr::Float(f.to_bits())
}

/// THE name-resolution helper — everything (WHERE, SELECT, ORDER BY, ON)
/// goes through here. Unqualified: search all rels, error on ambiguity.
/// Qualified: match the rel's qualifier (alias-or-table-name) exactly.
fn resolve(
    qualifier: Option<&str>,
    name: &str,
    rels: &[RelInfo],
    catalog: &Catalog,
) -> Result<ColRef> {
    let col_in = |rel: &RelInfo| -> Option<usize> {
        catalog
            .table(&rel.table)
            .and_then(|t| t.columns.iter().position(|(n, _)| n == name))
    };
    match qualifier {
        Some(q) => {
            let rel = match rels.iter().position(|r| r.qualifier == q) {
                Some(i) => i,
                None => bail!("unknown table or alias '{q}'"),
            };
            match col_in(&rels[rel]) {
                Some(col) => Ok(ColRef { rel, col }),
                None => bail!("no column '{name}' in '{q}'"),
            }
        }
        None => {
            let mut hits = rels
                .iter()
                .enumerate()
                .filter_map(|(rel, r)| col_in(r).map(|col| ColRef { rel, col }));
            match (hits.next(), hits.next()) {
                (Some(c), None) => Ok(c),
                (Some(_), Some(_)) => bail!("ambiguous column '{name}' (qualify it)"),
                (None, _) => bail!("unknown column '{name}'"),
            }
        }
    }
}
