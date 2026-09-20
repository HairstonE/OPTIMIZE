//! P0 — the common foundation every project stands on.
//! Spec + worksheet: src/common/common.md
//!
//! [scaffold] session, catalog, stats, harness, metrics, error — plumbing.
//! [yours]    ir, bind, lower, identity — the P0 build.
//! opt.rs     the Optimizer trait + the registry all six projects plug into.

pub mod bind;
pub mod catalog;
pub mod cost;
pub mod error;
pub mod harness;
pub mod identity;
pub mod ir;
pub mod lower;
pub mod metrics;
pub mod opt;
pub mod session;
pub mod stats;
