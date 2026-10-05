Red []
; Query dialect smoke test: filter + order + project + distinct,
; and the key/value-pair block record form.
people: [
    make object! [name: "Alice" age: 30 city: "NYC"]
    make object! [name: "Bob" age: 25 city: "LA"]
    make object! [name: "Carol" age: 41 city: "NYC"]
]
; Where + order desc + select + limit
foreach r query [from people where [age > 20] order [age desc] select [name] limit 2] [
    print r/name
]
; String-field ordering (form-based comparison)
foreach r query [from people order [city]] [
    print r/name
]
; Block-record form
data: [[name "Ann" age 50] [name "Eve" age 60]]
foreach r query [from data where [age > 55] select [name]] [
    print r/name
]
