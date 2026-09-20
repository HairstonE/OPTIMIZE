//! P1 — Ingres-style heuristics (1976). Spec: src/p1/p1.md
//!
//! Four hardcoded IR→IR passes, each a standalone function, applied once
//! in order: constant folding (+EmptyScan), predicate classification &
//! pushdown, projection pruning, redundancy. No cost, no stats, no search.
use crate::catalog;
use crate::common::ir::{ArithOp, CmpOp, Expr, Ir, Plan};
use crate::common::opt::{OptContext, Optimizer};
use crate::ir::ColRef;
use anyhow::Result;
use std::collections::HashSet;

pub struct HeuristicOptimizer;

impl Optimizer for HeuristicOptimizer {
    fn name(&self) -> &'static str {
        "heuristic"
    }

    fn optimize(&self, plan: Ir, _ctx: &OptContext) -> Result<Ir> {
        Ok(remove_redundancy(prune(pushdown(fold_constants(plan)))))
    }
}

fn fold_constants(ir: Ir) -> Ir {
    Ir {
        root: fold_plan(ir.root),
        rels: (ir.rels),
    }
}

fn fold_plan(p: Plan) -> Plan {
    match p {
        Plan::Scan { rel } => Plan::Scan { rel },
        Plan::EmptyScan { rels } => Plan::EmptyScan { rels },
        Plan::Filter { input, predicate } => {
            let input = fold_plan(*input);
            let predicate = fold_expr(predicate);

            if predicate == Expr::Bool(false) {
                Plan::EmptyScan {
                    rels: plan_rels(&input),
                }
            } else {
                Plan::Filter {
                    input: Box::new(input),
                    predicate: predicate,
                }
            }
        }
        Plan::Project { input, exprs } => Plan::Project {
            input: Box::new(fold_plan(*input)),
            exprs: exprs.into_iter().map(fold_expr).collect(),
        },
        Plan::Sort { input, keys } => {
            let input = fold_plan(*input);
            if matches!(input, Plan::EmptyScan { .. }) {
                input
            } else {
                Plan::Sort {
                    input: Box::new(input),
                    keys: keys
                        .into_iter()
                        .map(|(e, desc)| (fold_expr(e), desc))
                        .collect(),
                }
            }
        }
        Plan::Join {
            left,
            right,
            on,
            filter,
        } => {
            let left = fold_plan(*left);
            let right = fold_plan(*right);
            let filter = filter.map(fold_expr);
            // Inner join annihilation: Join(Empty(R), P) → Empty(R ∪ rels(P)).
            if matches!(left, Plan::EmptyScan { .. }) || matches!(right, Plan::EmptyScan { .. }) {
                let mut rels = plan_rels(&left);
                rels.extend(plan_rels(&right));
                rels.sort_unstable();
                rels.dedup();
                Plan::EmptyScan { rels }
            } else {
                Plan::Join {
                    left: Box::new(left),
                    right: Box::new(right),
                    on,
                    filter,
                }
            }
        }
        Plan::Limit { input, n } => {
            let input = fold_plan(*input);
            if matches!(input, Plan::EmptyScan { .. }) {
                input
            } else {
                Plan::Limit {
                    input: Box::new(input),
                    n,
                }
            }
        }
    }
}

pub fn plan_rels(p: &Plan) -> Vec<usize> {
    match p {
        Plan::Scan { rel } => vec![*rel],
        Plan::EmptyScan { rels } => rels.clone(),
        Plan::Filter { input, .. }
        | Plan::Project { input, .. }
        | Plan::Sort { input, .. }
        | Plan::Limit { input, .. } => plan_rels(input),
        Plan::Join { left, right, .. } => {
            let mut rels = plan_rels(left);
            rels.extend(plan_rels(right));
            rels.sort_unstable();
            rels.dedup();
            rels
        }
    }
}

