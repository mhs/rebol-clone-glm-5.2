Red []
; use scopes a set of words as locals for the duration of a block.
; Body SetWords for the listed words write to the child context, so they
; don't leak into the surrounding script.
print "use with one local:"
result: use [x][
    x: 10
    x + 5
]
print result

print "use locals don't leak:"
use [n][
    n: 42
    print n
]
; n is unbound here; referencing it would error. Use value? to check.
print value? 'n

print "two locals, computed in the use:"
print use [a b][
    a: 3
    b: 4
    a * a + b * b
]

print "use nested in a function:"
hypot: func [a b][
    use [sum][
        sum: a * a + b * b
        sum
    ]
]
print hypot 3 4
