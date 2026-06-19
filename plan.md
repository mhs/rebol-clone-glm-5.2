# Plan: Step-by-Step Build of the Red Clone POC

Execution checklist for the project described in `project-brief.md` and
architected in `architecture.md`. Work top-to-bottom; each milestone depends
on the one above unless noted.

## Milestone 1 — Scaffold workspace

- [x] Create root `Cargo.toml` with `[workspace]` and `members = ["crates/red-core", "crates/red-eval", "crates/red-cli"]`
- [x] Create `crates/red-core/Cargo.toml` (lib, deps: `string_cache`)
- [x] Create `crates/red-eval/Cargo.toml` (lib, deps: `red-core`)
- [x] Create `crates/red-cli/Cargo.toml` (bin, deps: `red-eval`, `rustyline`)
- [x] Create empty `src/lib.rs` in `red-core` and `red-eval`
- [x] Create `src/main.rs` in `red-cli` printing "red 0.0.1"
- [x] Add `[dev-dependencies]` `assert_cmd`, `predicates` to `red-cli`
- [x] Create `examples/` directory with a placeholder `.gitkeep`
- [x] Run `cargo build --workspace` — passes with no errors
- [x] Run `cargo test --workspace` — passes (no tests yet)
- [x] Add `.gitignore` for `/target`
- [ ] Commit "scaffold workspace" baseline

## Milestone 2 — Value, Symbol, Printer

- [x] Create `red-core/src/value.rs`
- [x] Define `Symbol` newtype over `Rc<str>` (defer `string_cache` until needed)
- [x] Define `Span { start: usize, end: usize }`
- [x] Define `Series { data: Rc<RefCell<Vec<Value>>>, index: usize }`
- [x] Define `Binding` enum (`Unbound`, `Local`, `Func`) — `Local`/`Func` variants can be unit for now
- [x] Define `FuncDef` struct (fields stubbed, `native: Option<NativeFn>`)
- [x] Define `Value` enum (all variants from brief, even if unused yet)
- [x] Implement `Clone` for `Value` (deep clone via `Rc::clone` for shared data)
- [x] Implement `Debug` for `Value` (Rust-side, not the Red mold)
- [x] Add `Value::span()` helper returning `Option<Span>` (None on literals until spans are attached)
- [x] Create `red-core/src/context.rs` with `Context` skeleton (empty slots + name map)
- [x] Create `red-core/src/printer.rs`
- [x] Implement `mold(&Value, &mut String)` recursive writer
- [x] Mold `None` → `none`, `Logic(true)` → `true`, `Logic(false)` → `false`
- [x] Mold `Integer` → decimal digits, `Float` → with `.` (even if `.0`)
- [x] Mold `String` → double-quoted with escapes (`"`, `\\`, `\n`, `\t`)
- [x] Mold `Word`/`SetWord`/`GetWord`/`LitWord` → sym + prefix/suffix
- [x] Mold `Block` → `[ ... ]` with single spaces, no leading/trailing whitespace
- [x] Mold `Paren` → `( ... )`
- [x] Mold `Func` → `#[function]` placeholder (POC)
- [x] Mold `Path` → `foo/bar`
- [x] Mold nested blocks recursively
- [x] Export `Value`, `Symbol`, `Span`, `Series`, `mold` from `lib.rs`
- [x] Inline `#[test]` for each value kind's mold
- [x] Inline `#[test]` for nested block molding
- [x] Inline `#[test]` for string escaping round-trip
- [x] `cargo test -p red-core` passes

## Milestone 3 — Lexer

