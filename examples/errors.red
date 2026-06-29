Red []
; error! values (Milestone 42)
;
; A first-class value type with the full Red field set: code, type, message,
; args, near, where, by. Constructed via `make error!` (from a string for a
; message-only error, or a block of keyword/value pairs for a structured one).
; Caught with `try` (returns the error) or `attempt` (returns none on error).

print "message-only error:"
probe make error! "boom"

print "structured error (all fields):"
probe make error! [code: 42 type: 'math message: "division failed"]

print "field accessors:"
e: make error! [code: 99 type: 'io args: [server 5] message: "no reply"]
print error-type e                          ; 'io
print error-code e                         ; 99
probe error-args e                         ; [server 5]

print "predicates:"
print error? e                              ; true
print attempted? e                          ; true  (alias of error?)
print error? 5                              ; false

print "try catches and classifies errors:"
e1: try [1 / 0]
print error-type e1                        ; 'math  (division by zero)
e2: try [1 + "a"]
print error-type e2                        ; 'script (type error)
e3: try [cause-error 'user "custom"]
print error-type e3                        ; 'user
print error? e1                            ; true

print "attempt returns none on error (vs an error value for try):"
print attempted? attempt [1 / 0]           ; true
print none? attempt [1 / 0]               ; false  (attempt returns an error!, not none)

print "catch (catches throws AND raised errors):"
probe catch [throw 42]                     ; 42
probe catch [cause-error 'user "caught!"]  ; make error! [type: 'user message: "caught!"]
print catch [throw "hello"]               ; "hello"

; --- error-driven control flow at the top level (try + either) ---
; NOTE: `try` inside a user func currently infinite-loops in the VM; this
; top-level form is the recommended pattern until that bug is fixed.
result: try [10 / 0]
either error? result [
    print error-type result                  ; 'math
    probe result                              ; make error! [type: 'math message: "division by zero"]
][
    print result
]
