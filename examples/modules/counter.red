; counter — a closure factory module demonstrating closures with
; encapsulated mutable state. Each counter closure captures a state
; block (snapshot at creation time; writes persist across invocations
; of the SAME closure via the RefCell cell mechanism).
;
; Uses a block as mutable state (poke) rather than SetWord, because
; SetWord inside a closure body is treated as a local by the binding
; pass (not a freevar capture) — a known limitation of the snapshot
; capture model. Shared-cell closures (v0.6) would allow SetWord
; capture directly.

module 'counter [
    ; Create a counter closure starting at `start`. Each call increments
    ; and returns the new value.
    make-counter: func [start] [
        state: reduce [start]
        closure [] [
            poke state 1 ((first state) + 1)
            first state
        ]
    ]

    ; Create a clamped counter that won't exceed `max`. Returns the
    ; current count (clamped) on each call.
    make-clamped: func [start max] [
        state: reduce [start max]
        closure [] [
            cur: (first state) + 1
            lim: second state
            either cur > lim [poke state 1 lim] [poke state 1 cur]
            first state
        ]
    ]

    export [make-counter make-clamped]
]
