-- disconnected join graph: a true cross product (tiny after filters).
-- P2's DP must refuse cross products and fall back to greedy.
SELECT c_name, p_name
FROM customer, part
WHERE c_custkey = 5 AND p_size = 33
ORDER BY c_name, p_name
LIMIT 20
