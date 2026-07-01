Red []
adder: module 'adder [
    base: 10
    add-base: closure [x][x + base]
    export 'add-base
]
import 'adder
print add-base 5
print add-base 20
