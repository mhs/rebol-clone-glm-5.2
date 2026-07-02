Red []
m: module [
    base: 100
    adder: closure [x][x + base]
    export 'adder
]
print m/adder 5
