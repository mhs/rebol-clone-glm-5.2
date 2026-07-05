Red []
print collect [keep 1 keep 2 keep 3]
print collect [
  repeat i 3 [keep i]
]
r: collect [
  foreach x [1 2 3 4 5] [if odd? x [keep x * x]]
]
print r
