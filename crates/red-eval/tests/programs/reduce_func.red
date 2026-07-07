Red []
; Exercise MakeFunc arm in dispatch_instr (reduce-mode). `does`/`func`
; expressions inside reduce create function values, exercising the MakeFunc
; instruction path in the reduce dispatcher.
f: does [42]
print f
g: func [x][x * x]
print g 7
probe reduce [does [99]]
