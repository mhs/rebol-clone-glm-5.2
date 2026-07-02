Red []
; /reverse
print mold sort/reverse [3 1 2]
; /skip — sort pairs by first element
print mold sort/skip [b 2 a 1] 2
; /compare — custom comparator (logic! result)
print mold sort/compare [3 1 2] func [a b][a < b]
; /compare — custom comparator (integer! sign result)
print mold sort/compare [3 1 2] func [a b][a - b]
; /case — case-sensitive string sort
print mold sort/case ["Banana" "apple" "Cherry"]
; default (case-insensitive) string sort
print mold sort ["Banana" "apple" "Cherry"]
; /reverse on a string
print mold sort/reverse "cba"
