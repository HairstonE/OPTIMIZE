# The Optimizer Ladder

**A self-paced lab course: six query optimizers, built by hand, in
historical order.** The scaffold, the corpus, the oracles, and the gates are pre-built and frozen.

---

## 0. How this course is run

### 0.1 The one diagram

```
  SQL text
     │
     ▼
  sqlparser-rs  ──────────  (borrowed: DataFusion's re-export)
     │  AST
     ▼
  Binder ──► Logical IR ──  (yours: minimal, designed in P0, frozen after P1)
     │
     ▼
  ┌───────────────────────────────────────────────┐
  │  Optimizer (trait object — one per project)   │
  │  P0 Identity · P1 Ingres · P2 Selinger        │
  │  P3 Starburst · P4 Volcano · P5 Cascades      │
  └───────────────────────────────────────────────┘
     │  optimized IR
     ▼
  Lowering ──► DataFusion LogicalPlan  (yours: mechanical translation)
     │
     ▼
  DataFusion, lobotomized  (borrowed: logical optimizer stripped)
     │
     ▼
  Arrow RecordBatches  ──►  differential vs TWO oracles:
                            stock DataFusion  ·  DuckDB
```

Every project follows the same loop:

1. **Read** the paper with the worksheet's guiding questions; write
   answers in `src/pN/notes.md` (bullet points are fine).
2. **Decide** — the worksheet lists the design decisions; write them down
   in `src/pN/notes.md` _before_ coding. Wrong is fine. Undocumented
   is not.
3. **Predict** — where the worksheet asks, write down what you expect an
   experiment to show before running it (which plan the DP picks for q13,
   how many rule applications q18 needs). Compare after. The gap is the
   lesson.
4. **Build** against the session plan.
5. **Gate** — `make gateN`. The suite runs strict (skips = failures),
   then prints the manual checklist.

## 1. Shared infrastructure

### 1.1 Logical IR — [yours], designed in P0

Minimum operator set, nothing more until a project forces it: `Scan`,
`Filter`, `Project`, `Join { on, filter }` (inner equi only), `Sort`,
`Limit` (pass-through until P4), plus `EmptyScan` from P1 (the last
permitted logical-IR change). Expressions: column refs, literals,
comparisons, AND/OR/NOT, arithmetic. No aggregates (`SEAM-AGG`).

Design constraints (= review criteria):

1. Column refs are positional post-bind; all remapping goes through one
   shared `ColumnRemap` utility with its own property tests.
2. Nodes cheap to clone or `Arc`-shared — pick one; P4's memo holds
   thousands.
3. Structural equality/hashing derivable, with a canonical form (e.g.
   sorted AND-conjuncts) defined in P0 — the memo's dedup depends on it.

### 1.2 Cost model — write once in P2, freeze (house rule 2)

```
cost(Scan)    = rows(t)
cost(Filter)  = rows(input)                     · out = rows · selectivity
cost(Join_NL) = rows(L) + rows(L) · rows(R)
cost(Join_HJ) = rows(L) + rows(R)               (P4+ only)
selectivity(col = lit)   = 1 / ndv(col)
selectivity(col = col)   = 1 / max(ndv(L), ndv(R))
selectivity(range)       = 1/3                  (Selinger's magic number)
```

Stats (`rows`, `ndv`) from `data/stats.toml` (truth, computed from the
generated data); `data/stats-lie.toml` ships alongside for the P2 lab
exercise (`OPTIMIZE_STATS=data/stats-lie.toml`). Anything fancier is
`SEAM-COST`.

### 1.3 Corpus — 25 queries

Four-table TPC-H subset (`customer` 1000 · `orders` 5000 · `lineitem`
10000 · `part` 800), seeded generator, CSVs + stats checked in.

Corpus-wide guarantees the harness leans on (see `data/gen_data.py`):
no NULLs anywhere; dates are ISO TEXT (lexicographic == chronological);
ints are BIGINT, decimals DOUBLE, so result types agree across engines;
every ORDER BY ends in a unique tiebreaker, so ordered comparison is
well-defined; `(l_orderkey, l_linenumber)` is a unique key.

