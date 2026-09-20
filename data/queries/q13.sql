-- 4-way chain in the worst possible textual order; filter at the far end
SELECT c_name, p_name
FROM part, customer, lineitem, orders
WHERE p_partkey = l_partkey
  AND o_orderkey = l_orderkey
  AND c_custkey = o_custkey
  AND c_acctbal < 0.0
