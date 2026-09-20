#!/usr/bin/env python3
"""[scaffold] Seeded data generator for the OPTIMIZE! corpus.

Writes data/{customer,orders,lineitem,part}.csv plus:
  data/stats.toml      — TRUE row counts and NDVs, computed from the data
  data/stats-lie.toml  — deliberately skewed stats for the P2 lab exercise

Stdlib only; python3 data/gen_data.py regenerates everything
byte-identically (seed 799). The CSVs are also checked into git so learners
don't even need Python to start.

Deliberate design decisions (mirrored in src/catalog.rs and COURSE.md §2.3):
  - no NULLs anywhere (two-valued logic suffices for the whole ladder)
  - dates are ISO strings, compared as TEXT (lexicographic == chronological)
  - money/discount values have exactly 2 decimals; both engines parse the
    same literal to the same f64, so results compare exactly
  - foreign keys always hit (no dangling l_orderkey etc.), so inner-join
    row counts are predictable from the stats
"""

import csv
import os
import random

random.seed(799)

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = HERE  # the script lives inside data/
os.makedirs(DATA, exist_ok=True)

N_CUSTOMER = 1_000
N_ORDERS = 5_000
N_LINEITEM = 10_000
N_PART = 800

SEGMENTS = ["AUTOMOBILE", "BUILDING", "FURNITURE", "HOUSEHOLD", "MACHINERY"]
STATUSES = ["F", "O", "P"]
PRIORITIES = ["1-URGENT", "2-HIGH", "3-MEDIUM", "4-NOT SPECIFIED", "5-LOW"]
RETURNFLAGS = ["A", "N", "R"]
BRANDS = [f"Brand#{i}{j}" for i in range(1, 6) for j in range(1, 6)]  # 25
PART_NOUNS = ["widget", "sprocket", "gear", "flange", "bolt", "washer",
              "bracket", "spindle", "gasket", "coupling"]
PART_COLORS = ["red", "green", "blue", "ivory", "khaki", "lavender",
               "maroon", "navy", "olive", "plum"]


def money(lo, hi):
    return round(random.uniform(lo, hi), 2)


def a_date(y0=1992, y1=1998):
    y = random.randint(y0, y1)
    m = random.randint(1, 12)
    d = random.randint(1, 28)
    return f"{y:04d}-{m:02d}-{d:02d}"


# ── generate ──────────────────────────────────────────────────────────────

customer = [
    {
        "c_custkey": k,
        "c_name": f"Customer#{k:09d}",
        "c_nationkey": random.randint(0, 24),
        "c_mktsegment": random.choice(SEGMENTS),
        "c_acctbal": money(-999.99, 9999.99),
    }
    for k in range(1, N_CUSTOMER + 1)
]

orders = [
    {
        "o_orderkey": k,
        "o_custkey": random.randint(1, N_CUSTOMER),
        "o_orderstatus": random.choice(STATUSES),
        "o_totalprice": money(1_000.00, 100_000.00),
        "o_orderdate": a_date(),
        "o_orderpriority": random.choice(PRIORITIES),
    }
    for k in range(1, N_ORDERS + 1)
]

# 10k lineitems over 5k orders: every order gets 1, the rest are spread
# randomly, l_linenumber is a per-order sequence (o_orderkey, l_linenumber
# is a unique key — ORDER BY tiebreakers rely on this).
lineitem = []
per_order = {k: 0 for k in range(1, N_ORDERS + 1)}
owners = list(range(1, N_ORDERS + 1)) + [
    random.randint(1, N_ORDERS) for _ in range(N_LINEITEM - N_ORDERS)
]
owners.sort()
for okey in owners:
    per_order[okey] += 1
    qty = random.randint(1, 50)
    price = money(100.00, 10_000.00)
    lineitem.append(
        {
            "l_orderkey": okey,
            "l_partkey": random.randint(1, N_PART),
            "l_linenumber": per_order[okey],
            "l_quantity": qty,
            "l_extendedprice": price,
            "l_discount": round(random.randint(0, 10) / 100, 2),
            "l_shipdate": a_date(1992, 1998),
            "l_returnflag": random.choice(RETURNFLAGS),
        }
    )

part = [
    {
        "p_partkey": k,
        "p_name": f"{random.choice(PART_COLORS)} {random.choice(PART_NOUNS)} #{k}",
        "p_brand": random.choice(BRANDS),
        "p_size": random.randint(1, 50),
        "p_retailprice": money(50.00, 2_000.00),
    }
    for k in range(1, N_PART + 1)
]

TABLES = {
    "customer": customer,
    "orders": orders,
    "lineitem": lineitem,
    "part": part,
}


def write_csv(name, rows):
    path = os.path.join(DATA, f"{name}.csv")
    with open(path, "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0].keys()))
        w.writeheader()
        w.writerows(rows)
    print(f"wrote {path} ({len(rows)} rows)")


for name, rows in TABLES.items():
    write_csv(name, rows)

# ── stats (truth) ─────────────────────────────────────────────────────────

def ndvs(rows):
    return {col: len({r[col] for r in rows}) for col in rows[0].keys()}


with open(os.path.join(DATA, "stats.toml"), "w") as f:
    f.write("# TRUE stats, computed from the generated data by gen_data.py.\n")
    f.write("# rows + per-column NDV only; anything fancier is SEAM-COST.\n\n")
    for name, rows in TABLES.items():
        f.write(f"[{name}]\nrows = {len(rows)}\n\n")
        f.write(f"[{name}.ndv]\n")
        for col, n in ndvs(rows).items():
            f.write(f"{col} = {n}\n")
        f.write("\n")
print("wrote stats.toml")

# ── stats (the lie) ───────────────────────────────────────────────────────
# P2 lab exercise: same schema, wrong numbers. The lies are chosen to
# invert the DP's decisions, not just perturb them:
#   - relative table sizes are REVERSED (lineitem claims tiny, customer
#     claims huge) → the DP builds from the wrong end
#   - join-key NDVs are crushed to 2 → equi-join selectivity 1/2 instead of
#     1/ndv → join outputs look enormous or trivial in the wrong places
#   - filter-column NDVs are inflated → filters look far more selective
#     than they are

LIE_ROWS = {"customer": 500_000, "orders": 40, "lineitem": 25, "part": 900_000}
JOIN_KEYS = {"c_custkey", "o_custkey", "o_orderkey", "l_orderkey",
             "l_partkey", "p_partkey"}

with open(os.path.join(DATA, "stats-lie.toml"), "w") as f:
    f.write("# DELIBERATELY WRONG stats — the P2 lab exercise.\n")
    f.write("# Table sizes reversed, join-key NDVs crushed, filter NDVs\n")
    f.write("# inflated. Run: OPTIMIZE_STATS=data/stats-lie.toml ... and\n")
    f.write("# watch a correct DP pick a terrible plan. Write three\n")
    f.write("# sentences in LEDGER.md about why.\n\n")
    for name, rows in TABLES.items():
        f.write(f"[{name}]\nrows = {LIE_ROWS[name]}\n\n")
        f.write(f"[{name}.ndv]\n")
        for col, n in ndvs(rows).items():
            lie = 2 if col in JOIN_KEYS else n * 100
            f.write(f"{col} = {lie}\n")
        f.write("\n")
print("wrote stats-lie.toml")
