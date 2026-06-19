Red []
; func defines a named function: spec block of params, body block.
; Functions see their own params as locals and can call themselves
; (recursion) since the function's name resolves via the user context.
print "square:"
square: func [x][x * x]
print square 5
print square 12

print "two params:"
sum: func [a b][a + b]
print sum 3 4

print "recursion (factorial):"
fact: func [n][either n <= 1 [1][n * fact n - 1]]
print fact 5
print fact 10

print "return exits early:"
classify: func [n][
    if n < 0 [return "negative"]
    if n = 0 [return "zero"]
    "positive"
]
print classify -5
print classify 0
print classify 7

print "does is a zero-arg func:"
greet: does [print "hello"]
greet
greet

print "make function! packed form:"
dbl: make function! [[x][x + x]]
print dbl 21
