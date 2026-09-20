//! [scaffold] Two marker error types the harness inspects.
//!
//! `NotYetImplemented` — a `[yours]` module that hasn't been written yet.
//! The differential/snapshot harnesses treat it as SKIP so `cargo test`
//! stays green while you work; gates set OPTIMIZE_REQUIRE=<projects>, which
//! turns that project's skips into failures. That's the whole mechanism: tests
//! activate themselves the moment your code exists, and gates refuse to
//! pass on skips.
//!
//! `Unsupported` — SQL outside the corpus contract (GROUP BY, subqueries,
//! outer joins, ...). Gate 0's negative control requires this to be a clean
//! error, never a panic and never a wrong answer.

use std::fmt;

#[derive(Debug)]
pub struct NotYetImplemented(pub &'static str);

impl fmt::Display for NotYetImplemented {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not yet implemented: {}", self.0)
    }
}
impl std::error::Error for NotYetImplemented {}

#[derive(Debug)]
pub struct Unsupported(pub String);

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsupported SQL feature: {}", self.0)
    }
}
impl std::error::Error for Unsupported {}

/// True if the error chain contains a `NotYetImplemented` marker.
pub fn is_todo(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|c| c.downcast_ref::<NotYetImplemented>().is_some())
}

/// True if the error chain contains an `Unsupported` marker.
pub fn is_unsupported(err: &anyhow::Error) -> bool {
    err.chain().any(|c| c.downcast_ref::<Unsupported>().is_some())
}

/// Convenience constructor for binder code: `return Err(unsupported("GROUP BY"))`.
pub fn unsupported(what: impl Into<String>) -> anyhow::Error {
    Unsupported(what.into()).into()
}
