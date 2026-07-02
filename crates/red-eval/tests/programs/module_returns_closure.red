Red []
m: module [
    make-cl: func [n][closure [][n]]
    export 'make-cl
]
c: m/make-cl 42
print c
