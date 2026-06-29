Red []
; map! type (Milestone 43)
;
; An insertion-ordered, heterogeneous-key dictionary. Built on `indexmap` so
; keys iterate in the order they were inserted. Keys can be any of: word,
; integer, string, char, logic, or none — the hashable, non-container Red
; values. Path syntax resolves keys: `m/word`, `m/1`, `m/"str"`, `m/char`.

print "construction (word keys via set-word syntax):"
m: make map! [a: 1 b: 2 c: 3]
print m                                     ; make map! [a: 1 b: 2 c: 3]
print map? m                                ; true
print map? []                               ; false
print type? m                               ; map!

print "path access (get + set):"
print m/a                                  ; 1
print m/b                                  ; 2
m/c: 9                                     ; overwrite
m/d: 4                                     ; new key
print m/c                                  ; 9
print m/d                                  ; 4
print length? m                            ; 4

print "non-word keys (integer, string, char):"
n: make map! [1 "one" 2 "two" #"c" 3]
print n/1                                  ; "one"
print n/2                                  ; "two"
print select n #"c"                       ; 3       (char key — select since #"c" isn't a path element)
print select n 2                          ; "two"   (select returns the value or none)
print find n 2                            ; 2       (find returns the key if present, else none)

print "heterogeneous map round-trips through mold:"
h: make map! [a 1 2 "two" #"c" 3]
print h                                   ; make map! [a 1 2 "two" #"c" 3]

print "iteration order preserved:"
print keys-of h                           ; [a 2 #"c"]
print values-of h                         ; [1 "two" 3]

print "convert from object + from block of pairs:"
o: make object! [a: 1 b: 2]
print to-map o                            ; make map! [a: 1 b: 2]
print make map! [[a 1] [b 2]]             ; make map! [a 1 b 2]

print "copy is independent (mutating copy doesn't affect original):"
c: copy m
c/x: 99                                   ; new key on copy only
print c/x                                 ; 99
print m/x                                 ; none

print "equality + identity:"
print (make map! [a 1]) = (make map! [a 1]) ; true  (deep equality on entries)
print same? m m                            ; true  (Rc identity)
print same? m c                           ; false (different Rc)
