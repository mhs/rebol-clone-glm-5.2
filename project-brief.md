# Plan: Proof-of-Concept Red Clone in Rust

> **Status (v0.2):** The original scope below (lexer/parser/evaluator/series/
> binding/functions/`parse`) shipped as `v0.1.0-poc`. `plan2.md` extends it to
> `v0.2.0` with refinements, type conversions, string natives, control-flow
> expansion, math/bitwise, **objects**, **real paths**, and **file/shell I/O**.
> This document has been updated to reflect the v0.2 value model, the actual
> crate/file layout (red-core gained `env.rs`/`error.rs`/`source.rs`; red-eval
> gained `strings.rs`/`math.rs`/`convert.rs`/`object.rs`/`path.rs`/`io.rs`),
> and the known-gap list; `architecture.md` covers the dispatch/path/object
> internals.

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
│   ├── red-core/                 # Value model, lexer, parser, printer, error/source
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── value.rs          # Value enum + Span/Symbol/Series/Binding/FuncDef/ErrorValue/ObjectDef
│   │   │   ├── context.rs        # Context (ordered Symbol→slot + Vec<RefCell<Value>>)
│   │   │   ├── env.rs            # Env, CallFrame, EvalError, NativeFn, RefineArgs
│   │   │   ├── error.rs          # unified Error enum + render_error
│   │   │   ├── source.rs         # LineMap (byte-offset → line:col)
│   │   │   ├── lexer.rs          # Source -> tokens (curly/bracket strings, comments, numbers, words, files, urls)
│   │   │   ├── parser.rs         # Tokens -> Value tree (with source spans); path assembly
│   │   │   └── printer.rs        # Value -> Red source text (mold + form)
│   │   └── tests/
│   │       ├── round_trip.rs     # golden: load -> mold == normalized source
│   │       ├── property.rs       # proptest printer/parser round-trip
│   │       ├── common/mod.rs     # fixture walker
│   │       └── golden/           # *.red + *.expected
│   │
│   ├── red-eval/                 # Tree-walking interpreter
│   │   ├── Cargo.toml            # depends on red-core; ureq for url reads
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── context.rs        # 9-line `pub use` re-export of Env/Context/... from red-core
│   │   │   ├── interp.rs         # eval(Value, &mut Env) + run_source* entry points + RunOptions
│   │   │   ├── natives.rs        # print/prin/probe, arithmetic, comparison, logic, control flow, eval, functions, binding natives, register_*/install_constants/install_system
│   │   │   ├── series.rs         # first/next/append/select/find/... series natives
│   │   │   ├── binding.rs        # bind_pass / bind_function_body + bind/use/in/get/set/value? natives
│   │   │   ├── parse.rs          # parse dialect (matcher subset)
│   │   │   ├── strings.rs        # rejoin/reform/join/split/trim/replace/uppercase/lowercase/suffix?
│   │   │   ├── math.rs           # abs/negate/min/max/round/random/power/and/or/xor/complement/shift-*/even?/odd?/prefix aliases
│   │   │   ├── convert.rs        # to-* family + make/to/form
│   │   │   ├── object.rs         # make object! + object?/same?/words-of/values-of/reflect/in/object/context
│   │   │   ├── path.rs           # path?/get-path?/lit-path?/to-path/to-get-path/to-lit-path
│   │   │   └── io.rs             # read/write/save/load/exists?/size?/modified?/dir?/make-dir/delete/rename/change-dir/what-dir/get-env/set-env/env/wait/call/shell
│   │   └── tests/
│   │       ├── programs.rs       # run .red file, compare stdout to .expected
│   │       ├── programs_errors.rs # run .red file, assert rendered error contains substring
│   │       ├── common/mod.rs     # fixture walker + BufferWriter
│   │       ├── programs/         # *.red + *.expected (golden programs)
│   │       ├── programs_errors/  # *.red + *.expected (golden errors)
│   │       └── fixtures/         # committed file fixtures for io.rs read tests
│   │
│   └── red-cli/                  # Binary entry point
│       ├── Cargo.toml            # depends on red-eval; rustyline for REPL
│       ├── src/
│       │   ├── main.rs           # `red [--allow-shell] path/to/file.red` and `red` (REPL)
│       │   └── repl.rs           # rustyline REPL with multi-line input
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

