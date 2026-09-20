-- constant-folding bait: literal arithmetic and a tautological conjunct
SELECT o_orderkey, o_totalprice
FROM orders
WHERE o_custkey = 100 + 23 AND 1 + 1 = 2
