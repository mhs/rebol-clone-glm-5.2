Red []
; JSON codec demo — encode Red values to JSON and decode them back.

; Encode scalars.
print rejoin ["integer: " to-json 42]
print rejoin ["float: " to-json 3.14]
print rejoin ["string: " to-json {Hello "World"}]
print rejoin ["boolean: " to-json true]
print rejoin ["none: " to-json none]

; Encode a block as a JSON array.
print rejoin ["array: " to-json [1 2 3]]

; Encode a map as a JSON object.
data: make map! [name "Ada" age 36 active true]
print rejoin ["object:" newline to-json/pretty data]

; Encode a block of objects (use reduce to evaluate make object! calls).
people: reduce [
    make object! [name: "Alice" age: 30]
    make object! [name: "Bob" age: 25]
]
print rejoin ["people: " to-json people]

; Decode JSON back to Red values.
json-str: {{"name": "Charles", "age": 45}}
decoded: load-json json-str
print rejoin ["decoded name: " decoded/name]
print rejoin ["decoded age: " decoded/age]

; Round-trip: encode then decode.
round-trip: load-json to-json data
print rejoin ["round-trip name: " round-trip/name]
print rejoin ["round-trip age: " round-trip/age]
