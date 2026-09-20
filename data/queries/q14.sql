-- star around lineitem with two dimension filters
SELECT l_orderkey, l_linenumber
FROM lineitem, part, orders
WHERE l_partkey = p_partkey
  AND l_orderkey = o_orderkey
  AND p_brand = 'Brand#44'
  AND o_orderstatus = 'O'
