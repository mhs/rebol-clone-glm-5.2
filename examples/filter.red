Red []
; Build a filtered block with foreach + append + if
; Even check: n/2*2 = n  (left-to-right: ((n / 2) * 2) = n)
nums: [1 2 3 4 5 6 7 8]
evens: []
foreach n nums [if n / 2 * 2 = n [append evens n]]
print evens
