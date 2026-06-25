# Plan: Proof-of-Concept Red Clone in Rust

> **Status (v0.2):** The original scope below (lexer/parser/evaluator/series/
> binding/functions/`parse`) shipped as `v0.1.0-poc`. `plan2.md` extends it to
> `v0.2.0` with refinements, type conversions, string natives, control-flow
> expansion, math/bitwise, **objects**, **real paths**, and **file/shell I/O**.
> This document has been updated to reflect the v0.2 value model and known-gap
> list; `architecture.md` covers the new dispatch/path/object internals.

## Goals
- Lexer → parser → tree-walking evaluator for a small Red subset
- `Red []` header convention, stdout-only I/O
- Cargo workspace with multiple crates
- Integration tests + golden files for parser/printer round-trips and program execution

## Workspace layout

```
rebol-clone/
├── Cargo.toml                    # [workspace] manifest, members only
├── crates/
│   ├── red-core/                 # Value model, lexer, parser, printer
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── value.rs          # Value enum + Word/Set-word/Block/Paren/Func/etc.
│   │   │   ├── context.rs        # Context, Binding, FuncDef (shared w/ red-eval)
│   │   │   ├── lexer.rs          # Source -> tokens (curly/bracket strings, comments, numbers, words)
│   │   │   ├── parser.rs         # Tokens -> Value tree (with source spans)
│   │   │   └── printer.rs        # Value -> Red source text (mold)
│   │   └── tests/
│   │       ├── round_trip.rs     # golden: load -> mold == normalized source
│   │       └── golden/           # *.red + *.expected
│   │
│   │   ├── red-eval/                 # Tree-walking interpreter
│   │   │   ├── Cargo.toml            # depends on red-core
│   │   │   ├── src/
│   │   │   │   ├── lib.rs
│   │   │   │   ├── context.rs        # re-exports + Env (user ctx + call stack)
│   │   │   │   ├── interp.rs         # eval(Value, &mut Env) -> Result<Value, Error>
│   │   │   │   ├── natives.rs        # print, prin, if, either, loop, repeat, +, -, *, =, etc.
│   │   │   │   ├── series.rs         # first/next/append/select/find/... natives
│   │   │   │   ├── binding.rs        # bind/use/in/get/set/value? natives
│   │   │   │   ├── parse.rs          # parse dialect (matcher subset)
│   │   │   │   └── error.rs          # Red-style errors as values
│   │   └── tests/
│   │       ├── programs.rs       # run .red file, compare stdout to .expected
│   │       └── programs/         # *.red + *.expected
│   │
│   └── red-cli/                  # Binary entry point
│       ├── Cargo.toml            # depends on red-eval
│       ├── src/
│       │   └── main.rs           # `red path/to/file.red` and `red` (REPL stub)
│       └── tests/
│           └── cli.rs            # assert_cmd against fixtures
│
└── examples/                     # Sample .red programs usable from CLI
```

## Value model (`red-core/src/value.rs`)

A single `Value` enum backed by shared storage + cursors for blocks (full
series semantics), and by `Rc<FuncDef>` for function values:

