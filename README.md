# rebol-clone

A proof-of-concept clone of the [Red](https://www.red-lang.org/) programming
language, implemented in Rust. Red is a homoiconic, block-structured
descendant of Rebol — code is data, evaluation is prefix-style and eager,
and "dialects" are blocks interpreted by custom mini-interpreters.

This repo is a **Red subset interpreter** (`v0.6.0`). It implements a usable slice of
Red — lexer, parser, **bytecode compiler + stack VM** (the default since
v0.3), tree-walking evaluator (retained as the `--walk` fallback), full
series model, real word binding, functions, **first-class closures**
(`closure!` with snapshot freevar capture), **modules** (`module`/
`export`/`import`), the `parse` dialect (with `collect`/`keep`/`match`/
lookahead/`/case`/`bitset!` charset/**named-rule recursion**), **objects**,
**real paths**, **refinements**, the full type-conversion/string/control-flow/
math surfaces (incl. trig + transcendentals), file & shell I/O, **first-class
`error!` values** with the full Red field set, the v0.4 value types (`char!`/
`binary!`/`map!`/`pair!`/`tuple!`/`date!`/`bitset!`), `compose`, an
auto-imported **stdlib** (~25 utility functions, suppressible via
`--no-stdlib`), a REPL, and a synchronous **`port!` abstraction** with
minimal HTTP/HTTPS networking (`open`/`close`/`create`/`read port`,
`read url!` for GET via `ureq`, gated behind `--allow-network`). The build
history is tracked in [`plan.md`](./plan.md) (v0.1),
[`plan2.md`](./plan2.md) (v0.2), [`plan3.md`](./plan3.md) (v0.3 — VM +
performance), [`plan5.md`](./plan5.md) (v0.4 — language completeness),
[`plan6-closures-modules.md`](./plan6-closures-modules.md) (v0.5 —
closures & modules), and [`plan11-functional-gaps.md`](./plan11-functional-gaps.md)
(v0.6 — core functional gaps).

## Status

- **Tagged:** `v0.6.0`
- **Workspace:** three crates — `red-core` (value model + lexer + parser + printer + VM IR types),
  `red-eval` (compiler + VM + tree-walker + natives + `parse`), `red-cli` (binary + REPL).
  A `fuzz/` crate (nightly-only, `libfuzzer-sys`) is excluded from the default workspace.
- **Default evaluator:** bytecode VM (`EvalMode::Vm`). `--walk` on the CLI or the `force-walk` cargo feature forces the tree-walker for debugging and parity comparison.
- **Dependencies:** `red-core` pulls `indexmap` (for `map!`) and `chrono` (for
  `date!`/`now`/timezone offsets); `red-eval` pulls `ureq` (http/https fetches
  for `read url!`); `red-cli` pulls `rustyline` (REPL line editing). No async,
  no proc-macros.
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
cargo run  -p red-cli -- --version               # → red 0.6.0
cargo run  -p red-cli -- --allow-shell examples/call.red   # enable call/shell
cargo run  -p red-cli -- --allow-network http://example.com/  # enable read url!/open url!
cargo run  -p red-cli -- --walk examples/fib.red          # force tree-walker
cargo run  -p red-cli -- --disasm examples/fib.red        # disassemble (no run)
cargo run  -p red-cli -- --disasm-func fib examples/fib.red  # disassemble named func
cargo run  -p red-cli -- --trace examples/arith.red       # per-instr VM trace to stderr
cargo run  -p red-cli -- --module-path examples/modules \  # search dir for import %file
                        examples/modules/main.red
cargo run  -p red-cli -- --no-stdlib examples/arith.red    # skip stdlib auto-import
```

v0.3 was a **performance release**: the bytecode VM delivers 2–4× speedups on compute-heavy programs (deep recursion, tight loops) over the v0.2 tree-walker, while preserving exact observable behavior (golden parity, error parity). v0.4 re-opens the language surface on top of the unchanged VM — new value types (`char!`/`binary!`/`map!`/`pair!`/`tuple!`/`date!`/`bitset!`), `compose`, trig math, the full `error!` model, and the completed `parse` dialect. v0.5 adds **first-class closures** (`closure!` with snapshot freevar capture, fixing the v0.3 escaping-closure bug) and **modules** (`module`/`export`/`import`, with named-module caching, file-based import, and `system/options/module-path` search), plus a small auto-imported stdlib. v0.5.1 closes the **control-flow vocabulary gap** — `unless`, `forever`, `for` (direction-aware counted loop over int/float/char), `forskip` (record-wise series iteration). v0.6 closes four **core functional gaps**: `parse` **named-rule recursion** (a bound word resolving to a `block!` is a sub-rule, with a depth guard); `mold` exposed as a callable native (`/only`); series **`sort`** (native, shadowing the stdlib version) + set operations `unique`/`intersect`/`union`/`difference`/`exclude` on `block!`/`string!`; and a synchronous **`port!` abstraction** with minimal HTTP/HTTPS GET networking via the existing `ureq` dep (TLS on by default — no new dependency), gated behind `--allow-network`. All additions are additive: they compile through the existing VM const-pool + native-call path with no new hot-path instrs. See `BENCHMARKS.md` for measurements.

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
`LitWord`, `Block`, `Paren`, `Func`, `Closure` (snapshot-capture first-class
closure), `Module` (self-contained namespace with exported words),
`Path`/`GetPath`/`LitPath`/`SetPath`, `Refinement`, `File`, `Url`, `Object`,
`Error`, `Char` (`#"a"`), `String8` (real `binary!`, `#{hex}`), `Map`
(heterogeneous insertion-ordered keys), `Pair` (`100x200`), `Tuple`
(`255.0.0` / `128.64.32.128` RGBA), `Date` (date-only / date+time /
date+time+zone; `29-Jun-2024/12:30:00+5:30`), `Bitset` (bit-packed charset
for `parse`), `Port` (synchronous I/O handle — file or HTTP; v0.6).

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
  `Lexical(depth, slot)` (VM-only) / `Closure(idx)` (freevar capture cell).
  Unbound words at eval time consult `user_ctx` first (so `import`-aliased
  words resolve), then the native registry, then error Red-style ("has no
  value").
- SetWord at script top level binds into the user context.
- Function bodies get bound at `func`/`does` creation time, not at load.
- **Debug ergonomics (M31):** `--disasm` prints the bytecode disassembly with
  per-instr `file:line:col` annotations (no run); `--disasm-func <name>` disassembles
  a named top-level func body; `--trace` emits one line per executed VM instr to stderr.

### Natives (~140)
- **I/O:** `print`, `prin`, `probe`, `form`, `mold` (`/only` — callable native
  wrapping the printer; v0.6).
- **Arithmetic / comparison / logic:** `+ - * / //`, `**` (power), `= <> < >
  <= >=`, `and or not` (logic + bitwise on integers), `abs`, `negate`,
  `add`/`subtract`/`multiply`/`divide`, `min`, `max`, `round` (`/to`/`/even`),
  `random` (`/seed`/`/only`/`/secure`), `even?`, `odd?`, `complement`,
  `shift-left`, `shift-right`. **Trig + transcendentals (v0.4):** `sin`/`cos`/
  `tan`/`asin`/`acos`/`atan`/`atan2`, `sqrt`, `exp`, `log-e`/`ln`, `log-10`,
  `log-2`, `degrees`/`radians`; `pi`/`e` constants. Pair/tuple arithmetic
  (componentwise: `pair + pair`, `tuple + tuple`, `pair * int`, etc.).
- **Control flow:** `if`, `unless`, `either`, `loop`, `repeat`, `until`, `while`,
  `forever`, `for`, `forskip`, `do`, `reduce`, `break`, `continue`, `switch`
  (`/default`/`/case`), `case` (`/default`/`/all`), `default`, `all`, `any`,
  `try`, `attempt`, `catch`/`throw`, `cause-error`, `function` (auto-locals),
  `comment`, `exit`/`quit`, `compose` (`/deep`/`/only` — v0.4).
- **Series (full model):** `first` `second` `third` `last` `next` `back` `at`
  `skip` `head` `tail` `index?` `length?` `pick` `poke` `select` `find`
  (`/case`/`/part`) `append` (`/only`) `insert` `change` `remove` `clear`
  `take` `copy` (`/part`) `empty?` `block?` `paren?` `series?` `any-block?`
  `foreach` `forall` `forskip`. `binary!` is byte-indexed (`pick`/`poke`/`copy`/`find`/
  `append`/`insert`); `map!` supports `select`/`find`/`keys-of`/`values-of`/
  `length?`/`empty?`/`clear`/`put`/`extend`/`copy`. **v0.6:** `sort`
  (`/case`/`/reverse`/`/skip size`/`/compare func` — native, destructive)
  and set operations `unique`/`intersect`/`union`/`difference`/`exclude`
  (`/case`/`/skip`) on `block!`/`string!` (the same names dispatch on
  `bitset!` to the M46 implementations).
- **Functions / binding:** `func`, `does`, `make function!`, `function?`,
  `return`, `bind`, `use`, `in`, `get`, `set`, `value?`. Recursion works.
- **Closures (v0.5):** `closure [spec] [body]` — like `func` but captures
  freevar *values* into an owned cell at creation time (snapshot semantics).
  `closure?` predicate; `function?` returns true on closures too. The
  closure can escape its defining frame (returned, stored, passed around)
  and still see the captured values. Inner writes to the capture cell
  persist across invocations of the same closure (the `RefCell` cell
  mechanism); outer writes after creation do NOT propagate inward (snapshot,
  not shared cell — a v0.6 candidate).
- **Modules (v0.5):** `module [body]` (anonymous) / `module 'name [body]`
  (named, cached singleton). `export 'word` / `export [words]` marks words
  for public visibility (only valid inside a module body). `import 'name` /
  `import %file.red` / `import <module-value>` aliases a module's exports
  into the current context as bare words. `module?` predicate.
  `words-of`/`values-of`/`reflect` on a module return only exported words
  from outside; all words are visible inside the module body. `module/word`
  path resolution checks exports from outside; private words error.
  `system/options/module-path` (a `block!` of `file!` directories,
  default `[%./]`) is searched by `import %file.red` when the file isn't
  found relative to cwd; the CLI `--module-path <dir>` flag (repeatable)
  appends entries.
- **Refinements:** general `/ref` dispatch — `copy/part`, `find/case`,
  `split/with`, `trim/auto`, `replace/all`, `round/to`, user `func [x /with y]`,
  etc. Refinement-arg exhaustion names the offending refinement.
- **Strings:** `rejoin`, `reform`, `join`, `split` (`/with`), `trim`
  (`/auto` `/with` `/lines` `/all`), `replace` (`/all`), `uppercase`/
  `lowercase` (`/part`), `suffix?`. `+` concatenates two strings; `find`
  does substring search; `copy` on a string honors `/part`. `append`/`insert`
  on a `string!` accept `char!`/`string!` (v0.4).
- **Type conversions:** `to-integer`, `to-float`, `to-string`, `to-block`,
  `to-word`/`to-set-word`/`to-get-word`/`to-lit-word`, `to-logic`, `to-file`,
  `to-url`, `to-path`/`to-get-path`/`to-lit-path`, `make`, `to`, `form`.
  **v0.4 additions:** `to-char`, `to-binary`, `to-map`, `to-pair`, `to-tuple`,
  `to-date`, `to-bitset`, `to-error`, `to-utc`.
- **Type predicates (v0.4 fill-in):** `integer?`, `float?`, `number?`,
  `string?`, `logic?`, `none?`, `char?`, `binary?`, `map?`, `pair?`, `tuple?`,
  `date?`, `time?`, `bitset?`, `error?`, `word?`, `set-word?`, `get-word?`,
  `lit-word?`, `refinement?`, `path?`, `get-path?`, `lit-path?`, `any-word?`,
  `any-path?`, `any-object?`, `function?`, `object?`, `series?`, `block?`,
  `paren?`, `file?`, `url?`, `same?`, `not-same?`, `value?`. `type?` returns
  the type word; `types-of` returns the block of matching type words.
- **Objects:** `make object!`, `object`, `context`, `in`, `words-of`,
  `values-of`, `reflect`, `object?`, `same?`. Prototype inheritance, `self`
  reference, method calls via `o/method` paths.
- **Paths:** `obj/field`, `block/2`, `string/3` (returns `char!`), `obj/field:
  value` (set-path), `:obj/method` (get-path), `'foo/bar` (lit-path), nested
  `obj/inner/x`, paren selectors `b/(1 + 1)`. **v0.4 additions:** `map/word`,
  `map/integer`, `map/string`, `map/char` (+ set-paths); `pair/x`/`pair/y`,
  `tuple/r`/`tuple/g`/`tuple/b`/`tuple/a` (+ set-paths); `date/year`/
  `month`/`day`/`time`/`weekday`/`yearday`/`week`/`zone` (+ `date/zone:`
  relabel); literal-headed paths `100x200/x`, `255.0.0/r`.
- **File & shell I/O:** `read` (`/lines`/`/binary`), `write`
  (`/lines`/`/append`/`/binary`), `load`, `save`, `exists?`, `size?`,
  `modified?` (returns `date!` with local timezone — v0.4), `dir?`, `make-dir`,
  `delete`, `rename`, `change-dir`, `what-dir`, `call`/`shell`
  (`--allow-shell` gated), `wait`, `env`/`get-env`/`set-env`, `system/options/args`.
  `read` of `url!` fetches via `ureq` (http/https). `read/binary`/`write/binary`
  de-stubbed in v0.4.
- **Ports & networking (v0.6):** `open` (`file!`/`url!`), `close`, `create`
  (`file!`), `port?` predicate, `read port` (streaming for HTTP, whole-file
  for files), `write port` (file ports only — HTTP is GET-only). `read url!`
  for `http://`/`https://` routes through the `net/` facade (GET via `ureq`,
  TLS on by default). All network access is gated behind `--allow-network`
  (default off, mirroring `--allow-shell`). Non-HTTP protocols
  (FTP/SMTP/POP3/NNTP/DNS/TCP/UDP/WHOIS/Finger/Daytime) are reserved
  `PortScheme` variants that error in v0.6; the async/`Channel`-backed port
  model is deferred to v0.7+.
- **Dialect:** `parse` — over string! and block! inputs. Rule set:
  `skip`, `to`, `thru`, `end`, `none`, `any`, `some`, `opt`, `while`, `|`
  (alternative), `copy 'word rule`, `set 'word rule`, `[...]` grouping,
  `(...)` Red side-effects. **v0.4 completions:** `collect 'word` /
  `collect into 'word`, `keep` (value / `'word` / `(expr)`), `match value`,
  `into 'word rule`, `fail`, `break`, `if (expr)`, `not rule`, `??` (debug),
  `accept value`, `reject`, `ahead rule`, `behind rule`, `bitset!` charset
  matching, `/case` refinement for case-sensitive string matching.
  **v0.7 integer-count rules:** `n rule` and `n m rule` (exact-count and
  range-count repetition), where `n`/`m` are `integer!` literals or words
  resolving to a non-negative `integer!`. Per Red semantics, *every*
  `integer!` in a rule block is a count prefix (literal-integer matching
  against block input uses `match`/lit-word/string forms instead).
  Negative counts and inverted ranges (`3 1 rule`) raise a parse error.
  **v0.6 named-rule recursion:** a bound word resolving to a `block!` is
  treated as a named sub-rule (parsed recursively against the same cursor);
  a word resolving to a `bitset!` still does charset matching. A depth
  guard raises `ParseRecursionLimit` on self/mutual-reference loops.
  Backtracking via cursor save/restore.
- **Dates (v0.4):** `now` (local time + local UTC offset), `today`,
  `to-utc`. Date arithmetic: `date + integer` (days), `date - date` (day
  diff, zone-adjusted), `date + time`. Timezone model: **fixed UTC offsets
  only** (`±HH:MM` / `Z`); no named zones, no DST.
- **Errors (v0.4):** first-class `error!` values with the full Red field
  set — `code`/`type`/`message`/`args`/`near`/`where`/`by`. `make error!`
  from string or block of keyword pairs; `try`/`attempt`/`catch` unwrap
  structured payloads; `cause-error` (1/2/4-arg + block forms);
  `error-type`/`error-code`/`error-args`/`error-near` accessors;
  `attempted?` predicate.
- **Bitset (v0.4):** `charset "ABC"`, `make bitset! [...]` (ranges, unions),
  `union`/`intersect`/`difference`/`complement`/`extract?` (membership test).
- **Constants:** `none`, `true`, `false`, `newline`, `pi`, `e` bound in the
  user context.
- **Stdlib (v0.5):** ~25 utility functions auto-imported into `user_ctx` at
  script startup (unless `--no-stdlib`): string utils (`str-upper`/
  `str-lower`/`starts-with?`/`ends-with?`/`contains?`/`str-join`/
  `repeat-str`/`pad-left`/`pad-right`), block utils (`block-sum`/
  `block-product`/`block-len`/`block-mean`/`mean`/`reverse-of`/`flatten`/
  `min-of`/`max-of`/`intersperse`/`chunk`), math utils (`gcd`/`lcm`/
  `sign-of`/`clamp`/`factorial-iter`), `range-of`. (The stdlib also defines
  a pure-Red `sort`, now shadowed by the v0.6 native `sort` — natives are
  looked up before stdlib bindings.) The stdlib is a module
  (`crates/red-eval/stdlib/stdlib.red`) embedded via `include_str!` and
  cached on `Env::stdlib` (so the REPL doesn't recompile per line).

### Errors
Unified `Error` (Lex / Parse / Eval). Every error carries a `Span`; the CLI
renders `*** Error: [file:line:col: ]<msg>` using a precomputed line map.
Path-step errors localize to the offending part's span; `load %file` parse
errors fold the loaded file's `file:line:col:` into the message body.
`Return`/`Break`/`Continue`/`Throw`/`Quit` are control-flow unwinds caught by
the function-call shim, loop natives, `catch`, and the script entry point.
`try`/`attempt` catch errors as `error!` values. **v0.4 structured errors:**
`EvalError::Raised(Rc<ErrorValue>)` transports first-class `error!` values with
the full Red field set (`code`/`type`/`args`/`near`/`where`/`by`); the VM and
walker auto-enrich `Native` errors with `where`/`near` via `enrich_error`;
structured errors render as `*** Error: [loc: ]<type> error: <message>` (e.g.
`math error: divide by zero`).

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
| `char.red` | `#"..."` literals, char arithmetic, char pick/poke (v0.4) |
| `binary.red` | `#{hex}` literals, `to-binary`, `read/binary`/`write/binary` (v0.4) |
| `compose.red` | `compose`/`compose/deep`/`compose/only` (v0.4) |
| `dates.red` | `date!`/`time!` literals, `now`/`today`/`to-utc`, zones, date arithmetic (v0.4) |
| `errors.red` | `make error!`, `try`/`attempt`/`catch`/`cause-error`, structured fields (v0.4) |
| `maps.red` | `make map!`, heterogeneous keys, path access (v0.4) |
| `pair-tuple.red` | `pair!`/`tuple!` literals, arithmetic, component paths (v0.4) |
| `trig.red` | `sin`/`cos`/`sqrt`/`log-*`/`atan2`/`degrees`/`radians`, `pi`/`e` (v0.4) |

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
│   │   ├── src/net/           # M113 port!/networking facade: mod/protocol/request/response/error/http
│   │   ├── src/vm/            # compiler.rs, vm.rs, lex.rs, pool.rs
│   │   ├── benches/eval.rs    # criterion A/B bench harness (VM vs walker)
│   │   └── tests/{programs,programs_errors,disasm,property,parity,bench_fixtures}.rs
│   └── red-cli/               # binary + REPL
│       ├── src/{main,repl}.rs
│       └── tests/cli.rs
├── fuzz/                      # cargo-fuzz targets (nightly-only, excluded from workspace)
├── examples/                  # sample .red programs
├── BENCHMARKS.md              # VM + walker bench numbers (v0.3 → v0.5)
├── KNOWN_ISSUES.md            # pre-existing bugs + VM/walker divergences
├── project-brief.md           # feature scope and design decisions
├── architecture.md            # implementation sketch (lexer/parser/compiler/VM/eval internals)
└── plan*.md                   # per-version build checklists (v0.1 → v0.6)
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
- **v0.4 value types are immutable where Red is immutable** (`char!`/`pair!`/
  `tuple!`/`date!`/`binary!`/`bitset!`). Set-paths on these return a new value
  and rebind; aliases don't see updates (mirrors Red semantics). `map!` uses
  `Rc<RefCell<MapDef>>` for in-place mutation like `object!`.

## Known gaps (v0.6)

See [`project-brief.md`](./project-brief.md) and
[`plan11-functional-gaps.md`](./plan11-functional-gaps.md) for the
authoritative list. Headlines:

- **Networking is a synchronous, GET-only subset** — `read http://`/`read
  https://` and `open`/`close`/`create`/`read port`/`write port` work (via
  `ureq`, TLS on by default), but: non-HTTP protocols (FTP/SMTP/POP3/NNTP/
  DNS/TCP/UDP/WHOIS/Finger/Daytime) are reserved `PortScheme` variants
  that error in v0.6; HTTP methods beyond GET, request headers/cookies/
  auth, redirect control, `write http://` (POST/PUT), and the async/
  `Channel`-backed port model are deferred to v0.7+. Network access is
  gated behind `--allow-network` (default off).
- **Closure capture is snapshot, not shared-cell** — each `closure` copies
  freevar values at creation time; outer writes after creation don't
  propagate inward, and SetWord inside a closure body is treated as a local
  (not a capture write — use block-as-state via `poke` for mutable closure
  state). Real Red `closure!` shares the cell across closures and across
  outer/inner; shared-cell is a v0.7 candidate.
- **`unimport` deferred to v0.7** — `import` aliases exports into `user_ctx`
  but there's no native to remove the aliases.
- **Timezones: fixed UTC offsets only** (`±HH:MM`/`Z`) — no named zones, no
  DST. Matches Red parity; named-zone support (`chrono-tz`) deferred to v0.7+.
- **`DD/MM/YYYY` date form not supported** — `/` is a lexer delimiter so the
  run splits before the date scanner. Use `DD-Mon-YYYY` or `YYYY-MM-DD`.
- **`pair!`/`tuple!` `same?`** returns `false` (immutable value types; use `=`
  for structural equality). `same?` is for reference-identity comparisons.
- **No `tag!`/`ref!`/`image!`/`vector!`/`hash!`/`regex!`**; advanced
  `bitset!`/`logic!` ops; `object!` `on-change` reactive slots; `routine!` FFI.
- **Object path method calls** work for `o/method` followed by trailing
  block args; `func/refinement` bound refinements references are deferred.
- **Reactivity (`react`/`is-thunk`) is a v0.7 candidate** (see
  `future-plan-reactivity.md`); concurrency (actors/channels) is a v0.7+
  candidate. GUI / `draw` / `vid` dialects are permanently out of scope.

## License

Licensed under the Apache License, Version 2.0 — see
[`LICENSE`](./LICENSE). The upstream [Red project](https://www.red-lang.org/)
holds its own license for the language and canonical spec.
