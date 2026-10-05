Red []
; JSON codec round-trip: nested structure through encode → decode.
data: make map! [
    name "Alice"
    tags ["a" "b"]
    nested: make map! [x 1 y [true false none]]
]
print load-json to-json data
print load-json to-json/pretty [1 2.5 "three" [4]]