```rust
struct Series {
    data: Rc<RefCell<Vec<Value>>>,
    index: usize,            // 0..=len; cursor for series natives
}

enum Binding {
    Unbound,
    Local(Rc<Context>, usize),   // shared context + slot index
    Func(usize),                 // function-local slot index (resolved via call frame)
}

struct FuncDef {
    params: Vec<Symbol>,
    refinements: Vec<(Symbol, Vec<Symbol>)>,  // (refinement word, its arg words) — M13
    locals: Vec<Symbol>,                      // explicit <local> words for `function` — M16
    body: Series,
    ctx: Context,            // definition context (parent for lookups)
    native: Option<NativeFn>,
    variadic: bool,
    infix: bool,
}

type NativeFn = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

enum Value {
    None,
    Logic(bool),
    Integer { n: i64, span: Span },
    Float { f: f64, span: Span },
    String { s: Rc<str>, span: Span },       // {"..."} and "..." alike
    Word { sym: Symbol, binding: Binding, span: Span },    // foo
    SetWord { sym: Symbol, binding: Binding, span: Span }, // foo:
    GetWord { sym: Symbol, binding: Binding, span: Span }, // :foo
    LitWord { sym: Symbol, span: Span },     // 'foo
    Block { series: Series, span: Span },    // [...] — code is data
    Paren { series: Series, span: Span },    // (...)
    Func(Rc<FuncDef>),                        // synthetic — no span
    Path { parts: Vec<Value>, span: Span },              // foo/bar      — M19
    GetPath { parts: Vec<Value>, span: Span },           // :foo/bar     — M19
    LitPath { parts: Vec<Value>, span: Span },           // 'foo/bar     — M19
    SetPath { parts: Vec<Value>, span: Span },           // obj/field:   — M19
    Refinement { sym: Symbol, span: Span },              // /foo         — M13
    File { path: Rc<str>, span: Span },                  // %foo/bar     — M20
    Url { url: Rc<str>, span: Span },                    // http://…     — M20
    String8(Vec<u8>),                        // binary! (POC stub — deferred)
    Error(Rc<ErrorValue>),                   // caught error value — M16
    Object(Rc<RefCell<ObjectDef>>),          // make object! — M18 (synthetic, no span)
}

struct ObjectDef {
    ctx: Rc<Context>,
    parent: Option<Rc<RefCell<ObjectDef>>>,
    self_word: Symbol,
}
```

`Symbol` = an `Rc<str>` newtype (the `string_cache` crate was tried early on
but dropped in favor of the simpler `Rc<str>`; no profiling need surfaced).

`Context` is defined in `red-core/src/context.rs` (see Evaluator section):
an ordered `Symbol -> slot index` map plus a `Vec<RefCell<Value>>` of slots.

## Red blocks — semantics notes

Blocks (`[...]`) are the central data structure in Red: **code is data**. The
POC implements the full series model *and* word binding.

- **Homoiconicity**: a block is a `Vec<Value>`; evaluating it walks values in
  order. The same block is usable as data (molded, indexed, sliced) and as
  code (via `do` / `reduce` / `compose` / top-level script `do`).

- **Evaluation rule**:
  - A `Block` value encountered by `eval` is returned **as-is** (data).
    Only `do`, `reduce`, `compose`, and the top-level script loader walk into
    a block.
  - A `Paren` value encountered by `eval` is evaluated **eagerly** in place
    (like an inline `do`). This distinction is load-bearing.

- **Argument convention**: a block passed as an argument is *not* evaluated
  by the caller; the callee decides. `if`/`either`/`loop`/`until`/`repeat`
  receive a block and `do` it themselves; `print`/`+`/etc. receive already-
  evaluated values.

### Series model (full)

A block is `Series { data: Rc<RefCell<Vec<Value>>>, index: usize }` so the
same underlying storage can be shared by multiple positioned views (Red's
`series!` semantics).

- Type predicates: `series?`, `block?`, `paren?`, `any-block?`, `empty?`.
- Navigation: `first`, `second`, `third`, `last`, `next`, `back`, `at`,
  `skip`, `head`, `tail`, `index?`, `length?`.
- Access: `pick`, `poke`, `select`, `find` (linear; no `match`/regex in POC).
- Mutation: `append`, `insert`, `change`, `remove`, `clear`, `take`, `poke`.
- Slicing: `copy/part`, `at`-based sub-series share storage (copy-on-write
  deferred — note as future work).
- Iteration: `foreach` (over block or series), `repeat`, `forall` (uses the
  series cursor), `while`/`until`.
- `series/head`, `series/index`, etc. paths are out of scope; use natives.

