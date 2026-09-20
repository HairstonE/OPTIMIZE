//! The Optimizer trait (frozen signature) and the registry.
//!
//! Every project is pre-registered here with a stub that returns
//! NotYetImplemented — so the harness, CLI, gates, and bench see all six
//! from day one. "Plugging in" a project = implementing the trait body in
//! src/pN/mod.rs. No wiring, ever.

use crate::common::cost::CostModel;
use crate::common::ir::{Ir, RelInfo};
use crate::common::stats::Stats;
use anyhow::Result;

/// Carries stats + (from P2) the cost model. Present from day one so the
/// `optimize` signature never churns. Frozen with the trait.
pub struct OptContext {
    pub stats: Stats,
}

impl OptContext {
    /// The per-query cost model (P2, frozen after): borrows this context's
    /// stats and the query's rel table. P4/P5 call the same functions.
    pub fn cost_model<'a>(&'a self, rels: &'a [RelInfo]) -> CostModel<'a> {
        CostModel { stats: &self.stats, rels }
    }
}

pub trait Optimizer {
    fn name(&self) -> &'static str;
    fn optimize(&self, plan: Ir, ctx: &OptContext) -> Result<Ir>;
}

pub struct Entry {
    /// Project id: "p0" .. "p5". What you type in CLI/test filters.
    pub project: &'static str,
    /// Highest corpus tier this optimizer's output can feasibly EXECUTE
    /// (see data/queries/corpus.toml). p0 = 0: an unoptimized 3+-way
    /// comma-join is a literal 10^10-row cross product. Everything with
    /// pushdown (p1+) = 1. EXPLAIN snapshots ignore tiers.
    pub tier: u8,
    pub optimizer: Box<dyn Optimizer>,
}

pub fn registry() -> Vec<Entry> {
    vec![
        Entry { project: "p0", tier: 0, optimizer: Box::new(crate::common::identity::IdentityOptimizer) },
        Entry { project: "p1", tier: 1, optimizer: Box::new(crate::p1::HeuristicOptimizer) },
        Entry { project: "p2", tier: 1, optimizer: Box::new(crate::p2::SelingerOptimizer) },
        Entry { project: "p3", tier: 1, optimizer: Box::new(crate::p3::StarburstOptimizer) },
        Entry { project: "p4", tier: 1, optimizer: Box::new(crate::p4::VolcanoOptimizer) },
        Entry { project: "p5", tier: 1, optimizer: Box::new(crate::p5::CascadesOptimizer) },
    ]
}

/// Look up by project id ("p1") or optimizer name ("heuristic").
pub fn find(key: &str) -> Option<Entry> {
    registry()
        .into_iter()
        .find(|e| e.project == key || e.optimizer.name() == key)
}
