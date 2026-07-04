Red []
; hash! vs map! — iteration order, series?, equality.
m: make map! [a: 1 b: 2]
h: make hash! [a: 1 b: 2]
print series? m
print series? h
; equality is order-independent for both
print (make hash! [a 1 b 2]) = (make hash! [b 2 a 1])
; same? is identity
print same? h h
print same? h (make hash! [a 1 b 2])
; convert map! to hash!
print make hash! m
