-- fold-true conjunct + duplicated predicate
SELECT p_partkey FROM part WHERE 1 = 1 AND p_size = 5 AND p_size = 5
