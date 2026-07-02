Red []
; unique
print mold unique [1 2 2 3 1]
print mold unique "aabbcc"
; intersect
print mold intersect [1 2 3] [2 3 4]
print mold intersect "abcdef" "cdefgh"
; union
print mold union [1 2] [2 3]
print mold union "abc" "cde"
; difference (symmetric)
print mold difference [1 2 3] [2 4]
; exclude (set difference)
print mold exclude [1 2 3] [2 4]
; /case refinement on set ops
print mold intersect/case ["A" "b" "C"] ["a" "B" "c"]
print mold intersect ["A" "b" "C"] ["a" "B" "c"]
