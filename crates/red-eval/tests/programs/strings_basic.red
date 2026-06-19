Red []
; Milestone 15: string-building natives — rejoin, reform, join, and `+`
; over strings. `print` molds strings (adds quotes), so each line of
; output is the molded form of the result.

print rejoin ["a" 1 "b"]
print reform ["a" "b"]
print join "a" "b"
print "abc" + "def"
