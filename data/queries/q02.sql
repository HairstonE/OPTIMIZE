-- conjunctive filter + narrow projection on one table
SELECT c_name, c_acctbal
FROM customer
WHERE c_acctbal > 7500.0 AND c_mktsegment = 'BUILDING'
