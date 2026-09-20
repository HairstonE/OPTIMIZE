//! Gate 3 item 2 — "Same destination, principled vehicle."
//! For every corpus query, p3's rewrite fixpoint IR must be
//! STRUCTURALLY IDENTICAL to p1's full output. The oracle is p1
//! itself — the ladder testing itself (p3.md: no scaffold coupons).

use optimize::harness::{self, corpus};
use optimize::p3::rewrite_fixpoint;
use optimize::{bind, catalog, opt, stats};

#[test]
fn fixpoint_ir_equals_p1_on_every_query() {
    let cat = catalog::catalog();
    let octx = opt::OptContext {
        stats: stats::load_default().expect("stats load"),
    };
    let p1 = opt::find("p1").expect("registered project");
    let mut failures = Vec::new();

    for q in corpus().expect("corpus loads") {
        let bound = || {
            let stmt = harness::parse_single(&q.sql).expect("parse");
            bind::bind(stmt, &cat).expect("bind")
        };
        let p1_ir = p1
            .optimizer
            .optimize(bound(), &octx)
            .unwrap_or_else(|e| panic!("{}/p1: {e:#}", q.name));
        let p3_ir = rewrite_fixpoint(bound());

        let (a, b) = (p1_ir.to_string(), p3_ir.to_string());
        if a != b {
            failures.push(format!("{}:\n── p1 ──\n{a}\n── p3 fixpoint ──\n{b}", q.name));
        }
    }

    assert!(
        failures.is_empty(),
        "p3 fixpoint diverges from p1 on {} queries:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
