; mathutils — a module of pure math utility functions.
; Demonstrates: a pure-function module with no closures, just func
; definitions exported for reuse.

module 'mathutils [
    sq: func [x][x * x]
    cube: func [x][x * x * x]
    is-even?: func [n][n // 2 = 0]
    is-odd?: func [n][n // 2 <> 0]
    ; Triangular number: 1+2+...+n = n*(n+1)/2.
    triangular: func [n][(n * (n + 1)) / 2]

    export [sq cube is-even? is-odd? triangular]
]