- [x] Create `red-core/src/lexer.rs`
- [x] Define `TokenKind` enum (Integer, Float, String, Word, SetWord, GetWord, LitWord, LBracket, RBracket, LParen, RParen)
- [x] Define `Token { kind: TokenKind, span: Span }`
- [x] Define `LexError` enum (UnterminatedString, InvalidNumber, InvalidWord, UnbalancedBrace)
- [x] Implement `pub fn lex(src: &str) -> Result<Vec<Token>, LexError>`
- [x] Skip whitespace (space, tab, CR, LF)
- [x] Skip `;` comments to EOL
- [x] Emit `LBracket`/`RBracket`/`LParen`/`RParen` with spans
- [x] Implement `scan_number`: digits, optional `.digits` → Float, optional `e[+-]digits`
- [x] Reject `1.2.3` with `InvalidNumber`
- [x] Implement `scan_quoted`: `"..."` with escape table (`\"`, `\\`, `\n`, `\t`, `\r`)
- [x] Error `UnterminatedString` on EOF
- [x] Implement `scan_braced`: `{...}` with depth counter, nested braces, multi-line
- [x] Error `UnbalancedBrace` on EOF with depth > 0
- [x] Implement `scan_word`: read run of non-delimiter chars
- [x] Delimiter set: whitespace, `[](){};",`
- [x] Classify leading `:` → GetWord, leading `'` → LitWord, trailing `:` → SetWord
- [x] Reject empty word (`InvalidWord`) e.g. `::` or `''`
- [x] Intern symbols via `Symbol::new`
- [x] Every token has correct byte-offset span
- [x] Inline `#[test]`: integer, negative integer, float, float with exponent
- [x] Inline `#[test]`: quoted string with each escape
- [x] Inline `#[test]`: braced string single-line and multi-line
- [x] Inline `#[test]`: nested braced string `{{a}}`
- [x] Inline `#[test]`: word, set-word, get-word, lit-word
- [x] Inline `#[test]`: block and paren delimiters intermixed
- [x] Inline `#[test]`: comment to EOL skipped
- [x] Inline `#[test]`: unterminated string/brace errors
- [x] Export `lex`, `Token`, `TokenKind`, `LexError` from `lib.rs`
- [x] Create `red-core/tests/round_trip.rs` empty harness (one trivial fixture)
- [x] Create `red-core/tests/golden/` with 2-3 trivial `.red` + `.expected` pairs
- [x] `cargo test -p red-core` passes

## Milestone 4 — Parser

- [x] Create `red-core/src/parser.rs`
- [x] Define `Parser<'a> { tokens: &'a [Token], pos: usize }`
- [x] Define `ParseError` enum (Unexpected, MissingClose, EmptyInput)
- [x] Implement `peek()`, `advance()`, `consume(kind)`
- [x] Implement `parse_value()` dispatch on token kind
- [x] Parse `LBracket ... RBracket` → `Value::Block(Series { index: 0, ... })`
- [x] Parse `LParen ... RParen` → `Value::Paren(Series)`
- [x] Parse atoms: Integer/Float/String → corresponding variants
- [x] Parse word-family → `Word`/`SetWord`/`GetWord`/`LitWord` with `Binding::Unbound`
- [x] Carry token spans onto parsed `Value`s (extend `Value` to hold spans)
- [x] Implement `parse_block()` and `parse_paren()` with EOF→MissingClose error
- [x] Implement `parse_program()`: detect `Red` word + header block + body block
- [x] Implement `load()` for bare body (no header)
- [x] Handle empty block `[]` and empty paren `()`
- [x] Handle nested blocks `[a [b c] d]`
- [x] Reject stray `]` or `)` with `Unexpected` error
- [x] Wire up end-to-end: `parse_program(lex(src)?)` returns `Result<(Series, Series), ParseError>`
- [x] Add `pub fn load_source(src: &str) -> Result<Series, Error>` convenience combining lex+parse
- [x] Inline `#[test]`: single integer parses to `Block[Integer]`
- [x] Inline `#[test]`: nested block structure
- [x] Inline `#[test]`: all word kinds parse correctly
- [x] Inline `#[test]`: header + body parse correctly
- [x] Inline `#[test]`: bare body via `load`
- [x] Inline `#[test]`: MissingClose error on `[1 2`
- [x] Inline `#[test]`: Unexpected error on stray `]`
- [x] Update `red-core/tests/round_trip.rs` to walk `tests/golden/` and compare `mold(parse(src))` to `.expected`
- [x] Add `tests/common/mod.rs` helper to enumerate fixture pairs
- [x] Add 8-10 golden fixtures covering: literals, strings, words, nested blocks, parens, comments, header
- [x] Run round-trip; all green
- [x] `cargo test -p red-core` passes

## Milestone 5 — Env, Context, minimal eval

