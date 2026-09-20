-- kitchen sink: 5-way (orders twice via alias), selective filters,
-- ORDER BY with unique tiebreak chain, LIMIT
SELECT c_name, o1.o_orderdate, p_name, l_extendedprice
FROM lineitem, orders AS o1, orders AS o2, customer, part
WHERE l_orderkey = o1.o_orderkey
  AND l_partkey = p_partkey
  AND o1.o_custkey = c_custkey
  AND o2.o_custkey = c_custkey
  AND o2.o_totalprice > 90000.0
  AND p_brand = 'Brand#12'
  AND o1.o_orderdate > '1997-01-01'
ORDER BY l_extendedprice DESC, o1.o_orderkey, l_linenumber, o2.o_orderkey
LIMIT 25
