//! P4 — Volcano (1993). Spec: src/p4/p4.md
//!
//! The memo: groups of logically-equivalent expressions, transformation +
//! implementation rules, top-down optimize-group with required physical
//! properties and enforcers. Physical IR appears; lowering goes physical.

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use crate::common::cost::CostModel;
use crate::common::ir::{canon_expr, ColRef, Expr, Ir, Plan};
use crate::common::opt::{OptContext, Optimizer};
use crate::p1::{free_cols, plan_rels, HeuristicOptimizer};
use crate::p4::MExpr::Join;
use anyhow::Result;

pub struct VolcanoOptimizer;

type GroupId = usize;

impl Optimizer for VolcanoOptimizer {
    fn name(&self) -> &'static str {
        "volcano"
    }

    /// Decision 8: the trait body runs the same search as the physical
    /// side door and returns the winner reconstructed as logical IR.
    /// Execution and physical EXPLAIN go through `optimize_physical` +
    /// `lower_phys` instead — this Ir serves the generic logical
    /// surfaces (registry, gates, the p5 plan-equality comparison).
    fn optimize(&self, plan: Ir, ctx: &OptContext) -> Result<Ir> {
        let (winner, _cost, ir) = optimize_physical(plan, ctx)?;
        Ok(Ir {
            root: phys_to_ir(&winner),
            rels: ir.rels,
        })
    }
}

/// The physical entry point (decision 8's side door). Normalize with
/// p1 (like p2/p3 — the memo expects pushed filters and formed
/// equi-joins), peel the operators above the join search — the Sort
/// becomes the demanded property (decision 7) — harvest, expand to
/// fixpoint, pick the winner, and reassemble the peeled operators
/// around it. Returns the plan, its estimated cost, and the
/// normalized Ir (its rels drive lowering).
pub fn optimize_physical(ir: Ir, octx: &OptContext) -> Result<(PhysPlan, f64, Ir)> {
    let (plan, cost, ir, _) = optimize_physical_report(ir, octx)?;
    Ok((plan, cost, ir))
}

/// Memo forensics (gate 4 item 4): what the exhaustive search did.
/// `rule_apps` counts rule firings that produced a NEW expression —
/// the same "productive applications" p3's ledger column counts.
pub struct SearchReport {
    pub groups: usize,
    pub exprs: usize,
    pub rule_apps: usize,
}

/// `optimize_physical` plus the search's own numbers, for LEDGER.md.
pub fn optimize_physical_report(
    ir: Ir,
    octx: &OptContext,
) -> Result<(PhysPlan, f64, Ir, SearchReport)> {
    let ir = HeuristicOptimizer.optimize(ir, octx)?;
    let cm = octx.cost_model(&ir.rels);

    let mut wraps: Vec<&Plan> = Vec::new();
    let mut prop = Prop::Any;
    let mut cur: &Plan = &ir.root;
    loop {
        match cur {
            Plan::Sort { input, keys } if plan_rels(cur).len() > 1 => {
                prop = Prop::Sorted(keys.clone());
                cur = input;
            }
            Plan::Project { input, .. }
            | Plan::Filter { input, .. }
            | Plan::Limit { input, .. }
                if plan_rels(cur).len() > 1 =>
            {
                wraps.push(cur);
                cur = input;
            }
            _ => break,
        }
    }

    let mut memo = Memo {
        groups: Vec::new(),
        by_rels: HashMap::default(),
    };
    let root = build_memo(cur, &mut memo);
    let rule_apps = memo.expand();
    let report = SearchReport {
        groups: memo.groups.len(),
        exprs: memo.groups.iter().map(|g| g.exprs.len()).sum(),
        rule_apps,
    };
    let (mut plan, mut cost) = memo.best_plan(root, &prop, &cm);
    let mut rows = memo.group_rows(root, &cm);

    // Reassemble innermost-first. Each peeled node is a real Plan whose
    // frozen cost arms read only the input-row slice, so it prices
    // itself (the Sort was already priced inside the enforcer).
    for w in wraps.into_iter().rev() {
        cost += cm.local_cost(w, &[rows]);
        rows = cm.output_row(w, &[rows]);
        plan = match w {
            Plan::Project { exprs, .. } => PhysPlan::ProjectPhys {
                input: Box::new(plan),
                exprs: exprs.clone(),
            },
            Plan::Filter { predicate, .. } => PhysPlan::FilterPhys {
                input: Box::new(plan),
                predicate: predicate.clone(),
            },
            Plan::Limit { n, .. } => PhysPlan::LimitPhys {
                input: Box::new(plan),
                n: *n,
            },
            _ => unreachable!("only Project/Filter/Limit are peeled"),
        };
    }
    Ok((plan, cost, ir, report))
}

/// Lowering v2 (notes decision 4, resolved): walk the PhysPlan and
/// build the DataFusion logical plan in the winner's exact shape,
/// reusing lower v1's helpers for every non-join node. The one thing
/// v1 cannot express is OUR algorithm choice: its `join_on` leaves
/// equi-keys as a join filter, and the rule that promotes them to hash
/// keys lives in the logical optimizer we removed. So joins are built
/// here — HashJoinPhys becomes a keyed join (→ HashJoinExec, build =
/// left under CollectLeft), NestedLoopPhys a predicate-only join
/// (→ NestedLoopJoinExec). Single partition stays (SEAM-PARALLEL).
pub async fn lower_phys(
    winner: &PhysPlan,
    ir: &Ir,
    ctx: &datafusion::prelude::SessionContext,
) -> Result<datafusion::logical_expr::LogicalPlan> {
    let sources = crate::common::lower::table_sources(ir, ctx).await?;
    Ok(phys_node(winner, ir, &sources)?.build()?)
}

