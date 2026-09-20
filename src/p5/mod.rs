//! P5 — Cascades (1995). Spec: src/p5/p5.md
//!
//! Same rules, cost model, and memo contents as P4 — the delta is pure
//! search strategy: an explicit LIFO task stack, promises, guidance,
//! on-demand exploration. Same plans, less work.

use crate::common::error::NotYetImplemented;
use crate::common::ir::Ir;
use crate::common::opt::{OptContext, Optimizer};
use anyhow::Result;

pub struct CascadesOptimizer;

impl Optimizer for CascadesOptimizer {
    fn name(&self) -> &'static str {
        "cascades"
    }

    fn optimize(&self, _plan: Ir, _ctx: &OptContext) -> Result<Ir> {
        Err(NotYetImplemented("p5 cascades — src/p5/p5.md").into())
    }
}
