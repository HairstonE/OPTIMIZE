-- 4-way; the highly selective filter (one customer) is textually last
SELECT o_orderkey, o_totalprice, p_retailprice
FROM orders, lineitem, part, customer
WHERE o_orderkey = l_orderkey
  AND l_partkey = p_partkey
  AND o_custkey = c_custkey
  AND c_custkey = 77
