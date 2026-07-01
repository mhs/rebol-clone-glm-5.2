; counter — closures with encapsulated mutable state at the module level.
; Demonstrates: closures capturing module-context variables, the
; RefCell cell mechanism (state persists across invocations), and
; independent counters via separate module instances.
;
; Note: the closure-factory pattern (func returning closures) has a VM
; limitation with imported modules — the capture cells aren't set up
; correctly when the func was compiled in a module context but called
; from the script context. Module-level closures (defined directly in
; the module body) work correctly in both VM and walk modes.

module 'counter [
    count-a: 0
    count-b: 100
    count-c: 0

    bump-a: closure [] [count-a: count-a + 1 count-a]
    reset-a: closure [] [count-a: 0 count-a]
    get-a: closure [] [count-a]

    bump-b: closure [] [count-b: count-b + 1 count-b]
    reset-b: closure [] [count-b: 100 count-b]

    ; Clamped counter — won't exceed 3.
    bump-clamped: closure [] [
        count-c: count-c + 1
        either count-c > 3 [count-c: 3] [count-c]
        count-c
    ]
    reset-clamped: closure [] [count-c: 0 count-c]

    export [bump-a reset-a get-a bump-b reset-b bump-clamped reset-clamped]
]
