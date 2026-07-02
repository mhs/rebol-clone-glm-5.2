Red []
m: module [
    outer: func [n][
        inner: closure [][n]
        inner
    ]
    export 'outer
]
add5: m/outer 5
print add5
