Red []
counter: make object! [
    n: 0
    inc: does [n: n + 1]
    get: does [n]
]
counter/inc
counter/inc
counter/inc
print counter/get
print counter/n
