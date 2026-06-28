# rebol-clone

A proof-of-concept clone of the [Red](https://www.red-lang.org/) programming
language, implemented in Rust. Red is a homoiconic, block-structured
descendant of Rebol — code is data, evaluation is prefix-style and eager,
and "dialects" are blocks interpreted by custom mini-interpreters.

This repo is a **Red subset interpreter** (`v0.3.0`). It implements a usable
slice of Red — lexer, parser, **bytecode compiler + stack VM** (the default
since v0.3), tree-walking evaluator (retained as the `--walk` fallback), full
series model, real word binding, functions, the `parse` dialect, **objects**,
**real paths**, **refinements**, type conversions, string/control-flow/math
natives, file & shell I/O, and a REPL. The build history is tracked in
[`plan.md`](./plan.md) (v0.1), [`plan2.md`](./plan2.md) (v0.2), and
[`plan3.md`](./plan3.md) (v0.3 — VM + performance).

## Status

- **Tagged:** `v0.3.0`
- **Workspace:** three crates — `red-core` (value model + lexer + parser + printer + VM IR types),
  `red-eval` (compiler + VM + tree-walker + natives + `parse`), `red-cli` (binary + REPL).
  A `fuzz/` crate (nightly-only, `libfuzzer-sys`) is excluded from the default workspace.
- **Default evaluator:** bytecode VM (`EvalMode::Vm`). `--walk` on the CLI or the `force-walk` cargo feature forces the tree-walker for debugging and parity comparison.
- **Tests:** `cargo test --workspace` green in VM mode; `--features force-walk` green in Walk mode (parity). Golden fixtures in
  `crates/red-core/tests/golden/` (round-trip), `crates/red-eval/tests/programs/`
  (program execution), `crates/red-eval/tests/programs_errors/` (error
  rendering), `crates/red-eval/tests/disasm/` (disassembly), and
  `crates/red-eval/tests/property.rs` (VM/Walk parity proptests). CLI tests via `assert_cmd`.

## Build & run

```sh
cargo build --workspace
cargo test  --workspace
cargo run  -p red-cli -- examples/hello.red     # → Hello, World!
cargo run  -p red-cli                            # → REPL (no args)
cargo run  -p red-cli -- --help
cargo run  -p red-cli -- --version               # → red 0.3.0
cargo run  -p red-cli -- --allow-shell examples/call.red   # enable call/shell
cargo run  -p red-cli -- --walk examples/fib.red          # force tree-walker
cargo run  -p red-cli -- --disasm examples/fib.red        # disassemble (no run)
cargo run  -p red-cli -- --disasm-func fib examples/fib.red  # disassemble named func
cargo run  -p red-cli -- --trace examples/arith.red       # per-instr VM trace to stderr
```

The v0.3 language surface is frozen at v0.2 — no new natives or value types. v0.3 is a **performance release**: the bytecode VM delivers 2–4× speedups on compute-heavy programs (deep recursion, tight loops) while preserving exact observable behavior (golden parity, error parity). See `BENCHMARKS.md` for measurements.

## What's implemented

### Language surface
- `Red []` header + bare-body scripts (`load` for the latter).
- Lexing: whitespace-delimited words, `;` line comments, `"..."` (with
  escapes) and `{...}` (multi-line, nested-brace) strings, integers, floats
  (with exponents), `[...]` blocks, `(...)` parens, `/refinement` words,
  `%file` literals, `scheme://url` literals.
- Parsing: recursive descent, every source-origin value carries a byte-offset
  `Span`; paths (`foo/bar`, `:foo/bar`, `'foo/bar`, `obj/field:`) fold from
  word + `/word` adjacency.
- Printing (`mold`): round-trips `mold(parse(s)) == normalize(s)` for the
  reparseable variants (property-tested).

### Value types
`None`, `Logic`, `Integer`, `Float`, `String`, `Word`/`SetWord`/`GetWord`/
`LitWord`, `Block`, `Paren`, `Func`, `Path`/`GetPath`/`LitPath`/`SetPath`,
`Refinement`, `File`, `Url`, `Object`, `Error`, `String8` (stub).

### Evaluation
- **Bytecode compiler + stack VM** (v0.3, default): blocks compile to a flat
  `Vec<Instr>` with a constant pool; the VM dispatches instrs with lexical
  addressing (frame depth + slot index) where statically analyzable, falling
  back to the dynamic `Context` slot mechanism for `bind`/`use`/`do`-on-data.
  Tail-call optimization (`TailCall`/`TailReenter`) bounds call-stack depth for
  tail-recursive programs. See `architecture.md` for the full design.
- **Tree-walking evaluator** (`interp_walker.rs`, the v0.2 default): retained
  as the `--walk` fallback and the path for `needs_rebind`-flagged blocks
  (`use`/`make object!`/foreign-bound). `--features force-walk` runs the entire
  test suite against the walker for byte-for-byte parity with the VM.
- `Block` is **data** (returned as-is); `Paren` is **eager** (evaluated in place).
- Word binding is real: `Unbound` / `Local(Context, slot)` / `Func(slot)` /
  `Lexical(depth, slot)` (VM-only). Unbound words at eval time error Red-style
  ("has no value"); there is no global fallback chain.
- SetWord at script top level binds into the user context.
- Function bodies get bound at `func`/`does` creation time, not at load.
- **Debug ergonomics (M31):** `--disasm` prints the bytecode disassembly with
  per-instr `file:line:col` annotations (no run); `--disasm-func <name>` disassembles
  a named top-level func body; `--trace` emits one line per executed VM instr to stderr.

### Natives (~140)
- **I/O:** `print`, `prin`, `probe`, `form`, `mold`.
- **Arithmetic / comparison / logic:** `+ - * / //`, `**` (power), `= <> < >
  <= >=`, `and or not` (logic + bitwise on integers), `abs`, `negate`,
  `add`/`subtract`/`multiply`/`divide`, `min`, `max`, `round` (`/to`/`/even`),
  `random` (`/seed`/`/only`/`/secure`), `even?`, `odd?`, `complement`,
  `shift-left`, `shift-right`.
- **Control flow:** `if`, `either`, `loop`, `repeat`, `until`, `while`,
  `do`, `reduce`, `break`, `continue`, `switch` (`/default`/`/case`), `case`
  (`/default`/`/all`), `default`, `all`, `any`, `try`, `attempt`, `catch`/
  `throw`, `cause-error`, `function` (auto-locals), `comment`, `exit`/`quit`.
- **Series (full model):** `first` `second` `third` `last` `next` `back` `at`
  `skip` `head` `tail` `index?` `length?` `pick` `poke` `select` `find`
  (`/case`/`/part`) `append` (`/only`) `insert` `change` `remove` `clear`
  `take` `copy` (`/part`) `empty?` `block?` `paren?` `series?` `any-block?`
  `foreach` `forall`.
- **Functions / binding:** `func`, `does`, `make function!`, `function?`,
  `return`, `bind`, `use`, `in`, `get`, `set`, `value?`. Recursion works;
  closures are explicitly out of scope.
- **Refinements:** general `/ref` dispatch — `copy/part`, `find/case`,
  `split/with`, `trim/auto`, `replace/all`, `round/to`, user `func [x /with y]`,
  etc. Refinement-arg exhaustion names the offending refinement.
- **Strings:** `rejoin`, `reform`, `join`, `split` (`/with`), `trim`
  (`/auto` `/with` `/lines` `/all`), `replace` (`/all`), `uppercase`/
  `lowercase` (`/part`), `suffix?`. `+` concatenates two strings; `find`
  does substring search; `copy` on a string honors `/part`.
- **Type conversions:** `to-integer`, `to-float`, `to-string`, `to-block`,
  `to-word`/`to-set-word`/`to-get-word`/`to-lit-word`, `to-logic`, `to-file`,
  `to-url`, `to-path`/`to-get-path`/`to-lit-path`, `make`, `to`, `form`.
- **Objects:** `make object!`, `object`, `context`, `in`, `words-of`,
  `values-of`, `reflect`, `object?`, `same?`. Prototype inheritance, `self`
  reference, method calls via `o/method` paths.
- **Paths:** `obj/field`, `block/2`, `string/3`, `obj/field: value`
  (set-path), `:obj/method` (get-path), `'foo/bar` (lit-path), nested
  `obj/inner/x`, paren selectors `b/(1 + 1)`.
- **File & shell I/O:** `read` (`/lines`), `write` (`/lines`/`/append`),
  `load`, `save`, `exists?`, `size?`, `modified?`, `dir?`, `make-dir`,
  `delete`, `rename`, `change-dir`, `what-dir`, `call`/`shell`
  (`--allow-shell` gated), `wait`, `env`/`get-env`/`set-env`, `system/options/args`.
  `read` of `url!` fetches via `ureq` (http/https).
- **Dialect:** `parse` — matcher subset over string! and block! inputs:
  `skip`, `to`, `thru`, `end`, `none`, `any`, `some`, `opt`, `while`, `|`
  (alternative), `copy 'word rule`, `set 'word rule`, `[...]` grouping,
  `(...)` Red side-effects. Backtracking via cursor save/restore.
- **Constants:** `none`, `true`, `false`, `newline` bound in the user context.

### Errors
Unified `Error` (Lex / Parse / Eval). Every error carries a `Span`; the CLI
renders `*** Error: [file:line:col: ]<msg>` using a precomputed line map.
Path-step errors localize to the offending part's span; `load %file` parse
errors fold the loaded file's `file:line:col:` into the message body.
`Return`/`Break`/`Continue`/`Throw`/`Quit` are control-flow unwinds caught by
the function-call shim, loop natives, `catch`, and the script entry point.
`try`/`attempt` catch errors as `Error` values.

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
| `strings.red` | `"..."` escapes and `{...}` multi-line braced strings, `prin`, M15 string natives (`rejoin`/`split`/`trim`/`replace`/`uppercase`/`lowercase`/`suffix?`, string `+`, string `find`/`copy`) |
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
├── Cargo.toml                 # [workspace] (excludes fuzz/)
├── crates/
│   ├── red-core/              # value model, lexer, parser, printer, VM IR types
│   │   ├── src/{lib,value,context,env,error,source,lexer,parser,printer,vm_ir}.rs
│   │   └── tests/{round_trip,property}.rs + golden/
│   ├── red-eval/              # compiler + VM + tree-walker + natives + parse
│   │   ├── src/{lib,interp,interp_runner,interp_walker,binding,series,parse,
│   │   │        strings,math,convert,object,path,io}.rs
│   │   ├── src/natives/       # split by concern: compare/control/eval/func/io/words/registry
│   │   ├── src/vm/            # compiler.rs, vm.rs, lex.rs, pool.rs
│   │   ├── benches/eval.rs    # criterion A/B bench harness (VM vs walker)
│   │   └── tests/{programs,programs_errors,disasm,property,parity,bench_fixtures}.rs
│   └── red-cli/               # binary + REPL
│       ├── src/{main,repl}.rs
│       └── tests/cli.rs
├── fuzz/                      # cargo-fuzz targets (nightly-only, excluded from workspace)
├── examples/                  # sample .red programs
├── BENCHMARKS.md              # v0.3 VM bench numbers
├── project-brief.md           # feature scope and design decisions
├── architecture.md            # implementation sketch (lexer/parser/compiler/VM/eval internals)
├── plan.md                    # v0.1 build checklist (complete)
├── plan2.md                   # v0.2 roadmap (complete)
└── plan3.md                   # v0.3 VM + performance roadmap
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

## Known gaps (v0.3)

See [`project-brief.md`](./project-brief.md) and [`plan3.md`](./plan3.md) for
the authoritative list. Headlines:

- **No closures** — `func` shallow-copies args per call.
- **No `char!`, `map!`, `pair!`, `tuple!`, `date!`, `bitset!`** (and
  `binary!`/`String8` is a stub). `now`/string char pick are deferred with
  `date!`/`char!`.
- **`parse` matcher subset only** — `collect`/`keep`/`match`/grammar
  extraction/`case` flag deferred.
- **No modules / `import` / `export`.**
- **Error values are partial** — `try`/`attempt` catch errors as `Error`
  values carrying just the message; a full error model (code/type/args,
  `make error!` with fields) is deferred to v0.3. `cause-error` is a stub.
- **No `compose`.**
- **Object path method calls** work for `o/method` followed by trailing
  block args; `func/refinement` bound refinement references are deferred.
- **`read/binary`/`write/binary`** stubbed pending `binary!`.
- **No trig math** (`sin`/`cos`/`tan`/`sqrt`/`log`).
- **GUI / `draw` / `vid` / reactive dialects are permanently out of scope.**

## License

Licensed under the Apache License, Version 2.0 — see
[`LICENSE`](./LICENSE). The upstream [Red project](https://www.red-lang.org/)
holds its own license for the language and canonical spec.
