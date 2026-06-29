Red []
; catch catches raised errors
e: try [1 / 0]
probe catch [throw 42]
; catch also catches cause-error raised errors
probe catch [cause-error 'user "caught!"]
; throw still works with catch
print catch [throw "hello"]
