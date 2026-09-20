-- interesting-orders bait: ORDER BY on the join key. A DP entry already
-- ordered on o_orderkey can beat cheapest-unordered + Sort (P2 gate 4).
SELECT o_orderkey, o_totalprice, l_linenumber
FROM orders, lineitem
WHERE o_orderkey = l_orderkey AND l_quantity > 40
ORDER BY o_orderkey, l_linenumber
