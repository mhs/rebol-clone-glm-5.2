Red []
o: make object! [
    x: 10
    y: 20
    z: 30
    get-sum: does [x + y + z]
]

print words-of o
print values-of o
print reflect o 'values
print o/get-sum
