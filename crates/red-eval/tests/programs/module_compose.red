Red []
adder: module 'adder [
    make-adder: func [n][closure [x][x + n]]
    export 'make-adder
]
import 'adder
add10: make-adder 10
print add10 5
print add10 20
