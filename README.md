# OPTIMIZE!

This repo holds five SQL query optimizers, written in Rust, in
historical order.
This is my main project at the [Recurse Center](https://www.recurse.com/).

Each project reproduces the core idea of one landmark paper. A
differential harness checks every optimizer against two independent
oracles (stock DataFusion and DuckDB) on every commit.

## Status

| Project | Optimizer | Status |
|---------|-----------|--------|
| p1 | INGRES heuristics | Complete |
| p2 | Selinger | Complete |
| p3 | Starburst | Complete |
| p4 | Volcano | Complete |
| p5 | Cascades | Future work |

## The optimizers

**p1 — INGRES heuristics** (Wong & Youssefi 1976). Four fixed rewrite
passes push filters below joins, narrow scans with projections, and
turn cross joins into equi-joins. There is no cost model.

**p2 — Selinger** (Selinger et al. 1979). The optimizer searches join
orders with bottom-up dynamic programming. It keeps one best plan per
relation subset and interesting order.

**p3 — Starburst** (Haas, Pirahesh, Lohman et al. 1988–92). This
project rewrites the INGRES heuristics as declarative condition/action
rules, and a generic engine applies them to a fixpoint. The rewritten
tree then goes through the Selinger join search. The result is the
same plan and performance as p2, through a different process.

**p4 — Volcano** (Graefe & McKenna 1993). A memo of equivalence groups
expands the join space to a fixpoint with two transformation rules,
then costed implementation rules pick physical winners. A Sort
enforcer delivers ORDER BY as a required property, and the engine
executes the winner.

**p5 — Cascades** (Graefe 1995). Future work.

## Project structure

All code for this project lives in `src/`:

- `src/pN/` — one optimizer per project.
- `src/common/` — the shared IR, cost model, catalog, and harness.

Everything else in the pipeline is DataFusion, a stock engine with its
logical optimizer removed (`src/common/session.rs`). DataFusion parses
the SQL and executes the plans. The optimization step comes only from
`src/pN/`. `tests/` and `data/` hold the
differential tests, the EXPLAIN snapshots, and the seeded data.

## Setup

1. Install DuckDB: `brew install duckdb` (or see
   [duckdb.org/install](https://duckdb.org/install)). This is oracle #2.
2. Run `cargo test`. The pinned toolchain (`rust-toolchain.toml`)
   installs itself through rustup. The first build compiles DataFusion
   once and is slow.

That is the full setup. `cargo test` runs the oracle self-check plus
every built project's differential and snapshot tests.

## Papers

Wong & Youssefi 1976 · Selinger et al. 1979 · Haas/Pirahesh/Lohman/Lee
(Starburst) 1988–92 · Graefe & McKenna 1993 · Graefe 1995 · Gassner et
al. 1993 · Chaudhuri 1998 · Ding et al., _Extensible Query Optimizers
in Practice_ (survey). A title search finds each paper.
