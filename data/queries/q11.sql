-- 3-way comma-join, biggest table listed first (bad textual order)
SELECT c_name, o_orderkey, l_quantity
FROM lineitem, customer, orders
WHERE c_custkey = o_custkey
  AND o_orderkey = l_orderkey
  AND c_nationkey = 3
