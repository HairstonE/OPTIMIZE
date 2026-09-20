//! [scaffold] Table statistics, loaded from data/stats.toml (truth,
//! computed by scripts/gen_data.py from the actual data) or
//! data/stats-lie.toml (deliberately skewed — the P2 lab exercise).
//!
//! Anything fancier than rows + per-column NDV is SEAM-COST.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TableStats {
    pub rows: u64,
    #[serde(default)]
    pub ndv: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub tables: BTreeMap<String, TableStats>,
}

impl Stats {
    pub fn rows(&self, table: &str) -> Option<u64> {
        self.tables.get(table).map(|t| t.rows)
    }
    pub fn ndv(&self, table: &str, column: &str) -> Option<u64> {
        self.tables.get(table).and_then(|t| t.ndv.get(column)).copied()
    }
}

pub fn load(path: &Path) -> Result<Stats> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading stats file {}", path.display()))?;
    let tables: BTreeMap<String, TableStats> =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Stats { tables })
}

/// Honors OPTIMIZE_STATS=<path> so the P2 stats-lie exercise is one env
/// var, not a code change: OPTIMIZE_STATS=data/stats-lie.toml cargo run ...
pub fn load_default() -> Result<Stats> {
    let path = match std::env::var("OPTIMIZE_STATS") {
        Ok(p) => Path::new(&p).to_path_buf(),
        Err(_) => crate::catalog::data_dir().join("stats.toml"),
    };
    load(&path)
}
