//! [scaffold] The lobotomized DataFusion session (subject engine) and the
//! stock session (oracle #1). Oracle #2 (DuckDB) lives in tests/common/.

use datafusion::execution::SessionStateBuilder;
use datafusion::prelude::*;

/// DataFusion with its logical optimizer removed. Analyzer rules are KEPT
/// (type coercion is semantics, not optimization — Pitfall #3). Physical
/// optimizer rules are KEPT in v1: EnforceDistribution/EnforceSorting are
/// correctness enforcers, not optimizations, and we don't take physical
/// control until P4 teaches us what an enforcer is (Pitfall #2).
pub fn lobotomized_ctx() -> SessionContext {
    // target_partitions=1: deterministic plans, minimal enforcement,
    // honest single-threaded cost comparisons. SEAM-PARALLEL to revisit.
    let config = SessionConfig::new().with_target_partitions(1);
    let state = SessionStateBuilder::new()
        .with_default_features()
        .with_config(config)
        .with_optimizer_rules(vec![]) // logical brain: removed
        .build();
    SessionContext::new_with_state(state)
}

/// Stock, fully-optimizing DataFusion. Oracle #1 in the differential.
pub fn oracle_ctx() -> SessionContext {
    let config = SessionConfig::new().with_target_partitions(1);
    SessionContext::new_with_config(config)
}
