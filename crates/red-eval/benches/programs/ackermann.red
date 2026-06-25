Red []
; Deep recursion — worst case for the tree-walker's call stack.
ackermann: func [m n][
    either m = 0 [
        n + 1
    ][
        either n = 0 [
            ackermann m - 1 1
        ][
            ackermann m - 1 ackermann m n - 1
        ]
    ]
]
print ackermann 3 5