fn phys_node(
    p: &PhysPlan,
    ir: &Ir,
    sources: &[std::sync::Arc<dyn datafusion::logical_expr::TableSource>],
) -> Result<datafusion::logical_expr::LogicalPlanBuilder> {
    use crate::common::lower::{df_col, df_expr, lower_node};
    use datafusion::logical_expr::{Expr as DfExpr, JoinType, SortExpr};

    Ok(match p {
        // The leaf sources: identical to v1 — delegate through the twin.
        PhysPlan::ScanPhys { .. } | PhysPlan::EmptyScanPhys { .. } => {
            lower_node(&phys_to_ir(p), ir, sources)?
        }
        PhysPlan::FilterPhys { input, predicate } => {
            phys_node(input, ir, sources)?.filter(df_expr(predicate, ir))?
        }
        PhysPlan::ProjectPhys { input, exprs } => {
            let exprs: Vec<DfExpr> = exprs.iter().map(|e| df_expr(e, ir)).collect();
            phys_node(input, ir, sources)?.project(exprs)?
        }
        PhysPlan::SortPhys { input, keys } => {
            let keys: Vec<SortExpr> = keys
                .iter()
                .map(|(e, desc)| SortExpr::new(df_expr(e, ir), !*desc, *desc))
                .collect();
            phys_node(input, ir, sources)?.sort(keys)?
        }
        PhysPlan::LimitPhys { input, n } => {
            phys_node(input, ir, sources)?.limit(0, Some(*n as usize))?
        }
        PhysPlan::HashJoinPhys { build, probe, on } => {
            // build = left input: CollectLeft hashes the left side.
            let l = phys_node(build, ir, sources)?;
            let r = phys_node(probe, ir, sources)?.build()?;
            let brels = phys_rels(build);
            let (mut lk, mut rk) = (Vec::new(), Vec::new());
            for (a, b) in on {
                let (bk, pk) = if brels.contains(&a.rel) { (a, b) } else { (b, a) };
                lk.push(df_column(*bk, ir));
                rk.push(df_column(*pk, ir));
            }
            l.join(r, JoinType::Inner, (lk, rk), None)?
        }
        PhysPlan::NestedLoopPhys {
            left,
            right,
            on,
            filter,
        } => {
            let l = phys_node(left, ir, sources)?;
            let r = phys_node(right, ir, sources)?.build()?;
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
    })
}

/// ColRef → qualified DataFusion Column (join-key form of lower v1's
/// df_col, which wraps the same lookup in an Expr).
fn df_column(c: ColRef, ir: &Ir) -> datafusion::common::Column {
    let rel = &ir.rels[c.rel];
    let name = crate::common::catalog::columns_for(&rel.table)[c.col].0;
    datafusion::common::Column::new(Some(rel.qualifier.clone()), name)
}

/// The base relations under a physical subtree — sides for key
/// orientation (canon_on erased them).
fn phys_rels(p: &PhysPlan) -> BTreeSet<usize> {
    match p {
        PhysPlan::ScanPhys { rel } => [*rel].into(),
        PhysPlan::EmptyScanPhys { rels } => rels.iter().copied().collect(),
        PhysPlan::FilterPhys { input, .. }
        | PhysPlan::ProjectPhys { input, .. }
        | PhysPlan::SortPhys { input, .. }
        | PhysPlan::LimitPhys { input, .. } => phys_rels(input),
        PhysPlan::HashJoinPhys { build, probe, .. } => {
            phys_rels(build).union(&phys_rels(probe)).copied().collect()
        }
        PhysPlan::NestedLoopPhys { left, right, .. } => {
            phys_rels(left).union(&phys_rels(right)).copied().collect()
        }
    }
}

/// Harvest: a p1-normalized join tree into the memo WITH its real
/// predicates (decision 6 — arguments live inside the expressions).
fn build_memo(p: &Plan, memo: &mut Memo) -> GroupId {
    match p {
        Plan::Join {
            left,
            right,
            on,
            filter,
        } => {
            let l = build_memo(left, memo);
            let r = build_memo(right, memo);
            memo.intern(MExpr::Join {
                l,
                r,
                on: canon_on(on.clone()),
                filter: filter.clone(),
            })
            .0
        }
        Plan::Project { input, .. }
        | Plan::Filter { input, .. }
        | Plan::Sort { input, .. }
        | Plan::Limit { input, .. }
            if plan_rels(p).len() > 1 =>
        {
            build_memo(input, memo)
        }
        _ => memo.intern(MExpr::Leaf(p.clone())).0,
    }
}

/// Decision 8: winner → logical IR for the frozen trait. Mechanical
/// reverse-twin mapping; both join algorithms collapse to Plan::Join.
fn phys_to_ir(p: &PhysPlan) -> Plan {
    match p {
        PhysPlan::ScanPhys { rel } => Plan::Scan { rel: *rel },
        PhysPlan::EmptyScanPhys { rels } => Plan::EmptyScan { rels: rels.clone() },
        PhysPlan::FilterPhys { input, predicate } => Plan::Filter {
            input: Box::new(phys_to_ir(input)),
            predicate: predicate.clone(),
        },
        PhysPlan::ProjectPhys { input, exprs } => Plan::Project {
            input: Box::new(phys_to_ir(input)),
            exprs: exprs.clone(),
        },
        PhysPlan::SortPhys { input, keys } => Plan::Sort {
            input: Box::new(phys_to_ir(input)),
            keys: keys.clone(),
        },
        PhysPlan::LimitPhys { input, n } => Plan::Limit {
            input: Box::new(phys_to_ir(input)),
            n: *n,
        },
        // Convention (notes decision 8): probe = left/outer, build =
        // right/inner — the frozen Join cost's roles.
        PhysPlan::HashJoinPhys { build, probe, on } => {
            rebuild_join(phys_to_ir(probe), phys_to_ir(build), on, &None)
        }
        PhysPlan::NestedLoopPhys {
            left,
            right,
            on,
            filter,
        } => rebuild_join(phys_to_ir(left), phys_to_ir(right), on, filter),
    }
}

/// `canon_on` erased sides, and logical Join `on` pairs read
/// (left column, right column) — re-orient by the left child's rels.
fn rebuild_join(left: Plan, right: Plan, on: &[(ColRef, ColRef)], filter: &Option<Expr>) -> Plan {
    let lrels: BTreeSet<usize> = plan_rels(&left).into_iter().collect();
    let on = on
        .iter()
        .map(|&(a, b)| if lrels.contains(&a.rel) { (a, b) } else { (b, a) })
        .collect();
    Plan::Join {
        left: Box::new(left),
        right: Box::new(right),
        on,
        filter: filter.clone(),
    }
}

/// Memo expressions carry their arguments (decision 6: paper-faithful —
/// Volcano expressions are operators WITH arguments). Equality is dedup,
/// so `on` must ALWAYS be in `canon_on` form and `filter` in P0
/// canonical form before an expression is built.
#[derive(Clone, PartialEq, Eq)]
enum MExpr {
    Leaf(Plan),
    Join {
        l: GroupId,
        r: GroupId,
        on: Vec<(ColRef, ColRef)>,
        filter: Option<Expr>,
    },
}

/// Required physical property (paper §2: goals are (expression,
/// property, limit); our property vector has one dimension — order).
/// Decision 7: the harvest consumes the logical Sort and demands its
/// keys here instead.
#[derive(Debug, Clone, PartialEq)]
enum Prop {
    Any,
    Sorted(Vec<(Expr, /*desc:*/ bool)>),
}

struct Group {
    rels: BTreeSet<usize>,
    exprs: Vec<MExpr>,
    /// One winner per demanded property (paper: Selinger's "interesting
    /// orders" generalized). Tiny per group — a Vec beats a map.
    winners: Vec<(Prop, PhysPlan, f64)>,
}

struct Memo {
    groups: Vec<Group>,
    by_rels: HashMap<BTreeSet<usize>, GroupId>,
}

/// Physical algebra (Volcano 1993 design decision 1: two algebras;
/// our notes.md decision 2: full twins, separate enum). Lives BESIDE
/// the frozen logical `Plan` — p1–p3 never see this type. One
/// `SortPhys` serves both the ORDER-BY role and the enforcer role
/// (paper §2.2: the enforcer IS the sort operator).
///
/// Cost extensions to the frozen P2 model (extension, not change):
///   cost(HashJoinPhys)   = rows(build) + rows(probe)
///   cost(NestedLoopPhys) = unchanged frozen Join entry
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhysPlan {
    ScanPhys {
        rel: usize,
    },
    EmptyScanPhys {
        rels: Vec<usize>,
    },
    FilterPhys {
        input: Box<PhysPlan>,
        predicate: Expr,
    },
    ProjectPhys {
        input: Box<PhysPlan>,
        exprs: Vec<Expr>,
    },
    /// Build side is hashed in full, probe side streams (build = the
    /// smaller input by the cost model's row estimate).
    HashJoinPhys {
        build: Box<PhysPlan>,
        probe: Box<PhysPlan>,
        on: Vec<(ColRef, ColRef)>,
    },
    NestedLoopPhys {
        left: Box<PhysPlan>,
        right: Box<PhysPlan>,
        on: Vec<(ColRef, ColRef)>,
        filter: Option<Expr>,
    },
    SortPhys {
        input: Box<PhysPlan>,
        keys: Vec<(Expr, /*desc:*/ bool)>,
    },
    LimitPhys {
        input: Box<PhysPlan>,
        n: u64,
    },
}

// Display mirrors the frozen logical rendering (one node per line,
// two-space indent) so physical EXPLAIN diffs read side-by-side with
// p1–p3 snapshots.
impl fmt::Display for PhysPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_at(f, 0)
    }
}

