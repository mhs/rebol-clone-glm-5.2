Red []
m: module [
    make-adder: func [n][closure [x][x + n]]
    export 'make-adder
]
add5: m/make-adder 5
print add5 10
add20: m/make-adder 20
print add20 1
