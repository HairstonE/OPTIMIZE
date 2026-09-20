-- interesting-orders bait (added in P2 — COURSE.md changelog 2026-09-01).
-- The join EXPANDS rows (1000 customers -> ~5000 output), so sorting the
-- small outer early beats sorting the big output late; q20's join
-- contracts, so there the root Sort honestly wins.
-- Projects ONLY c_custkey: tied rows are identical, so the result is
-- deterministic even though the sort key alone underdetermines tie order.
SELECT c_custkey
FROM customer, orders
WHERE c_custkey = o_custkey
ORDER BY c_custkey
