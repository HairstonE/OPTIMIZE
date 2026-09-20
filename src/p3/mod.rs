//! P3 — Starburst (1987–92). Spec: src/p3/p3.md
//!
//! P1's pass sequence becomes a rule engine; P1's passes become data.
//! Fixpoint driving, rule classes, termination budget. Join planning
//! still calls P2's DP (rewrite-then-plan).

use crate::common::ir::{CmpOp, ColRef, Expr, Ir, Plan};
use crate::common::opt::{OptContext, Optimizer};
use crate::p1::{classify_conjunct, dedup_expr, fold_expr, plan_rels, prune, ConjunctClass};
use crate::p2::reorder;
use anyhow::Result;
use std::collections::HashMap;
pub struct StarburstOptimizer;

impl Optimizer for StarburstOptimizer {
    fn name(&self) -> &'static str {
        "starburst"
    }

    fn optimize(&self, plan: Ir, ctx: &OptContext) -> Result<Ir> {
        let Ir { root, rels } = rewrite_fixpoint(plan);
        let model = ctx.cost_model(&rels);
        let (root, _) = reorder(root, &model, None);
        Ok(Ir { root, rels })
    }
}

trait RewriteRule {
    fn name(&self) -> &'static str;
    fn matches(&self, node: &Plan) -> bool;
    fn apply(&self, node: Plan) -> Plan;
}

enum Scheme {
    Sequential,
    Priority,
}

struct RuleClass {
    name: &'static str,
    scheme: Scheme,
    rules: Vec<Box<dyn RewriteRule>>,
    budget: usize,
}

#[derive(Default)]
struct RunState {
    considered: usize,
    per_rule: HashMap<&'static str, (usize, usize)>,
    log: Vec<&'static str>,
}
pub fn rewrite_fixpoint(plan: Ir) -> Ir {
    let (root, _) = run_class(plan.root, &fold_class());
    let (root, _) = run_class(root, &migrate_class());
    let Ir { root, rels } = prune(Ir {
        root,
        rels: plan.rels,
    });
    let (root, _) = run_class(root, &cleanup_class());
    Ir { root, rels }
}

struct FoldExprs;
struct FalseFilters;

impl RunState {
    fn tick(&mut self, rule: &'static str) {
        self.considered += 1;
        self.per_rule.entry(rule).or_default().0 += 1;
    }
    fn fired(&mut self, rule: &'static str) {
        self.per_rule.entry(rule).or_default().1 += 1;
        self.log.push(rule);
        if self.log.len() > 6 {
            self.log.remove(0);
        }
    }
}

impl RewriteRule for FoldExprs {
    fn name(&self) -> &'static str {
        "FOLDEXPRS"
    }

    fn matches(&self, node: &Plan) -> bool {
        match node {
            Plan::Filter { predicate, .. } => fold_expr(predicate.clone()) != *predicate,
            Plan::Project { exprs, .. } => exprs.iter().any(|e| fold_expr(e.clone()) != *e),
            Plan::Sort { keys, .. } => keys.iter().any(|(e, _)| fold_expr(e.clone()) != *e),
            _ => false,
        }
    }

    fn apply(&self, node: Plan) -> Plan {
        match node {
            Plan::Filter { input, predicate } => Plan::Filter {
                input,
                predicate: fold_expr(predicate),
            },
            Plan::Project { input, exprs } => Plan::Project {
                input,
                exprs: exprs.into_iter().map(fold_expr).collect(),
            },
            Plan::Sort { input, keys } => Plan::Sort {
                input,
                keys: keys.into_iter().map(|(e, d)| (fold_expr(e), d)).collect(),
            },
            other => other,
        }
    }
}

impl RewriteRule for FalseFilters {
    fn name(&self) -> &'static str {
        "FALSEFILTERS"
    }

    fn matches(&self, node: &Plan) -> bool {
        match node {
            Plan::Filter { predicate, .. } => *predicate == Expr::Bool(false),
            _ => false,
        }
    }

    fn apply(&self, node: Plan) -> Plan {
        match node {
            Plan::Filter {
                input,
                predicate: _,
            } => Plan::EmptyScan {
                rels: plan_rels(&input),
            },
            other => other,
        }
    }
}

