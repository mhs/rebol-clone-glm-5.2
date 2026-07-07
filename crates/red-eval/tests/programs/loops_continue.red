Red []
; Exercise `continue` in loop/repeat/for — the Err(Continue) arms in every
; loop native are uncovered (loops.red only uses break).
repeat i 3 [if i = 2 [continue] prin i]
print ""
for j 1 5 1 [if j = 3 [continue] prin j]
print ""
