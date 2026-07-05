Red []
o: make object! [x: 1 y: 2]
print set? 'x
print value? 'x
print has o 'x
print has o 'z
f: func [a b /ref c] [a + b]
print spec-of :f
print body-of :f
o2: make object! [z: 99]
resolve o o2
print o/z
extend o [w: 42]
print o/w
print context? o
b: [1 2 3]
protect b
print mold b
unprotect b
append b 4
print b
