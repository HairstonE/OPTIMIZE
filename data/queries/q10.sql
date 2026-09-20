-- projection pruning bait: one output column, wide tables underneath
SELECT o_orderkey
FROM orders, customer
WHERE o_custkey = c_custkey AND c_nationkey = 7
