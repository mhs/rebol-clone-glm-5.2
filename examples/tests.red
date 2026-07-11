Red []
; Unit testing dialect demo — colocate tests with code.
;
; Run with:  cargo run -p red-cli -- --test examples/tests.red
; Or inline: cargo run -p red-cli -- examples/tests.red  (runs run-tests explicitly)

; --- production code ---
add: func [x y] [x + y]
multiply: func [x y] [x * y]

; --- colocated tests ---

test "add basic" [assert-equal [add 2 3 5]]
test "add zero" [assert-equal [add 0 0 0]]
test "add negative" [assert-equal [add -5 5 0]]

test "multiply basic" [assert-equal [multiply 3 4 12]]
test "multiply zero" [assert-equal [multiply 99 0 0]]

suite "edge cases" [
    before-test [probe "running edge case..."]
    test "float arithmetic" [assert-equal [1.0 + 1.0 2.0]]
    test "string concat" [assert-equal [rejoin ["a" "b"] "ab"]]
    test "error expected" [assert-error [1 / 0]]
    test "no error expected" [assert-no-error [add 1 2]]
]

suite "nested" [
    suite "deep" [
        test "deeply nested" [assert [true]]
    ]
]

; Run tests explicitly (also auto-run with --test flag).
run-tests