pub fn fold_expr(e: Expr) -> Expr {
    match e {
        Expr::Arith { op, l, r } => {
            let l = fold_expr(*l);
            let r = fold_expr(*r);

            // Checked arithmetic (post-gate review #1): on i64 overflow the
            // checked op yields None and we leave the Arith unfolded rather
            // than panic; `checked_div` also covers the divide-by-zero case.
            let folded = match (&l, &r, &op) {
                (Expr::Int(a), Expr::Int(b), ArithOp::Add) => a.checked_add(*b),
                (Expr::Int(a), Expr::Int(b), ArithOp::Sub) => a.checked_sub(*b),
                (Expr::Int(a), Expr::Int(b), ArithOp::Mul) => a.checked_mul(*b),
                (Expr::Int(a), Expr::Int(b), ArithOp::Div) => a.checked_div(*b),
                _ => None,
            };
            match folded {
                Some(n) => Expr::Int(n),
                None => Expr::Arith {
                    op,
                    l: Box::new(l),
                    r: Box::new(r),
                },
            }
        }
        Expr::Cmp { op, l, r } => {
            let l = fold_expr(*l);
            let r = fold_expr(*r);

            match (&l, &r) {
                (Expr::Int(a), Expr::Int(b)) => Expr::Bool(match op {
                    CmpOp::Eq => a == b,
                    CmpOp::Ne => a != b,
                    CmpOp::Lt => a < b,
                    CmpOp::Le => a <= b,
                    CmpOp::Gt => a > b,
                    CmpOp::Ge => a >= b,
                }),
                _ => Expr::Cmp {
                    op,
                    l: Box::new(l),
                    r: Box::new(r),
                },
            }
        }
        Expr::Not(inner) => match fold_expr(*inner) {
            Expr::Bool(b) => Expr::Bool(!b),
            other => Expr::Not(Box::new(other)),
        },
        Expr::And(cs) => {
            let mut cs: Vec<Expr> = cs.into_iter().map(fold_expr).collect();
            if cs.iter().any(|c| *c == Expr::Bool(false)) {
                Expr::Bool(false)
            } else {
                cs.retain(|c| *c != Expr::Bool(true));
                match cs.len() {
                    0 => Expr::Bool(true),
                    1 => cs.pop().unwrap(),
                    _ => Expr::And(cs),
                }
            }
        }
        Expr::Or(cs) => {
            let mut cs: Vec<Expr> = cs.into_iter().map(fold_expr).collect();
            if cs.iter().any(|c| *c == Expr::Bool(true)) {
                Expr::Bool(true)
            } else {
                cs.retain(|c| *c != Expr::Bool(false));
                match cs.len() {
                    0 => Expr::Bool(false),
                    1 => cs.pop().unwrap(),
                    _ => Expr::Or(cs),
                }
            }
        }
        leaf => leaf,
    }
}

fn pushdown(ir: Ir) -> Ir {
    Ir {
        root: sink(ir.root, Vec::new()),
        rels: ir.rels,
    }
}

fn split_conjunctions(e: Expr) -> Vec<Expr> {
    match e {
        Expr::And(cs) => cs,
        _ => vec![e],
    }
}

fn sink(p: Plan, mut carried: Vec<Expr>) -> Plan {
    match p {
        Plan::Scan { rel: _ } => materialize_bag(carried, p),
        Plan::Filter { input, predicate } => {
            carried.extend(split_conjunctions(predicate));
            sink(*input, carried)
        }
        Plan::Project { input, exprs } => Plan::Project {
            input: Box::new(sink(*input, carried)),
            exprs: exprs,
        },
        Plan::Join {
            left,
            right,
            mut on,
            filter,
        } => {
            carried.extend(filter.map(split_conjunctions).unwrap_or_default());
            let left_rel = plan_rels(&left);
            let right_rel = plan_rels(&right);

            let mut left_bag: Vec<Expr> = vec![];
            let mut right_bag: Vec<Expr> = vec![];
            let mut stuck: Vec<Expr> = vec![];
            for c in carried {
                match classify_conjunct(&c, &left_rel, &right_rel) {
                    ConjunctClass::Stuck => stuck.push(c),
                    ConjunctClass::Right => right_bag.push(c),
                    ConjunctClass::Left => left_bag.push(c),
                    ConjunctClass::Key(a, b) => on.push((a, b)),
                }
            }

            let left = Box::new(sink(*left, left_bag));
            let right = Box::new(sink(*right, right_bag));

            materialize_bag(
                stuck,
                Plan::Join {
                    left,
                    right,
                    on,
                    filter: None,
                },
            )
        }
        Plan::EmptyScan { rels: _ } => materialize_bag(carried, p),
        Plan::Sort { input, keys } => materialize_bag(
            carried,
            Plan::Sort {
                input: Box::new(sink(*input, Vec::new())),
                keys,
            },
        ),
        Plan::Limit { input, n } => materialize_bag(
            carried,
            Plan::Limit {
                input: Box::new(sink(*input, Vec::new())),
                n,
            },
        ),
    }
}

