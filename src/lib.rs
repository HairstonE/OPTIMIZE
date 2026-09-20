//! OPTIMIZE! — six query optimizers, built by hand, in historical order.
//!
//! Layout:
//!   src/common/   P0 — shared foundation: IR, binder, lowering, errors,
//!                 catalog, session, harness, metrics. Spec: common/common.md
//!   src/p1..p5/   one self-contained project per optimizer, each with its
//!                 spec (pN.md) beside the code. All are pre-registered in
//!                 common::opt::registry(); implementing the trait body is
//!                 the only wiring a project ever needs.

pub mod common;
pub mod p1;
pub mod p2;
pub mod p3;
pub mod p4;
pub mod p5;

// Re-export the common submodules at the crate root so both spellings work:
// `optimize::harness` and `optimize::common::harness`.
pub use common::{bind, catalog, error, harness, identity, ir, lower, metrics, opt, session, stats};
