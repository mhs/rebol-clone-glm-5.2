Red []
o: make object! [
    name: "Widget"
    count: 0
    inc: does [count: count + 1]
    reset: does [count: 0]
]

o/inc
o/inc
o/inc
print o/count
o/reset
print o/count
print o/name
