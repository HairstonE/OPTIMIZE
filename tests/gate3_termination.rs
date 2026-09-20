//! Gate 3 item 3 — the seeded ping-pong. SPLITAND and MERGEFILTER are
//! exact inverses; decision 3 exiled them to separate classes. This test
//! runs them in ONE class on purpose (p3::seeded_ping_pong) and asserts
//! the engine's promises: the budget trips, the diagnostic names the
//! guilty pair with its alternating tail, and the tree comes out VALID —
//! a budget stop lands between atomic rules, never inside one.

use optimize::p3::seeded_ping_pong;

#[test]
fn ping_pong_pair_halts_at_budget_with_diagnostic() {
    let (tree, tripped, report) = seeded_ping_pong(60);

    assert!(tripped, "inverse rules in one class must exhaust the budget");

    assert!(
        report.contains("SPLITAND") && report.contains("MERGEFILTER"),
        "diagnostic must name both rules:\n{report}"
    );
    assert!(
        report.contains("SPLITAND -> MERGEFILTER") || report.contains("MERGEFILTER -> SPLITAND"),
        "the last-firings tail must show the alternation:\n{report}"
    );

    // Valid stop states only: the packed AND, or the split stack.
    let packed = tree.contains("((#0.0 > 1) AND (#0.0 > 2))");
    let split = tree.contains("Filter (#0.0 > 1)") && tree.contains("Filter (#0.0 > 2)");
    assert!(
        packed || split,
        "tree must be one of the two valid forms, never a hybrid:\n{tree}"
    );
}

#[test]
fn generous_budget_changes_nothing_but_the_stop_point() {
    // Ten times the budget: still trips (the loop never converges), and
    // the tree still lands in a valid form. Budgets bound damage; they
    // don't change semantics.
    let (tree, tripped, _) = seeded_ping_pong(600);
    assert!(tripped);
    assert!(tree.contains("Scan #0"));
}

/// [scaffold] Show the diagnostic + count rule applications per query
/// (the ledger's p3 column). Run:
/// `cargo test --test gate3_termination print_ -- --ignored --nocapture`
#[test]
#[ignore]
fn print_trip_report_and_rule_apps() {
    let (_, _, report) = seeded_ping_pong(60);
    println!("{report}\n");
    let cat = optimize::catalog::catalog();
    for q in optimize::harness::corpus().expect("corpus") {
        let stmt = optimize::harness::parse_single(&q.sql).expect("parse");
        let ir = optimize::bind::bind(stmt, &cat).expect("bind");
        let (_, apps) = optimize::p3::rewrite_fixpoint_counted(ir);
        println!("{}: {apps} rule applications", q.name);
    }
}
