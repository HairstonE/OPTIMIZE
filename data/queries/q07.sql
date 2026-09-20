-- the same join spelled as a comma-join: P0 executes this as a literal
-- cross product + filter. Feel the difference vs q06; that pain is P1.
SELECT o_orderkey, l_partkey, l_quantity
FROM orders, lineitem
WHERE o_orderkey = l_orderkey AND l_quantity > 30
