Red []
; Milestone 15: `split` returns a block of string parts. Molded blocks
; render as `[a b c]` with each string part molded (quoted).

print split "a,b,c" ","
print split "abc" ""
print split/with "x-y-z" "-"
