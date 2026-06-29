Red []
; char! type (Milestone 38)
;
; Single characters with their own type — distinct from both 1-char strings
; and integer codepoints. Literals use the #"..." form, supporting escapes:
;   #"a"           single character
;   #"^-"          caret escape (^- tab, ^/ newline, ^@ null, ^M-C meta)
;   #"^(41)"       codepoint in hex
; `mold` always round-trips back to the same value.

print "literals:"
print #"a"
print #"A"
print #"^-"               ; tab character (molded as the escape form)
print #"^(41)"            ; codepoint 0x41 -> 'A'
print #"^(1F600)"         ; emoji grin

print "predicates + type:"
print char? #"a"          ; true
print char? 5             ; false
print char? "a"           ; false — a string is not a char
print type? #"a"          ; char!

print "conversions:"
print to-char 66          ; #"B"  (integer codepoint)
print to-char "Z"         ; #"Z"  (first char of string)
print make char! 67       ; #"C"
print to-integer #"A"     ; 65

print "arithmetic (char + int -> char, char - char -> int):"
print #"a" + 1            ; #"b"
print #"a" + 25           ; #"z"
print 5 + #"a"            ; #"f"  (int + char works too)
print #"z" - #"a"         ; 25
print #"b" - #"a"         ; 1

print "comparison + min/max (by codepoint):"
print #"A" < #"B"         ; true
print #"A" = #"A"         ; true
print min #"a" #"z"       ; #"a"
print max #"a" #"z"       ; #"z"

print "string char pick/poke (was integer-codepoint stubs pre-M38):"
s: "abc"
print s/1                 ; #"a"  (was Integer 97 before M38)
print s/2                 ; #"b"
print s/-1                ; #"c"  (negative index from the end)
print s/1 + s/3           ; 196  (char + char promotes to int)
s/2: #"X"                ; poke a char into a string
print s                   ; "aXc"
