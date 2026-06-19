Red []
; Milestone 14: form vs molded printing
;
; `form` returns a human-readable string! (no quotes, no block brackets,
; bare word names). `print` molds its argument, so a string! printed via
; `print` appears quoted — but the underlying *value* from `form` is the
; raw string, which we can verify by length and by indexing.

; form of a block joins elements with spaces (no brackets)
s: form [1 2 3]
print s

; form of a word yields the bare name
print form 'hello

; form of an integer yields its decimal text
n: form 4096
print n

; to-string is an alias for form
print to-string [a b c]

; form of a string returns the same string value
print form "unchanged"

; round-trip: number -> string -> number
print to-integer to-string 123
