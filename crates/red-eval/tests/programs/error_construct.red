Red []
; Construct errors via make error!
probe make error! "simple message"
probe make error! [code: 42 type: 'math message: "division failed"]
; Access fields
print error-type make error! [type: 'io message: "disk full"]
print error-code make error! [code: 99 message: "oops"]
probe error-args make error! [args: [1 2 3] message: "bad args"]