impl PhysPlan {
    fn fmt_at(&self, f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
        let pad = "  ".repeat(depth);
        match self {
            PhysPlan::ScanPhys { rel } => writeln!(f, "{pad}ScanPhys #{rel}"),
            PhysPlan::EmptyScanPhys { rels } => {
                let list: Vec<String> = rels.iter().map(|r| format!("#{r}")).collect();
                writeln!(f, "{pad}EmptyScanPhys [{}]", list.join(", "))
            }
            PhysPlan::FilterPhys { input, predicate } => {
                writeln!(f, "{pad}FilterPhys {predicate}")?;
                input.fmt_at(f, depth + 1)
            }
            PhysPlan::ProjectPhys { input, exprs } => {
                let list: Vec<String> = exprs.iter().map(|e| e.to_string()).collect();
                writeln!(f, "{pad}ProjectPhys [{}]", list.join(", "))?;
                input.fmt_at(f, depth + 1)
            }
            PhysPlan::HashJoinPhys { build, probe, on } => {
                let pairs: Vec<String> = on.iter().map(|(l, r)| format!("{l} = {r}")).collect();
                writeln!(
                    f,
                    "{pad}HashJoinPhys on=[{}] (build, probe)",
                    pairs.join(", ")
                )?;
                build.fmt_at(f, depth + 1)?;
                probe.fmt_at(f, depth + 1)
            }
            PhysPlan::NestedLoopPhys {
                left,
                right,
                on,
                filter,
            } => {
                let pairs: Vec<String> = on.iter().map(|(l, r)| format!("{l} = {r}")).collect();
                match filter {
                    Some(pred) => writeln!(
                        f,
                        "{pad}NestedLoopPhys on=[{}] filter={pred}",
                        pairs.join(", ")
                    )?,
                    None => writeln!(f, "{pad}NestedLoopPhys on=[{}]", pairs.join(", "))?,
                }
                left.fmt_at(f, depth + 1)?;
                right.fmt_at(f, depth + 1)
            }
            PhysPlan::SortPhys { input, keys } => {
                let list: Vec<String> = keys
                    .iter()
                    .map(|(e, desc)| format!("{e} {}", if *desc { "DESC" } else { "ASC" }))
                    .collect();
                writeln!(f, "{pad}SortPhys [{}]", list.join(", "))?;
                input.fmt_at(f, depth + 1)
            }
            PhysPlan::LimitPhys { input, n } => {
                writeln!(f, "{pad}LimitPhys {n}")?;
                input.fmt_at(f, depth + 1)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bind, catalog, harness, opt, stats};

    /// A corpus query through p1: the normalized logical IR the memo
    /// will hold.
    fn p1_ir(name: &str) -> Ir {
        let q = harness::corpus()
            .expect("corpus")
            .into_iter()
            .find(|q| q.name == name)
            .expect("query in corpus");
        let stmt = harness::parse_single(&q.sql).expect("parse");
        let ir = bind::bind(stmt, &catalog::catalog()).expect("bind");
        let octx = opt::OptContext {
            stats: stats::load_default().expect("stats"),
        };
        opt::find("p1")
            .expect("p1 registered")
            .optimizer
            .optimize(ir, &octx)
            .expect("p1 optimize")
    }

    fn q11_p1() -> Plan {
        p1_ir("q11").root
    }

    /// Collect the maximal non-Join subtrees under `p` — the memo leaves.
    fn shred(p: &Plan, out: &mut Vec<Plan>) {
        match p {
            Plan::Join { left, right, .. } => {
                shred(left, out);
                shred(right, out);
            }
            // Everything above the top join (Project/Sort/Limit) is not
            // part of the join search; step through it.
            Plan::Project { input, .. }
            | Plan::Filter { input, .. }
            | Plan::Sort { input, .. }
            | Plan::Limit { input, .. }
                if plan_rels(p).len() > 1 =>
            {
                shred(input, out)
            }
            _ => out.push(p.clone()),
        }
    }

    /// Hand-built join with an empty (canonical) predicate set —
    /// structural tests only; real predicates arrive at harvest time.
    fn jn(l: GroupId, r: GroupId) -> MExpr {
        MExpr::Join {
            l,
            r,
            on: canon_on(Vec::new()),
            filter: None,
        }
    }

    #[test]
    fn intern_dedups_hand_inserted_q11() {
        let mut leaves = Vec::new();
        shred(&q11_p1(), &mut leaves);
        assert_eq!(leaves.len(), 3, "q11 joins three relations");

        let mut memo = Memo {
            groups: Vec::new(),
            by_rels: HashMap::default(),
        };
        let g: Vec<GroupId> = leaves
            .into_iter()
            .map(|p| memo.intern(MExpr::Leaf(p)).0)
            .collect();
        assert_eq!(memo.groups.len(), 3, "three leaf groups");

        // Hand-insert a join, then its commutation: SAME group, two exprs.
        let (ab, new_ab) = memo.intern(jn(g[0], g[1]));
        let (ba, new_ba) = memo.intern(jn(g[1], g[0]));
        assert_eq!(ab, ba, "commuted join lands in the same group");
        assert!(new_ab && new_ba, "both orientations reported as new");
        assert_eq!(memo.groups.len(), 4, "no duplicate group was born");
        assert_eq!(memo.groups[ab].exprs.len(), 2, "both orientations kept");

        // Re-insert an existing expression: nothing changes anywhere.
        let (again, new_again) = memo.intern(jn(g[0], g[1]));
        assert_eq!(again, ab);
        assert!(!new_again, "re-insert reported as not new");
        assert_eq!(memo.groups[ab].exprs.len(), 2, "duplicate expr deduped");

        // The 3-way root group, reached two different ways: one id.
        let top1 = memo.intern(jn(ab, g[2])).0;
        let bc = memo.intern(jn(g[1], g[2])).0;
        let top2 = memo.intern(jn(g[0], bc)).0;
        assert_eq!(top1, top2, "different shapes of {{0,1,2}} share a group");
        assert_eq!(
            memo.groups.len(),
            6,
            "groups: 3 leaves + {{0,1}} + {{1,2}} + {{0,1,2}}"
        );
    }

    #[test]
    fn t1_flips_every_join_then_goes_quiet() {
        let mut leaves = Vec::new();
        shred(&q11_p1(), &mut leaves);
        let mut memo = Memo {
            groups: Vec::new(),
            by_rels: HashMap::default(),
        };
        let g: Vec<GroupId> = leaves
            .into_iter()
            .map(|p| memo.intern(MExpr::Leaf(p)).0)
            .collect();

        // One join in one orientation only.
        let ab = memo.intern(jn(g[0], g[1])).0;
        assert_eq!(memo.groups[ab].exprs.len(), 1);

        // Pass 1 adds exactly the flip, into the SAME group.
        assert_eq!(memo.apply_commutativity(), 1, "one new expression");
        assert_eq!(memo.groups.len(), 4, "no new group from T1");
        assert_eq!(memo.groups[ab].exprs.len(), 2, "both orientations present");

        // Pass 2 is the fixpoint signal: everything already known.
        assert_eq!(memo.apply_commutativity(), 0, "T1 goes quiet");
    }

    /// Builds the q11 memo with the left-deep tree only:
    /// leaves g0,g1,g2, group {0,1}, and root (g0⋈g1)⋈g2.
    fn left_deep_memo() -> (Memo, Vec<GroupId>) {
        let mut leaves = Vec::new();
        shred(&q11_p1(), &mut leaves);
        let mut memo = Memo {
            groups: Vec::new(),
            by_rels: HashMap::default(),
        };
        let g: Vec<GroupId> = leaves
            .into_iter()
            .map(|p| memo.intern(MExpr::Leaf(p)).0)
            .collect();
        let ab = memo.intern(jn(g[0], g[1])).0;
        memo.intern(jn(ab, g[2]));
        (memo, g)
    }

    #[test]
    fn t2_interns_the_inner_group_then_goes_quiet() {
        let (mut memo, _) = left_deep_memo();
        assert_eq!(memo.groups.len(), 5, "3 leaves + {{0,1}} + {{0,1,2}}");

        // Pass 1 rewrites (g0⋈g1)⋈g2 → g0⋈(g1⋈g2): the inner join
        // {1,2} is a NEW group (born via intern), the outer expression
        // joins the existing root group. Two new expressions total.
        assert_eq!(memo.apply_associativity(), 2, "inner + outer are new");
        assert_eq!(memo.groups.len(), 6, "exactly one new group: {{1,2}}");

        // Pass 2: the same triple is generated again, intern dedups both.
        assert_eq!(memo.apply_associativity(), 0, "T2 goes quiet");
    }

    #[test]
    fn expansion_fixpoint_fills_the_q11_space() {
        let (mut memo, g) = left_deep_memo();

        // Run both rules to fixpoint.
        loop {
            let added = memo.apply_commutativity() + memo.apply_associativity();
            if added == 0 {
                break;
            }
        }

        // Expected memo for 3 relations (computed, not measured):
        //   groups: 3 leaves + {0,1} + {0,2} + {1,2} + {0,1,2} = 7
        //   exprs:  3 leaf exprs
        //         + 2 per pair group (both orientations)  = 6
        //         + root: 3 pair-partners × 2 orientations = 6
        //   total 15 — vs 12 distinct TREES: groups share subtrees,
        //   so the memo holds the whole space in fewer pieces.
        assert_eq!(memo.groups.len(), 7, "all relation subsets present");
        let total_exprs: usize = memo.groups.iter().map(|gr| gr.exprs.len()).sum();
        assert_eq!(total_exprs, 15, "3 + 3×2 + 6 expressions");

        let root = memo
            .by_rels
            .iter()
            .find(|(rels, _)| rels.len() == 3)
            .map(|(_, id)| *id)
            .expect("root group {0,1,2} exists");
        assert_eq!(memo.groups[root].exprs.len(), 6, "root: every 2-way split");
        // Leaves untouched by expansion: still one expr each.
        for id in g {
            assert_eq!(memo.groups[id].exprs.len(), 1, "leaf groups stay single");
        }
    }

    /// Step through the ops above the top join (Project/Sort/Limit).
    fn top_join(p: &Plan) -> &Plan {
        match p {
            Plan::Join { .. } => p,
            Plan::Project { input, .. }
            | Plan::Filter { input, .. }
            | Plan::Sort { input, .. }
            | Plan::Limit { input, .. } => top_join(input),
            _ => p,
        }
    }

    #[test]
    fn best_plan_q11_winner_beats_all_nested_loops() {
        let ir = p1_ir("q11");
        let mut memo = Memo {
            groups: Vec::new(),
            by_rels: HashMap::default(),
        };
        let root = build_memo(&ir.root, &mut memo);
        memo.expand();

        let st = stats::load_default().expect("stats");
        let cm = CostModel {
            stats: &st,
            rels: &ir.rels,
        };
        let (winner, cost) = memo.best_plan(root, &Prop::Any, &cm);

        // Baseline: p1's own join tree priced by the frozen model — an
        // all-nested-loop left-deep plan. The searched space CONTAINS
        // that tree, so the winner can never cost more.
        let baseline = cm.plan_cost(top_join(&ir.root)).1;
        assert!(
            cost <= baseline,
            "winner {cost} must not exceed the p1 baseline {baseline}"
        );

        // q11 has real equi-keys, and hash join prices linear vs the
        // nested loop's quadratic term: it must appear in the winner.
        let rendered = winner.to_string();
        assert!(
            rendered.contains("HashJoinPhys"),
            "expected a hash join in the winner:\n{rendered}"
        );

        // Winner slot memoizes: the second call answers identically.
        let (_, cost2) = memo.best_plan(root, &Prop::Any, &cm);
        assert_eq!(cost, cost2, "second call served from the winner slot");
    }

    #[test]
    fn sorted_demand_enforces_on_top_of_the_any_winner() {
        let ir = p1_ir("q11");
        let mut memo = Memo {
            groups: Vec::new(),
            by_rels: HashMap::default(),
        };
        let root = build_memo(&ir.root, &mut memo);
        memo.expand();

        let st = stats::load_default().expect("stats");
        let cm = CostModel {
            stats: &st,
            rels: &ir.rels,
        };

        let (any_plan, any_cost) = memo.best_plan(root, &Prop::Any, &cm);
        let key: Vec<(Expr, bool)> = vec![(
            Expr::Col(ColRef { rel: 0, col: 0 }),
            false,
        )];
        let sorted = Prop::Sorted(key.clone());
        let (plan, cost) = memo.best_plan(root, &sorted, &cm);

        // The enforcer is the ONLY sorted candidate: SortPhys on top of
        // the unchanged Any winner, at Any cost + one pass over the rows.
        match &plan {
            PhysPlan::SortPhys { input, keys } => {
                assert_eq!(**input, any_plan, "child IS the Any winner");
                assert_eq!(*keys, key, "enforcer carries the demanded keys");
            }
            other => panic!("expected SortPhys at the top, got:\n{other}"),
        }
        let rows = memo.group_rows(root, &cm);
        assert_eq!(cost, any_cost + rows, "cost = child + frozen Sort entry");

        // Two separate slots on one group: Any is not overwritten.
        let (_, any_again) = memo.best_plan(root, &Prop::Any, &cm);
        assert_eq!(any_again, any_cost, "Any winner survives untouched");
        let (_, sorted_again) = memo.best_plan(root, &sorted, &cm);
        assert_eq!(sorted_again, cost, "Sorted winner memoized");
    }

    #[test]
    fn q20_delivers_order_via_the_enforcer_end_to_end() {
        // q20: ORDER BY on the join key. Decision 7: the harvest turns
        // the Sort into a demand; the search re-inserts it as SortPhys.
        let ir = p1_ir("q20");
        let octx = opt::OptContext {
            stats: stats::load_default().expect("stats"),
        };
        let (plan, cost, _) = optimize_physical(ir, &octx).expect("p4 physical");
        let rendered = plan.to_string();
        assert!(
            rendered.contains("SortPhys"),
            "order must come from the explicit enforcer:\n{rendered}"
        );
        assert!(
            rendered.contains("HashJoinPhys"),
            "q20's equi-join should hash:\n{rendered}"
        );
        assert!(cost.is_finite() && cost > 0.0, "priced plan");

        // Decision 8: the trait body reconstructs the same winner
        // logically — Sort explicit, joins collapsed to Plan::Join.
        let logical = VolcanoOptimizer
            .optimize(p1_ir("q20"), &octx)
            .expect("trait body");
        let text = logical.to_string();
        assert!(text.contains("Sort"), "logical twin keeps the sort:\n{text}");
        assert!(text.contains("Join"), "logical twin keeps the join:\n{text}");
    }

    /// [ledger helper] Memo forensics per corpus query. Run with:
    /// `cargo test p4 --lib print_memo_forensics -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_memo_forensics() {
        let octx = opt::OptContext {
            stats: stats::load_default().expect("stats"),
        };
        for q in harness::corpus().expect("corpus") {
            let stmt = harness::parse_single(&q.sql).expect("parse");
            let ir = bind::bind(stmt, &catalog::catalog()).expect("bind");
            let octx_ref = &octx;
            let (_, _, _, r) = optimize_physical_report(ir, octx_ref)
                .unwrap_or_else(|e| panic!("{}: {e:#}", q.name));
            println!(
                "{}: groups={} exprs={} rule_apps={}",
                q.name, r.groups, r.exprs, r.rule_apps
            );
        }
    }
}

impl Memo {
    fn rels_of(&self, e: &MExpr) -> BTreeSet<usize> {
        match e {
            MExpr::Leaf(p) => plan_rels(p).into_iter().collect(),
            MExpr::Join { l, r, .. } => self.groups[*l]
                .rels
                .union(&self.groups[*r].rels)
                .copied()
                .collect(),
        }
    }

