Red []
; parse dialect: matcher mini-DSL over string! or block! input.

print "string match:"
print parse "abc" ["a" "b" "c"]

print "block match:"
print parse [a b c] ['a 'b 'c]

print "integer-count repetition:"
digit: charset "0123456789"
print parse "2026-07-03" [4 digit "-" 2 digit "-" 2 digit]
print parse "123" [2 5 digit]

print "capture with copy:"
parse "hello world" [copy w to " " skip copy rest to end]
print w
print rest

print "alternatives with |:"
print parse "b" ["a" | "b" | "c"]

print "repetition with some:"
parse "a;b;c" [some [skip to ";"] to end]
print "ok"

print "side-effects in parens:"
count: 0
parse "xy" ["x" (count: count + 1) "y" (count: count + 1)]
print count