fn apply_rules_at(mut node: Plan, class: &RuleClass, state: &mut RunState) -> (Plan, bool) {
    let mut i = 0;
    let mut any_fired = false;
    while i < class.rules.len() {
        let rule = &class.rules[i];
        if state.considered >= class.budget {
            break;
        }
        state.tick(rule.name());
        if rule.matches(&node) {
            #[cfg(debug_assertions)]
            let before = node.clone();
            node = rule.apply(node);
            #[cfg(debug_assertions)]
            debug_assert!(
                before != node,
                "rule {} fired without changing the node — a no-progress rule loops to budget",
                rule.name()
            );
            state.fired(rule.name());
            any_fired = true;
            match class.scheme {
                Scheme::Priority => i = 0,
                Scheme::Sequential => continue,
            };
        } else {
            i += 1;
        }
    }
    (node, any_fired)
}

fn sweep(node: Plan, class: &RuleClass, state: &mut RunState) -> (Plan, bool) {
    let (node, fired) = apply_rules_at(node, class, state);

    let (rebuilt, child_fired) = match node {
        Plan::Project { input, exprs } => {
            let (child, cf) = sweep(*input, class, state);
            (
                Plan::Project {
                    input: Box::new(child),
                    exprs,
                },
                cf,
            )
        }
        Plan::Filter { input, predicate } => {
            let (child, cf) = sweep(*input, class, state);
            (
                Plan::Filter {
                    input: Box::new(child),
                    predicate,
                },
                cf,
            )
        }
        Plan::Sort { input, keys } => {
            let (child, cf) = sweep(*input, class, state);
            (
                Plan::Sort {
                    input: Box::new(child),
                    keys,
                },
                cf,
            )
        }
        Plan::Limit { input, n } => {
            let (child, cf) = sweep(*input, class, state);
            (
                Plan::Limit {
                    input: Box::new(child),
                    n,
                },
                cf,
            )
        }
        Plan::Scan { rel } => (Plan::Scan { rel }, false),
        Plan::EmptyScan { rels } => (Plan::EmptyScan { rels }, false),
        Plan::Join {
            left,
            right,
            on,
            filter,
        } => {
            let (l, lf) = sweep(*left, class, state);
            let (r, rf) = sweep(*right, class, state);
            (
                Plan::Join {
                    left: Box::new(l),
                    right: Box::new(r),
                    on,
                    filter,
                },
                lf | rf,
            )
        }
    };

    (rebuilt, fired | child_fired)
}

fn run_class(root: Plan, class: &RuleClass) -> (Plan, RunState) {
    let mut state = RunState::default();
    let mut root = root;
    loop {
        let (new_root, fired) = sweep(root, class, &mut state);
        root = new_root;
        if !fired {
            break; // fixpoint: a full sweep changed nothing — the normal exit
        }
        if state.considered >= class.budget {
            eprintln!("{}", trip_report(class, &state));
            break; // budget trip: stop between rules, tree still valid
        }
    }
    (root, state)
}

fn trip_report(class: &RuleClass, state: &RunState) -> String {
    let mut lines = vec![format!(
        "rule class '{}' tripped its budget ({} considerations):",
        class.name, class.budget
    )];
    let mut rows: Vec<_> = state.per_rule.iter().collect();
    rows.sort(); // deterministic output — HashMap order is random
    for (rule, (considered, fired)) in rows {
        lines.push(format!(
            "  {rule:<14} considered {considered:>5}  fired {fired:>5}"
        ));
    }
    lines.push(format!("  last firings: {}", state.log.join(" -> ")));
    lines.join("\n")
}

/// Zero rows sorted is zero rows. P1 fold, Sort arm.
struct EmptySort;
impl RewriteRule for EmptySort {
    fn name(&self) -> &'static str {
        "EMPTYSORT"
    }
    fn matches(&self, node: &Plan) -> bool {
        match node {
            Plan::Sort { input, .. } => matches!(**input, Plan::EmptyScan { .. }),
            _ => false,
        }
    }
    fn apply(&self, node: Plan) -> Plan {
        match node {
            Plan::Sort { input, .. } => *input,
            other => other,
        }
    }
}

