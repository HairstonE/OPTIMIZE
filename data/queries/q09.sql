-- comma-join with single-relation filters on BOTH sides (pushdown bait)
SELECT c_name, o_orderdate
FROM customer, orders
WHERE c_custkey = o_custkey
  AND c_mktsegment = 'MACHINERY'
  AND o_orderstatus = 'F'
