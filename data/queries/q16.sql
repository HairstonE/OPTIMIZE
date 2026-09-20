-- explicit JOIN chain written largest-first: fine at P0 (hash joins),
-- but the ORDER of those hash joins is still textual until P2
SELECT c_name, l_extendedprice
FROM lineitem
INNER JOIN orders ON l_orderkey = o_orderkey
INNER JOIN customer ON o_custkey = c_custkey
WHERE c_mktsegment = 'AUTOMOBILE' AND l_discount > 0.05
