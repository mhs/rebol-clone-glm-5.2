Red []
; try catches an error and returns an error value (molds as make error! "...")
probe try [1 + "a"]

; attempt returns none on error
probe attempt [1 + "a"]

; catch catches a thrown value
print catch [throw 42]

; cause-error raises an error that try can catch
probe try [cause-error "my-error"]