/// Zero rows limited is zero rows. P1 fold, Limit arm.
struct EmptyLimit;
impl RewriteRule for EmptyLimit {
    fn name(&self) -> &'static str {
        "EMPTYLIMIT"
    }
    fn matches(&self, node: &Plan) -> bool {
        match node {
            Plan::Limit { input, .. } => matches!(**input, Plan::EmptyScan { .. }),
            _ => false,
        }
    }
    fn apply(&self, node: Plan) -> Plan {
        match node {
            Plan::Limit { input, .. } => *input,
            other => other,
        }
    }
}

/// Zero rows joined against anything is zero rows; the EmptyScan must
/// cover BOTH sides' rels. P1 fold, Join arm.
struct EmptyJoin;
impl RewriteRule for EmptyJoin {
    fn name(&self) -> &'static str {
        "EMPTYJOIN"
    }
    fn matches(&self, node: &Plan) -> bool {
        match node {
            Plan::Join { left, right, .. } => {
                matches!(**left, Plan::EmptyScan { .. })
                    || matches!(**right, Plan::EmptyScan { .. })
            }
            _ => false,
        }
    }
    fn apply(&self, node: Plan) -> Plan {
        match node {
            Plan::Join { left, right, .. } => {
                let mut rels = plan_rels(&left);
                rels.extend(plan_rels(&right));
                Plan::EmptyScan { rels }
            }
            other => other,
        }
    }
}

struct DedupExprs;
impl RewriteRule for DedupExprs {
    fn name(&self) -> &'static str {
        "DEDUPEXPRS"
    }

    fn matches(&self, node: &Plan) -> bool {
        match node {
            Plan::Filter { predicate, .. } => dedup_expr(predicate.clone()) != *predicate,
            Plan::Project { exprs, .. } => exprs.iter().any(|e| dedup_expr(e.clone()) != *e),
            Plan::Sort { keys, .. } => keys.iter().any(|(e, _)| dedup_expr(e.clone()) != *e),
            _ => false,
        }
    }

    fn apply(&self, node: Plan) -> Plan {
        match node {
            Plan::Filter { input, predicate } => Plan::Filter {
                input,
                predicate: dedup_expr(predicate),
            },
            Plan::Project { input, exprs } => Plan::Project {
                input,
                exprs: exprs.into_iter().map(dedup_expr).collect(),
            },
            Plan::Sort { input, keys } => Plan::Sort {
                input,
                keys: keys.into_iter().map(|(e, d)| (dedup_expr(e), d)).collect(),
            },
            other => other,
        }
    }
}

fn fold_class() -> RuleClass {
    RuleClass {
        name: "FOLD",
        scheme: Scheme::Sequential,
        rules: vec![
            Box::new(FoldExprs),
            Box::new(FalseFilters),
            Box::new(EmptySort),
            Box::new(EmptyLimit),
            Box::new(EmptyJoin),
        ],
        budget: 10_000,
    }
}

fn cleanup_class() -> RuleClass {
    RuleClass {
        name: "CLEANUP",
        scheme: Scheme::Sequential,
        rules: vec![Box::new(DedupExprs), Box::new(MergeFilter)],
        budget: 10_000,
    }
}

/// A packed AND can only travel as a convoy; split so each conjunct
/// sinks alone. First conjunct ends outermost (P1's order, preserved
/// when CLEANUP's MergeFilter reassembles parked stacks).
struct SplitAnd;
impl RewriteRule for SplitAnd {
    fn name(&self) -> &'static str {
        "SPLITAND"
    }
    fn matches(&self, node: &Plan) -> bool {
        matches!(
            node,
            Plan::Filter {
                predicate: Expr::And(_),
                ..
            }
        )
    }
    fn apply(&self, node: Plan) -> Plan {
        match node {
            Plan::Filter {
                input,
                predicate: Expr::And(cs),
            } => {
                let mut acc = *input;
                for c in cs.into_iter().rev() {
                    acc = Plan::Filter {
                        input: Box::new(acc),
                        predicate: c,
                    };
                }
                acc
            }
            other => other,
        }
    }
}

