-- explicit join with a selective dimension filter
SELECT p_name, l_extendedprice
FROM part INNER JOIN lineitem ON p_partkey = l_partkey
WHERE p_brand = 'Brand#23'
