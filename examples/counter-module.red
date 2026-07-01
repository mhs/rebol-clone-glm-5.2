Red []
; A mini counter module using closures for encapsulated mutable state.
; Each closure captures `count` by value (snapshot at creation time).
; Writes to the capture cell persist across invocations of the SAME
; closure (the RefCell cell mechanism). Demonstrates closures + modules
; + import + control-flow inside closure bodies (Bug 3 fix).

c1: module 'c1 [
    count: 0
    bump: closure [] [count: count + 1 count]
    reset: closure [] [count: 0 count]
    export 'bump
    export 'reset
]

c2: module 'c2 [
    count: 100
    bump: closure [] [count: count + 1 count]
    reset: closure [] [count: 100 count]
    export 'bump
    export 'reset
]

; Method-call path: `module/word` resolves the exported closure and
; calls it with the module's ctx as user_ctx.
prin "c1/bump = " print c1/bump     ; 1
prin "c1/bump = " print c1/bump     ; 2
prin "c1/bump = " print c1/bump     ; 3
prin "c2/bump = " print c2/bump     ; 101 (independent counter)
; reset has its own capture cell (starts at 0), independent of bump's.
prin "c1/reset = " print c1/reset   ; 0
prin "c1/bump = " print c1/bump     ; 4 (bump's cell continues, unaffected by reset)

; import aliases the exported words as bare words (Bug 4 fix: imported
; functions are now callable bare in VM mode).
import 'c1
prin "bump = " print bump           ; 5 (continues from c1's bump cell)
prin "reset = " print reset         ; 0 (reset's own cell)

; module? confirms the type.
prin "module? c1 = " print module? c1         ; true
prin "words-of c1 = " print words-of c1       ; [bump reset]