`Context` is defined in `red-core/src/context.rs` (an ordered
`Symbol -> slot index` map plus a `Vec<RefCell<Value>>` of slots). Note the
split: `context.rs` holds **only** `Context`; `Binding`, `FuncDef`,
`Symbol`, `Series`, `Value`, `ErrorValue`, and `ObjectDef` all live in
`value.rs`. `Env`, `CallFrame`, `EvalError`, `NativeFn`, and `RefineArgs`
live in `red-core/src/env.rs` (so red-core's printer/parser can mention
`EvalError` without a red-eval dependency). `red-eval/src/context.rs` is a
9-line `pub use` re-export of all those names.

## Red blocks — semantics notes

Blocks (`[...]`) are the central data structure in Red: **code is data**. The
POC implements the full series model *and* word binding.

- **Homoiconicity**: a block is a `Vec<Value>`; evaluating it walks values in
  order. The same block is usable as data (molded, indexed, sliced) and as
  code (via `do` / `reduce` / top-level script `do`). (`compose` is **not**
  implemented — deferred to v0.3.)

- **Evaluation rule**:
  - A `Block` value encountered by `eval` is returned **as-is** (data).
    Only `do`, `reduce`, and the top-level script loader walk into a block.
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
- `Word` carries a `Binding`: `Unbound`, `Local(Rc<Context>, slot)`, or
  `Func(param_index)` (resolved via the active call frame). Binding is
  attached by the **binding pass** in `red-eval/src/binding.rs` (run before
  eval, not inside the parser) for script-level words, and at
  `make`/`func`/`function`/`make object!`-creation time for function bodies.
- `set-word` in a script binds into the **user context** (a single top-level
  context for the POC, held as `Rc<Context>` on `Env`).
- `func` / `does` / `function` / `make function!` create function values
  with their own context (parent = definition context for closures-less
  `func`).
- Lookup walks: word's binding → if bound, read slot; if unbound, error
  (Red-style "has no value") rather than falling back to a global chain.
- `bind`, `use`, `in`, `value?`, `get`, `set` natives to manipulate bindings
  explicitly. (`in` is registered in `object.rs`.)
- **Objects** implemented in v0.2 (M18): `Value::Object`, `make object!`/
  `object`/`context`, prototype inheritance (copy-based), `in`, `self`
  reference, `words-of`/`values-of`/`reflect`, predicates `object?`/
  `same?`/`not-same?`.
- **Closures** (`closure!`) deferred; `func` uses shallow copy of args on
  each call.
- Known gap (v0.3): `char!`, `map!`, `pair!`, `tuple!`, `date!`, `bitset!`,
  full `binary!` (only the `String8` stub exists), modules/`import`, the
  structured error model (code/type/args — basic `Value::Error` IS in v0.2),
  `compose`, the full port model, trig math, and `parse` advanced rules
  (`collect`/`keep`/`match`/`case` flag) remain deferred. Block-integer
  SetPath (`b/2: 99`) is also unreachable from source (lexer can't tokenize
  `2:`). See `plan2.md`.

### Spans
Each `Block`/`Paren` retains the span of its `[...]`/`(...)` delimiters;
inner values already carry their own spans. Required for `do`-time errors and
for `bind` to report unbound words with a location.

### Built-ins (full block set)
- Type predicates: `block?`, `paren?`, `series?`, `any-block?`, `empty?`,
  `object?`, `same?`, `not-same?`, `file?`, `url?`, `function?`,
  `path?`, `get-path?`, `lit-path?`.
- Series nav: `first` `second` `third` `last` `next` `back` `at` `skip`
  `head` `tail` `index?` `length?`.
- Series access: `pick` `poke` `select` `find` (with `/case` refinement).
- Series mutate: `append` (`/only`) `insert` `change` `remove` `clear` `take`
  `copy` (`/part`).
