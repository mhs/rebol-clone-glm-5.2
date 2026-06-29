Red []
; try classifies errors by type
e1: try [1 / 0]
print error-type e1
e2: try [1 + "a"]
print error-type e2
e3: try [cause-error 'user "custom"]
print error-type e3
