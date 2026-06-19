Red []
; Build a mapped block with foreach + append
nums: [1 2 3 4]
squares: []
foreach n nums [append squares n * n]
print squares
