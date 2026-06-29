Red []
m: make map! [a: 1 b: 2]
print m/a
print m/b
m/c: 3
print m/c
m/a: 9
print m/a
print length? m
n: make map! [1 "one" 2 "two"]
print n/1
print n/2
print select n 2
print find n 2