fn materialize_bag(mut carried: Vec<Expr>, input: Plan) -> Plan {
    match carried.len() {
        0 => input,
        1 => Plan::Filter {
            input: Box::new(input),
            predicate: carried.pop().unwrap(),
        },
        _ => Plan::Filter {
            input: Box::new(input),
            predicate: Expr::And(carried),
        },
    }
}


/// Where one WHERE-conjunct belongs relative to a join. The single
/// source of truth (P1 review finding #2, extracted at P3 review):
/// P1's sink, P3's PredPdJoin/Pred2JoinKey — and P4's transformation
/// rules next — all classify through here. Branch order preserved from
/// the original sink loop.
pub(crate) enum ConjunctClass {
    /// Constants (no free columns) or cross-side non-equalities: stays put.
    Stuck,
    /// All free columns on the left input.
    Left,
    /// All free columns on the right input.
    Right,
    /// Cross-side column equality — a join key, oriented (left_col, right_col).
    Key(ColRef, ColRef),
}

pub(crate) fn classify_conjunct(
    c: &Expr,
    left_rels: &[usize],
    right_rels: &[usize],
) -> ConjunctClass {
    let needs = free_cols(c);
    if needs.is_empty() {
        return ConjunctClass::Stuck;
    }
    if needs.iter().all(|r| right_rels.contains(&r.rel)) {
        return ConjunctClass::Right;
    }
    if needs.iter().all(|r| left_rels.contains(&r.rel)) {
        return ConjunctClass::Left;
    }
    if let Expr::Cmp {
        op: CmpOp::Eq,
        l,
        r,
    } = c
    {
        if let (Expr::Col(a), Expr::Col(b)) = (l.as_ref(), r.as_ref()) {
            if left_rels.contains(&a.rel) && right_rels.contains(&b.rel) {
                return ConjunctClass::Key(*a, *b);
            }
            if left_rels.contains(&b.rel) && right_rels.contains(&a.rel) {
                return ConjunctClass::Key(*b, *a);
            }
        }
    }
    ConjunctClass::Stuck
}

pub(crate) fn free_cols(e: &Expr) -> Vec<ColRef> {
    match e {
        Expr::Col(c) => vec![*c],
        Expr::Not(inner) => free_cols(inner),
        Expr::Cmp { op: _, l, r } => {
            let mut res = free_cols(l);
            res.extend(free_cols(r));
            res
        }
        Expr::Arith { op: _, l, r } => {
            let mut res = free_cols(l);
            res.extend(free_cols(r));
            res
        }
        Expr::And(cs) => {
            let mut res = vec![];
            for c in cs {
                res.extend(free_cols(c));
            }
            res
        }
        Expr::Or(cs) => {
            let mut res = vec![];
            for c in cs {
                res.extend(free_cols(c));
            }
            res
        }
        _ => vec![],
    }
}

pub(crate) fn prune(ir: Ir) -> Ir {
    let widths: Vec<usize> = ir
        .rels
        .iter()
        .map(|rel| catalog::columns_for(&rel.table).len())
        .collect();
    let mut demands: HashSet<ColRef> = HashSet::new();

    for (rel, w) in widths.iter().enumerate() {
        for col in 0..*w {
            demands.insert(ColRef { rel, col });
        }
    }
    Ir {
        root: narrow(ir.root, demands, &widths),
        rels: ir.rels,
    }
}

