Red []
; Milestone 14: type conversions, make/to, form

; --- to-integer ---
print to-integer 3.7
print to-integer "42"
print to-integer true
print to-integer none

; --- to-float ---
print to-float 5
print to-float "3.25"

; --- to-string (== form) ---
print to-string 42
print to-string [1 2 3]
print to-string 'foo

; --- to-block ---
print to-block "10 20 30"
print to-block 'bar

; --- to-word family ---
print to-word "abc"
print to-set-word "x"
print to-get-word "y"
print to-lit-word "z"

; --- to-logic ---
print to-logic 0
print to-logic false
print to-logic none
print to-logic "anything"

; --- make ---
print make integer! 3.9
print make string! 5
print make block! 3
print make float! 7

; make function! still works (regression)
f: make function! [[a][a * a]]
print f 6

; --- to (alias) ---
print to integer! 2.5
print to string! 99

; --- form (human-readable rendering) ---
print form [a b c]
print form "raw string"
print form 42
