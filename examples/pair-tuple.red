Red []
; pair! and tuple! (Milestone 44)
;
; pair!  — a 2D point (x, y) of integers or floats. Literal: `NxM`
;          (e.g. `100x200`, `1.5x2.5`).
; tuple! — an RGB(A) color (3 or 4 bytes, 0-255). Literal: `R.G.B` or `R.G.B.A`
;          (e.g. `255.0.0`, `128.64.32.128`). Two dots (not one) disambiguates
;          from a float.
; Both are immutable value types — set-path returns a new value.

print "literals:"
print 100x200                              ; 100x200
print 0x0                                  ; 0x0
print 1.5x2.5                              ; 1.5x2.5
print 255.0.0                              ; 255.0.0
print 128.64.32.128                        ; 128.64.32.128   (RGBA)
print 0.0.0                                ; 0.0.0

print "type + predicates:"
print type? 1x2                            ; pair!
print type? 1.2.3                         ; tuple!
print pair? 1x2                           ; true
print tuple? 1.2.3                       ; true
print length? 1x2                         ; 2
print length? 1.2.3.4                     ; 4

print "pair arithmetic (componentwise + scalar):"
print 100x200 + 50x50                     ; 150x250
print 1x2 + 10                            ; 11x12   (int scalar to both components)
print 10 + 1x2                           ; 11x12   (scalar on the left works too)
print 100x200 - 50x50                     ; 50x150
print 2x3 * 3x4                           ; 6x12
print 2x3 * 2                            ; 4x6
print 10x20 / 2                          ; 5x10
print 100x200 + 1.5x2.5                   ; 101.5x202.5  (int+float -> float pair)
print negate 5x10                         ; -5x-10
print abs -5x-10                         ; 5x10
print min 1x2 3x4                        ; 1x2     (componentwise)
print max 1x2 3x4                        ; 3x4

print "tuple arithmetic (clamped to 0-255):"
print 255.0.0 + 0.10.0                    ; 255.10.0
print 255.0.0 - 10.20.30                  ; 245.0.0
print 100.50.25 * 0.5                     ; 50.25.13
print 100.50.25 * 2                       ; 200.100.50

print "pair path access (x/y or 1/2):"
p: 100x200
print p/x                                  ; 100
print p/y                                  ; 200
print p/1                                  ; 100   (positional)
print p/2                                  ; 200

print "pair set-path (returns new value — immutable):"
p/x: 5
print p                                   ; 5x200  (set-path returns the new pair, p now holds it)
p/2: 7
print p                                   ; 5x7

print "tuple path access (r/g/b/a, or aliases red/green/blue, or 1-4):"
t: 255.0.0
print t/r                                 ; 255
print t/red                               ; 255   (alias)
print t/g                                 ; 0
print t/b                                 ; 0
print t/1                                 ; 255   (positional)
ta: 128.64.32.128
print ta/a                                ; 128   (alpha — only RGBA tuples have this)
print ta/4                                ; 128

print "tuple set-path:"
t/r: 100
print t                                   ; 100.0.0
t/2: 50
print t                                   ; 100.50.0
ta/a: 255
print ta                                  ; 128.64.32.255

print "converters + make:"
print to-pair [100 200]                   ; 100x200
print make pair! [5 10]                   ; 5x10
print make pair! 3                        ; 3x0   (int -> pair of (n, 0))
print to-tuple [255 0 0]                  ; 255.0.0
print make tuple! [10 20 30]              ; 10.20.30
print make tuple! 3                       ; 0.0.0    (3-component zero tuple)
print make tuple! 4                       ; 0.0.0.0  (4-component zero tuple)
print to-tuple 1.2.3                     ; 1.2.3   (identity)
print to-pair [1.5 2.5]                  ; 1.5x2.5

print "equality (no ordering for either):"
print 255.0.0 = 255.0.0                  ; true
print 255.0.0 <> 255.0.0                 ; false
print 1x2 = 1x2                          ; true
print 1x2 <> 2x1                         ; true