| block                 | queries | teaches                                                                                                                                                                     |
| --------------------- | ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| single table          | q01–q05 | point/conjunctive filters, constant folding (q03), OR/AND (q04), double negation + duplicate conjuncts (q05)                                                                |
| 2-way joins           | q06–q10 | q06/q08 explicit `JOIN…ON` (equi-join even at P0) vs q07/q09 comma-joins (literal cross products at P0 — the pain that motivates P1), projection pruning (q10)              |
| bad-order multi-joins | q11–q18 | 3–5-way chains/stars in hostile textual order; q16 explicit-JOIN chain (fast at P0, order still textual until P2); q18 5-way with aliased self-join                         |
| pathological          | q19–q25 | ORDER BY/LIMIT (q19), interesting-orders bait (q20), `WHERE 1=0` (q21), tautologies/redundancy (q22–q23), disconnected join graph → cross product (q24), kitchen sink (q25) |

**Execution tiers.** An unoptimized 3-way comma-join is a 10^10-row cross
product; no identity pipeline can execute it, ever. `data/queries/corpus.toml`
marks each query tier 0 (feasible unoptimized) or tier 1 (needs pushdown);
the registry (`src/common/opt.rs`) marks p0 as tier 0 and everything from
p1 on as tier 1. The differential pairs accordingly. EXPLAIN snapshots ignore
tiers — identity's rendering of q11–q25 is the "before" photograph.

---

## 2. The ladder

Each project carries its worksheet beside its code (`src/pN/pN.md`;
P0 = `src/common/common.md`) — reading questions,
design decisions, session plan, gate details, scaffold coupons, and the
post-gate Claude Code review prompt (once correctness is proven, the
review checks faithfulness of the historical reproduction and lists
improvements). The one-paragraph versions:

**P0 — Foundation (identity).** SQL in, correct rows out, optimizer does
nothing. Deliberately hand-holding: set up operators, expressions, and
errors (§1.1) with a foundation micro-gate (`make foundation`) proving
the types are solid; then binder (AST→IR; comma-join conjuncts stay
_unclassified_ — optimizers discover join predicates; that asymmetry is
the point), the one-line identity optimizer, and lowering — running
simple queries with zero optimization. Gate 0 "Do no harm": tier-0
corpus matches both oracles; snapshots exist for all 25; unsupported SQL
fails clean. → `src/common/common.md`

**P1 — Ingres-style heuristics, 1976.** Four hardcoded passes, applied
once, in order: constant folding (adds `EmptyScan`, then the logical IR
freezes — house rule 3), predicate classification & pushdown (cross-join
shapes become `Join{on}`), projection pruning, redundancy. No cost, no
search; join _order_ stays textual — that gap is P2's opening sentence.
Gate 1 "Better and still right": full corpus vs both oracles, snapshots
reviewed, rows-into-joins strictly reduced on q11–q18, `WHERE 1=0`
touches zero rows. → `src/p1/p1.md`

**P2 — Selinger, 1979.** Run P1's passes, then replace the join tree:
join graph extraction, bottom-up DP over connected subsets, left-deep,
cost model §1.2 — plus **interesting orders**, the part everyone forgets:
keep per subset one plan per useful order; an ordered DP entry can beat
cheapest-unordered + Sort (q20 is the bait). No cross products in the DP;
disconnected graphs (q24) fall back to greedy. Gate 2: dominance
(`cost(selinger) ≤ cost(heuristic)`, strict on q11–q18), remap-safety
(column-for-column vs the P1 pipeline), one ordered-entry win, and the
stats-lie lab. Cost model freezes here. → `src/p2/p2.md`

**P3 — Starburst, 1987–92.** P1's pass _sequence_ becomes a rule
_engine_; P1's passes become data. Condition/action rules, rule classes
with control strategies, fixpoint driving, termination budget (a seeded
ping-pong pair must be caught, not spin). Pushdown decomposes into small
local rules that _compose into_ pushdown — the decomposition is the
lesson. Join planning still P2's DP (rewrite-then-plan, as Starburst
itself). Gate 3: fixpoint IR structurally identical to P1's output on all
25 — your own earlier optimizer is the oracle. → `src/p3/p3.md`

**P4 — Volcano, 1993.** New territory: memo of logically-equivalent
groups, transformation + implementation rules, top-down optimize-group
with required physical properties and enforcers; physical IR appears
(extending, not replacing, the frozen logical IR) and lowering goes
physical — you now own operator choice and enforcer placement. Volcano's
exhaustive discipline preserved _as the paper describes it_: its
inefficiency is P5's measured foil. Gate 4: cross-paradigm dominance
(`cost(volcano) ≤ cost(selinger)` — any violation is a search bug by
construction), property honesty on ORDER BY, memo forensics in the
ledger. → `src/p4/p4.md`

