//! [scaffold] Gate 0's negative control. SQL outside the corpus contract
//! must fail with a clean Unsupported error: not a panic, and above all
//! not a wrong answer. A binder that silently drops a GROUP BY would sail
//! through every positive test while being completely broken.
//! Runs through p0; the binder is common, so passing once covers everyone.

mod common;

use optimize::error;
use optimize::harness;

fn expect_unsupported(sql: &str) {
    let rt = common::test_runtime();
    let required = harness::is_required("p0");
    match rt.block_on(harness::subject_rows("p0", sql)) {
        Err(e) if error::is_todo(&e) && !required => {
            eprintln!("SKIP negative control (binder not yet implemented): {e}");
        }
        Err(e) if error::is_unsupported(&e) => {
            eprintln!("✅ clean Unsupported: {e}");
        }
        Err(e) => panic!("expected Unsupported, got a different error: {e:#}\nsql: {sql}"),
        Ok(rows) => panic!(
            "expected Unsupported, got {} rows — the binder silently mis-handled\nsql: {sql}",
            rows.len()
        ),
    }
}

#[test]
fn p0_group_by_is_unsupported() {
    expect_unsupported("SELECT c_mktsegment, COUNT(*) FROM customer GROUP BY c_mktsegment");
}

#[test]
fn p0_subquery_is_unsupported() {
    expect_unsupported(
        "SELECT c_name FROM customer WHERE c_custkey IN (SELECT o_custkey FROM orders)",
    );
}

#[test]
fn p0_outer_join_is_unsupported() {
    // SEAM-OUTER: outer joins break reorder validity; refusing them is a
    // feature until the post-P5 reading.
    expect_unsupported(
        "SELECT c_name, o_orderkey FROM customer LEFT JOIN orders ON c_custkey = o_custkey",
    );
}
