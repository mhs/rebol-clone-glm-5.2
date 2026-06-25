Red []
; repeat accumulator to 1,000,000 — loop overhead hot path.
acc: 0
repeat i 1000000 [acc: acc + i]
print acc
