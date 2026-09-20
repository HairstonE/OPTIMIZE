//! [yours — P0, src/common/common.md Step 1] The logical IR.
//!
//! ── CONTRACT (COURSE.md §2.1) ─────────────────────────────────────────────
//! Minimum operator set, nothing more until a project forces it:
//!   Scan · Filter · Project · Join { on, filter } (inner equi only) ·
//!   Sort · Limit (pass-through until P4) · EmptyScan (added in P1, the
//!   last permitted logical-IR change).
//! Expressions: column refs, literals, comparisons, AND/OR/NOT, arithmetic.
//! No aggregates (SEAM-AGG).
//!
//! Design constraints (= P0 review criteria):
//!   1. Column refs are positional post-bind (e.g. ColRef { rel_idx,
//!      col_idx } or a flat output ordinal — your call, write down why).
//!      ALL remapping goes through one shared ColumnRemap utility with its
//!      own property tests.
//!   2. Nodes cheap to clone or Arc-shared — pick one; P4's memo holds
//!      thousands.
//!   3. Structural equality/hashing derivable, with a canonical form
//!      (e.g. sorted AND-conjuncts) defined NOW — P4's memo dedup depends
//!      on it.
//!
//! Display: stable, one node per line, indented children. The EXPLAIN
//! snapshot gate locks this rendering, so make it something you want to
//! read for six projects.
//!
//! The IR freezes logically at end of P1 (house rule 3). Additions after
//! that require a written note in COURSE.md's changelog.
//! ──────────────────────────────────────────────────────────────────────────
//!
//! The scaffold treats `Ir` as opaque: it only moves values around and calls
//! `Display`. Replace this placeholder with your design; nothing in
//! [scaffold] code will need to change.

use std::fmt;

// ── Design choices (P0, reviewed at gate 0) ───────────────────────────────
// ColRef = { rel, col }: rel is a bind-time relation IDENTITY (index into
//   Ir::rels, assigned once, never renumbered), col indexes that table's
//   catalog columns. Both are invariant under everything P1–P5 do (join
//   reorder, pushdown, pruning) — unlike output ordinals, which would shift
//   every time an optimizer moves a node. q18's two `customer` instances
//   stay distinct rels forever.
// Box + derive(Clone), not Arc: P1–P3 are whole-tree rewrites, pleasant
//   over owned values; P4's memo stores group-refs, not these subtrees, so
//   the "thousands of nodes" pressure lands on the memo's own types.
//   Bonus: derived Eq/Hash/Ord stay structural with zero indirection.
// Float(u64) = f64::to_bits: f64 is not Eq/Hash/Ord, so we store bits and
//   convert at the edges. -0.0 is folded to 0.0 at construction (bind.rs)
//   so the two spellings of zero are one value; NaN never occurs (no NULLs,
//   no 0/0 in the corpus). Derived Ord over bits is arbitrary but STABLE —
//   exactly what canonical sorting needs; it is never used numerically.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ir {
    pub root: Plan,
    pub rels: Vec<RelInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelInfo {
    pub table: String,
    pub qualifier: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Plan {
    Scan {
        rel: usize,
    },
    // Zero rows, standing in for the (sorted, deduped) set of rels it
    // absorbed — the typed zero of inner join: ∅-of-lineitem ≠ ∅-of-customer,
    // and lowering needs to know whose schema to emit. Produced only by P1's
    // constant-folding pass; the LAST logical-IR addition (house rule 3).
    EmptyScan {
        rels: Vec<usize>,
    },
    Filter {
        input: Box<Plan>,
        predicate: Expr,
    },
    Project {
        input: Box<Plan>,
        exprs: Vec<Expr>,
    },
    Join {
        left: Box<Plan>,
        right: Box<Plan>,
        on: Vec<(ColRef, ColRef)>,
        filter: Option<Expr>,
    },
    Sort {
        input: Box<Plan>,
        keys: Vec<(Expr, /*desc: */ bool)>,
    },
    Limit {
        input: Box<Plan>,
        n: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ColRef {
    pub rel: usize,
    pub col: usize,
}

impl fmt::Display for ColRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}.{}", self.rel, self.col)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Expr {
    Col(ColRef),
    Int(i64),
    Str(String),
    Bool(bool),
    Float(u64),
    Cmp {
        op: CmpOp,
        l: Box<Expr>,
        r: Box<Expr>,
    },
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    Arith {
        op: ArithOp,
        l: Box<Expr>,
        r: Box<Expr>,
    },
}

impl fmt::Display for Ir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.root.fmt_at(f, 0)
    }
}

