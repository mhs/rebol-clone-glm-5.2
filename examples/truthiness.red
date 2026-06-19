Red []
; Truthiness: only false and none are falsy. Everything else — including
; 0, the empty string, the empty block — is truthy. This is Red's rule,
; distinct from C/Python/Rust.

print "logic values:"
print if true ["true is truthy"]
print if false ["you will NOT see this"]
print if none ["you will NOT see this either"]

print "integers, including 0, are truthy:"
if 0 [print "0 is truthy"]
if 42 [print "42 is truthy"]

print "empty string and empty block are truthy:"
if "" [print "empty string is truthy"]
if [] [print "empty block is truthy"]

print "either picks based on truthiness:"
print either 0 ["zero is truthy"]["zero is falsy"]
print either none ["none is truthy"]["none is falsy"]

print "comparisons produce logic, which itself follows the rule:"
print if (3 < 5) ["3 < 5 is true"]
print if not (3 > 5) ["not (3 > 5) is true"]

; and/or operate on logic values.
print true and true
print true and false
print false or true
print not (true and false)

; Building a conditional chain with nested if (since all/any are v0.2).
classify: func [n][
    either n < 0 ["negative"][
        either n = 0 ["zero"]["positive"]
    ]
]
print classify -7
print classify 0
print classify 7

; A combined predicate: is n in [lo, hi]?
in-range?: func [n lo hi][
    (lo <= n) and (n <= hi)
]
print in-range? 5 1 10
print in-range? 0 1 10