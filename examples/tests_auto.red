Red []
; Tests only — no explicit run-tests call.
; Run with: cargo run -p red-cli -- --test examples/tests_auto.red

test "auto-run basic" [assert [1 + 1 = 2]]
test "auto-run equal" [assert-equal [2 * 3 6]]

suite "math" [
    test "auto-run nested" [assert-equal [10 - 3 7]]
]
