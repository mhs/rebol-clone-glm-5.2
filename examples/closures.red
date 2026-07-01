Red []
; Closures (closure!) capture free-variable VALUES at creation time
; (snapshot semantics). Unlike `func`, which walks the frame chain to
; read freevars, a `closure` copies them into an owned cell — so the
; closure can escape its defining frame (be returned, stored, passed
; around) and still see the captured values.

; --- basic capture ---
; `y` is captured by value at `closure`-creation time. Later writes to
; `y` do NOT propagate inward (snapshot, not shared cell).
y: 10
f: closure [x][x + y]
prin "f 5 = " print f 5          ; 15
y: 99
prin "f 5 still = " print f 5    ; still 15 — capture was a snapshot

; --- escaping closure (the v0.3 bug fix) ---
; `make-adder` returns a closure closing over `n`. The closure outlives
; `make-adder`'s frame; without capture-by-value, this would read a
; stale/dead frame slot. Multiple closures from the same factory each
; capture their own independent `n` (Bug 1 fix).
make-adder: func [n][closure [x][x + n]]
add5: make-adder 5
add10: make-adder 10
prin "add5 100 = " print add5 100    ; 105
prin "add10 100 = " print add10 100  ; 110

; --- zero-arg closure (does-equivalent) ---
; `closure [] [body]` is the zero-arg form. Useful when you want snapshot
; capture but no parameters.
greeting: "hi"
say-hi: closure [] [print greeting]
say-hi                          ; hi
greeting: "bye"
say-hi                          ; still hi (snapshot)

; --- closure with persistent captured state ---
; `count` is a freevar of the closure (set in the enclosing scope,
; captured by value at creation). Each invocation's `count: count + 1`
; reads AND writes the capture cell, so the value persists across calls
; of the SAME closure (the RefCell cell mechanism).
base: 0
inc: closure [] [
    base: base + 1
    base
]
prin "inc 1 = " print inc     ; 1
prin "inc 2 = " print inc     ; 2
prin "inc 3 = " print inc     ; 3

; --- control-flow inside a closure body ---
; Bug 3 fix: `if`/`do`/`loop` inside a closure body now work correctly —
; the closure's capture cell is propagated through `dispatch_block`.
counter: 0
tick: closure [] [
    if true [counter: counter + 1]
    counter
]
prin "tick 1 = " print tick    ; 1
prin "tick 2 = " print tick    ; 2
prin "tick 3 = " print tick    ; 3

; --- recursive closure ---
; A closure referencing its own name resolves via the outer SetWord
; slot (not via a capture — matches `func` recursion semantics).
fact: closure [n][either n <= 1 [1][n * fact n - 1]]
prin "fact 5 = " print fact 5   ; 120

; --- closure? / function? predicates ---
; Use `:word` (GetWord) to fetch the closure value without calling it.
; A bare `f` in argument position would invoke the closure.
prin "closure? :f = " print closure? :f              ; true
prin "closure? func [] [] = " print closure? func [] []  ; false
prin "function? :f = " print function? :f            ; true (closure is a function)