/// Sort never changes membership and a Filter treats rows
/// independently, so they commute; below is cheaper (fewer rows sorted).
struct PredPdSort;
impl RewriteRule for PredPdSort {
    fn name(&self) -> &'static str {
        "PREDPDSORT"
    }
    fn matches(&self, node: &Plan) -> bool {
        match node {
            Plan::Filter { input, .. } => matches!(**input, Plan::Sort { .. }),
            _ => false,
        }
    }
    fn apply(&self, node: Plan) -> Plan {
        match node {
            Plan::Filter { input, predicate } => match *input {
                Plan::Sort { input, keys } => Plan::Sort {
                    input: Box::new(Plan::Filter { input, predicate }),
                    keys,
                },
                other => Plan::Filter {
                    input: Box::new(other),
                    predicate,
                },
            },
            other => other,
        }
    }
}

/// A conjunct whose free columns all live on ONE side of a Join sinks
/// into that side (plan_rels = the decision-1 property ask). A conjunct
/// with NO free columns must NOT sink: all-of-empty-set is vacuously
/// true, and P1 keeps constants stuck above the join.
struct PredPdJoin;
impl PredPdJoin {
    /// Some(true) = fits left, Some(false) = fits right, None = neither.
    fn side(predicate: &Expr, left: &Plan, right: &Plan) -> Option<bool> {
        match classify_conjunct(predicate, &plan_rels(left), &plan_rels(right)) {
            ConjunctClass::Left => Some(true),
            ConjunctClass::Right => Some(false),
            _ => None,
        }
    }
}
impl RewriteRule for PredPdJoin {
    fn name(&self) -> &'static str {
        "PREDPDJOIN"
    }
    fn matches(&self, node: &Plan) -> bool {
        match node {
            Plan::Filter { input, predicate } => match &**input {
                Plan::Join { left, right, .. } => {
                    PredPdJoin::side(predicate, left, right).is_some()
                }
                _ => false,
            },
            _ => false,
        }
    }
    fn apply(&self, node: Plan) -> Plan {
        match node {
            Plan::Filter { input, predicate } => match *input {
                Plan::Join {
                    left,
                    right,
                    on,
                    filter,
                } => match PredPdJoin::side(&predicate, &left, &right) {
                    Some(true) => Plan::Join {
                        left: Box::new(Plan::Filter {
                            input: left,
                            predicate,
                        }),
                        right,
                        on,
                        filter,
                    },
                    _ => Plan::Join {
                        left,
                        right: Box::new(Plan::Filter {
                            input: right,
                            predicate,
                        }),
                        on,
                        filter,
                    },
                },
                other => Plan::Filter {
                    input: Box::new(other),
                    predicate,
                },
            },
            other => other,
        }
    }
}

/// An equality with one column per side IS a join key: move it into
/// the Join's `on` list, oriented (left_col, right_col) like P1.
struct Pred2JoinKey;
impl Pred2JoinKey {
    /// Some((l, r)) oriented left-first when the conjunct is a
    /// cross-side column equality.
    fn key(predicate: &Expr, left: &Plan, right: &Plan) -> Option<(ColRef, ColRef)> {
        match classify_conjunct(predicate, &plan_rels(left), &plan_rels(right)) {
            ConjunctClass::Key(a, b) => Some((a, b)),
            _ => None,
        }
    }
}
impl RewriteRule for Pred2JoinKey {
    fn name(&self) -> &'static str {
        "PRED2JOINKEY"
    }
    fn matches(&self, node: &Plan) -> bool {
        match node {
            Plan::Filter { input, predicate } => match &**input {
                Plan::Join { left, right, .. } => {
                    Pred2JoinKey::key(predicate, left, right).is_some()
                }
                _ => false,
            },
            _ => false,
        }
    }
    fn apply(&self, node: Plan) -> Plan {
        match node {
            Plan::Filter { input, predicate } => match *input {
                Plan::Join {
                    left,
                    right,
                    mut on,
                    filter,
                } => {
                    if let Some(key) = Pred2JoinKey::key(&predicate, &left, &right) {
                        on.insert(0, key);
                    }
                    Plan::Join {
                        left,
                        right,
                        on,
                        filter,
                    }
                }
                other => Plan::Filter {
                    input: Box::new(other),
                    predicate,
                },
            },
            other => other,
        }
    }
}

