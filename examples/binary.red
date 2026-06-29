Red []
; binary! type (Milestone 41)
;
; A byte string. Literals use the #{hex} form (uppercase hex, even/odd count
; — odd is high-nibble zero-padded). Distinct from string! (UTF-8 chars) and
; char! (single codepoint). Byte-indexed with value semantics — like string!,
; poke/append/insert return a new binary; aliases don't see updates.

print "literals:"
print #{48656C6C6F}                        ; "Hello" as bytes
print #{00FF}                              ; two bytes
print #{ABC}                               ; odd count -> zero-padded to 0ABC
print #{}                                  ; empty binary
print type? #{0102}                        ; binary!

print "predicates:"
print binary? #{0102}                      ; true
print binary? 5                            ; false
print binary? "hi"                         ; false  (a string is not a binary)

print "conversions:"
print to-binary "hi"                       ; #{6869}        (string -> UTF-8 bytes)
print to-binary 1                          ; #{0000000000000001}  (int -> big-endian 8 bytes)
print make binary! [65 66 67]              ; #{414243}      (block of ints -> bytes)
print make binary! [#"A" "xy"]            ; #{417879}      (char/string elements too)
print to-string #{6869}                   ; "hi"           (UTF-8 decode)

print "series ops (byte-indexed):"
print length? #{0102}                     ; 2
print length? #{}                          ; 0
print pick #{4142} 1                       ; 65   ('A' as integer 0-255)
print pick #{4142} 2                       ; 66
print pick #{4142} -1                      ; 66   (negative index from end)
print pick #{4142} 3                       ; none (out of range)
print poke #{4142} 1 99                    ; #{6342}
print poke #{4142} 2 #"Z"                 ; #{415A}
print append #{4142} #{43}                ; #{414243}
print append #{} 99                        ; #{63}
print insert #{42} #{41}                  ; #{4142}
print copy #{0102}                        ; #{0102}
print copy/part #{01020304} 2             ; #{0102}

print "find on a binary:"
print find #{01020301} #{01}             ; 1   (position of first match)
print find #{48656C6C6F} #{65}            ; 2
print find #{0102} #{0304}                ; none

print "equality (byte-by-byte):"
print #{00} = #{00}                       ; true
print #{01} = #{02}                       ; false
print #{48} = "H"                         ; false (binary! and string! are never equal)
