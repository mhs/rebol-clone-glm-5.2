# Program fixtures

This directory holds golden fixtures for `red-eval`'s integration tests. Each
fixture is a pair of files:

| File             | Contents                                  |
|------------------|-------------------------------------------|
| `<name>.red`     | A Red source program.                     |
| `<name>.expected`| The expected output (see below).          |

Drop a new `<name>.red` + `<name>.expected` pair into the appropriate
subdirectory and the test harness picks it up automatically — no code changes
required.

## `programs/` — successful programs (exact stdout match)

`tests/programs/*.red` are programs that run to completion without error.
The harness (`tests/programs.rs`) runs each through
`run_source_with_output` with an in-memory buffer and compares the captured
**stdout** byte-for-byte to the sibling `.expected` file.

- The `.expected` file is read **verbatim** (no trimming) because program
  stdout legitimately ends with newlines. Include the trailing `\n` in the
  fixture.
- If the program errors, the test fails. Error cases belong in
  `programs_errors/` (see below).

Example pair:

`hello.red`:
```red
Red []
print "Hello, World!"
```

`hello.expected`:
```
"Hello, World!"
```

## `programs_errors/` — error cases (stderr substring match)

`tests/programs_errors/*.red` are programs that are expected to **fail** with
an error. The harness (`tests/programs_errors.rs`) runs each through
`run_source_with_output`, asserts the result is `Err`, renders the error via
`render_error(None, &src, &err)` (which produces the full
`*** Error: [line:col: ]<msg>` line), and asserts the rendered string
**contains** the `.expected` file's content as a substring.

- The `.expected` file holds a **substring of the message body** — e.g.
  `has no value`, `expected integer!, found string!`, `division by zero` —
  **not** the full `*** Error: line:col:` line. This keeps fixtures robust
  to span/formatting changes.
- Trailing whitespace/newlines in `.expected` are trimmed before matching.
- Word/symbol names in error messages are quoted (e.g. `"if" expects ...`),
  matching Rust's `{:?}` formatting of the symbol's string — include the
  quotes in the substring when matching a native name.

Example pair:

`unbound_word.red`:
```red
Red []
foo
```

`unbound_word.expected`:
```
has no value
```

## Error kinds covered

One fixture per error kind (see the plan's Milestone 12 checklist):

| Fixture                  | Error kind         | Substring                          |
|--------------------------|--------------------|------------------------------------|
| `unbound_word`           | `EvalError::UnboundWord`    | `has no value`              |
| `type_error`             | `EvalError::TypeError`      | `expected ... found ...`    |
| `arity_error`            | `EvalError::Arity`          | `"if" expects ... got ...`  |
| `div_by_zero`            | `EvalError::Native`         | `division by zero`          |
| `out_of_range`           | `EvalError::Native`         | `index out of range`        |
| `empty_series`           | `EvalError::Native`         | `empty series`              |
| `poke_out_of_range`      | `EvalError::Native`         | `index out of range`        |
| `parse_missing_close`    | `ParseError::MissingClose`  | `missing closing ... delimiter` |
| `lex_unterminated`       | `LexError::UnterminatedString` | `unterminated string`    |

## Adding a fixture

1. Pick a unique `<name>` (lowercase, underscores).
2. Write `<name>.red` — the program. For error cases, make sure it actually
   triggers the error kind you intend.
3. Write `<name>.expected` — the expected stdout (success) or message-body
   substring (error).
4. Run `cargo test -p red-eval` — the harness discovers the new pair
   automatically.
