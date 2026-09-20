//! Gate 4 item 2 — cross-paradigm dominance: cost(volcano) ≤
//! cost(selinger) on every query. Any violation is a search bug BY
//! CONSTRUCTION: the memo's expanded space strictly contains p2's
//! left-deep space, and every shared plan prices identically under
//! the frozen model (hash join is a cost EXTENSION, never a change).

use optimize::harness::{self, corpus};
use optimize::{bind, catalog, opt, p4, stats};

/// For every corpus query: (name, volcano winner cost, selinger cost).
/// Selinger prices its logical output through the frozen `plan_cost`;
/// volcano reports its own winner cost (same model + the extension).
fn pairs() -> Vec<(String, f64, f64)> {
    let cat = catalog::catalog();
    let octx = opt::OptContext {
        stats: stats::load_default().expect("stats load"),
    };
    let mut out = Vec::new();
    for q in corpus().expect("corpus loads") {
        let stmt = harness::parse_single(&q.sql).expect("parse");
        let ir = bind::bind(stmt, &cat).expect("bind");
        let p2 = opt::find("p2")
            .expect("p2 registered")
            .optimizer
            .optimize(ir, &octx)
            .unwrap_or_else(|e| panic!("{}/p2: optimize failed: {e:#}", q.name));
        let s_cost = octx.cost_model(&p2.rels).plan_cost(&p2.root).1;

        let stmt = harness::parse_single(&q.sql).expect("parse");
        let ir = bind::bind(stmt, &cat).expect("bind");
        let (_winner, v_cost, _ir) = p4::optimize_physical(ir, &octx)
            .unwrap_or_else(|e| panic!("{}/p4: optimize failed: {e:#}", q.name));
        out.push((q.name.clone(), v_cost, s_cost));
    }
    out
}

#[test]
fn dominance_volcano_never_costlier() {
    for (name, v_cost, s_cost) in pairs() {
        assert!(
            v_cost <= s_cost,
            "{name}: volcano got COSTLIER: {v_cost} vs selinger {s_cost} — \
             a search bug by construction (the space contains left-deep)"
        );
    }
}

/// [scaffold-style] Ledger helper, not a gate test. Run with:
/// `cargo test --test gate4_dominance print_cost_table -- --ignored --nocapture`
#[test]
#[ignore]
fn print_cost_table() {
    for (name, v_cost, s_cost) in pairs() {
        println!("{name}: volcano={v_cost} selinger={s_cost}");
    }
}