fn narrow(p: Plan, mut demands: HashSet<ColRef>, widths: &[usize]) -> Plan {
    match p {
        Plan::Project { input, exprs } => Plan::Project {
            input: Box::new(narrow(
                *input,
                exprs.iter().flat_map(free_cols).collect(),
                widths,
            )),
            exprs: exprs,
        },
        Plan::Join {
            left,
            right,
            on,
            filter,
        } => {
            demands.extend(on.iter().flat_map(|(a, b)| [*a, *b]));
            demands.extend(filter.iter().flat_map(free_cols));
            Plan::Join {
                left: Box::new(narrow(*left, demands.clone(), widths)),
                right: Box::new(narrow(*right, demands, widths)),
                on,
                filter,
            }
        }
        Plan::Filter { input, predicate } => {
            demands.extend(free_cols(&predicate));
            return Plan::Filter {
                input: Box::new(narrow(*input, demands, widths)),
                predicate,
            };
        }
        Plan::Limit { input, n } => Plan::Limit {
            input: Box::new(narrow(*input, demands, widths)),
            n,
        },
        Plan::Sort { input, keys } => {
            demands.extend(keys.iter().flat_map(|(e, _)| free_cols(e)));

            return Plan::Sort {
                input: Box::new(narrow(*input, demands, widths)),
                keys,
            };
        }
        Plan::Scan { rel } => {
            let mut demanded_rels: Vec<ColRef> =
                demands.into_iter().filter(|c| c.rel == rel).collect();

            demanded_rels.sort();
            if demanded_rels.len() < widths[rel] {
                return Plan::Project {
                    input: Box::new(Plan::Scan { rel }),
                    exprs: demanded_rels.into_iter().map(Expr::Col).collect(),
                };
            } else {
                return Plan::Scan { rel };
            }
        }
        Plan::EmptyScan { rels } => Plan::EmptyScan { rels },
    }
}

fn remove_redundancy(ir: Ir) -> Ir {
    Ir {
        root: dedup_plan(ir.root),
        rels: (ir.rels),
    }
}
fn dedup_plan(p: Plan) -> Plan {
    match p {
        Plan::Scan { rel } => Plan::Scan { rel },
        Plan::EmptyScan { rels } => Plan::EmptyScan { rels },
        Plan::Filter { input, predicate } => Plan::Filter {
            input: Box::new(dedup_plan(*input)),
            predicate: dedup_expr(predicate),
        },
        Plan::Project { input, exprs } => Plan::Project {
            input: Box::new(dedup_plan(*input)),
            exprs: exprs.into_iter().map(dedup_expr).collect(),
        },
        Plan::Sort { input, keys } => {
            let input = dedup_plan(*input);
            if matches!(input, Plan::EmptyScan { .. }) {
                input
            } else {
                Plan::Sort {
                    input: Box::new(input),
                    keys: keys
                        .into_iter()
                        .map(|(e, desc)| (dedup_expr(e), desc))
                        .collect(),
                }
            }
        }
        Plan::Join {
            left,
            right,
            on,
            filter,
        } => Plan::Join {
            left: Box::new(dedup_plan(*left)),
            right: Box::new(dedup_plan(*right)),
            on,
            filter: filter.map(dedup_expr),
        },

        Plan::Limit { input, n } => {
            let input = dedup_plan(*input);
            if matches!(input, Plan::EmptyScan { .. }) {
                input
            } else {
                Plan::Limit {
                    input: Box::new(input),
                    n,
                }
            }
        }
    }
}

pub fn dedup_expr(e: Expr) -> Expr {
    match e {
        Expr::And(cs) => {
            let mut new_cs: Vec<Expr> = Vec::new();
            for c in cs.into_iter().map(dedup_expr) {
                if !new_cs.contains(&c) {
                    new_cs.push(c);
                }
            }
            match new_cs.len() {
                1 => new_cs.pop().unwrap(),
                _ => Expr::And(new_cs),
            }
        }
        Expr::Or(cs) => {
            let mut new_cs: Vec<Expr> = Vec::new();
            for c in cs.into_iter().map(dedup_expr) {
                if !new_cs.contains(&c) {
                    new_cs.push(c);
                }
            }
            match new_cs.len() {
                1 => new_cs.pop().unwrap(),
                _ => Expr::Or(new_cs),
            }
        }
        Expr::Not(child) => {
            let rebuilt_child = dedup_expr(*child);

            match rebuilt_child {
                Expr::Not(c) => *c,
                other => Expr::Not(Box::new(other)),
            }
        }
        Expr::Cmp { op, l, r } => Expr::Cmp {
            op,
            l: Box::new(dedup_expr(*l)),
            r: Box::new(dedup_expr(*r)),
        },
        Expr::Arith { op, l, r } => Expr::Arith {
            op,
            l: Box::new(dedup_expr(*l)),
            r: Box::new(dedup_expr(*r)),
        },
        _ => return e,
    }
}