- [x] Create `red-eval/src/context.rs`
- [x] Re-export `Context`, `Binding`, `FuncDef` from `red-core`
- [x] Implement `Context::new()` with empty slots + name map
- [x] Implement `Context::slot_mut(&mut self, sym: Symbol) -> &mut RefCell<Value>` (allocate if absent)
- [x] Implement `Context::get(&self, sym: Symbol) -> Option<Value>`
- [x] Implement `Context::set(&mut self, sym: Symbol, val: Value)`
- [x] Define `Env { user_ctx: Context, call_stack: Vec<CallFrame>, natives: HashMap<Symbol, NativeFn> }`
- [x] Define `CallFrame { ctx: Context, func: Option<Rc<FuncDef>> }`
- [x] Define `EvalError` enum (UnboundWord, TypeError, Arity, Return, Native)
- [x] Create `red-eval/src/interp.rs`
- [x] Implement `pub fn eval(block: &Value, env: &mut Env) -> Result<Value, EvalError>`
- [x] Eval arm: literals return self
- [x] Eval arm: `Block` returns as-is (data)
- [x] Eval arm: `Paren` walks eagerly
- [x] Eval arm: `Word` resolves via binding or native lookup
- [x] Eval arm: `SetWord` evals next value, writes to bound slot
- [x] Eval arm: `GetWord` returns slot value without calling
- [x] Eval arm: `LitWord` returns as-is
- [x] Implement `resolve_word(sym, binding, env, span)`
- [x] Implement `write_setword(sym, binding, val, env, span)`
- [x] Implement binding pass: walk parsed tree, attach `Local(user_ctx, slot)` to top-level SetWords and matching Words
- [x] Expose `pub fn run_source(src: &str) -> Result<Value, Error>` combining load + bind + eval
- [x] Inline `#[test]`: `5` evaluates to `Integer(5)`
- [x] Inline `#[test]`: `foo: 5 foo` evaluates to `Integer(5)`
- [x] Inline `#[test]`: unbound word errors
- [x] Inline `#[test]`: paren evaluates eagerly
- [x] Inline `#[test]`: block returns as-is
- [x] `cargo test -p red-eval` passes

## Milestone 6 — print/prin + hello world

- [x] Create `red-eval/src/natives.rs`
- [x] Implement native registration: `pub fn register_natives(env: &mut Env)`
- [x] Implement `print` native: mold each arg, join with space, append newline, write to stdout
- [x] Implement `prin` native: like print, no trailing newline
- [x] Implement `probe` native: mold arg, print `== <mold>`
- [x] Register `none`, `true`, `false`, `newline` as constants (Values bound in user_ctx)
- [x] Update CLI `red-cli/src/main.rs`:
- [x] Parse args (`file.red` or no args)
- [x] Read file, call `run_source`, print result via `mold`
- [x] Exit code 0 on success, 1 on error (print `*** Error: ...` to stderr)
- [x] Add `--help` and `--version`
- [x] Create `examples/hello.red`: `Red [] print "Hello, World!"`
- [x] Run `cargo run -p red-cli -- examples/hello.red` → prints `Hello, World!`
- [x] Create `red-cli/tests/cli.rs` with `assert_cmd` test for `hello.red`
- [x] Add error-path CLI test (file with unbound word)
- [x] Inline `#[test]`: `print 5` → stdout "5\n"
- [x] Inline `#[test]`: `prin "a" prin "b"` → stdout "ab"
- [x] Inline `#[test]`: `print [1 2 3]` → stdout "[1 2 3]\n"
- [x] `cargo test --workspace` passes

## Milestone 7 — Arithmetic, conditionals, loops

- [x] Implement `+` native: Integer/Float, mixed promotes to Float
- [x] Implement `-`, `*`, `/` (division by zero → EvalError)
- [x] Implement `=` `<>` `<` `>` `<=` `>=` returning `Logic`
- [x] Implement `and`, `or`, `not` for `Logic`
- [x] Implement `if cond block` → evaluates block if cond is truthy, else `None`
- [x] Implement `either cond t-block f-block`
- [x] Implement `loop block` — infinite loop until `break` (return `none` for now)
- [x] Implement `repeat 'word count block` — binds counter, runs block N times
- [x] Implement `until block` — runs block until it returns truthy
- [x] Implement `while cond-block body-block`
- [x] Implement `break`/`continue` via `EvalError` variants caught by loop natives
- [x] Implement `do block` — walks block, returns last value
- [x] Implement `reduce block` — evals each value, returns block of results
- [x] Truthiness rule: only `false` and `none` are falsy; everything else truthy
- [x] Inline `#[test]`: `1 + 2 = 3`
- [x] Inline `#[test]`: `10 / 0` errors
- [x] Inline `#[test]`: `if true [42]` → 42
- [x] Inline `#[test]`: `if false [42]` → none
- [x] Inline `#[test]`: `either 1 > 0 ["y"]["n"]` → "y"
- [x] Inline `#[test]`: `repeat i 3 [print i]` → "1\n2\n3\n"
- [x] Inline `#[test]`: `until [i: i + 1 i > 3]` → true, i == 4
- [x] Inline `#[test]`: `while [a < 3][a: a + 1]` → terminates
- [x] Inline `#[test]`: `reduce [1 + 1 2 + 2]` → `[2 4]`
- [x] Add 3-4 golden program fixtures exercising arithmetic + loops
- [x] `cargo test --workspace` passes

