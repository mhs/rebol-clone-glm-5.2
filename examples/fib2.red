Red []
a: 0 b: 1 result: copy []
repeat i 30 [
    append result a
    tmp: a + b
    a: b
    b: tmp
]
print result