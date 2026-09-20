-- explicit INNER JOIN ... ON: binds to a real equi-join even at P0
SELECT c_name, o_orderkey, o_totalprice
FROM customer INNER JOIN orders ON c_custkey = o_custkey
WHERE o_totalprice > 50000.0
