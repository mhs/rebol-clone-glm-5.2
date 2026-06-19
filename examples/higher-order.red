Red []
; Higher-order: functions are first-class values. Bind a name to a func,
; fetch it via `get 'word`, pass it to other functions, and check
; value?/function?.
;
; POC note: a block like `[inc dbl]` stores *word* values, not the resolved
; functions (blocks are data). To obtain a function value, use `get 'word`
; or `:word` (GetWord). Storing funcs in a block for later iteration would
; require `reduce`, which this POC evaluates by *calling* each word — so we
; demonstrate the simpler patterns here.

inc: func [x][x + 1]
dbl: func [x][x * 2]
neg: func [x][0 - x]

; Fetch a function value and call it directly.
f: get 'inc
print f 10

; GetWord (':inc') returns the value without invoking, so it can be passed
; as an argument or stored.
g: :dbl
print g 10

; function? checks whether a value is a function.
print function? get 'inc
print function? 5

; value? checks whether a word currently has a value.
print value? 'inc
print value? 'undefined_word

; Pre-declare, then set, then call. `set 'word value` only works on words
; that already have a slot (appeared as a set-word at parse time).
triple: none
set 'triple func [x][x * 3]
print triple 4

; A function that takes and invokes another function.
apply-twice: func [f x][(f x) + f x]
print apply-twice get 'inc 5
print apply-twice get 'dbl 5

; A `does` block can't reassign outer words directly (body SetWords become
; function-locals that shadow the outer name — a POC limitation). To share
; mutable state across calls, keep it in a block and mutate via `poke`,
; which writes through the shared Rc<RefCell<...>> storage.
state: [0]
bump: does [
    poke state 1 (first state) + 1
    first state
]
print bump
print bump
print bump