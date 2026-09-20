//! [scaffold] Bench helper, not a gate test: rows moved by STOCK
//! DataFusion (its own parser/optimizer/executor) per corpus query —
//! the "what does a mature 2020s optimizer do" column for the ledger
//! and talks. Run with:
//! `cargo test --test bench_oracle -- --ignored --nocapture`

mod common;

use optimize::harness::{self, corpus};
use optimize::metrics;

#[test]
#[ignore]
fn print_oracle_rows_moved() {
    let rt = common::test_runtime();
    for q in corpus().expect("corpus loads") {
        let work = rt.block_on(async {
            let ctx = harness::ready_oracle_ctx().await?;
            let df = ctx.sql(&q.sql).await?;
            let phys = df.create_physical_plan().await?;
            let (_, work) = metrics::measure(phys, ctx.task_ctx()).await?;
            anyhow::Ok(work)
        });
        match work {
            Ok(w) => println!("{}: rows moved {}", q.name, w.total_rows),
            Err(e) => println!("{}: FAILED {e:#}", q.name),
        }
    }
}

/// [scaffold] The fair fight: stock DataFusion WITH statistics access.
/// Same data converted to Parquet (real metadata: row counts, min/max),
/// statistics collection ON. Run:
/// `cargo test --test bench_oracle parquet -- --ignored --nocapture`
#[test]
#[ignore]
fn print_oracle_parquet_stats_rows_moved() {
    use datafusion::dataframe::DataFrameWriteOptions;
    use datafusion::prelude::*;
    let rt = common::test_runtime();
    let dir = std::env::temp_dir().join("optimize_parquet_oracle");
    std::fs::create_dir_all(&dir).expect("mkdir");

    rt.block_on(async {
        // 1. Convert each CSV table to Parquet via the oracle ctx.
        let csv = harness::ready_oracle_ctx().await?;
        for t in optimize::catalog::TABLES {
            let out = dir.join(format!("{t}.parquet"));
            let _ = std::fs::remove_file(&out);
            csv.table(t)
                .await?
                .write_parquet(
                    out.to_str().unwrap(),
                    DataFrameWriteOptions::new(),
                    None,
                )
                .await?;
        }
        // 2. Fresh stock ctx: statistics collection ON, Parquet sources.
        let config = SessionConfig::new()
            .with_target_partitions(1)
            .with_collect_statistics(true);
        let ctx = SessionContext::new_with_config(config);
        for t in optimize::catalog::TABLES {
            ctx.register_parquet(
                t,
                dir.join(format!("{t}.parquet")).to_str().unwrap(),
                ParquetReadOptions::default(),
            )
            .await?;
        }
        // 3. Measure the join queries.
        for q in corpus().expect("corpus") {
            let df = ctx.sql(&q.sql).await?;
            let phys = df.create_physical_plan().await?;
            let (_, work) = metrics::measure(phys, ctx.task_ctx()).await?;
            println!("{}: rows moved {}", q.name, work.total_rows);
        }
        anyhow::Ok(())
    })
    .expect("parquet oracle bench");
}

/// [scaffold] Wall-clock for the talk (never the ledger — house rule:
/// ledger numbers stay hardware-agnostic). Times EXECUTION of q13 under
/// p1, p2, and stock DataFusion. Run RELEASE or the numbers lie:
/// `cargo test --release --test bench_oracle timing -- --ignored --nocapture`
#[test]
#[ignore]
fn timing_q13() {
    let rt = common::test_runtime();
    let q13 = corpus()
        .expect("corpus")
        .into_iter()
        .find(|q| q.name == "q13")
        .expect("q13");
    for key in ["p1", "p2"] {
        // warm once, then time three runs
        rt.block_on(harness::subject_rows(key, &q13.sql)).expect("warm");
        let t = std::time::Instant::now();
        for _ in 0..3 {
            rt.block_on(harness::subject_rows(key, &q13.sql)).expect("run");
        }
        println!("{key}: {:.3} s/run", t.elapsed().as_secs_f64() / 3.0);
    }
    rt.block_on(harness::oracle_datafusion(&q13.sql)).expect("warm");
    let t = std::time::Instant::now();
    for _ in 0..3 {
        rt.block_on(harness::oracle_datafusion(&q13.sql)).expect("run");
    }
    println!("datafusion: {:.3} s/run", t.elapsed().as_secs_f64() / 3.0);
}
