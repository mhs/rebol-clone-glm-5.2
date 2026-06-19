Red []
; foreach iterates values from cursor to tail, binding a word each pass
print "foreach over words:"
foreach x [red green blue][print x]

print "foreach over numbers:"
foreach n [1 2 3 4][print n + 10]

; forall binds the word to the positioned series itself, advancing the
; cursor between iterations. Each pass uses first to read the current value.
print "forall prints each value:"
forall 's [a b c][print first s]
