//! The cost model — written in P2, FROZEN at the end of P2 (house rule 2).
//! Formulas: COURSE.md §1.2. Predicate composition (AND/OR/NOT) follows
//! Selinger et al. 1979 §4 (independence assumption).
//!
//! Shape (p2 notes, design decision 1): a per-query context struct with
//! LOCAL methods — `output_rows` and `local_cost` take an operator and its
//! inputs' row counts, never a whole tree — so P4/P5's memo can call the
//! same functions. `plan_cost` is the whole-tree recursion built on top.
//! Cardinality belongs to the relation set + predicates, not the plan
//! shape: `output_rows` must not depend on which join side is outer.

use crate::common::catalog;
use crate::common::ir::{CmpOp, ColRef, Expr, Plan, RelInfo};
use crate::common::stats::Stats;
use crate::ir::Expr::Col;

pub struct CostModel<'a> {
    pub stats: &'a Stats,
    pub rels: &'a [RelInfo],
}

impl<'a> CostModel<'a> {
    fn ndv(&self, c: ColRef) -> f64 {
        let table_name = &self.rels[c.rel].table;

        let column_name = catalog::columns_for(table_name)[c.col].0;
        self.stats
            .ndv(table_name, column_name)
            .expect("stats list every column") as f64
    }

    fn selectivity(&self, e: &Expr) -> f64 {
        match e {
            Expr::Cmp {
                op: CmpOp::Eq,
                l,
                r,
            } => match (l.as_ref(), r.as_ref()) {
                (Col(a), Col(b)) => 1.0 / self.ndv(*a).max(self.ndv(*b)),
                (Col(c), _) | (_, Col(c)) => 1.0 / self.ndv(*c),
                _ => 1.0,
            },
            Expr::Cmp { .. } => 1.0 / 3.0,
            Expr::And(cs) => cs.iter().map(|c| self.selectivity(c)).product(),
            Expr::Or(cs) => {
                1.0 - cs
                    .iter()
                    .map(|c| 1.0 - self.selectivity(c))
                    .product::<f64>()
            }
            Expr::Not(x) => 1.0 - self.selectivity(x),
            Expr::Bool(true) => 1.0,
            Expr::Bool(false) => 0.0,
            _ => 1.0,
        }
    }

    pub fn output_row(&self, op: &Plan, input_rows: &[f64]) -> f64 {
        match op {
            Plan::Scan { rel } => self.row(*rel),
            Plan::EmptyScan { .. } => 0.0,
            Plan::Filter { predicate, .. } => input_rows[0] * self.selectivity(predicate),
            Plan::Project { .. } | Plan::Sort { .. } => input_rows[0],
            Plan::Limit { n, .. } => input_rows[0].min(*n as f64),
            Plan::Join { on, filter, .. } => {
                on.iter()
                    .map(|(a, b)| 1.0 / self.ndv(*a).max(self.ndv(*b)))
                    .product::<f64>()
                    * filter.as_ref().map_or(1.0, |f| self.selectivity(f))
                    * input_rows[0]
                    * input_rows[1]
            }
        }
    }

    fn row(&self, rel: usize) -> f64 {
        self.stats
            .rows(&self.rels[rel].table)
            .expect("stats list every column") as f64
    }

    pub fn local_cost(&self, op: &Plan, input_rows: &[f64]) -> f64 {
        match op {
            Plan::Scan { rel } => self.row(*rel),
            Plan::EmptyScan { .. } => 0.0,

            Plan::Filter { .. } | Plan::Project { .. } | Plan::Sort { .. } | Plan::Limit { .. } => {
                input_rows[0]
            }
            Plan::Join { .. } => input_rows[0] + input_rows[0] * input_rows[1],
        }
    }

    pub fn plan_cost(&self, p: &Plan) -> (f64, f64) {
        match p {
            Plan::Scan { .. } | Plan::EmptyScan { .. } => {
                (self.output_row(p, &[]), self.local_cost(p, &[]))
            }
            Plan::Filter { input, .. }
            | Plan::Project { input, .. }
            | Plan::Sort { input, .. }
            | Plan::Limit { input, .. } => {
                let (r, c) = self.plan_cost(input);
                (self.output_row(p, &[r]), self.local_cost(p, &[r]) + c)
            }
            Plan::Join { left, right, .. } => {
                let (rows_l, cost_l) = self.plan_cost(left);
                let (rows_r, cost_r) = self.plan_cost(right);

                (
                    self.output_row(p, &[rows_l, rows_r]),
                    self.local_cost(p, &[rows_l, rows_r]) + cost_r + cost_l,
                )
            }
        }
    }
}

