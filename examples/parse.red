Red []
; parse dialect: matcher mini-DSL over string! or block! input.

print "string match:"
print parse "abc" ["a" "b" "c"]

print "block match:"
print parse [1 2 3] [1 2 3]

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
