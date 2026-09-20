//! [scaffold] EXPLAIN snapshot gate (insta). One snapshot per
//! (project × query) under tests/snapshots/<optimizer-name>/<query>.snap.
//!
//! Snapshots show behavior CHANGED; the differential shows it's still
//! CORRECT. First capture: `make snapshots`, then review
//! `git diff tests/snapshots/` by eye — that's the review that gets
//! locked forever.
//!
//! Tiers don't apply here: rendering a plan is free, so every project
//! snapshots every query. p0's snapshots of tier-1 queries are the
//! "before" photographs the whole ladder gets measured against.
//! `cargo test p1_snapshots` runs exactly project 1's set.

mod common;

use optimize::harness::{self, corpus};
use optimize::opt;

fn run_snapshots(project: &str) {
    let entry = opt::find(project).expect("registered project");
    let name = entry.optimizer.name();
    let required = harness::is_required(project);
    let mut skipped = 0usize;
    for q in corpus().expect("corpus loads") {
        let label = format!("explain {project}/{}", q.name);
        let rendered =
            match harness::skip_or_fail(harness::explain_ir(project, &q.sql), &label, required) {
                Ok(Some(r)) => r,
                Ok(None) => {
                    skipped += 1;
                    continue;
                }
                Err(e) => panic!("{label}: {e:#}"),
            };
        insta::with_settings!({
            snapshot_path => format!("snapshots/{name}"),
            prepend_module_to_snapshot => false,
            description => q.sql.clone(),
        }, {
            insta::assert_snapshot!(q.name.clone(), rendered);
        });
    }
    if required {
        assert!(skipped == 0, "{project} is required but {skipped} snapshots skipped");
    }
}

#[test]
fn p0_snapshots() {
    run_snapshots("p0");
}
#[test]
fn p1_snapshots() {
    run_snapshots("p1");
}
#[test]
fn p2_snapshots() {
    run_snapshots("p2");
}
#[test]
fn p3_snapshots() {
    run_snapshots("p3");
}
#[test]
fn p4_snapshots() {
    run_snapshots("p4");
}
#[test]
fn p5_snapshots() {
    run_snapshots("p5");
}
