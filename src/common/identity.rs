//! [yours — P0, src/common/common.md Step 3] The identity optimizer.
//!
//! Yes, the real body is one line. It's left as a stub anyway, on
//! principle: the first green Gate 0 should be produced entirely by code
//! you wrote — IR, binder, lowering, and this. Every later weekend is
//! *only* an optimizer because this rung exists.

use crate::common::ir::Ir;
use crate::common::opt::{OptContext, Optimizer};
use anyhow::Result;

pub struct IdentityOptimizer;

impl Optimizer for IdentityOptimizer {
    fn name(&self) -> &'static str {
        "identity"
    }

    fn optimize(&self, plan: Ir, _ctx: &OptContext) -> Result<Ir> {
        Ok(plan)
    }
}
