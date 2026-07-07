Red []
; Exercise dispatch_instr arms beyond the simple ConstInt/Call/Return paths
; covered by reduce.red. This block puts an `if` (JumpIfFalse), a `none`
; (ConstNone), a string (Const pool), and arithmetic (Call) inside reduce.
print reduce [if true [1] 2 + 3 none "x"]
