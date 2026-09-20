//! P2 — Selinger (1979). Spec: src/p2/p2.md
//!
//! P1's passes first, then replace the join tree: bottom-up DP over
//! connected subsets, left-deep, frozen cost model, interesting orders.

use std::collections::HashMap;

use crate::common::cost::CostModel;
use crate::common::ir::{ColRef, Expr, Ir, Plan};
use crate::common::opt::{OptContext, Optimizer};
use crate::p1::{plan_rels, HeuristicOptimizer};
use anyhow::Result;
pub struct SelingerOptimizer;

impl Optimizer for SelingerOptimizer {
    fn name(&self) -> &'static str {
        "selinger"
    }

    fn optimize(&self, plan: Ir, ctx: &OptContext) -> Result<Ir> {
        let heuristic_plan = HeuristicOptimizer.optimize(plan, ctx)?;

        let Ir { root, rels } = heuristic_plan;
        let model = ctx.cost_model(&rels);
        let (root, _) = reorder(root, &model, None);
        Ok(Ir { root, rels })
    }
}

pub(crate) fn reorder(
    root: Plan,
    model: &CostModel,
    target: Option<&[(ColRef, bool)]>,
) -> (Plan, bool) {
    match root {
        Plan::Project { input, exprs } => {
            let (child, delivered) = reorder(*input, model, target);
            (
                Plan::Project {
                    input: Box::new(child),
                    exprs,
                },
                delivered,
            )
        }
        Plan::Filter { input, predicate } => {
            let (child, delivered) = reorder(*input, model, target);
            (
                Plan::Filter {
                    input: Box::new(child),
                    predicate,
                },
                delivered,
            )
        }
        Plan::Sort { input, keys } => {
            let target: Option<Vec<(ColRef, bool)>> = keys
                .iter()
                .map(|(e, desc)| match e {
                    Expr::Col(c) => Some((*c, *desc)),
                    _ => None,
                })
                .collect();
            let (child, delivered) = reorder(*input, model, target.as_deref());
            if delivered {
                (child, false)
            } else {
                (
                    Plan::Sort {
                        input: Box::new(child),
                        keys,
                    },
                    false,
                )
            }
        }
        Plan::Limit { input, n } => {
            let (child, delivered) = reorder(*input, model, target);
            (
                Plan::Limit {
                    input: Box::new(child),
                    n,
                },
                delivered,
            )
        }
        Plan::EmptyScan { rels } => (Plan::EmptyScan { rels }, false),
        Plan::Scan { rel } => (Plan::Scan { rel }, false),
        Plan::Join { .. } => {
            let mut nodes = Vec::new();
            let mut edges = Vec::new();
            harvest(root, &mut nodes, &mut edges, model);
            dp(nodes, edges, model, target)
        }
    }
}

fn dp(
    nodes: Vec<Node>,
    edges: Vec<(ColRef, ColRef)>,
    model: &CostModel,
    target: Option<&[(ColRef, bool)]>,
) -> (Plan, bool) {
    assert!(
          nodes.len() <= 12 && nodes.iter().all(|n| n.rel < 64),
          "DP cap: >12 rels needs a greedy path (p2.md 'documented paranoia'); rel index must fit u64 bitset"
      );

    let mut table: HashMap<u64, Vec<Entry>> = HashMap::new();

    let full: u64 = nodes.iter().map(|n| 1 << n.rel).fold(0, |a, b| a | b);
    let rels_list: Vec<usize> = nodes.iter().map(|n| n.rel).collect();
    for n in nodes {
        // does THIS rel own every target column? (asked per node)
        let owns = target.is_some_and(|t| t.iter().all(|(c, _)| c.rel == n.rel));
        let mut list = Vec::new();
        if owns {
            let t = target.unwrap();
            // ordered variant: Sort below the join, paid for up front
            list.push(Entry {
                plan: Plan::Sort {
                    input: Box::new(n.plan.clone()),
                    keys: t.iter().map(|(c, d)| (Expr::Col(*c), *d)).collect(),
                },
                rows: n.rows,          // sorting drops nothing
                cost: n.cost + n.rows, // the Sort costs its input rows
                ordered: true,
            });
        }
        list.push(Entry {
            plan: n.plan,
            rows: n.rows,
            cost: n.cost,
            ordered: false,
        });
        table.insert(1u64 << n.rel, list);
    }

    for size in 2..=rels_list.len() {
        let mut subsets: Vec<u64> = table
            .keys()
            .copied()
            .filter(|s| s.count_ones() as usize == size - 1)
            .collect();
        subsets.sort();

        for s in subsets {
            for &r in &rels_list {
                if s & (1 << r) != 0 {
                    continue;
                }
                let mut keys: Vec<(ColRef, ColRef)> = Vec::new();
                for (a, b) in &edges {
                    if s & (1 << a.rel) != 0 && b.rel == r {
                        keys.push((*a, *b));
                    } else if s & (1 << b.rel) != 0 && a.rel == r {
                        keys.push((*b, *a));
                    }
                }
                if keys.is_empty() {
                    continue;
                }

                let r_entry = &table[&(1u64 << r)]
                    .iter()
                    .find(|e| !e.ordered)
                    .expect("Base Entry");

                let mut candidates: Vec<Entry> = Vec::new();
                for s_entry in &table[&s] {
                    let inputs = [s_entry.rows, r_entry.rows];
                    let base_cost = s_entry.cost + r_entry.cost;

                    let join = Plan::Join {
                        left: Box::new(s_entry.plan.clone()),
                        right: Box::new(r_entry.plan.clone()),
                        on: keys.clone(),
                        filter: None,
                    };

                    let rows = model.output_row(&join, &inputs);
                    let cost = base_cost + model.local_cost(&join, &inputs);

                    candidates.push(Entry {
                        plan: join,
                        rows,
                        cost,
                        ordered: s_entry.ordered,
                    });
                }

                let key = s | (1u64 << r);
                for c in candidates {
                    admit(table.entry(key).or_default(), c);
                }
            }
        }
    }
    let Some(list) = table.remove(&full) else {
        return fallback(table, edges, rels_list, model);
    };
    let (ord, mut unord): (Vec<Entry>, Vec<Entry>) = list.into_iter().partition(|e| e.ordered);
    unord.sort_by(|a, b| a.cost.total_cmp(&b.cost));
    let u = unord.swap_remove(0);
    match ord.into_iter().min_by(|a, b| a.cost.total_cmp(&b.cost)) {
        Some(o) if o.cost < u.cost + u.rows => (o.plan, true), // beats plan + root Sort
        _ => (u.plan, false),
    }
}

