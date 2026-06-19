Red []
; Blocks are data first, code second (homoiconicity). A block sitting in an
; evaluated position is returned as-is; only do/reduce/parse/etc. walk it.

data: [red green blue 1 2 3 [nested [deeply] block]]
print "mold the block:"
print data

print "first item:"
print first data

print "length:"
print length? data

print "nested block at index 7:"
print first at data 7

print "block? check:"
print block? data

; reduce evaluates every value in a block and collects the results.
; Here each word evaluates to its bound value (a color name).
red: "RED"
green: "GREEN"
blue: "BLUE"
print reduce [red green blue]

; do evaluates a block and returns the last value.
print do [1 + 2 3 + 4 5 * 6]

; A block is data: you can mold it and re-load it losslessly.
roundtrip: [foo: 5 bar: foo + 1]
probe roundtrip
; Re-evaluating the roundtrip block would bind foo/bar into the user context
; and run the assignments. Try it:
do roundtrip
print bar