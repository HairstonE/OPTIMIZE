-- ORDER BY + LIMIT, single table (tiebreaker c_custkey keeps it total)
SELECT c_custkey, c_name, c_acctbal
FROM customer
WHERE c_mktsegment = 'HOUSEHOLD'
ORDER BY c_acctbal DESC, c_custkey
LIMIT 10
