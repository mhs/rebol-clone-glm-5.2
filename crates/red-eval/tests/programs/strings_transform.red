Red []
; Milestone 15: string transformation natives — trim, replace, case
; changes, suffix?, and the string extensions to find/copy. `print`
; forms its argument (no quotes for strings); `none` (from `suffix?`
; with no extension, or `find` with no match) forms to the bare word
; `none`.

print trim "  hi  "
print trim/all "  a  b  "
print replace "a-a" "a" "b"
print replace/all "a-a" "a" "b"
print uppercase "abc"
print uppercase/part "abc" 2
print lowercase "ABC"
print suffix? "foo.txt"
print suffix? "foo"
print find "hello" "ll"
print find "hello" "zz"
print copy "abc"
print copy/part "abc" 2
