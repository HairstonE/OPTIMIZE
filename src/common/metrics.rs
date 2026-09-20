//! [scaffold] Hardware-agnostic work measurement: **rows moved**.
//!
//! Wall-clock numbers don't transfer between machines; row counts do. We
//! execute the subject's physical plan and sum `output_rows` over every
//! operator, from DataFusion's own metrics. The total — rows moved — is a
//! machine-independent proxy for how much work a plan does:
//!
//!   - identity on q07 moves ~50M rows (a literal cross product);
//!     heuristic moves thousands. Same machine or not, that ratio is the
//!     result.
//!   - `scan_rows` isolated separately: q21 (`WHERE 1=0`) must scan ZERO
//!     rows from P1 on — that's Gate 1's adversarial check.
//!
//! This is what `optimize bench` prints and what LEDGER.md records.
//! Estimated cost (P2+, frozen model) and search effort (P3+ rule
//! applications, P4+ memo sizes) are the other two hardware-agnostic
//! axes; those come from your optimizers, this one comes free.

use anyhow::Result;
use datafusion::physical_plan::ExecutionPlan;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct RowWork {
    /// Sum of output_rows over every operator in the plan.
    pub total_rows: u64,
    /// Sum of output_rows over scan operators only (rows read from disk).
    pub scan_rows: u64,
    /// (operator name, output_rows) per node, execution-tree order.
    pub per_operator: Vec<(String, u64)>,
}

fn walk(plan: &Arc<dyn ExecutionPlan>, out: &mut RowWork) {
    let name = plan.name().to_string();
    let rows = plan
        .metrics()
        .and_then(|m| m.output_rows())
        .unwrap_or(0) as u64;
    out.total_rows += rows;
    if name.contains("Csv") || name.contains("DataSource") {
        out.scan_rows += rows;
    }
    out.per_operator.push((name, rows));
    for child in plan.children() {
        walk(child, out);
    }
}

/// Execute an already-built physical plan and collect row-work. The
/// batches are returned too so callers don't execute twice.
pub async fn measure(
    plan: Arc<dyn ExecutionPlan>,
    task_ctx: Arc<datafusion::execution::TaskContext>,
) -> Result<(Vec<datafusion::arrow::record_batch::RecordBatch>, RowWork)> {
    let batches = datafusion::physical_plan::collect(plan.clone(), task_ctx).await?;
    let mut work = RowWork::default();
    walk(&plan, &mut work);
    Ok((batches, work))
}

impl std::fmt::Display for RowWork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "rows moved: {} (scans: {})",
            self.total_rows, self.scan_rows
        )?;
        for (name, rows) in &self.per_operator {
            writeln!(f, "  {rows:>12}  {name}")?;
        }
        Ok(())
    }
}