    fn intern(&mut self, e: MExpr) -> (GroupId, bool) {
        let rels = self.rels_of(&e);
        match self.by_rels.get(&rels) {
            Some(&g) => {
                if !self.groups[g].exprs.contains(&e) {
                    self.groups[g].exprs.push(e);
                    return (g, true);
                } else {
                    return (g, false);
                }
            }
            None => {
                self.groups.push(Group {
                    rels: rels.clone(),
                    exprs: vec![e],
                    winners: Vec::new(),
                });
                let g = self.groups.len() - 1;
                self.by_rels.insert(rels, g);
                return (g, true);
            }
        }
    }

    fn apply_commutativity(&mut self) -> usize {
        let mut pending: Vec<MExpr> = Vec::new();

        for g in &self.groups {
            for e in &g.exprs {
                match e {
                    Join { l, r, on, filter } => pending.push(Join {
                        l: *r,
                        r: *l,
                        on: on.clone(),
                        filter: filter.clone(),
                    }),
                    _ => continue,
                }
            }
        }
        let mut count = 0;
        for pe in pending {
            match self.intern(pe) {
                (_, true) => count += 1,
                _ => continue,
            }
        }
        count
    }

    fn apply_associativity(&mut self) -> usize {
        let mut pending: Vec<(GroupId, GroupId, GroupId, Vec<(ColRef, ColRef)>, Vec<Expr>)> =
            Vec::new();

        for g in &self.groups {
            for e in &g.exprs {
                if let MExpr::Join { l, r, on, filter } = e {
                    for inner_expr in &self.groups[*l].exprs {
                        if let MExpr::Join {
                            l: x,
                            r: y,
                            on: on_i,
                            filter: filter_i,
                        } = inner_expr
                        {
                            let mut on_pool = on.clone();
                            on_pool.extend(on_i.iter().copied());

                            let mut filter_pool = Vec::new();
                            for f in [filter, filter_i].into_iter().flatten() {
                                match f {
                                    Expr::And(cs) => filter_pool.extend(cs.iter().cloned()),
                                    other => filter_pool.push(other.clone()),
                                }
                            }

                            pending.push((*x, *y, *r, on_pool, filter_pool));
                        }
                    }
                }
            }
        }

        let mut count = 0;
        for (x, y, z, on_pool, filter_pool) in pending {
            let yz: BTreeSet<usize> = self.groups[y]
                .rels
                .union(&self.groups[z].rels)
                .copied()
                .collect();

            let (mut on_in, mut on_out) = (Vec::new(), Vec::new());
            for (a, b) in on_pool {
                let spans_yz = (self.groups[y].rels.contains(&a.rel)
                    && self.groups[z].rels.contains(&b.rel))
                    || (self.groups[y].rels.contains(&b.rel)
                        && self.groups[z].rels.contains(&a.rel));
                if spans_yz {
                    on_in.push((a, b));
                } else {
                    on_out.push((a, b));
                }
            }

            let (mut f_in, mut f_out) = (Vec::new(), Vec::new());
            for c in filter_pool {
                if free_cols(&c).iter().all(|cr| yz.contains(&cr.rel)) {
                    f_in.push(c);
                } else {
                    f_out.push(c);
                }
            }
            let rebuild = |v: Vec<Expr>| -> Option<Expr> {
                if v.is_empty() {
                    None
                } else {
                    Some(canon_expr(Expr::And(v)))
                }
            };

            let (inner, new_inner) = self.intern(MExpr::Join {
                l: y,
                r: z,
                on: canon_on(on_in),
                filter: rebuild(f_in),
            });
            let (_, new_outer) = self.intern(MExpr::Join {
                l: x,
                r: inner,
                on: canon_on(on_out),
                filter: rebuild(f_out),
            });
            count += new_inner as usize;
            count += new_outer as usize;
        }
        count
    }

