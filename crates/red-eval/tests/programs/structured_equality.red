Red []
; Exercise values_equal arms for structured/container types (compare.rs).
; The existing golden suite only tested scalar equality (integer/string/...).
a: make object! [x: 1]
b: make object! [x: 1]
print a = b
m1: make map! [a 1]
m2: make map! [a 1]
print m1 = m2
h1: make hash! [a 1]
h2: make hash! [a 1]
print h1 = h2
v1: make vector! [1 2 3]
v2: make vector! [1 2 3]
print v1 = v2
img1: make image! [1 1 [10 20 30 40]]
img2: make image! [1 1 [10 20 30 40]]
print img1 = img2
bs1: make bitset! #{FF}
bs2: make bitset! #{FF}
print bs1 = bs2
ts1: make typeset! [integer!]
ts2: make typeset! [integer!]
print ts1 = ts2
print 1-Jan-2024 = 1-Jan-2024
; Decimal cross-type ordering (Dec vs Int, Dec vs Float).
print 3.14dec < 5
print 3.14dec > 1.0
