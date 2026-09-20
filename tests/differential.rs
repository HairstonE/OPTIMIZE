//! [scaffold] The differential harness — house rule 1: this runs on every
//! commit; a red differential blocks everything else.
//!
//! Layout:
//!   `oracle_selfcheck` — stock DataFusion vs DuckDB on the full corpus.
//!     No course code involved; active from day zero. If THIS fails, fix
//!     the scaffold, not your optimizer.
//!   `p0_differential` .. `p5_differential` — one test per project, so
//!     `cargo test p1` runs exactly project 1's suite. Each runs its
//!     optimizer over every corpus query at its execution tier and
//!     compares against BOTH oracles.
//!
//! Stubs SKIP unless their project is listed in OPTIMIZE_REQUIRE
//! ("p0,p1" — set by `make gateN`), where a skip = failure: gates can
//! never pass on unimplemented code.

mod common;

use optimize::harness::{self, corpus};
use optimize::opt;

#[test]
fn oracle_selfcheck_datafusion_vs_duckdb() {
    let rt = common::test_runtime();
    let conn = common::duckdb_conn().expect("duckdb oracle setup");
    let mut failures = Vec::new();
    for q in corpus().expect("corpus loads") {
        let ordered = q.has_order_by();
        let df = rt
            .block_on(harness::oracle_datafusion(&q.sql))
            .unwrap_or_else(|e| panic!("{}: datafusion oracle failed: {e:#}", q.name));
        let dk = common::run_duckdb(&conn, &q.sql)
            .unwrap_or_else(|e| panic!("{}: duckdb oracle failed: {e:#}", q.name));
        let df = harness::normalize(df, ordered);
        let dk = harness::normalize(dk, ordered);
        if let Some(report) = harness::diff_report(&q.name, &df, &dk) {
            failures.push(report);
        } else {
            eprintln!("selfcheck {}: ✅ ({} rows)", q.name, df.len());
        }
    }
    assert!(
        failures.is_empty(),
        "oracles disagree — scaffold problem, not optimizer problem:\n{}",
        failures.join("\n")
    );
}

fn run_differential(project: &str) {
    let rt = common::test_runtime();
    let conn = common::duckdb_conn().expect("duckdb oracle setup");
    let entry = opt::find(project).expect("registered project");
    let required = harness::is_required(project);
    let mut failures = Vec::new();
    let mut ran = 0usize;
    let mut skipped = 0usize;

    for q in corpus().expect("corpus loads").iter().filter(|q| q.tier <= entry.tier) {
        let label = format!("{project}/{}", q.name);
        let subject = rt.block_on(harness::subject_rows(project, &q.sql));
        let subject = match harness::skip_or_fail(subject, &label, required) {
            Ok(Some(rows)) => rows,
            Ok(None) => {
                skipped += 1;
                continue;
            }
            Err(e) => {
                failures.push(format!("{label}: pipeline error: {e:#}"));
                continue;
            }
        };
        ran += 1;
        let ordered = q.has_order_by();
        let subject = harness::normalize(subject, ordered);

        let df = harness::normalize(
            rt.block_on(harness::oracle_datafusion(&q.sql))
                .unwrap_or_else(|e| panic!("{label}: datafusion oracle failed: {e:#}")),
            ordered,
        );
        if let Some(report) = harness::diff_report(&format!("{label} vs datafusion"), &df, &subject) {
            failures.push(report);
        }

        let dk = harness::normalize(
            common::run_duckdb(&conn, &q.sql)
                .unwrap_or_else(|e| panic!("{label}: duckdb oracle failed: {e:#}")),
            ordered,
        );
        if let Some(report) = harness::diff_report(&format!("{label} vs duckdb"), &dk, &subject) {
            failures.push(report);
        }
    }

    eprintln!("differential {project}: {ran} ran, {skipped} skipped");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    if required {
        assert!(skipped == 0, "{project} is required but {skipped} runs were skipped");
        assert!(ran > 0, "{project} is required but nothing ran");
    }
}

#[test]
fn p0_differential() {
    run_differential("p0");
}
#[test]
fn p1_differential() {
    run_differential("p1");
}
#[test]
fn p2_differential() {
    run_differential("p2");
}
#[test]
fn p3_differential() {
    run_differential("p3");
}
#[test]
fn p4_differential() {
    run_differential("p4");
}
#[test]
fn p5_differential() {
    run_differential("p5");
}