impl Plan {
    fn fmt_at(&self, f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
        let pad = "  ".repeat(depth);
        match self {
            Plan::Scan { rel } => writeln!(f, "{pad}Scan #{rel}"),
            Plan::EmptyScan { rels } => {
                let list: Vec<String> = rels.iter().map(|r| format!("#{r}")).collect();
                writeln!(f, "{pad}EmptyScan [{}]", list.join(", "))
            }
            Plan::Filter { input, predicate } => {
                writeln!(f, "{pad}Filter {predicate}")?;
                input.fmt_at(f, depth + 1)
            }
            Plan::Project { input, exprs } => {
                let list: Vec<String> = exprs.iter().map(|e| e.to_string()).collect();
                writeln!(f, "{pad}Project [{}]", list.join(", "))?;
                input.fmt_at(f, depth + 1)
            }
            Plan::Join {
                left,
                right,
                on,
                filter,
            } => {
                let pairs: Vec<String> = on.iter().map(|(l, r)| format!("{l} = {r}")).collect();
                match filter {
                    Some(pred) => writeln!(f, "{pad}Join on=[{}] filter={pred}", pairs.join(", "))?,
                    None => writeln!(f, "{pad}Join on=[{}]", pairs.join(", "))?,
                }
                left.fmt_at(f, depth + 1)?;
                right.fmt_at(f, depth + 1)
            }
            Plan::Sort { input, keys } => {
                let list: Vec<String> = keys
                    .iter()
                    .map(|(e, desc)| format!("{e} {}", if *desc { "DESC" } else { "ASC" }))
                    .collect();
                writeln!(f, "{pad}Sort [{}]", list.join(", "))?;
                input.fmt_at(f, depth + 1)
            }
            Plan::Limit { input, n } => {
                writeln!(f, "{pad}Limit {n}")?;
                input.fmt_at(f, depth + 1)
            }
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Col(c) => write!(f, "{c}"),
            Expr::Int(n) => write!(f, "{n}"),
            Expr::Str(s) => write!(f, "'{s}'"),
            Expr::Bool(b) => write!(f, "{b}"),
            // {:?} keeps a trailing ".0" on whole floats, so float literals
            // stay visibly distinct from ints in frozen snapshots.
            Expr::Float(bits) => write!(f, "{:?}", f64::from_bits(*bits)),
            Expr::Cmp { op, l, r } => {
                let sym = match op {
                    CmpOp::Eq => "=",
                    CmpOp::Ne => "<>",
                    CmpOp::Lt => "<",
                    CmpOp::Le => "<=",
                    CmpOp::Gt => ">",
                    CmpOp::Ge => ">=",
                };
                write!(f, "({l} {sym} {r})")
            }
            Expr::And(cs) => {
                let parts: Vec<String> = cs.iter().map(|e| e.to_string()).collect();
                write!(f, "({})", parts.join(" AND "))
            }
            Expr::Or(cs) => {
                let parts: Vec<String> = cs.iter().map(|e| e.to_string()).collect();
                write!(f, "({})", parts.join(" OR "))
            }
            Expr::Not(e) => write!(f, "NOT ({e})"),
            Expr::Arith { op, l, r } => {
                let sym = match op {
                    ArithOp::Add => "+",
                    ArithOp::Sub => "-",
                    ArithOp::Mul => "*",
                    ArithOp::Div => "/",
                };
                write!(f, "({l} {sym} {r})")
            }
        }
    }
}

// ── Canonical form ────────────────────────────────────────────────────────
// One official spelling per meaning, so derived Eq/Hash answer "same
// predicate?" structurally (P3 compares plans against P1's; P4's memo
// dedups on it). Erases ONLY differences that carry no meaning:
//   - AND/OR conjunct order and nesting (flatten, sort by derived Ord,
//     collapse singletons)
//   - Join `on`-pair order (but never orientation within a pair)
// Deliberately does NOT: dedup conjuncts or eliminate double negation
// (P1's redundancy pass), normalize comparison direction (deferred), or
// reorder Join children / Project exprs / Sort keys (semantics — join
// commutativity is a P4 transformation rule).
pub fn canon_expr(e: Expr) -> Expr {
    match e {
        Expr::Cmp { op, l, r } => Expr::Cmp {
            op,
            l: Box::new(canon_expr(*l)),
            r: Box::new(canon_expr(*r)),
        },
        Expr::And(children) => {
            let mut flat = Vec::new();
            for c in children {
                match canon_expr(c) {
                    Expr::And(inner) => flat.extend(inner),
                    other => flat.push(other),
                }
            }
            flat.sort_unstable();
            if flat.len() == 1 {
                flat.pop().unwrap()
            } else {
                Expr::And(flat)
            }
        }
        Expr::Or(children) => {
            let mut flat = Vec::new();
            for c in children {
                match canon_expr(c) {
                    Expr::Or(inner) => flat.extend(inner),
                    other => flat.push(other),
                }
            }
            flat.sort_unstable();
            if flat.len() == 1 {
                flat.pop().unwrap()
            } else {
                Expr::Or(flat)
            }
        }
        Expr::Not(inner) => Expr::Not(Box::new(canon_expr(*inner))),
        Expr::Arith { op, l, r } => Expr::Arith {
            op,
            l: Box::new(canon_expr(*l)),
            r: Box::new(canon_expr(*r)),
        },
        leaf => leaf,
    }
}

