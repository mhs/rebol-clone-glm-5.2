Red []
; Naive recursive fib — function-call + recursion hot path.
fib: func [n][
    either n < 2 [n] [(fib n - 1) + (fib n - 2)]
]
print fib 30