- Iteration: `foreach` `forall` `while` `until` (plus `loop`/`repeat`).
- Binding: `bind` `use` `in` `value?` `get` `set`.
- Functions: `func` `does` `function` `make` `function?` `return` `exit`
  `quit`.
- Control flow: `if` `either` `switch` (`/default` `/case`) `case`
  (`/default` `/all`) `default` `all` `any` `try` `attempt` `catch` `throw`
  `cause-error` `comment` `break` `continue`.
- Arithmetic (infix + prefix): `+` `-` `*` `/` `//` (modulo) `**` (power)
  `add` `subtract` `multiply` `divide` `abs` `negate` `min` `max` `round`
  (`/to` `/even`) `random` (`/seed` `/only` `/secure`) `power`.
- Comparison: `=` `<>` `<` `>` `<=` `>=`.
- Logic / bitwise: `and` `or` `not` `xor` `complement` `shift-left`
  `shift-right` `even?` `odd?`.
- Eval: `do` `reduce` `load` (string→block; file/url-aware override in `io.rs`).
- Strings: `rejoin` `reform` `join` `suffix?` `split` (`/with`) `trim`
  (`/auto` `/with` `/lines` `/all`) `replace` (`/all`) `uppercase`
  (`/part`) `lowercase` (`/part`).
- Conversions: `to-integer` `to-float` `to-string` `to-block` `to-word`
  `to-set-word` `to-get-word` `to-lit-word` `to-logic` `to-file` `to-url`
  `to-path` `to-get-path` `to-lit-path` `make` `to` `form`.
- Objects: `make object!` `object` `context` `words-of` `values-of`
  `reflect` `in`.
- File / shell I/O (M20): `read` (`/lines` `/binary`) `write` (`/append`
  `/lines` `/binary`) `save` `load` `exists?` `size?` `modified?` `dir?`
  `make-dir` `delete` `rename` `change-dir` `what-dir` `get-env` `set-env`
  `env` `wait` `call` `shell` (the last two gated on `--allow-shell`).
- Constants: `none` `true` `false` `newline` `system` (object exposing
  `system/options/{args, allow-shell, path}`).
- Implemented in v0.2 (M13–M20): refinements (`/part`, `/case`, … as a
  general dispatch mechanism), real paths (`obj/field`, `block/2`,
  `set-path`), `Object`/`make object!`, `File`/`Url` literals + the I/O
  surface above, the type-conversion and string/math surfaces above.
