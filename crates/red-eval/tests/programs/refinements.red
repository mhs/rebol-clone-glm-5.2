Red []
; Milestone 13: refinements

; copy/part limits the copied length
print copy/part [1 2 3 4 5] 3

; find/case on strings (case-sensitive match)
print find/case ["apple" "Banana" "cherry"] "Banana"

; append/only keeps a block as a single element
print append/only [1 2] [3 4]

; append default splices a block argument
print append [1 2] [3 4]

; user function with a refinement flag
sign: func [n /strict][
    either strict [
        either n > 0 [1][either n < 0 [-1][0]]
    ][
        either n > 0 ["positive"]["non-positive"]
    ]
]
print sign 5
print sign/strict 5
print sign/strict 0

; user function with a refinement argument
combine: func [x /times y][
    if times [return x * y]
    x + x
]
print combine 5
print combine/times 3 4
