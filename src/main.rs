//! [scaffold] CLI. Project-keyed and terse:
//!
//!   optimize list                    registered projects + corpus
//!   optimize p0 q01                  run q01 through p0, diff vs oracle
//!   optimize p1 q11 --explain        plan only, no execution (any tier)
//!   optimize p0 q07 --no-compare     skip the oracle diff
//!   optimize bench                   rows-moved matrix: every project × query
//!   optimize bench p1                per-operator row-work for one project
//!
//! Projects are addressable by id ("p1") or optimizer name ("heuristic").
//! Queries by id ("q07") or path. The DuckDB oracle lives in `cargo test`
//! (dev-dependency), not here — `--compare` diffs against stock DataFusion.

use anyhow::{Context, Result};
use datafusion::physical_plan::displayable;
use optimize::{error, harness, metrics, opt};
use std::process::ExitCode;

const USAGE: &str = "usage:
  optimize list
  optimize bench [<project>]
  optimize <project> <query> [--explain] [--no-compare]
      <project> = p0..p5 or optimizer name (identity, heuristic, ...)
      <query>   = q01..q25 or a path to a .sql file";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match real_main(&args).await {
        Ok(code) => code,
        Err(e) if error::is_todo(&e) => {
            eprintln!("⏳ {e:#}");
            eprintln!("   (that module is still a stub — the message says which spec to open)");
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn resolve_query(q: &str) -> Result<(String, String)> {
    let path = if std::path::Path::new(q).exists() {
        std::path::PathBuf::from(q)
    } else {
        harness::queries_dir().join(format!("{q}.sql"))
    };
    let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let sql = std::fs::read_to_string(&path)
        .with_context(|| format!("no such query: {q} (looked at {})", path.display()))?;
    Ok((name, sql.trim().to_string()))
}

async fn real_main(args: &[String]) -> Result<ExitCode> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    match cmd {
        "list" => list(),
        "bench" => bench(args.get(1).map(String::as_str)).await,
        "" | "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        key => run(key, args).await,
    }
}

fn list() -> Result<ExitCode> {
    println!("projects:");
    for e in opt::registry() {
        println!("  {}  {:10} (executes corpus tier ≤ {})", e.project, e.optimizer.name(), e.tier);
    }
    println!("\ncorpus (data/queries/):");
    for q in harness::corpus()? {
        println!("  {}  tier {}  {}", q.name, q.tier, q.sql.lines().next().unwrap_or(""));
    }
    Ok(ExitCode::SUCCESS)
}

async fn run(key: &str, args: &[String]) -> Result<ExitCode> {
    let entry = opt::find(key).with_context(|| format!("unknown project '{key}'\n{USAGE}"))?;
    let qarg = args.get(1).with_context(|| format!("missing <query>\n{USAGE}"))?;
    let explain_only = args.iter().any(|a| a == "--explain");
    let compare = !args.iter().any(|a| a == "--no-compare");
    let (qname, sql) = resolve_query(qarg)?;

    if explain_only {
        println!("── IR after {} ({}) ──", entry.project, entry.optimizer.name());
        println!("{}", harness::explain_ir(key, &sql)?);
        return Ok(ExitCode::SUCCESS);
    }

    let ctx = harness::ready_subject_ctx().await?;
    let (ir_explain, plan) = harness::subject_plan(&ctx, key, &sql).await?;

    println!("── IR after {} ({}) ──", entry.project, entry.optimizer.name());
    println!("{ir_explain}");

    let df = ctx.execute_logical_plan(plan).await?;
    let phys = df.create_physical_plan().await?;
    println!("── physical plan (DataFusion, lobotomized) ──");
    println!("{}", displayable(phys.as_ref()).indent(false));

    let (batches, work) = metrics::measure(phys, ctx.task_ctx()).await?;
    println!("── results ──");
    println!("{}", datafusion::arrow::util::pretty::pretty_format_batches(&batches)?);
    println!("── work (hardware-agnostic) ──");
    print!("{work}");

    if compare {
        let ordered = sql.to_uppercase().contains("ORDER BY");
        let subject = harness::normalize(harness::batches_to_rows(&batches)?, ordered);
        let oracle = harness::normalize(harness::oracle_datafusion(&sql).await?, ordered);
        match harness::diff_report(&qname, &oracle, &subject) {
            None => println!("── differential vs stock DataFusion: ✅ identical ──"),
            Some(report) => {
                println!("── differential vs stock DataFusion: ❌ ──\n{report}");
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The rows-moved matrix (all projects × all queries), or one project's
/// per-operator breakdown. Skips stubs ("–") and infeasible tiers ("∞").
async fn bench(key: Option<&str>) -> Result<ExitCode> {
    let corpus = harness::corpus()?;
    match key {
        Some(key) => {
            let entry = opt::find(key).with_context(|| format!("unknown project '{key}'"))?;
            println!("row-work per operator — {} ({})\n", entry.project, entry.optimizer.name());
            for q in corpus.iter().filter(|q| q.tier <= entry.tier) {
                let ctx = harness::ready_subject_ctx().await?;
                match harness::subject_plan(&ctx, entry.project, &q.sql).await {
                    Err(e) if error::is_todo(&e) => {
                        println!("{}: stub, skipped", q.name);
                        continue;
                    }
                    Err(e) => return Err(e),
                    Ok((_, plan)) => {
                        let df = ctx.execute_logical_plan(plan).await?;
                        let phys = df.create_physical_plan().await?;
                        let (_, work) = metrics::measure(phys, ctx.task_ctx()).await?;
                        println!("{}:", q.name);
                        print!("{work}");
                    }
                }
            }
        }
        None => {
            let entries = opt::registry();
            print!("{:6}", "query");
            for e in &entries {
                print!("{:>12}", e.project);
            }
            println!("    (total rows moved; – = stub, ∞ = infeasible unoptimized)");
            for q in &corpus {
                print!("{:6}", q.name);
                for e in &entries {
                    if q.tier > e.tier {
                        print!("{:>12}", "∞");
                        continue;
                    }
                    let ctx = harness::ready_subject_ctx().await?;
                    let cell = match harness::subject_plan(&ctx, e.project, &q.sql).await {
                        Err(err) if error::is_todo(&err) => "–".to_string(),
                        Err(err) => return Err(err),
                        Ok((_, plan)) => {
                            let df = ctx.execute_logical_plan(plan).await?;
                            let phys = df.create_physical_plan().await?;
                            let (_, work) = metrics::measure(phys, ctx.task_ctx()).await?;
                            work.total_rows.to_string()
                        }
                    };
                    print!("{cell:>12}");
                }
                println!();
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