## Milestone 8 — Series model

- [x] Create `red-eval/src/series.rs`
- [x] Implement `block?`, `paren?`, `series?`, `any-block?`, `empty?`
- [x] Implement `first`, `second`, `third`, `last`
- [x] Implement `next`, `back` (return new Series with adjusted index)
- [x] Implement `at`, `skip` (index-based navigation)
- [x] Implement `head`, `tail` (index 0 / index == len)
- [x] Implement `index?`, `length?`
- [x] Implement `pick` (by 1-based index)
- [x] Implement `poke` (mutate by index)
- [x] Implement `select` (linear search, return value after match)
- [x] Implement `find` (return positioned series or none)
- [x] Implement `append` (mutate shared storage)
- [x] Implement `insert` (at cursor)
- [x] Implement `change` (replace at cursor)
- [x] Implement `remove` (at cursor, optional /part)
- [x] Implement `clear` (truncate from cursor)
- [x] Implement `take` (remove + return)
- [x] Implement `copy` (shallow; /part optional)
- [x] Implement `foreach 'word series block` — iterate, bind word, do block
- [x] Implement `forall 'word series block` — advance series cursor between iterations
- [x] Register all series natives in `register_natives`
- [x] Inline `#[test]`: `first [1 2 3]` → 1
- [x] Inline `#[test]`: `next [1 2 3]` then `first` → 2
- [x] Inline `#[test]`: `append [1 2] 3` → `[1 2 3]` and original mutated
- [x] Inline `#[test]`: `select [a 1 b 2] 'b` → 1
- [x] Inline `#[test]`: `find [1 2 3] 2` returns positioned series
- [x] Inline `#[test]`: `foreach x [1 2 3][print x]` → "1\n2\n3\n"
- [x] Inline `#[test]`: shared storage mutation visible via aliases
- [x] Add 4-5 golden fixtures exercising series ops
- [x] `cargo test --workspace` passes

## Milestone 9 — Functions + binding

- [x] Create `red-eval/src/binding.rs`
- [x] Implement `func` native: takes spec block + body block, returns `Value::Func`
- [x] Bind function body words to fresh function context at creation time
- [x] Implement `does` native: zero-arg `func`
- [x] Implement `make function!` (same as `func`)
- [x] Implement `function?` predicate
- [x] Implement `return value` native — unwinds via `EvalError::Return`
- [x] Function call shim: push `CallFrame`, bind params, eval body, pop, catch Return
- [x] Support default arg evaluation: caller evaluates args before call
- [x] Implement `bind block context` — rebind words in a block to a context
- [x] Implement `use [words] block` — creates local context, binds words, evals block
- [ ] Implement `in context 'word` — returns bound word value — *deferred (objects out of scope; `in` documented out-of-scope in `binding.rs`)*
- [x] Implement `get 'word` — returns value bound to word
- [x] Implement `set 'word value` — sets value in word's context
- [x] Implement `value? 'word` — returns true if word has a value
- [x] Recursive functions work (function can call itself)
- [x] Closures explicitly out of scope (document in code comment)
- [x] Inline `#[test]`: `square: func [x][x * x] square 5` → 25
- [x] Inline `#[test]`: `does` zero-arg call
- [x] Inline `#[test]`: `return` exits early
- [x] Inline `#[test]`: recursive factorial
- [x] Inline `#[test]`: `use [x][x: 5 x]` → 5, x unbound outside
- [x] Inline `#[test]`: `value? 'foo` before/after `foo: 5`
- [x] Inline `#[test]`: `bind` rebinds words to a context
- [x] Add 5-6 golden fixtures using functions, recursion, locals
- [x] `cargo test --workspace` passes

## Milestone 10 — `parse` dialect