/// Plan-level canonical form: rebuilds the tree, running every embedded
/// expression through `canon_expr`. Structure is never reordered except
/// Join's `on` list, where pair order is meaningless.
pub fn canonicalize(plan: Plan) -> Plan {
    match plan {
        Plan::Scan { rel } => Plan::Scan { rel },
        Plan::EmptyScan { mut rels } => {
            rels.sort_unstable();
            rels.dedup();
            Plan::EmptyScan { rels }
        }
        Plan::Filter { input, predicate } => Plan::Filter {
            input: Box::new(canonicalize(*input)),
            predicate: canon_expr(predicate),
        },
        Plan::Project { input, exprs } => Plan::Project {
            input: Box::new(canonicalize(*input)),
            exprs: exprs.into_iter().map(canon_expr).collect(),
        },
        Plan::Join { left, right, on, filter } => {
            let mut on = on;
            on.sort_unstable();
            Plan::Join {
                left: Box::new(canonicalize(*left)),
                right: Box::new(canonicalize(*right)),
                on,
                filter: filter.map(canon_expr),
            }
        }
        Plan::Sort { input, keys } => Plan::Sort {
            input: Box::new(canonicalize(*input)),
            keys: keys.into_iter().map(|(e, d)| (canon_expr(e), d)).collect(),
        },
        Plan::Limit { input, n } => Plan::Limit {
            input: Box::new(canonicalize(*input)),
            n,
        },
    }
}

// ── ColumnRemap ───────────────────────────────────────────────────────────
// The ONE utility through which every "column indices shifted" translation
// goes; nothing else in the codebase may fiddle indices by hand (P0 design
// constraint 1). Inert at P0 — P1's pushdown-through-Project and P2's join
// reorder are the real customers. Refs absent from the map pass through
// unchanged, so a remap built from one operator's columns can be applied
// to any expression without enumerating the world.
#[derive(Debug, Clone, Default)]
pub struct ColumnRemap {
    map: std::collections::BTreeMap<ColRef, ColRef>,
}

impl ColumnRemap {
    pub fn from_pairs(pairs: impl IntoIterator<Item = (ColRef, ColRef)>) -> Self {
        Self {
            map: pairs.into_iter().collect(),
        }
    }

    /// The reverse translation. Panics if the map is not a bijection —
    /// remaps built from permutations always are, and a non-bijective
    /// inverse is a caller bug, not user input.
    pub fn inverse(&self) -> Self {
        let map: std::collections::BTreeMap<ColRef, ColRef> =
            self.map.iter().map(|(k, v)| (*v, *k)).collect();
        assert_eq!(map.len(), self.map.len(), "inverse of non-bijective remap");
        Self { map }
    }

    /// Mapped ref, or the ref unchanged if absent from the map.
    pub fn get(&self, c: ColRef) -> ColRef {
        self.map.get(&c).copied().unwrap_or(c)
    }

