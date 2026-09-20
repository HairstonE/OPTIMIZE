-- OR/AND mix with inequality
SELECT p_partkey, p_brand, p_size
FROM part
WHERE (p_size = 5 OR p_size = 10) AND p_brand <> 'Brand#11'
