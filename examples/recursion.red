Red []
; Recursion: a function calls itself by name. The body's reference to the
; function's own name resolves via the user context, so the function can
; invoke itself. Each call gets its own fresh param/local slots.

print "factorial (linear recursion):"
fact: func [n][either n <= 1 [1][n * fact n - 1]]
print fact 5
print fact 20

print "fibonacci (binary recursion):"
fib: func [n][either n < 2 [n][(fib n - 1) + fib n - 2]]
print fib 6
print fib 30

print "list length (recursing over a block):"
len: func [blk][
    either empty? blk [0][1 + len next blk]
]
print len [a b c d]
print len []

print "list sum (recursing over a block of integers):"
; Parenthesize `first blk` so the `+` chains at the outer level — the POC
; collects prefix-native args as full expressions, so `first blk + ...`
; would otherwise parse as `first (blk + ...)`.
sum-of: func [blk][
    either empty? blk [0][(first blk) + sum-of next blk]
]
print sum-of [1 2 3 4 5]