- Optional/deferred: `compose`, closures, `char!`/`map!`/`date!`/`pair!`/
  `tuple!`/`bitset!` (v0.3). (`parse` is in scope — see "Dialects".)

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
  "reduce dialect" (eval each value, collect results). (`compose` would be a
  single native eval-parens-leave-rest, but it is **not** implemented in
  this POC — deferred to v0.3.)

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
- Whitespace-delimited tokens (Red's defining feature); `,` is also whitespace
- Comments: `;` to EOL
- Strings: `"..."` (backslash escapes only; unknown escapes kept verbatim)
  and `{...}` (multi-line, balanced braces) — both supported
- Integers and floats (both supported from the start)
- Words (incl. `word:`, `:word`, `'word`), refinements (`/word`),
  files (`%foo` or `%"quoted"`), urls (`scheme://…`)
- `/` is a delimiter (so `foo/bar` splits and the parser re-folds into a
  path); bare `/` → division Word; `//` → modulo Word (one token)
- Blocks `[ ]`, parens `( )`
- Header `Red [...]` recognized at parser level
- Each token carries a `Span { start, end }`

## Parser (`red-core/src/parser.rs`)
- Recursive descent over token stream
- `parse_program` -> expects `Red` word + header block + body (rest as a
  flat `Series`); `load` -> bare body; `load_source` -> lex+load in one call
- Returns a body `Series` (not a wrapped `Value::Block`)
- Errors with spans
- **Path assembly** happens inline during `parse_value` — adjacent
  `Refinement` tokens (and `Word("/")`+value pairs) fold into
  `Path`/`GetPath`/`LitPath`/`SetPath`. `SetPath` is detected via span
  overlap between the trailing `SetWord` and the last `Refinement`.
- **Binding** is NOT done by the parser — it's a separate pass in
  `red-eval/src/binding.rs` (`bind_pass`/`bind_pass_into`) run before eval,
  which collects set-words, loop vars, parse capture words, and `use`/`get`/
  `set`/`value?` operands. Function bodies bind at `func`/`does`/`function`/
  `make object!` creation time.

## Printer / `mold` (`red-core/src/printer.rs`)
- Inverse of parser, used by REPL and tests
- Exports `mold`, `mold_to_string`, `form`, `form_to_string`
- Round-trip property: `mold(load_source(s)) == normalize(s)`

## Evaluator (`red-eval/src/interp.rs`)

`pub fn eval(block: &Value, env: &mut Env) -> Result<Value, EvalError>`

`Env` (defined in **`red-core/src/env.rs`**, re-exported by red-eval) holds
the `user_ctx: Rc<Context>`, the call stack of function `CallFrame`s, the
`natives: HashMap<Symbol, Rc<FuncDef>>` registry, an `out: Box<dyn Write>`
sink, `allow_shell: bool`, and `cwd: PathBuf`. It is not a flat `HashMap`.

- Walks the block, evaluating each value in order; last value returned
- Word: resolve its **binding** → read slot from the bound context; error
  Red-style ("has no value") if `Unbound`. No dynamic global fallback.
- SetWord: eval next value, write into its bound context slot (or bind into
  user context if unbound at script top-level)
- Block: returned **as-is** (data). Only `do`/`reduce`/natives that take a
  block arg walk it. (See "Red blocks" section. `compose` is not implemented.)
- Paren: evaluated **eagerly** in place when its enclosing block is walked
- Path: full M19 path resolution — function-headed (refinements),
  object-headed (field/method), data-headed (`block/2`, `string/3`),
  SetPath writes. See `architecture.md` for the caveats (string char pick
  returns integer; block-integer SetPath unreachable from source).

### Built-in natives (POC set)
See the "Red blocks → Built-ins (full block set)" list above for the
complete set. Headline groups: I/O (`print`, `prin`, `probe`), arithmetic
(`+ - * / // **` plus `add`/`subtract`/… prefix aliases), comparison
(`= <> < > <= >=`), logic/bitwise (`and or not xor complement shift-*`),
control flow (`if`, `either`, `switch`, `case`, `all`, `any`, `try`,
`attempt`, `catch`, `throw`, `default`, `cause-error`, `loop`, `repeat`,
`until`, `while`, `foreach`, `forall`, `break`, `continue`, `exit`, `quit`,
`comment`), eval (`do`, `reduce`, `load`), series ops (full set), binding
(`bind`, `use`, `in`, `value?`, `get`, `set`), functions (`func`, `does`,
`function`, `make`, `function?`, `return`), strings, math, conversions,
objects, file/shell I/O, constants (`none`/`true`/`false`/`newline`/
`system`).

Native calls are implemented in Rust directly against `&[Value]` and
`&mut Env`; `func`/`does` bodies are evaluated by recursing into `eval`
with a fresh child context. Entry points `run_source_with_exit` /
`run_source_with_exit_opts` / `run_series_with_exit_opts` drive a full
script (binding pass + eval) and return `(Option<Value>, i32)` so the CLI
can propagate `quit`/`exit` codes; they take a `RunOptions { allow_shell,
args }`.

## Error model
- `EvalError` enum: `UnboundWord`/`TypeError`/`Arity`/`Native` carry a `Span`;
  `Return`/`Break`/`Continue`/`Throw`/`Quit` are control-flow unwinds (no
  span). `LexError`/`ParseError` also carry spans. All three are unified
  under `Error` (Lex/Parse/Eval), defined in `red-core/src/error.rs`.
- `render_error(file, src, err)` produces
  `*** Error: [file:line:col: ]<msg>` using a `LineMap` (in
  `red-core/src/source.rs`) to translate the
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
  filesystem relative to `env.cwd` (set from `std::env::current_dir()` and
  mirrored to `system/options/path`; `change-dir` updates both). Write tests
  use the `tempfile` dev-dep for scratch directories; read tests use
  committed fixtures under `crates/red-eval/tests/fixtures/`.
- `read/binary` and `write/binary` are stubbed (error) pending the `binary!`
  type work deferred to v0.3.

## CLI (`red-cli/src/main.rs`)
- `red [--allow-shell] file.red [args...]` — load, parse, do; exit code from
  last value or from `quit`/`exit`. Trailing args are exposed as
  `system/options/args`.
- `red` (no args) — REPL using `rustyline` (`repl.rs`): read line, `load`,
  `do`, `mold` result, print. Multi-line input is accumulated when the
  parser reports `MissingClose`; `quit`/`exit` at a fresh prompt exits;
  Ctrl-C discards partial input, Ctrl-D exits. Non-tty stdin reads plain
  lines without rustyline.
- `--help` / `-h`, `--version` / `-V`, `--allow-shell` (gates `call`/`shell`).

## Testing strategy

**Integration + golden:**

1. `red-core/tests/round_trip.rs` — for each `tests/golden/*.red`:
   - Read source, parse, mold, compare to `*.expected` (normalized form). New test files can be added with no code changes.

2. `red-core/tests/property.rs` — `proptest` printer/parser round-trip,
   excluding the non-round-trippable variants (`Func`, `String8`, `Error`,
   `Object`, `NaN`/`inf` floats, positioned series).

3. `red-eval/tests/programs.rs` — for each `tests/programs/*.red`:
   - Capture stdout, compare to `*.expected`.

4. `red-eval/tests/programs_errors.rs` — for each
   `tests/programs_errors/*.red`: run, assert the rendered `*** Error:` line
   contains the `*.expected` substring.

5. `red-cli/tests/cli.rs` — uses `assert_cmd` to run the binary end-to-end
   (hello world, unbound word, `--version`/`--help`, missing file, REPL via
   stdin, trailing args, `--allow-shell` gating).

6. Inline `#[test]` only for tight unit checks (lexer token kinds, specific
   parser edge cases, `io.rs` url tests marked `#[ignore]`) — kept minimal
   per the "Integration + golden" preference.

A small `tests/common/mod.rs` helper in each crate walks a directory and generates one test per fixture.

## Dependencies (kept minimal)
- `rustyline` (red-cli) — REPL line editing
- `ureq` (red-eval) — http/https fetches for `read url!`
- `proptest` (dev, red-core) — printer/parser round-trip property test
- `tempfile` (dev, red-eval) — scratch dirs for write tests
- `assert_cmd` + `predicates` (dev, red-cli) — CLI tests
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
13. v0.2 (M13–M20): refinements, paths, objects, file/shell I/O, strings,
    math/bitwise, conversions, control-flow expansion — see `plan2.md`.

## Decisions confirmed
- Floats: included from the start
- Strings: both `"..."` and `{...}` multiline supported
- REPL: uses `rustyline`
- README: skipped for now
- Series model: full (cursor + mutation); copy-on-write deferred
- Binding: real contexts (user + function + object); closures deferred.
  The binding pass lives in `red-eval/src/binding.rs`, not the parser.
- Functions: in scope (`func`, `does`, `make function!`, `function`, `return`)
- Dialects: no typed framework; a dialect is a native that walks a block.
- `parse`: matcher subset in scope (string + block input);
  `collect`/`keep`/`match`/`case` deferred.
- `compose` is **not** implemented (deferred to v0.3) despite earlier
  mentions; `do`/`reduce` are the only block-walking eval natives.
- Block-integer SetPath (`b/2: 99`) is unreachable from source (lexer gap);
  object-field SetPath works.
- Basic error-as-value (`Value::Error` + `try`/`catch`/`throw`) IS in v0.2;
  the structured error model (code/type/args) is deferred.
