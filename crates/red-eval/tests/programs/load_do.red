Red []
; `load` parses a string into a block; `do` evaluates it.
; Together they enable string->code->eval (calculator, templating, etc).
calc: function [expr][do load expr]
print calc "1 + 2 * 3"
print calc "10 - 4"
print calc "2 * (3 + 4)"

; `do` also accepts a string directly:
print do "5 + 5"

; `load` returns a block (code is data):
probe load "1 + 2"
