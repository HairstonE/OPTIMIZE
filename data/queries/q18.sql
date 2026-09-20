-- 5-way with a self-join through aliases (customers sharing a nation),
-- in hostile textual order. The only corpus query where aliases matter.
SELECT c1.c_name, c2.c_name, o1.o_orderkey
FROM lineitem, orders AS o1, customer AS c1, customer AS c2, part
WHERE o1.o_orderkey = l_orderkey
  AND o1.o_custkey = c1.c_custkey
  AND c1.c_nationkey = c2.c_nationkey
  AND l_partkey = p_partkey
  AND p_brand = 'Brand#55'
  AND c2.c_acctbal < -900.0
