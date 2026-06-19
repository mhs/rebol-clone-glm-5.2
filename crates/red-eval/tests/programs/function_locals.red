Red []
; `function` declares locals via <local>; body SetWords also auto-local
f: function [x <local> sum][
    sum: x * 2
    sum + 1
]
print f 10

; locals default to none before assignment
g: function [x <local> y][
    if y [print "y has value"]
    if not y [print "y is none"]
]
g 5
