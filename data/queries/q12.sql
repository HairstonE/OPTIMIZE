-- 3-way chain; the selective filter sits on the middle-listed table
SELECT p_name, l_quantity, o_orderdate
FROM lineitem, part, orders
WHERE l_partkey = p_partkey
  AND l_orderkey = o_orderkey
  AND p_size = 41