fn fallback(
    mut table: HashMap<u64, Vec<Entry>>,
    edges: Vec<(ColRef, ColRef)>,
    rels_list: Vec<usize>,
    model: &CostModel,
) -> (Plan, bool) {
    let mut comps: Vec<u64> = rels_list.iter().map(|&r| 1u64 << r).collect();
    for (a, b) in &edges {
        let ia = comps.iter().position(|m| m & (1 << a.rel) != 0).unwrap();
        let ib = comps.iter().position(|m| m & (1 << b.rel) != 0).unwrap();
        if ia == ib {
            continue;
        }

        let (lo, hi) = (ia.min(ib), ia.max(ib));
        let merged = comps.swap_remove(hi);
        comps[lo] |= merged;
    }

    let mut islands: Vec<Entry> = Vec::new();
    for c in &comps {
        let entries = table
            .remove(c)
            .unwrap()
            .into_iter()
            .filter(|e| !e.ordered)
            .min_by(|a, b| a.cost.total_cmp(&b.cost))
            .unwrap();
        islands.push(entries);
    }
    islands.sort_by(|a, b| a.rows.total_cmp(&b.rows));

    let mut islands = islands.into_iter();
    let mut acc = islands.next().expect("at least one island");
    for next in islands {
        let inputs = [acc.rows, next.rows];
        let base_cost = acc.cost + next.cost;
        let join = Plan::Join {
            left: Box::new(acc.plan),
            right: Box::new(next.plan),
            on: Vec::new(),
            filter: None,
        };
        let rows = model.output_row(&join, &inputs);
        let cost = base_cost + model.local_cost(&join, &inputs);
        acc = Entry {
            plan: join,
            rows,
            cost,
            ordered: false,
        };
    }
    (acc.plan, false)
}

fn admit(list: &mut Vec<Entry>, c: Entry) {
    if list
        .iter()
        .any(|e| e.cost <= c.cost && (e.ordered || !c.ordered))
    {
        return;
    }

    list.retain(|e| !(c.cost <= e.cost && (c.ordered || !e.ordered)));
    list.push(c);
}

struct Node {
    rel: usize,
    plan: Plan,
    rows: f64,
    cost: f64,
}
struct Entry {
    plan: Plan,
    rows: f64,
    cost: f64,
    ordered: bool,
}

fn harvest(p: Plan, nodes: &mut Vec<Node>, edges: &mut Vec<(ColRef, ColRef)>, model: &CostModel) {
    match p {
        Plan::Join {
            left, right, on, ..
        } => {
            harvest(*left, nodes, edges, model);
            harvest(*right, nodes, edges, model);
            edges.extend(on);
        }
        _ => {
            let (rows, cost) = model.plan_cost(&p);
            nodes.push(Node {
                rel: plan_rels(&p)[0],
                plan: p,
                rows,
                cost,
            });
        }
    }
}
