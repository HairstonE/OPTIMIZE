-- redundancy bait: double negation + duplicate conjunct (P1 pass 4)
SELECT l_orderkey, l_quantity
FROM lineitem
WHERE NOT (NOT (l_quantity > 45)) AND l_quantity > 45