/// Stacked Filters merge into one flat AND, outer conjuncts first.
/// Lives in CLEANUP, never beside SplitAnd (they are inverses —
/// decision 3's ping-pong).
struct MergeFilter;
impl MergeFilter {
    fn conjuncts(e: Expr) -> Vec<Expr> {
        match e {
            Expr::And(cs) => cs,
            other => vec![other],
        }
    }
}
impl RewriteRule for MergeFilter {
    fn name(&self) -> &'static str {
        "MERGEFILTER"
    }
    fn matches(&self, node: &Plan) -> bool {
        match node {
            Plan::Filter { input, .. } => matches!(**input, Plan::Filter { .. }),
            _ => false,
        }
    }
    fn apply(&self, node: Plan) -> Plan {
        match node {
            Plan::Filter { input, predicate } => match *input {
                Plan::Filter {
                    input: inner_input,
                    predicate: inner_predicate,
                } => {
                    let mut cs = MergeFilter::conjuncts(predicate);
                    cs.extend(MergeFilter::conjuncts(inner_predicate));
                    Plan::Filter {
                        input: inner_input,
                        predicate: Expr::And(cs),
                    }
                }
                other => Plan::Filter {
                    input: Box::new(other),
                    predicate,
                },
            },
            other => other,
        }
    }
}

fn migrate_class() -> RuleClass {
    RuleClass {
        name: "PREDMIGRATE",
        scheme: Scheme::Sequential,
        rules: vec![
            Box::new(SplitAnd),
            Box::new(PredPdSort),
            Box::new(Pred2JoinKey),
            Box::new(PredPdJoin),
            Box::new(PredPdProj),
        ],
        budget: 10_000,
    }
}

struct PredPdProj;
impl RewriteRule for PredPdProj {
    fn name(&self) -> &'static str {
        "PREDPDPROJ"
    }
    fn matches(&self, node: &Plan) -> bool {
        match node {
            Plan::Filter { input, .. } => matches!(**input, Plan::Project { .. }),
            _ => false,
        }
    }
    fn apply(&self, node: Plan) -> Plan {
        match node {
            Plan::Filter { input, predicate } => match *input {
                Plan::Project { input, exprs } => Plan::Project {
                    input: Box::new(Plan::Filter { input, predicate }),
                    exprs,
                },
                other => Plan::Filter {
                    input: Box::new(other),
                    predicate,
                },
            },
            other => other,
        }
    }
}

pub fn seeded_ping_pong(budget: usize) -> (String, bool, String) {
    let class = RuleClass {
        name: "BROKEN",
        scheme: Scheme::Priority, // MERGEFILTER outranks: the barge-in loop
        rules: vec![Box::new(MergeFilter), Box::new(SplitAnd)],
        budget,
    };
    let c = |n: i64| Expr::Cmp {
        op: CmpOp::Gt,
        l: Box::new(Expr::Col(ColRef { rel: 0, col: 0 })),
        r: Box::new(Expr::Int(n)),
    };
    let tree = Plan::Filter {
        input: Box::new(Plan::Scan { rel: 0 }),
        predicate: Expr::And(vec![c(1), c(2)]),
    };
    let (root, state) = run_class(tree, &class);
    let tripped = state.considered >= class.budget;
    let rendered = Ir { root, rels: vec![] }.to_string();
    (rendered, tripped, trip_report(&class, &state))
}

pub fn rewrite_fixpoint_counted(plan: Ir) -> (Ir, usize) {
    let (root, s1) = run_class(plan.root, &fold_class());
    let (root, s2) = run_class(root, &migrate_class());
    let Ir { root, rels } = prune(Ir {
        root,
        rels: plan.rels,
    });
    let (root, s3) = run_class(root, &cleanup_class());
    let apps = [s1, s2, s3]
        .iter()
        .flat_map(|s| s.per_rule.values())
        .map(|(_, fired)| fired)
        .sum();
    (Ir { root, rels }, apps)
}
