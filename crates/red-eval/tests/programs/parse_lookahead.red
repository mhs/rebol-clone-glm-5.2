Red []
print parse "abc" [ahead "a" "a" "b" "c"]
print parse "abc" [not "z" "a" "b" "c"]
print parse "abc" [fail | "a" "b" "c"]
