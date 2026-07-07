Red []
; Exercise try/attempt control-flow propagation arms (control.rs L646,648).
; `break` is a control-flow signal that `try` re-raises rather than catching.
; Outside a loop, the re-raised Break becomes "break used outside a loop".
print try [break]
