-- 4-way star, comma-spelled, filters on two different dimensions
SELECT c_name, p_name, l_quantity
FROM part, orders, customer, lineitem
WHERE l_partkey = p_partkey
  AND l_orderkey = o_orderkey
  AND o_custkey = c_custkey
  AND p_size < 10
  AND c_mktsegment = 'FURNITURE'