**P5 — Cascades, 1995 (two weekends).** Same rules, same cost model, same
memo _contents_ — the delta is pure search strategy. Weekend A: the task
engine (explicit LIFO task stack; interim gate: plans and costs identical
to P4 — the engine must replicate Volcano before it may improve on it).
Weekend B: promises, guidance, on-demand exploration; branch-and-bound
prunes task subtrees. Gate 5 "Same plans, less work": plan equivalence
with P4 everywhere; strictly fewer rule applications _and_ expressions on
≥80% of multi-join queries, never more anywhere — the paper's own claim,
checked. → `src/p5/p5.md`

---

## 3. Completion criteria

| #   | Project   | Gate one-liner                                         | Budget  |
| --- | --------- | ------------------------------------------------------ | ------- |
| 0   | Identity  | Tier-0 corpus identical vs both oracles                | 1 wknd  |
| 1   | Ingres    | Rows-into-joins strictly reduced; still identical      | 1 wknd  |
| 2   | Selinger  | cost ≤ P1 everywhere, < on bad-order block; remap-safe | 1 wknd  |
| 3   | Starburst | Fixpoint plans ≡ P1's, plus termination proof          | 1 wknd  |
| 4   | Volcano   | cost ≤ P2 everywhere; physical plan fully yours        | 1 wknd  |
| 5   | Cascades  | Plans ≡ P4, measurably fewer rule applications         | 2 wknds |

**Epilogue (unbudgeted, optional):** point the ladder at JOB-light; port
the Volcano/Cascades cores into IDB behind its `Planner` trait — the lab
was the rehearsal, that is the performance.

## 4. Named seams (deferred, on purpose — house rule 4)

| Seam            | Deferred item                                  | Earliest natural landing           |
| --------------- | ---------------------------------------------- | ---------------------------------- |
| `SEAM-AGG`      | GROUP BY / aggregates in IR + binder           | Only if a future corpus demands it |
| `SEAM-COST`     | Histograms, correlated stats, learned costs    | IDB V3, not this lab               |
| `SEAM-PARALLEL` | target_partitions > 1, distribution properties | Post-P5, if ever                   |
| `SEAM-OUTER`    | Outer joins (breaks reorder validity)          | Post-P5 reading                    |
| `SEAM-UNPARSE`  | DF `Unparser` path (IR→SQL→any engine)         | Curiosity item                     |
| `SEAM-EDGE-PRUNE` | Per-edge column narrowing (drop a column at its last consumer, not just at the scan; late-materialization flavor) | P4 physical work, if ever |
| `SEAM-BOUND`    | Branch-and-bound cost limits (Volcano §3: goal budgets + first-plan bounds). Moot under P4's fixpoint-first simplification — nothing partial exists to abandon | P5, where directed search makes bounds real |
| `SEAM-PRED-SUBSUME` | Predicate subsumption / range simplification (`x < 45 AND x <= 45` → `x < 45`; semantic, needs comparator + literal-order reasoning, dates-as-text aware) — pass 4 stays syntactic (exact duplicates, NOT NOT) | P3 rule with conditions, if the corpus ever baits it |

## 5. House rules (non-negotiable)

1. The differential harness runs on every commit; a red differential
   blocks everything else.
2. The cost model freezes at end of P2. A change re-runs _every_ prior
   gate.
3. The IR freezes at end of P1 (logical) / P4 (physical extension).
   Additions require a changelog note below.
4. Every deferred idea gets a seam name in §4 within the same session it
   was deferred. No silent drops.
5. LEDGER.md numbers are self-reported, single-machine, labeled as such.
6. Course code ([yours] modules) is written by hand. AI comes in at two
   points only: pre-approved scaffold coupons listed in the worksheets,
   and the post-gate Claude Code review — after correctness is proven,
   never before.

## 6. Changelog

