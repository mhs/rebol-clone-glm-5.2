Red []
; Trig & transcendentals (Milestone 40)
;
; New math natives operating on Integer (promotes to Float) and Float. The
; constants `pi` and `e` are installed in the global context alongside
; `true`/`false`/`none`/`newline`. All angles are radians.

print "constants:"
print pi                                    ; 3.141592653589793
print e                                     ; 2.718281828459045

print "degrees <-> radians:"
print degrees pi                            ; 180.0
print degrees (pi / 2)                      ; 90.0
print radians 180                           ; 3.141592653589793

print "basic trig:"
print sin 0                                 ; 0.0
print cos 0                                 ; 1.0
print tan 0                                 ; 0.0
print sin (pi / 2)                          ; 1.0
print cos pi                                ; -1.0
print sin (radians 90)                      ; 1.0

print "inverse trig (radians):"
print asin 1                               ; 1.5707963267948966  (pi/2)
print acos 1                               ; 0.0
print atan 1                               ; 0.7853981633974483  (pi/4)

print "atan2 (2-arg: y, x):"
print atan2 1 1                            ; pi/4
print atan2 0 1                            ; 0.0
print atan2 1 0                            ; pi/2

print "powers, roots, logs:"
print sqrt 16                              ; 4.0
print sqrt 2                               ; 1.4142135623730951
print exp 1                                ; 2.718281828459045   (= e)
print exp 0                                ; 1.0
print log-e e                              ; 1.0
print log-10 1000                          ; 3.0
print log-2 8                              ; 3.0

; --- a practical use: convert polar (r, theta) to cartesian (x, y) ---
r: 2.0
theta: pi / 4                              ; 45 degrees
x: r * cos theta
y: r * sin theta
print x                                    ; ~1.4142
print y                                    ; ~1.4142
