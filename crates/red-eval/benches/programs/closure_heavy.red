Red []
; Closure-heavy: create + call a closure capturing one freevar, 100k times.
; Exercises the MakeClosure + LoadCapture path (the v0.5 closure machinery).
; Snapshot capture: each iteration builds a fresh closure capturing `base`,
; then calls it. 100k iterations (not 1M — closure creation allocates a
; Vec<Value> per call, so this is ~10x heavier per iter than func_call_heavy).
base: 100
acc: 0
repeat i 100000 [
    adder: closure [x][x + base]
    acc: adder i
]
print acc
