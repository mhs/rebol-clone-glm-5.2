Red []
; Shared storage: aliases see mutations; copy breaks sharing
a: [1 2]
b: a
append a 3
append b 4
print a
; copy produces an independent series
c: copy a
append c 99
print a
print c
