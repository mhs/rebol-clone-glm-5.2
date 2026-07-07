Red []
; Exercise the `bind` native — block form (happy path) and function form.
; No prior golden test called `bind` at all.
x: 10
y: 20
b: bind [x y] 'x
print do b
; Function form: bind a func's body to the user context, then call it.
f: func [] [x + y]
g: bind :f 'x
print g
