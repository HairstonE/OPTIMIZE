//! [scaffold] The four-table TPC-H subset: explicit schemas, CSV
//! registration on a SessionContext, and the `Catalog` the binder consumes.
//!
//! Schemas are declared, never inferred — inference drift between oracle
//! and subject would poison the differential. The same declarations are
//! mirrored in tests/common/mod.rs as DuckDB DDL; if you change one, change
//! both (and re-run the oracle self-check).
//!
//! Deliberate corpus-wide simplifications (documented in COURSE.md §2.3):
//!   - no NULLs anywhere in the data (two-valued logic suffices)
//!   - dates are TEXT in ISO form (lexicographic == chronological)
//!   - every integer is Int64/BIGINT, every decimal Float64/DOUBLE, so
//!     arithmetic result types agree across engines

use anyhow::Result;
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::prelude::*;
use std::path::PathBuf;

pub const TABLES: [&str; 4] = ["customer", "orders", "lineitem", "part"];

/// (column name, type) per table, in file/positional order.
pub fn columns_for(table: &str) -> Vec<(&'static str, DataType)> {
    use DataType::{Float64, Int64, Utf8};
    match table {
        "customer" => vec![
            ("c_custkey", Int64),
            ("c_name", Utf8),
            ("c_nationkey", Int64),
            ("c_mktsegment", Utf8),
            ("c_acctbal", Float64),
        ],
        "orders" => vec![
            ("o_orderkey", Int64),
            ("o_custkey", Int64),
            ("o_orderstatus", Utf8),
            ("o_totalprice", Float64),
            ("o_orderdate", Utf8),
            ("o_orderpriority", Utf8),
        ],
        "lineitem" => vec![
            ("l_orderkey", Int64),
            ("l_partkey", Int64),
            ("l_linenumber", Int64),
            ("l_quantity", Int64),
            ("l_extendedprice", Float64),
            ("l_discount", Float64),
            ("l_shipdate", Utf8),
            ("l_returnflag", Utf8),
        ],
        "part" => vec![
            ("p_partkey", Int64),
            ("p_name", Utf8),
            ("p_brand", Utf8),
            ("p_size", Int64),
            ("p_retailprice", Float64),
        ],
        other => panic!("unknown table {other}"),
    }
}

pub fn schema_for(table: &str) -> Schema {
    Schema::new(
        columns_for(table)
            .into_iter()
            .map(|(name, dt)| Field::new(name, dt, false)) // nullable=false: no NULLs by construction
            .collect::<Vec<_>>(),
    )
}

pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
}

/// Register all four CSVs on a context (subject or oracle — same call, same
/// schemas, so the only difference between the two pipelines is the plan).
pub async fn register_all(ctx: &SessionContext) -> Result<()> {
    for t in TABLES {
        let schema = schema_for(t);
        let path = data_dir().join(format!("{t}.csv"));
        ctx.register_csv(
            t,
            path.to_str().expect("utf8 path"),
            CsvReadOptions::new().has_header(true).schema(&schema),
        )
        .await?;
    }
    Ok(())
}

/// What the binder sees. Positional: `rel_idx` into the FROM list is
/// yours to assign during binding; `col_idx` indexes into `columns`.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub tables: Vec<TableMeta>,
}

#[derive(Debug, Clone)]
pub struct TableMeta {
    pub name: String,
    pub columns: Vec<(String, DataType)>,
}

impl Catalog {
    pub fn table(&self, name: &str) -> Option<&TableMeta> {
        self.tables.iter().find(|t| t.name == name)
    }
}

pub fn catalog() -> Catalog {
    Catalog {
        tables: TABLES
            .iter()
            .map(|t| TableMeta {
                name: t.to_string(),
                columns: columns_for(t)
                    .into_iter()
                    .map(|(n, d)| (n.to_string(), d))
                    .collect(),
            })
            .collect(),
    }
}