- **2026-09-20 — P4 physical path in the harness** (addition BESIDE the
  frozen `Optimizer` trait; p4 notes.md decision 8): `explain_ir` and
  `subject_plan` branch for p4 to `p4::optimize_physical` (winner) +
  `p4::lower_phys` (lowering v2: DataFusion plan rebuilt in the
  winner's shape — join order, hash build side = left/CollectLeft,
  explicit Sort; keyed joins → HashJoinExec, predicate-only joins →
  NestedLoopJoinExec, so the executed algorithm IS the winner's). The
  trait body is ALSO implemented (winner reconstructed as logical IR),
  so every generic registry surface still works. lower.rs helpers
  (table_sources/lower_node/df_expr/df_col) became pub(crate) —
  visibility only, no logic change.
- **2026-09-15 — P4 physical IR: `PhysPlan`** (house rule 3, the
  planned P4 physical extension): a SEPARATE enum in `src/p4/mod.rs` —
  `ScanPhys | EmptyScanPhys | FilterPhys | ProjectPhys | HashJoinPhys |
  NestedLoopPhys | SortPhys | LimitPhys`. The frozen logical `Plan` is
  untouched; p1–p3 never see the new type. One `SortPhys` serves both
  ORDER-BY and enforcer roles (Volcano §2.2). Cost model EXTENSION
  (no existing entry changes): `cost(HashJoinPhys) = rows(build) +
  rows(probe)`; `NestedLoopPhys` keeps the frozen Join entry.
- **2026-09-01 — P2 corpus addition: q26** (per p2.md "write a new
  query if you need a cleaner one"): interesting-orders bait. q20's
  join *contracts* rows (3333 out of 5000×3333 in), so cheapest-
  unordered + root Sort honestly beats any ordered plan there — sorting
  late is sorting less. q26 (`customer ⋈ orders ORDER BY c_custkey`)
  *expands* 1000 → ~5000 rows: Sort-below-join on 1000 rows beats a
  root Sort on 5000. q20 stays as the negative control.
- **2026-07-22 — course revision v2** (scaffold construction):
  - Second oracle added: DuckDB (bundled) beside stock DataFusion; the
    differential now cross-checks two independent engines.
  - Full 25-query corpus shipped and frozen up front (was: staged through
    P2), with execution tiers replacing corpus staging — identity
    executes tier 0, snapshots cover everything.
  - Binder scope: table aliases (`AS`) added (q18/q25 need self-joins to
    reach 5-way over 4 tables); NOT added to the expression set (q05/q23
    need it for the redundancy pass to have work).
  - Skip-vs-require mechanism: stubs SKIP in `cargo test`, gates run
    strict via a require env var (now `OPTIMIZE_REQUIRE=<projects>`).
  - Worksheets split out of the monolith; P0 rewritten hand-holdy (typed
    foundation + `make foundation` micro-gate); post-gate Claude Code
    review replaces the reflection/retro ritual.
- **2026-07-22 — course revision v3** (self-contained projects):
  - Crate renamed `optimize`; repo root is the project root.
  - P0 → `src/common/` (types, errors, binder, lowering, harness,
    metrics, trait+registry); each optimizer → self-contained `src/pN/`
    with its spec (`pN.md`) beside the code. All six pre-registered:
    implementing the trait body is the only wiring.
  - Project-keyed commands: `cargo run -- p1 q07`, `cargo test p1`;
    gates require `OPTIMIZE_REQUIRE=p0..pN` (per-project strictness).
  - Hardware-agnostic performance: `cargo run -- bench` measures rows
    moved (sum of operator output_rows) per project × query; with
    estimated cost (p2+) and search effort (p3+) these are the ledger's
    three axes. No wall-clock comparisons.
  - Root consolidated: `queries/` + `scripts/` → `data/`; `snapshots/`
    → `tests/snapshots/`; `projects/` + `notes/` dissolved into the
    project dirs.
  - DuckDB oracle switched from the bundled crate to the system CLI:
    the crate's C++ doesn't compile on older Xcode toolchains and costs
    30+ min when it does. `brew install duckdb`; version printed per
    test run.
- **2026-08-13 — IR change (P1): `EmptyScan { rels: Vec<usize> }`** — the
  planned P1 addition (house rule 3; the logical IR freezes after this).
  A zero-row leaf standing in for the sorted set of rels it absorbed: the
  typed zero of inner join (∅-of-lineitem ≠ ∅-of-customer). Produced only
  by P1's constant folding — `Filter(false, …)` collapse plus inner-join
  annihilation; Project above it always survives to define output columns.
  Lowered to DataFusion `EmptyRelation { produce_one_row: false }` carrying
  those rels' qualified scan schemas so ColRefs above it still resolve.
  Design rationale in `src/p1/notes.md` decision 3.
