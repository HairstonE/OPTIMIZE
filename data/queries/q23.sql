-- pathological nesting: NOT over OR, duplicate subterms both branches
SELECT o_orderkey
FROM orders
WHERE NOT (o_orderstatus = 'P' OR (o_totalprice < 1000.0 AND o_totalprice < 1000.0))
  AND (o_orderpriority = '1-URGENT' OR o_orderpriority = '1-URGENT')
