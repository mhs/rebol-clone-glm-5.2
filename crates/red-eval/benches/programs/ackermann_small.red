Red []
; Smaller, CI-friendly ackermann variant for the regress guard.
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
print ackermann 2 5
