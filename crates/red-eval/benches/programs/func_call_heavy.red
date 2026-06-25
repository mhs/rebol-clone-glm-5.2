Red []
; Tight does invocation 1M times — pure function-call overhead,
; the canonical VM win case.
f: does [1]
acc: 0
repeat i 1000000 [acc: f]
print acc
