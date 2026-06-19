Red []
; Type conversions + make/to/form (Milestone 14)
;
; The to-* family converts any value to the named type. `make <type> <spec>`
; constructs a new value of that type from a spec; `to <type> <value>` is the
; conversion alias. `form` renders a value as human-readable text (no quotes,
; no block brackets) — distinct from `mold`, which produces reparseable source.

; --- to-integer: truncate floats, parse strings, coerce logic ---
print to-integer 3.9          ; 3
print to-integer "-42"        ; -42
print to-integer true         ; 1
print to-integer none         ; 0

; --- to-float ---
print to-float 7              ; 7.0
print to-float "3.14"         ; 3.14

; --- to-string is `form`: human-readable, space-joined, no brackets ---
print to-string 42            ; "42"
print to-string [1 2 3]       ; "1 2 3"
print to-string 'word         ; "word"

; --- to-block: load a string, or wrap a word ---
print to-block "10 20 30"     ; [10 20 30]
print to-block 'foo           ; [foo]

; --- to-word family: build words from strings or other words ---
print to-word "dynamic"       ; dynamic
print to-set-word "field"     ; field:
print to-get-word "ref"       ; :ref
print to-lit-word "quoted"    ; 'quoted

; --- to-logic: Red truthiness (only false/none are falsy) ---
print to-logic 0              ; true
print to-logic false          ; false
print to-logic none           ; false
print to-logic ""             ; true

; --- make: constructor dispatch on the type word ---
print make integer! 3.9       ; 3   (truncates)
print make float! 5           ; 5.0
print make string! 5          ; ""  (integer is a capacity hint, per Red)
print make block! 3           ; []  (capacity hint)

; make function! still works (the original M9 form)
square: make function! [[x][x * x]]
print square 9                ; 81

; --- to: conversion alias (differs from make for string!) ---
print to integer! 2.5         ; 2
print to string! 99           ; "99"  (value rendered, unlike make string! 99)

; --- form vs mold ---
; form returns a string! with no delimiters; print then molds that string
; (quotes appear because print molds every arg — a documented POC choice).
print form [a b c]            ; "a b c"
print form "raw text"         ; "raw text"
print form 'symbol            ; "symbol"

; Round-trip: number -> string -> number
n: to-integer to-string 123
print n                       ; 123
print n + 1                   ; 124

; Parse user input as a number
age: to-integer "29"
next-age: age + 1
prin "age next year: "
print next-age               ; age next year: 30
