Red []
; Access error fields
e: try [1 / 0]
print error-type e
print error-code e
probe error-args e
print error? e
print attempted? e
print error? 5
