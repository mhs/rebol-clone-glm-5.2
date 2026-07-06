Red []
; M150: decimal! value type — literal syntax, mold round-trip, predicates.
print 3.14dec
print 100dec
print 0dec
print -2.5dec
print mold 3.14dec
print mold 100dec
print mold 0dec
print decimal? 3.14dec
print decimal? 3.14
print type? 3.14dec
; M150: exact arithmetic — the float! surprise fixed.
print 0.1dec + 0.2dec
print 3.14dec + 1
print 10dec - 3dec
print 6dec * 7dec
print 10dec / 4dec
print 10dec // 3dec
print 2dec ** 3
; M150: mixed-type promotion — Float wins on mix.
print 3.14dec + 1.0
print 3.14dec + 1
; M150: equality and ordering.
print 3.14dec = 3.14dec
print 3.14dec = 3.14
print 3.14dec < 4dec
print 3.14dec > 2dec
; M150: conversions.
print to-decimal 5
print to-decimal 3.14
print to-decimal "1.5"
print to-integer 3.7dec
print to-float 3.14dec
print make decimal! 5
print make decimal! "2.5"
; M150: math helpers preserve decimal! type.
print abs -3.5dec
print negate 5dec
print floor 3.7dec
print ceiling 3.2dec
print truncate 3.9dec
print min 3dec 5dec
print max 3dec 5dec
print round 3.7dec
; M150: transcendentals return float! (computed via f64 internally).
print sin 0dec
print cos 0dec
print sqrt 16dec
print log-10 100dec
; M150: typeset integration.
ts: make typeset! [decimal!]
print ts
print make typeset! [number!]
