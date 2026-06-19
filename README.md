# rebol-clone

A proof-of-concept clone of the [Red](https://www.red-lang.org/) programming
language, implemented in Rust. Red is a homoiconic, block-structured
descendant of Rebol — code is data, evaluation is prefix-style and eager,
and "dialects" are blocks interpreted by custom mini-interpreters.

This repo is a **POC** (`v0.1.0-poc`). It implements a usable Red subset —
lexer, parser, tree-walking evaluator, full series model, real word
binding, functions, the `parse` dialect, and a REPL. The roadmap to `v0.2.0`
with objects, file I/O, refinements, and more is tracked in
[`plan2.md`](./plan2.md).

## Status

- **Tagged:** `v0.1.0-poc`
- **Workspace:** three crates — `red-core` (value model + lexer + parser + printer),
  `red-eval` (interpreter + natives + `parse`), `red-cli` (binary + REPL).
- **Tests:** `cargo test --workspace` green. Golden fixtures in
  `crates/red-core/tests/golden/` (round-trip) and
  `crates/red-eval/tests/programs/` (program execution). CLI tests via
  `assert_cmd`.

## Build & run

```sh
cargo build --workspace
cargo test  --workspace
cargo run  -p red-cli -- examples/hello.red     # → Hello, World!
cargo run  -p red-cli                            # → REPL (no args)
cargo run  -p red-cli -- --help
cargo run  -p red-cli -- --version               # → red 0.1.0
```

## What's implemented

### Language surface
- `Red []` header + bare-body scripts (`load` for the latter).
- Lexing: whitespace-delimited words, `;` line comments, `"..."` (with
  escapes) and `{...}` (multi-line, nested-brace) strings, integers, floats
  (with exponents), `[...]` blocks, `(...)` parens.
- Parsing: recursive descent, every value carries a byte-offset `Span`.
- Printing (`mold`): round-trips `mold(parse(s)) == normalize(s)`.

### Value types
`None`, `Logic`, `Integer`, `Float`, `String`, `Word`/`SetWord`/`GetWord`/
`LitWord`, `Block`, `Paren`, `Func`, `Path` (stub — full paths deferred to v0.2,
see `plan2.md` M19).

### Evaluation
- Tree-walk `eval(Value, &mut Env)`.
- `Block` is **data** (returned as-is); `Paren` is **eager** (evaluated in place).
- Word binding is real: `Unbound` / `Local(Context, slot)` / `Func(slot)`.
  Unbound words at eval time error Red-style ("has no value"); there is no
  global fallback chain.
- SetWord at script top level binds into the user context.
- Function bodies get bound at `func`/`does` creation time, not at load.

### Natives (~70)
- **I/O:** `print`, `prin`, `probe`.
- **Arithmetic / comparison / logic:** `+ - * /`, `= <> < > <= >=`, `and or not`.
- **Control flow:** `if`, `either`, `loop`, `repeat`, `until`, `while`,
  `do`, `reduce`, `break`, `continue`.
- **Series (full model):** `first` `second` `third` `last` `next` `back` `at`
  `skip` `head` `tail` `index?` `length?` `pick` `poke` `select` `find`
  `append` `insert` `change` `remove` `clear` `take` `copy` `empty?` `block?`
  `paren?` `series?` `any-block?` `foreach` `forall`.
- **Functions / binding:** `func`, `does`, `make function!`, `function?`,
  `return`, `bind`, `use`, `get`, `set`, `value?`. Recursion works; closures
  are explicitly out of scope.
- **Dialect:** `parse` — matcher subset over string! and block! inputs:
  `skip`, `to`, `thru`, `end`, `none`, `any`, `some`, `opt`, `while`, `|`
  (alternative), `copy 'word rule`, `set 'word rule`, `[...]` grouping,
  `(...)` Red side-effects. Backtracking via cursor save/restore.
- **Constants:** `none`, `true`, `false`, `newline` bound in the user context.

### Errors
Unified `Error` (Lex / Parse / Eval). Every error carries a `Span`; the CLI
renders `*** Error: [file:line:col: ]<msg>` using a precomputed line map.
`Return`/`Break`/`Continue` are control-flow unwinds caught by the function
call shim and loop natives respectively.

## Examples

See [`examples/`](./examples) — each is a single self-contained `.red` script
runnable via `cargo run -p red-cli -- examples/<name>.red`:

| File | Demonstrates |
|------|--------------|
| `hello.red` | `Red []` header, `print`, string literal |
| `arith.red` | arithmetic, mixed int/float promotion |
| `assign.red` | set-word + word lookup |
| `conditionals.red` | `if` / `either` / comparisons |
| `truthiness.red` | truthiness rules (`0`, `""`, `[]` are truthy; only `false`/`none` falsy), `and`/`or`/`not`, nested `either` chains |
| `loops.red` | `loop` / `repeat` / `until` / `while` / `break` |
| `foreach.red` | iteration over blocks |
| `func.red` | `func` definition and call |
| `recursion.red` | recursive factorial |
| `use.red` | local contexts via `use` |
| `higher-order.red` | functions as values, `get`/`set`/`value?`/`function?`, `does`, passing funcs to funcs |
| `word-kinds.red` | the four word forms (`word`/`set-word`/`get-word`/`lit-word`) and their evaluation rules |
| `series.red` | `first` / `next` / `append` / etc. |
| `mutation.red` | `insert` / `change` / `remove` / `take` / `clear` / `poke`, shared-storage aliasing, `copy` |
| `shared.red` | shared-storage semantics via aliases |
| `blocks.red` | blocks as data (homoiconicity), `mold`/`do`/`reduce`, nested blocks |
| `strings.red` | `"..."` escapes and `{...}` multi-line braced strings, `prin` |
| `sort.red` | insertion sort with `forall`/`insert` |
| `map.red` | `reduce`-style mapping |
| `filter.red` | filtering with series ops |
| `lookup.red` | `pick` / `select` / `find` |
| `reduce.red` | `reduce` collecting results |
| `parse.red` | `parse` dialect on string + block input |
| `parse-csv.red` | `parse` applied to a CSV string with `copy`/`to`/`skip`/`some` + paren side-effects |
| `calculator.red` | cursor-based `take`/`append` queue calculator with `does` and `func` |
| `tree-walk.red` | recursive walk of a nested-block tree (leaf count, flatten, depth) |
| `probe.red` | `probe` debug output |

## Repository layout

```
rebol-clone/
├── Cargo.toml                 # [workspace]
├── crates/
│   ├── red-core/              # value model, lexer, parser, printer
│   │   ├── src/{lib,value,context,lexer,parser,printer,error}.rs
│   │   └── tests/
│   │       ├── round_trip.rs       # golden load → mold
│   │       ├── golden/*.red *.expected
│   │       └── property.rs
│   ├── red-eval/              # tree-walk interpreter
│   │   ├── src/{lib,context,interp,natives,series,binding,parse,error}.rs
│   │   └── tests/programs/{*.red *.expected}
│   └── red-cli/               # binary + REPL
│       ├── src/main.rs
│       └── tests/cli.rs
├── examples/                  # sample .red programs
├── project-brief.md           # feature scope and design decisions
├── architecture.md            # implementation sketch (lexer/parser/eval internals)
├── plan.md                    # v0.1 build checklist (complete)
└── plan2.md                   # v0.2 roadmap
```

## Design notes

- **`Symbol` = `Rc<str>`** newtype. `string_cache` was tried early and dropped;
  no profiling need surfaced.
- **`Series = { data: Rc<RefCell<Vec<Value>>>, index: usize }`** — positioned
  views over shared storage. Mutation natives (`append`, `insert`) affect the
  shared `RefCell`, so aliases see updates (Red's reference semantics).
- **No precedence parsing.** Red is prefix/eager; every value is one token or
  one bracketed group.
- **Single-threaded.** `Env` holds `Rc<RefCell<...>>` and is `!Send`. No GC.
- **No native pre-binding.** Unbound words at eval time fall back to a
  `HashMap<Symbol, NativeFn>` lookup. Real Red pre-binds native references at
  load; deferred.

## Known gaps (v0.1)

See [`project-brief.md`](./project-brief.md) and [`plan2.md`](./plan2.md) for
the authoritative list. Headlines:

- **No objects / `make object!` / `in`** — only user context + function
  contexts. (plan2 M18)
- **No closures** — `func` shallow-copies args per call.
- **Paths are a stub** (`Value::Path(Vec<Value>)`, no real navigation).
  (plan2 M19)
- **No file/shell I/O, `read`/`write`/`now`/`call`.** (plan2 M20)
- **No refinements** (`/part`, `/case`, etc.) as a general mechanism — a few
  behaviors are hard-coded. (plan2 M13)
- **No `char!`, `map!`, `pair!`, `tuple!`, `date!`, `bitset!`, `binary!`.**
- **`parse` matcher subset only** — `collect`/`keep`/`match`/grammar
  extraction/`case` flag deferred.
- **No modules / `import` / `export`.**
- **No error values** as first-class data (`make error!`, `try` returning
  them). `cause-error` is a stub.
- **No `compose`.**
- **GUI / `draw` / `vid` / reactive dialects are permanently out of scope.**

## License

Licensed under the Apache License, Version 2.0 — see
[`LICENSE`](./LICENSE). The upstream [Red project](https://www.red-lang.org/)
holds its own license for the language and canonical spec.
