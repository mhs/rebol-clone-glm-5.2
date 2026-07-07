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
; Block form of get/set (M135): get [a b c] returns a block of values;
; set [a b c] [v1 v2 v3] sets each; set [a b c] val sets all to val.
a: 1 b: 2 c: 3
print get [a b c]
set [a b c] [10 20 30]
print a
print b
print c
set [a b c] 99
print a
print b
print c