    /// Estimated output rows of a group. Cardinality belongs to the
    /// relation set + predicates (cost.rs decision), and T2 preserves
    /// the predicate pool — so every expr in a group agrees and the
    /// first one is as good as any.
    fn group_rows(&self, g: GroupId, cm: &CostModel) -> f64 {
        match &self.groups[g].exprs[0] {
            MExpr::Leaf(p) => cm.plan_cost(p).0,
            MExpr::Join { l, r, on, filter } => {
                let rows_l = self.group_rows(*l, cm);
                let rows_r = self.group_rows(*r, cm);
                // Throwaway op: the frozen Join arm reads only on/filter
                // and the input-row slice, never the children.
                let op = Plan::Join {
                    left: Box::new(Plan::EmptyScan { rels: vec![] }),
                    right: Box::new(Plan::EmptyScan { rels: vec![] }),
                    on: on.clone(),
                    filter: filter.clone(),
                };
                cm.output_row(&op, &[rows_l, rows_r])
            }
        }
    }

    /// Implementation rules + winner selection: the cheapest physical
    /// plan for (group, required property). Recursive; memoized per
    /// property in the group's winner list.
    fn best_plan(&mut self, g: GroupId, prop: &Prop, cm: &CostModel) -> (PhysPlan, f64) {
        if let Some((_, p, c)) = self.groups[g].winners.iter().find(|(pr, ..)| pr == prop) {
            return (p.clone(), *c);
        }

        // Enforcer (paper §2.2: the sort operator, inserted to deliver a
        // demanded property). None of our join algorithms produces order,
        // so a Sorted demand has exactly one candidate: enforce on top of
        // the cheapest order-free plan. The child demand shrinks
        // Sorted → Any — that is what keeps enforcers from looping.
        if let Prop::Sorted(keys) = prop {
            let (child, child_cost) = self.best_plan(g, &Prop::Any, cm);
            let rows = self.group_rows(g, cm);
            let op = Plan::Sort {
                input: Box::new(Plan::EmptyScan { rels: vec![] }),
                keys: keys.clone(),
            };
            let cost = child_cost + cm.local_cost(&op, &[rows]);
            let plan = PhysPlan::SortPhys {
                input: Box::new(child),
                keys: keys.clone(),
            };
            self.groups[g]
                .winners
                .push((prop.clone(), plan.clone(), cost));
            return (plan, cost);
        }
        let exprs = self.groups[g].exprs.clone();
        let mut best: Option<(PhysPlan, f64)> = None;
        for e in exprs {
            let candidates: Vec<(PhysPlan, f64)> = match &e {
                MExpr::Leaf(p) => {
                    let (_rows, cost) = cm.plan_cost(p);
                    vec![(lower_leaf(p), cost)]
                }
                MExpr::Join { l, r, on, filter } => {
                    let (pl, cl) = self.best_plan(*l, &Prop::Any, cm);
                    let (pr, cr) = self.best_plan(*r, &Prop::Any, cm);
                    let rows_l = self.group_rows(*l, cm);
                    let rows_r = self.group_rows(*r, cm);

                    // Implementation rule 1: NestedLoopPhys, frozen cost.
                    let op = Plan::Join {
                        left: Box::new(Plan::EmptyScan { rels: vec![] }),
                        right: Box::new(Plan::EmptyScan { rels: vec![] }),
                        on: on.clone(),
                        filter: filter.clone(),
                    };
                    let nl = PhysPlan::NestedLoopPhys {
                        left: Box::new(pl.clone()),
                        right: Box::new(pr.clone()),
                        on: on.clone(),
                        filter: filter.clone(),
                    };
                    let mut cands =
                        vec![(nl, cl + cr + cm.local_cost(&op, &[rows_l, rows_r]))];

                    // Implementation rule 2: HashJoinPhys, extension cost
                    // rows(build) + rows(probe). Applicability: real keys
                    // only, and no residual filter (our variant carries
                    // none). A keyless hash join is not a thing.
                    if !on.is_empty() && filter.is_none() {
                        let (build, probe) = if rows_l <= rows_r {
                            (pl, pr)
                        } else {
                            (pr, pl)
                        };
                        let hj = PhysPlan::HashJoinPhys {
                            build: Box::new(build),
                            probe: Box::new(probe),
                            on: on.clone(),
                        };
                        cands.push((hj, cl + cr + rows_l + rows_r));
                    }
                    cands
                }
            };
            for c in candidates {
                if best.as_ref().is_none_or(|b| c.1 < b.1) {
                    best = Some(c);
                }
            }
        }
        let w = best.expect("expanded group always holds an expr");
        self.groups[g]
            .winners
            .push((Prop::Any, w.0.clone(), w.1));
        w
    }