// Hand-checked against COURSE.md §1.2 (p2.md Sat AM: "unit-test costs by
// hand on tiny plans"). Numbers derived in src/p2/notes.md; real stats.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ir::Ir;
    use crate::common::opt::{OptContext, Optimizer};
    use crate::{bind, catalog, harness, p1, stats};

    /// SQL → bound Ir → P1-normalized Ir, the tree P2 will cost.
    fn p1_ir(sql: &str) -> (Ir, Stats) {
        let st = stats::load(&catalog::data_dir().join("stats.toml")).unwrap();
        let stmt = harness::parse_single(sql).unwrap();
        let ir = bind::bind(stmt, &catalog::catalog()).unwrap();
        let ctx = OptContext { stats: st.clone() };
        (p1::HeuristicOptimizer.optimize(ir, &ctx).unwrap(), st)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6 * b.abs().max(1.0)
    }

    #[test]
    fn selectivity_rules() {
        let (ir, st) = p1_ir("SELECT o_orderkey FROM orders, customer WHERE o_custkey = c_custkey AND c_nationkey = 7");
        let m = CostModel {
            stats: &st,
            rels: &ir.rels,
        };
        let c = |rel, col| Box::new(Expr::Col(ColRef { rel, col }));
        let eq_lit = Expr::Cmp {
            op: CmpOp::Eq,
            l: c(1, 2),
            r: Box::new(Expr::Int(7)),
        };
        let eq_col = Expr::Cmp {
            op: CmpOp::Eq,
            l: c(0, 1),
            r: c(1, 0),
        };
        let range = Expr::Cmp {
            op: CmpOp::Gt,
            l: c(0, 3),
            r: Box::new(Expr::Int(5)),
        };
        assert!(
            close(m.selectivity(&eq_lit), 1.0 / 25.0),
            "col = lit → 1/ndv"
        );
        assert!(
            close(m.selectivity(&eq_col), 1.0 / 1000.0),
            "col = col → 1/max(993, 1000)"
        );
        assert!(close(m.selectivity(&range), 1.0 / 3.0), "range → 1/3");
        let and = Expr::And(vec![eq_lit.clone(), range.clone()]);
        assert!(close(m.selectivity(&and), 1.0 / 75.0), "AND multiplies");
        let or = Expr::Or(vec![range.clone(), range.clone()]);
        assert!(
            close(m.selectivity(&or), 1.0 - (2.0 / 3.0) * (2.0 / 3.0)),
            "OR = 1 - Π(1-F)"
        );
        assert!(
            close(m.selectivity(&Expr::Not(Box::new(range))), 2.0 / 3.0),
            "NOT = 1 - F"
        );
        assert!(close(m.selectivity(&Expr::Bool(false)), 0.0));
    }

    #[test]
    fn join_cost_is_asymmetric_but_rows_are_not() {
        let (ir, st) = p1_ir("SELECT o_orderkey FROM orders, customer WHERE o_custkey = c_custkey AND c_nationkey = 7");
        let m = CostModel {
            stats: &st,
            rels: &ir.rels,
        };
        let join = Plan::Join {
            left: Box::new(Plan::Scan { rel: 0 }),
            right: Box::new(Plan::Scan { rel: 1 }),
            on: vec![(ColRef { rel: 0, col: 1 }, ColRef { rel: 1, col: 0 })],
            filter: None,
        };
        assert!(close(m.output_row(&join, &[5000.0, 40.0]), 200.0));
        assert!(
            close(m.output_row(&join, &[40.0, 5000.0]), 200.0),
            "cardinality is shape-free"
        );
        assert!(
            close(m.local_cost(&join, &[5000.0, 40.0]), 205_000.0),
            "orders outer"
        );
        assert!(
            close(m.local_cost(&join, &[40.0, 5000.0]), 200_040.0),
            "customer outer"
        );
    }

    #[test]
    fn q10_whole_tree_by_hand() {
        let (ir, st) = p1_ir("SELECT o_orderkey FROM orders, customer WHERE o_custkey = c_custkey AND c_nationkey = 7");
        let m = CostModel {
            stats: &st,
            rels: &ir.rels,
        };
        // Scan cust 1000/1000 → Project 1000/2000 → Filter 40/3000
        // Scan orders 5000/5000 → Project 5000/10000
        // Join rows 200, local 205000, cost 218000 → Project 200/218200
        let (rows, cost) = m.plan_cost(&ir.root);
        assert!(close(rows, 200.0), "rows {rows}");
        assert!(close(cost, 218_200.0), "cost {cost}");
    }

    #[test]
    fn empty_scan_costs_nothing() {
        let (ir, st) = p1_ir("SELECT * FROM lineitem WHERE 1 = 0");
        let m = CostModel {
            stats: &st,
            rels: &ir.rels,
        };
        let (rows, cost) = m.plan_cost(&ir.root);
        assert!(close(rows, 0.0) && close(cost, 0.0));
    }
}
