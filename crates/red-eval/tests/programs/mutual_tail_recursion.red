Red []
; Exercise tail_call different-function path (L1440-1450 in vm.rs).
; even?/odd? mutual tail recursion — the existing tail-recursion tests
; (countdown/fact-tail) only test TailReenter (same-function reuse).
; Uses a small N to avoid stack overflow in Walk mode (the walker has no
; tail-call optimization — parity runs both VM and Walk).
even?: func [n][either n = 0 [true][odd? n - 1]]
odd?: func [n][either n = 0 [false][even? n - 1]]
print even? 100
