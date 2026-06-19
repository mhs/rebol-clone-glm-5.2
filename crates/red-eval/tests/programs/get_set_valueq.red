Red []
foo: 42
print value? 'foo
print value? 'missing
print get 'foo
bar: 0
set 'bar 99
print bar
square: make function! [[x][x * x]]
print square 7
print function? :square
print function? 5