- [x] Create `red-eval/src/parse.rs`
- [x] Implement `pub fn parse_native(args: &[Value], env: &mut Env) -> Result<Value, EvalError>`
- [x] Input: `string!` or `block!` (Series); rules: `block!`
- [x] Maintain input cursor (string byte index or Series index)
- [x] Rule: literal value matches against current input, advances cursor
- [x] Rule: `skip` — advance one element/char
- [x] Rule: `to value` — advance until value found (cursor before match)
- [x] Rule: `thru value` — advance past value
- [x] Rule: `end` — assert cursor at end
- [x] Rule: `none` — always matches, no advance
- [x] Rule: `any rule` — zero-or-more
- [x] Rule: `some rule` — one-or-more
- [x] Rule: `opt rule` — zero-or-one
- [x] Rule: `while rule` — greedy like `any` but checks end condition
- [x] Rule: `|` — alternative (try left, on fail try right)
- [x] Rule: `copy 'word rule` — capture matched sub-input, bind word in user context
- [x] Rule: `set 'word rule` — bind word to single matched value
- [x] Rule: `[...]` — sub-rule group
- [x] Rule: `(...)` — Red code side-effect, evaluated via `eval`
- [x] Return `Logic` (true = matched entirely, false = failed)
- [x] Backtracking: save cursor before each alternative/repetition; restore on failure
- [x] Register `parse` in `register_natives`
- [x] Inline `#[test]`: `parse "abc" ["a" "b" "c"]` → true
- [x] Inline `#[test]`: `parse "abc" ["a" "z"]` → false
- [x] Inline `#[test]`: `parse [1 2 3] [1 2 3]` → true
- [x] Inline `#[test]`: `parse "hello" [copy w to end]` → true, w == "hello"
- [x] Inline `#[test]`: `parse "a;b;c" [some [skip to ";"]]` → true
- [x] Inline `#[test]`: `parse` with `(...)` side-effect runs
- [x] Add 4-5 golden fixtures for `parse` (string + block inputs)
- [x] `cargo test --workspace` passes

## Milestone 11 — REPL

- [x] Add `rustyline` to `red-cli` deps
- [x] Implement REPL loop: prompt → read line → `load_source` → `eval` → `mold` → print
- [x] Persist `Env` across lines (user context + natives carry over)
- [x] Handle multi-line blocks: if parse reports unclosed `[`/`(`, prompt for continuation
- [x] Handle empty input (just prompt again)
- [x] Print errors as `*** Error: <msg>` but don't exit REPL
- [x] Bind Ctrl-C / Ctrl-D to clean exit
- [x] Add `--repl` flag explicitly (or no-args = REPL)
- [x] Support `quit`/`exit` words as aliases for Ctrl-D
- [x] Mold result of each line unless it's `none` (matches Red REPL behavior)
- [x] Inline `#[test]` (or integration test) feeding "5\n" → captures "5\n"
- [x] Inline `#[test]` feeding `x: 10\n x\n` → captures `10`
- [x] CLI test: `assert_cmd` spawning REPL with piped stdin
- [x] `cargo test --workspace` passes

## Milestone 12 — Golden suite + error polish

- [x] Audit `EvalError` rendering: every variant produces a clear `*** Error:` line
- [x] Include span info in error messages (`file.red:line:col:`)
- [x] Implement line/col lookup from byte offset (precompute line starts in lexer)
- [x] Unbound word error names the symbol
- [x] Type errors name expected vs. found
- [x] Arity errors name the native and counts
- [x] Errors from natives carry a span (use first arg's span as fallback)
- [x] Add golden fixtures for each error case (one per error kind)
- [x] Expand `red-eval/tests/programs/` to 15-20 fixtures covering all features
- [x] Include fixtures for: arithmetic, strings, blocks, parens, functions, recursion, series ops, `parse`, errors
- [x] Add a `tests/programs/README.md` explaining fixture format
- [x] Audit `mold` output for printer edge cases (empty block, nested quotes, floats)
- [x] Add property-style test: `mold(parse(mold(v))) == mold(v)` for random-ish `Value`s
- [x] Run clippy on workspace; fix warnings
- [x] Run `cargo fmt --all --check`
- [x] Final `cargo test --workspace` green
- [x] Update `project-brief.md` and `architecture.md` if any drift was discovered
- [ ] Tag release `v0.1.0-poc`