Series natives operate on the cursor; `next` returns a new `Series` pointing
one ahead; mutation via `append`/`insert` affects shared storage (matches
Red's reference semantics).

### Binding & contexts (real implementation)

Words inside blocks are **bound** to contexts. The POC implements Red-style
binding, not just dynamic lookup.

- `Context` = an ordered map of `Symbol -> slot index` plus a
  `Vec<RefCell<Value>>` of slots. Self-referential (a context can hold a
  value that references itself) via `Rc<RefCell<...>>`.
- `Word` carries a `Binding`: `Unbound`, `Local(Context, slot)`, or
  `Func(func_rc, param_index)`. Binding is attached at `load`-time for
  script-level words and at `make`/`func`-creation time for function bodies.
- `set-word` in a script binds into the **user context** (a single top-level
  context for the POC; `context?` / `object` not modeled yet).
- `func` / `does` / `make function!` create function values with their own
  context (parent = definition context for closures-less `func`).
- Lookup walks: word's binding → if bound, read slot; if unbound, error
  (Red-style "has no value") rather than falling back to a global chain.
- `bind`, `use`, `in`, `value?`, `get`, `set` natives to manipulate bindings
  explicitly.
- Known gap: **objects** (`make object!`, `object` context inheritance) —
  implemented in v0.2 (M18): `Value::Object`, `make object!`/`object`/
  `context`, prototype inheritance, `in`, `words-of`/`values-of`/`reflect`,
  `self` reference.
- Known gap: **closures** (`closure!`) deferred; `func` uses shallow copy of
  args on each call.
- Known gap (v0.3): `char!`, `map!`, `pair!`, `tuple!`, `date!`, `bitset!`,
  full `binary!`, modules/`import`, error values as first-class data,
  `compose`, the full port model, trig math, and `parse` advanced rules
  (`collect`/`keep`/`match`/`case` flag) remain deferred. See `plan2.md`.

### Spans
Each `Block`/`Paren` retains the span of its `[...]`/`(...)` delimiters;
inner values already carry their own spans. Required for `do`-time errors and
for `bind` to report unbound words with a location.

### Built-ins (full block set)
- Type predicates: `block?`, `paren?`, `series?`, `any-block?`, `empty?`.
- Series nav: `first` `second` `third` `last` `next` `back` `at` `skip`
  `head` `tail` `index?` `length?`.
- Series access: `pick` `poke` `select` `find`.
- Series mutate: `append` `insert` `change` `remove` `clear` `take`.
- Iteration: `foreach` `forall` `while` `until` (plus `loop`/`repeat`).
- Binding: `bind` `use` `in` `value?` `get` `set`.
- Functions: `func` `does` `make` `function?` `return` (local).
- Implemented in v0.2 (M13–M20): refinements (`/part`, `/case`, … as a
  general dispatch mechanism), real paths (`obj/field`, `block/2`,
  `set-path`), `Object`/`make object!`, `File`/`Url` literals + `read`/
  `write`/`load`/`save`/`exists?`/`size?`/`modified?`/dir ops, type
  conversions (`to-*`/`form`/`make`/`to`), string natives (`rejoin`/
  `split`/`trim`/`replace`/`uppercase`/`lowercase`), control-flow
  expansion (`switch`/`case`/`all`/`any`/`try`/`attempt`/`catch`/`throw`/
  `function`/`comment`), math + bitwise (`//`/`abs`/`min`/`max`/`round`/
  `random`/`power`/`and`/`or`/`xor`/`shift-*`), `call`/`shell` (gated).
- Optional/deferred: `compose`, closures, `char!`/`map!`/`date!` (v0.3).
  (`parse` is in scope — see "Dialects".)

## Dialects

A **dialect** in Red is any block evaluated by a custom interpreter instead
of the default `do` evaluator. Blocks are data, so any native can walk a
block with its own rules. The POC implements one concrete dialect (`parse`);
no typed framework — a dialect is simply a native that interprets a block.

### Dialect concept
- A dialect is just a function `fn(&[Value], &mut Env) -> Result<Value, EvalError>`
  taking a block's contents and interpreting them however it likes.
- `parse` is the only built-in dialect in the POC.
- User-defined dialects are possible by passing a block to a native that
  interprets it (e.g. a future `draw` dialect); no special syntax needed.
- Contrast: `do` is the "Red dialect" (normal eval); `reduce` is the
  "reduce dialect" (eval each value, collect results); `compose` is a
  single native (eval parens, leave rest) — not a dialect framework user.

### `parse` dialect (in scope for POC)
Mini-DSL on blocks/strings, implemented as a native that walks its rule
block. Works on **both string! and block! input**.

```red
parse "abc" ["a" "b" "c"]          ; => true
parse [1 2 3] [1 2 3]              ; => true
parse "hello world" [copy name to " " skip copy rest to end]
```

POC rule set (matcher subset):
- Literal values (string/integer/word) — match against input.
- `skip`, `to`, `thru`, `end`, `none`.
- `any`, `some`, `opt`, `while`.
- `|` (alternative).
- `copy word rule` (capture sub-match), `set word rule` (single value).
- `[...]` grouping (sub-rules).
- `(...)` (Red code side-effect, evaluated via `eval`).
- Return `logic!` (matched/not).

Deferred: `collect`/`keep`/`match`/`gather`, rule compilation, BNF-style
grammar extraction, error rule blocks, `case` flag. Just the matcher.

### Other dialects (illustrative, NOT implemented)
- `load` dialect — already the parser; not a runtime dialect.
- `draw` dialect, `vid` dialect (GUI), `secure` dialect — all out of scope.

### Implications for the rest of the plan
- `parse` is a non-trivial native → gets its own milestone and source file
  (`red-eval/src/parse.rs`).
- `parse` depends on the **series model** (cursor-based input scanning for
  both strings and blocks) and on **binding** (for `copy`/`set` to write
  into the user context).
- Dialects motivate keeping `eval`'s public surface small: a dialect only
  needs `&[Value]` + `&mut Env`, never a re-entry into the parser.

## Lexer (`red-core/src/lexer.rs`)
- Whitespace-delimited tokens (Red's defining feature)
- Comments: `;` to EOL
- Strings: `"..."` (escaped) and `{...}` (multi-line, balanced braces) — both supported
- Integers and floats (both supported from the start)
- Words (incl. `word:`, `:word`, `'word`)
- Blocks `[ ]`, parens `( )`
- Header `Red [...]` recognized at parser level
- Each token carries a `Span { start, end }`

## Parser (`red-core/src/parser.rs`)
- Recursive descent over token stream
- `parse_program` -> expects `Red` word + header block + body block (or bare body for `load`)
- Returns `Value::Block` of body
- Errors with spans
- **Binding pass**: after constructing the value tree, walk it and attach
  `Binding`s to words using the user context (script-level `set-word!`s and
  references). Function bodies get bound at `func`/`does` creation time
  (runtime), not at load.

## Printer / `mold` (`red-core/src/printer.rs`)
- Inverse of parser, used by REPL and tests
- Round-trip property: `mold(parse(s)) == normalize(s)`

## Evaluator (`red-eval/src/interp.rs`)

`pub fn eval(block: &Value, env: &mut Env) -> Result<Value, EvalError>`

`Env` is the **user context** plus the call stack of function contexts (not a
flat `HashMap`). Defined in `red-eval/src/context.rs` (re-exports
`red_core::context::{Context, Binding, FuncDef}`).

- Walks the block, evaluating each value in order; last value returned
- Word: resolve its **binding** → read slot from the bound context; error
  Red-style ("has no value") if `Unbound`. No dynamic global fallback.
- SetWord: eval next value, write into its bound context slot (or bind into
  user context if unbound at script top-level)
- Block: returned **as-is** (data). Only `do`/`reduce`/`compose`/natives
  that take a block arg walk it. (See "Red blocks" section.)
- Paren: evaluated **eagerly** in place when its enclosing block is walked
- Path: leave unsupported for POC or implement simple object-less path as
  `select` on a block

### Built-in natives (POC set)
See the "Red blocks → Built-ins (full block set)" list above for the
complete set. Headline groups: I/O (`print`, `prin`, `probe`), arithmetic
(`+ - * /`), comparison (`= <> < > <= >=`), logic (`and or not`),
control flow (`if`, `either`, `loop`, `repeat`, `until`, `while`, `foreach`,
`forall`), eval (`do`, `reduce`, `compose` optional), series ops (full set),
binding (`bind`, `use`, `in`, `value?`, `get`, `set`), functions (`func`,
`does`, `make`, `function?`, `return`), constants (`none`/`true`/`false`/
`newline`).

Native calls are implemented in Rust directly against `&[Value]` and
`&mut Env`; `func`/`does` bodies are evaluated by recursing into `eval`
with a fresh child context.

## Error model
- `EvalError` enum: `UnboundWord`/`TypeError`/`Arity`/`Native` carry a `Span`;
  `Return`/`Break`/`Continue` are control-flow unwinds. `LexError`/`ParseError`
  also carry spans. All three are unified under `Error` (Lex/Parse/Eval).
- `render_error(file, src, err)` produces
  `*** Error: [file:line:col: ]<msg>` using a `LineMap` to translate the
  error's byte-offset span into 1-based line/col. The CLI passes the file
  path + source; the REPL passes `None` + the line buffer.
- Tests assert against the message-body substring (error fixtures) or the
  rendered `*** Error:` line (CLI tests).

### Sandbox policy (file & shell I/O — M20)
- `call`/`shell` natives are **off by default** and raise `EvalError::Native`
  ("shell disabled") unless the CLI is invoked with `--allow-shell`. No test
  fixture invokes the shell; the one inline test that does is gated on
  `env.allow_shell = true` set directly in Rust.
- `read` of a `url!` performs real network I/O via `ureq` (http/https only).
  Network-dependent tests are marked `#[ignore]` so `cargo test` stays
  hermetic; run with `cargo test -- --ignored`.
- File I/O (`read`/`write`/`save`/`exists?`/etc.) operates on the real
  filesystem relative to `env.cwd` (set from `std::env::current_dir()`).
  Write tests use the `tempfile` dev-dep for scratch directories; read tests
  use committed fixtures under `crates/red-eval/tests/fixtures/`.
- `read/binary` and `write/binary` are stubbed (error) pending the `binary!`
  type work deferred to v0.3.

## CLI (`red-cli/src/main.rs`)
- `red file.red` — load, parse, do, exit code from last value
- `red` (no args) — minimal REPL using `rustyline`: read line, `load`, `do`, `mold` result, print
- `--help`, `--version`

## Testing strategy

**Integration + golden:**

1. `red-core/tests/round_trip.rs` — for each `tests/golden/*.red`:
   - Read source, parse, mold, compare to `*.expected` (normalized form). New test files can be added with no code changes.

2. `red-eval/tests/programs.rs` — for each `tests/programs/*.red`:
   - Capture stdout, compare to `*.expected`. Also capture stderr for error cases.

3. `red-cli/tests/cli.rs` — uses `assert_cmd` to run the binary on a couple of fixtures end-to-end.

4. Inline `#[test]` only for tight unit checks (lexer token kinds, specific parser edge cases) — kept minimal per the "Integration + golden" preference.

A small `tests/common/mod.rs` helper in each crate walks a directory and generates one test per fixture.

## Dependencies (kept minimal)
- `rustyline` — REPL line editing
- `proptest` (dev) — printer/parser round-trip property test
- `assert_cmd` + `predicates` (dev) — CLI tests
- No async, no proc-macros.

## Build/test commands
- `cargo build -p red-cli`
- `cargo test --workspace`
- `cargo run -p red-cli -- examples/hello.red`

## Implementation order (milestones)
1. Scaffold workspace + 3 crate skeletons, empty tests pass
2. `Value` + `Symbol` + `printer` (mold) with unit tests
3. Lexer (token stream + spans), golden round-trip tests added
4. Parser producing `Value::Block`; full round-trip green
5. `Env`/`Context` + minimal `eval` (literals, words, set-words, do) + user-context binding pass
6. Natives: `print`/`prin` first → "hello world" runs end-to-end via CLI
7. Natives: arithmetic, conditionals (`if`/`either`), `loop`/`repeat`/`until`/`while`
8. **Series model**: `Series` cursor, nav/access/mutate natives, `foreach`/`forall`, golden tests
9. **Functions + binding**: `func`/`does`/`return`, `bind`/`use`/`in`/`get`/`set`/`value?`, function-context call frames
10. **`parse` dialect**: matcher subset (`copy`/`set`/`to`/`thru`/`some`/`any`/`opt`/`while`/`|`), string + block input, golden tests
11. REPL mode in CLI
12. Golden program suite for eval; error handling polish

## Decisions confirmed
- Floats: included from the start
- Strings: both `"..."` and `{...}` multiline supported
- REPL: uses `rustyline`
- README: skipped for now
- Series model: full (cursor + mutation); copy-on-write deferred
- Binding: real contexts (user + function); objects/closures deferred
- Functions: in scope (`func`, `does`, `make function!`, `return`)
- Dialects: no typed framework; a dialect is a native that walks a block.
- `parse`: matcher subset in scope (string + block input); `collect`/`compile`/`case` deferred.
