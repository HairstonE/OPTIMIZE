# [scaffold] Course driver. `make help` lists everything — though most of
# the time you'll use cargo directly:
#
#   cargo test                 everything (stubs skip silently)
#   cargo test p1              just project 1's differential + snapshots
#   cargo run -- p0 q01        run a query through a project
#   cargo run -- bench         rows-moved matrix, all projects × corpus
#
# Gates run the FULL suite with OPTIMIZE_REQUIRE=<projects so far>: for
# required projects, "not yet implemented" = failure, so a gate can never
# pass on stubs — while later projects' stubs stay silent. Learner-written
# gate tests live in tests/ and are picked up automatically. Each gate then
# prints its manual checklist (the part a test can't check for you).

P ?= p0
Q ?= q01

.PHONY: help data test foundation snapshots run explain bench list gate0 gate1 gate2 gate3 gate4 gate5

help:
	@echo "make data       regenerate data/ CSVs + stats (seeded, byte-identical)"
	@echo "make test       full differential + snapshot suite (stubs skip)"
	@echo "make foundation P0 step-2 micro-gate: IR unit tests only"
	@echo "make run P=p0 Q=q07        run one query (plans + rows + row-work + oracle diff)"
	@echo "make explain P=p0 Q=q11    plan only, no execution (any tier)"
	@echo "make bench [P=p1]          hardware-agnostic row-work numbers"
	@echo "make snapshots  (re)capture EXPLAIN snapshots — review the diff by eye"
	@echo "make list       registered projects + corpus"
	@echo "make gate0 ... gate5       run a project gate (strict + checklist)"

data:
	python3 data/gen_data.py

test:
	cargo test

foundation:
	cargo test --lib

snapshots:
	INSTA_UPDATE=always cargo test --test explain_snapshots
	@echo "review changed snapshots: git diff tests/snapshots/"

run:
	cargo run -- $(P) $(Q)

explain:
	cargo run -- $(P) $(Q) --explain

bench:
	cargo run -- bench $(P_ARG)

list:
	cargo run -- list

gate0:
	OPTIMIZE_REQUIRE=p0 cargo test
	@echo ""
	@echo "── GATE 0 'Do no harm' — manual checklist ──────────────────────"
	@echo "[ ] make foundation green (IR unit tests: display, canonical"
	@echo "    equality, ColumnRemap round-trip)"
	@echo "[ ] tests/snapshots/identity/ has a .snap for all 25 queries"
	@echo "[ ] read q07's snapshot: comma-join IS a cross+filter"
	@echo "[ ] read q06's snapshot: JOIN..ON IS an equi-join"
	@echo "[ ] design choices (clone-vs-Arc, float literals, canonical"
	@echo "    form) documented as comments in src/common/ir.rs"
	@echo "[ ] LEDGER.md: P0 row filled in"
	@echo "[ ] Claude Code review run (prompt in src/common/common.md),"
	@echo "    agreed improvements applied, gate re-run"

gate1:
	OPTIMIZE_REQUIRE=p0,p1 cargo test
	@echo ""
	@echo "── GATE 1 'Better and still right' — manual checklist ──────────"
	@echo "[ ] q06–q18 snapshots reviewed: filters below joins, scans"
	@echo "    narrowed, cross-joins became equi-joins; then locked"
	@echo "[ ] rows-into-joins check via 'cargo run -- bench' recorded in"
	@echo "    LEDGER.md (p1 strictly below p0 on q11–q18)"
	@echo "[ ] q21 (WHERE 1=0): scan_rows = 0 in bench output"
	@echo "[ ] LEDGER.md: P1 rows; Claude Code review (src/p1/p1.md)"

gate2:
	OPTIMIZE_REQUIRE=p0,p1,p2 cargo test
	@echo ""
	@echo "── GATE 2 'Cheaper, same answer' — manual checklist ────────────"
	@echo "[ ] dominance test green: cost(selinger) <= cost(heuristic)"
	@echo "    everywhere, strictly < on q11–q18 (tests/gate2_*.rs)"
	@echo "[ ] column-for-column equality vs p1 pipeline on q11–q18"
	@echo "[ ] >=1 query won by an ordered DP entry (interesting orders)"
	@echo "[ ] stats-lie exercise: OPTIMIZE_STATS=data/stats-lie.toml,"
	@echo "    three sentences in LEDGER.md on what broke and why"
	@echo "[ ] cost model FROZEN from here (house rule 2)"

gate3:
	OPTIMIZE_REQUIRE=p0,p1,p2,p3 cargo test
	@echo ""
	@echo "── GATE 3 'Same destination, principled vehicle' ───────────────"
	@echo "[ ] fixpoint IR structurally identical to p1's on all 25"
	@echo "    (tests/gate3_*.rs — your own p1 is the oracle)"
	@echo "[ ] seeded ping-pong rule pair halts at budget with diagnostic"
	@echo "[ ] LEDGER.md: rule applications per query recorded"

gate4:
	OPTIMIZE_REQUIRE=p0,p1,p2,p3,p4 cargo test
	@echo ""
	@echo "── GATE 4 'Top-down finds it too' ──────────────────────────────"
	@echo "[ ] cost(volcano) <= cost(selinger) on every query (test)"
	@echo "[ ] ORDER BY queries: order via ordered path or explicit"
	@echo "    enforcer — never by accident"
	@echo "[ ] memo forensics in LEDGER.md: groups/exprs/rule apps per query"

gate5:
	OPTIMIZE_REQUIRE=all cargo test
	@echo ""
	@echo "── GATE 5 'Same plans, less work' ──────────────────────────────"
	@echo "[ ] plan + cost identical to p4 on every query (test)"
	@echo "[ ] strictly fewer rule apps AND exprs than p4 on >=80% of"
	@echo "    multi-join queries; never more, anywhere (test)"
	@echo "[ ] LEDGER.md final table: 1976→1995, one corpus, shrinking"
	@echo "    search effort — the ladder's punchline"

# `make bench P=p1` → pass p1 through; bare `make bench` → matrix
ifeq ($(origin P), command line)
P_ARG := $(P)
endif
