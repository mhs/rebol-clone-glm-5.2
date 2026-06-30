Red []
make-adder: func [n][closure [x][x + n]]
add5: make-adder 5
print add5 10
