Red []
o: make object! [a: 1 b: 2]
m: to-map o
print m
print m/a
print map? m
m2: make map! o
print m2/b
p: make map! [[a 1] [b 2]]
print p
print p/a
c: copy m
c/x: 99
print c/x
print m/x
print same? m m
print same? m c
print (make map! [a 1]) = (make map! [a 1])
