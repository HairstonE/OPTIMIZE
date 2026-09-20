#![allow(dead_code)] // each test binary uses a subset of these helpers
//! [scaffold] Oracle #2: DuckDB, via the system CLI. A genuinely
//! independent engine — different parser, different optimizer, different
//! executor — so a bug shared by our pipeline and DataFusion's execution
//! layer can't hide.
//!
//! Why the CLI and not the `duckdb` crate: the bundled crate compiles the
//! whole DuckDB C++ engine (30+ min, and it doesn't build at all on older
//! Xcode toolchains). The CLI is a one-line install and versioned
//! independently; the version in use is printed once per test run for the
//! ledger. Binary discovery: $OPTIMIZE_DUCKDB → `duckdb` on PATH →
//! ~/.duckdb/cli/latest/duckdb.
//!
//! DDL mirrors src/common/catalog.rs exactly (BIGINT/DOUBLE/VARCHAR). If
//! you change a schema, change both and re-run oracle_selfcheck.
//!
//! Result normalization: DuckDB prints CSV; floats are re-formatted
//! through the shared `fmt_f64` so both engines produce identical
//! strings. Column types come from `DESCRIBE (query)` — value-shape
//! guessing would misclassify integral doubles.

use optimize::harness::{fmt_f64, Rows};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub struct DuckDb {
    bin: PathBuf,
    preamble: String,
}

fn find_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OPTIMIZE_DUCKDB") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let on_path = Command::new("duckdb")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if on_path {
        return Some(PathBuf::from("duckdb"));
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".duckdb/cli/latest/duckdb");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

pub fn duckdb_conn() -> anyhow::Result<DuckDb> {
    let bin = find_binary().ok_or_else(|| {
        anyhow::anyhow!(
            "DuckDB CLI not found — the differential's second oracle needs it.\n\
             Install with `brew install duckdb` (or https://duckdb.org/install),\n\
             or set OPTIMIZE_DUCKDB=/path/to/duckdb"
        )
    })?;
    if let Ok(out) = Command::new(&bin).arg("--version").output() {
        eprintln!(
            "duckdb oracle: {} ({})",
            String::from_utf8_lossy(&out.stdout).trim(),
            bin.display()
        );
    }
    let d = optimize::catalog::data_dir();
    let d = d.display();
    let preamble = format!(
        r#"
CREATE TABLE customer(
  c_custkey BIGINT, c_name VARCHAR, c_nationkey BIGINT,
  c_mktsegment VARCHAR, c_acctbal DOUBLE);
CREATE TABLE orders(
  o_orderkey BIGINT, o_custkey BIGINT, o_orderstatus VARCHAR,
  o_totalprice DOUBLE, o_orderdate VARCHAR, o_orderpriority VARCHAR);
CREATE TABLE lineitem(
  l_orderkey BIGINT, l_partkey BIGINT, l_linenumber BIGINT,
  l_quantity BIGINT, l_extendedprice DOUBLE, l_discount DOUBLE,
  l_shipdate VARCHAR, l_returnflag VARCHAR);
CREATE TABLE part(
  p_partkey BIGINT, p_name VARCHAR, p_brand VARCHAR,
  p_size BIGINT, p_retailprice DOUBLE);
COPY customer FROM '{d}/customer.csv' (HEADER);
COPY orders   FROM '{d}/orders.csv'   (HEADER);
COPY lineitem FROM '{d}/lineitem.csv' (HEADER);
COPY part     FROM '{d}/part.csv'     (HEADER);
"#
    );
    Ok(DuckDb { bin, preamble })
}

impl DuckDb {
    /// One CLI invocation: fresh in-memory DB, preamble, then `sql`.
    /// -bail: die loudly on the first SQL error.
    fn raw(&self, sql: &str) -> anyhow::Result<String> {
        let mut child = Command::new(&self.bin)
            .args(["-csv", "-noheader", "-bail", ":memory:"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .as_mut()
            .expect("piped stdin")
            .write_all(format!("{}\n{sql}\n", self.preamble).as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            anyhow::bail!(
                "duckdb failed: {}\nsql: {sql}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(String::from_utf8(out.stdout)?)
    }
}

pub fn run_duckdb(db: &DuckDb, sql: &str) -> anyhow::Result<Rows> {
    // Column types first, so float columns normalize through fmt_f64.
    let describe = db.raw(&format!("DESCRIBE ({sql});"))?;
    let types: Vec<String> = parse_csv(&describe)
        .into_iter()
        .map(|r| r.get(1).cloned().unwrap_or_default())
        .collect();

    let mut rows = parse_csv(&db.raw(&format!("{sql};"))?);
    for row in &mut rows {
        for (i, cell) in row.iter_mut().enumerate() {
            if matches!(
                types.get(i).map(String::as_str),
                Some("DOUBLE") | Some("FLOAT") | Some("REAL")
            ) {
                if let Ok(v) = cell.parse::<f64>() {
                    *cell = fmt_f64(v);
                }
            }
        }
    }
    Ok(rows)
}

/// Minimal RFC-4180 line parser (quoted fields, "" escapes). Multi-line
/// quoted fields are unsupported — the corpus contains none.
fn parse_csv(text: &str) -> Rows {
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let mut row = Vec::new();
        let mut field = String::new();
        let mut in_quotes = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if in_quotes {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        field.push('"');
                        chars.next();
                    } else {
                        in_quotes = false;
                    }
                } else {
                    field.push(c);
                }
            } else {
                match c {
                    '"' => in_quotes = true,
                    ',' => row.push(std::mem::take(&mut field)),
                    _ => field.push(c),
                }
            }
        }
        row.push(field);
        rows.push(row);
    }
    rows
}

pub fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}
