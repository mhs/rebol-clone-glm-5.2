Red []
; rejoin over a 1k-iteration string accumulation — string + reduce path.
; `rejoin [acc i]` evaluates `acc` and `i`, forms both, concatenates.
; The deterministic result is the final accumulated string's tail,
; verified via `find` from the last separator onward.
acc: copy ""
repeat i 1000 [acc: rejoin [acc i "-"]]
print find acc "1000-"
