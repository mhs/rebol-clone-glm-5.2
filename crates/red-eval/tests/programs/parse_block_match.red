Red []
; M138: every integer! in a rule block is a count prefix, so literal
; integer matching against block input uses lit-words / strings / `match`
; instead.
print parse [a b c] ['a 'b 'c]
print parse ["x" "y"] ["x" "y"]
; count prefix: `3 match 3` = match literal-3 exactly 3 times.
print parse [3 3 3] [3 match 3]
print parse [3 3] [3 match 3]
