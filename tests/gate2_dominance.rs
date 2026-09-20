//! [scaffold] Gate 2 coupon (p2.md §Scaffold coupons): runs the p1 and
//! p2 pipelines over the corpus and exposes (rows, cost) pairs plus
//! executed row sets. The assertions themselves are YOURS — fill the
//! two `unimplemented!()` blocks (gate 2 items 2 and 3).

mod common;

use optimize::harness::{self, corpus};
use optimize::{bind, catalog, opt, stats};

/// For every corpus query: (name, heuristic (rows, cost), selinger (rows, cost)).
/// Costs come from the frozen model via `plan_cost` on each optimizer's output.
fn cost_pairs() -> Vec<(String, (f64, f64), (f64, f64))> {
    let cat = catalog::catalog();
    let octx = opt::OptContext {
        stats: stats::load_default().expect("stats load"),
    };
    let mut out = Vec::new();
    for q in corpus().expect("corpus loads") {
        let mut pair = Vec::new();
        for key in ["p1", "p2"] {
            let stmt = harness::parse_single(&q.sql).expect("parse");
            let ir = bind::bind(stmt, &cat).expect("bind");
            let entry = opt::find(key).expect("registered project");
            let ir = entry
                .optimizer
                .optimize(ir, &octx)
                .unwrap_or_else(|e| panic!("{}/{key}: optimize failed: {e:#}", q.name));
            let model = octx.cost_model(&ir.rels);
            pair.push(model.plan_cost(&ir.root));
        }
        out.push((q.name.clone(), pair[0], pair[1]));
    }
    out
}

/// Gate 2 item 2 — "the plan got cheaper":
/// cost(selinger) ≤ cost(heuristic) on every query, strictly < on q11–q18.
#[test]
fn dominance_selinger_never_costlier() {
    for (name, (_h_rows, h_cost), (_s_rows, s_cost)) in cost_pairs() {
        let join_reordered = name.as_str() >= "q11" && name.as_str() <= "q18";
        assert!(
            s_cost <= h_cost,
            "{name}: selinger got COSTLIER: {s_cost} vs heuristic {h_cost}"
        );
        if join_reordered {
            assert!(
                s_cost < h_cost,
                "{name}: selinger only tied ({s_cost}); DP found nothing on a join query"
            );
        }
    }
}

/// [scaffold] Ledger helper, not a gate test. Run with:
/// `cargo test --test gate2_dominance print_cost_table -- --ignored --nocapture`
#[test]
#[ignore]
fn print_cost_table() {
    for (name, (_, h_cost), (_, s_cost)) in cost_pairs() {
        println!("{name}: heuristic={h_cost} selinger={s_cost}");
    }
}

/// Gate 2 item 3 — "the answer didn't change": q11–q18 through the p2
/// pipeline equal the p1 pipeline column-for-column (the ColumnRemap gate).
#[test]
fn remap_correctness_under_reorder() {
    let rt = common::test_runtime();
    for q in corpus().expect("corpus loads") {
        if !(q.name.as_str() >= "q11" && q.name.as_str() <= "q18") {
            continue;
        }
        let ordered = q.has_order_by();
        let p1 = rt
            .block_on(harness::subject_rows("p1", &q.sql))
            .unwrap_or_else(|e| panic!("{}/p1: execution failed: {e:#}", q.name));
        let p2 = rt
            .block_on(harness::subject_rows("p2", &q.sql))
            .unwrap_or_else(|e| panic!("{}/p2: execution failed: {e:#}", q.name));
        let p1 = harness::normalize(p1, ordered);
        let p2 = harness::normalize(p2, ordered);
        let _ = (&p1, &p2);
        assert!(
            harness::diff_report(&q.name, &p1, &p2).is_none(),
            "{}: p2 rows differ from p1 rows after reorder",
            q.name
        );
    }
}