    pub fn apply_expr(&self, e: Expr) -> Expr {
        match e {
            Expr::Col(c) => Expr::Col(self.get(c)),
            Expr::Cmp { op, l, r } => Expr::Cmp {
                op,
                l: Box::new(self.apply_expr(*l)),
                r: Box::new(self.apply_expr(*r)),
            },
            Expr::And(cs) => Expr::And(cs.into_iter().map(|c| self.apply_expr(c)).collect()),
            Expr::Or(cs) => Expr::Or(cs.into_iter().map(|c| self.apply_expr(c)).collect()),
            Expr::Not(inner) => Expr::Not(Box::new(self.apply_expr(*inner))),
            Expr::Arith { op, l, r } => Expr::Arith {
                op,
                l: Box::new(self.apply_expr(*l)),
                r: Box::new(self.apply_expr(*r)),
            },
            leaf => leaf,
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn col(rel: usize, col: usize) -> Expr {
        Expr::Col(ColRef { rel, col })
    }

    fn rel(table: &str) -> RelInfo {
        RelInfo {
            table: table.into(),
            qualifier: table.into(),
        }
    }

    fn cmp(op: CmpOp, l: Expr, r: Expr) -> Expr {
        Expr::Cmp {
            op: op,
            l: Box::new(l),
            r: Box::new(r),
        }
    }

    fn hash_of<T: std::hash::Hash>(t: &T) -> u64 {
        use std::hash::Hasher;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        t.hash(&mut h);
        h.finish()
    }

    #[test]
    fn golden_display_q07_shape() {
        // q07: SELECT o_orderkey, l_partkey, l_quantity
        //      FROM orders, lineitem
        //      WHERE o_orderkey = l_orderkey AND l_quantity > 30
        // Bound per the P0 rules: comma-join → cross-join shape (on=[]),
        // WHERE left whole on top, Project at the very top.

        let ir = Ir {
            rels: vec![rel("orders"), rel("lineitem")],
            root: Plan::Project {
                input: Box::new(Plan::Filter {
                    input: Box::new(Plan::Join {
                        left: Box::new(Plan::Scan { rel: 0 }),
                        right: Box::new(Plan::Scan { rel: 1 }),
                        on: vec![],
                        filter: None,
                    }),
                    predicate: Expr::And(vec![
                        Expr::Cmp {
                            op: CmpOp::Eq,
                            l: Box::new(col(0, 0)), // o_orderkey
                            r: Box::new(col(1, 0)), // l_orderkey
                        },
                        Expr::Cmp {
                            op: CmpOp::Gt,
                            l: Box::new(col(1, 3)), // l_quantity
                            r: Box::new(Expr::Int(30)),
                        },
                    ]),
                }),
                exprs: vec![col(0, 0), col(1, 1), col(1, 3)],
            },
        };

        let expected = "\
Project [#0.0, #1.1, #1.3]
  Filter ((#0.0 = #1.0) AND (#1.3 > 30))
    Join on=[]
      Scan #0
      Scan #1
";
        assert_eq!(ir.to_string(), expected);
    }

    #[test]
    fn canonical_equality() {
        let a = cmp(CmpOp::Eq, col(0, 0), col(1, 0));
        let b = cmp(CmpOp::Gt, col(1, 3), Expr::Int(30));

        let x = canon_expr(Expr::And(vec![a.clone(), b.clone()]));
        let y = canon_expr(Expr::And(vec![b, a])); // last use — no clone needed

        assert_eq!(x, y);
        assert_eq!(hash_of(&x), hash_of(&y));
    }

    #[test]
    fn canonical_inequality() {
        let a = cmp(CmpOp::Eq, col(0, 0), col(1, 0));
        let b = cmp(CmpOp::Gt, col(1, 3), Expr::Int(30));

        let x = canon_expr(Expr::Or(vec![a.clone(), b.clone()]));
        let y = canon_expr(Expr::And(vec![b, a])); // last use — no clone needed

        assert_ne!(x, y);
        assert_ne!(hash_of(&x), hash_of(&y));
    }
    #[test]
    fn column_remap_round_trip() {
        // A 3-cycle permutation over one relation's columns.
        let p = |r: usize, c: usize| ColRef { rel: r, col: c };
        let remap = ColumnRemap::from_pairs([
            (p(0, 0), p(0, 2)),
            (p(0, 2), p(0, 1)),
            (p(0, 1), p(0, 0)),
        ]);

        let e = Expr::And(vec![
            cmp(CmpOp::Eq, col(0, 0), col(0, 1)),
            cmp(CmpOp::Gt, col(0, 2), Expr::Int(5)),
            cmp(CmpOp::Lt, col(1, 4), Expr::Int(9)), // unmapped rel: must pass through
        ]);

        let there = remap.apply_expr(e.clone());
        assert_ne!(there, e, "permutation must actually move refs");
        let back = remap.inverse().apply_expr(there);
        assert_eq!(back, e, "remap then inverse must be identity");
    }

    #[test]
    fn canonicalize_plan_reaches_predicates() {
        let a = cmp(CmpOp::Eq, col(0, 0), col(1, 0));
        let b = cmp(CmpOp::Gt, col(1, 3), Expr::Int(30));

        let scrambled = Plan::Filter {
            input: Box::new(Plan::Scan { rel: 0 }),
            predicate: Expr::And(vec![b.clone(), a.clone()]),
        };
        let sorted = Plan::Filter {
            input: Box::new(Plan::Scan { rel: 0 }),
            predicate: canon_expr(Expr::And(vec![a, b])),
        };
        assert_eq!(canonicalize(scrambled), sorted);
    }

    #[test]
    fn canonical_idempotent() {
        let a = cmp(CmpOp::Eq, col(0, 0), col(1, 0));
        let b = cmp(CmpOp::Gt, col(1, 3), Expr::Int(30));
        let c = cmp(CmpOp::Ne, col(0, 0), col(2, 3));

        let x = canon_expr(Expr::And(vec![Expr::And(vec![b, a]), c]));

        assert_eq!(canon_expr(x.clone()), x);
    }
}