    fn expand(&mut self) -> usize {
        let mut total = 0;
        loop {
            let added = self.apply_commutativity() + self.apply_associativity();
            if added == 0 {
                break;
            }
            total += added;
        }
        return total;
    }
}

/// Mechanical logical→physical twin mapping for the single-way
/// operators (decision 2: one implementation rule each). Joins never
/// reach here — the memo implements them via `best_plan`, where the
/// algorithm CHOICE (hash vs nested-loop) lives.
fn lower_leaf(p: &Plan) -> PhysPlan {
    match p {
        Plan::Scan { rel } => PhysPlan::ScanPhys { rel: *rel },
        Plan::EmptyScan { rels } => PhysPlan::EmptyScanPhys { rels: rels.clone() },
        Plan::Filter { input, predicate } => PhysPlan::FilterPhys {
            input: Box::new(lower_leaf(input)),
            predicate: predicate.clone(),
        },
        Plan::Project { input, exprs } => PhysPlan::ProjectPhys {
            input: Box::new(lower_leaf(input)),
            exprs: exprs.clone(),
        },
        Plan::Sort { input, keys } => PhysPlan::SortPhys {
            input: Box::new(lower_leaf(input)),
            keys: keys.clone(),
        },
        Plan::Limit { input, n } => PhysPlan::LimitPhys {
            input: Box::new(lower_leaf(input)),
            n: *n,
        },
        Plan::Join { .. } => unreachable!("joins are implemented via the memo, not lowered as leaves"),
    }
}

fn canon_on(mut on: Vec<(ColRef, ColRef)>) -> Vec<(ColRef, ColRef)> {
    for pair in &mut on {
        if pair.1 < pair.0 {
            std::mem::swap(&mut pair.0, &mut pair.1);
        }
    }
    on.sort();
    on
}
