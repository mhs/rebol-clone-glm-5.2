# Plan 2: Toward a Useful Red Subset (v0.2)

Execution checklist extending the v0.1.0-poc baseline in `plan.md`. Pulls
objects and file I/O up early so the CLI is usable for real scripts, plus the
core-language completeness items (refinements, conversions, strings, control
flow, math). Work top-to-bottom; each milestone depends on the one above
unless noted.

Per `project-brief.md`, GUI/draw/VID/reactive dialects are **permanently out of
scope** and will not appear in this or future plans.

Deferred to v0.3+ (acknowledged but not built here): `char!`, `map!`, `pair!`,
`tuple!`, `date!`, `bitset!`, modules/`import`, error values as first-class,
`compose`, full port model, trig math, `parse` advanced (`collect`/`keep`).

## Milestone 13 — Refinements

- [x] Extend `FuncDef` with `refinements: Vec<(Symbol, Vec<Symbol>)>` (refinement
      word + the words it introduces) parsed from the spec block
- [x] Update `func`/`does`/`make function!` to capture refinement slots + arg
      order from the spec
- [x] Define `ArgSpec` representation for native signatures (arity + refinement
      names + each refinement's arity)
- [x] Replace fixed-arity native dispatch with refinement-aware collector:
      caller may pass `/ref` flags and their args inline
- [x] At call site, build an `args: &[Value]` plus
      `refinements: &[(Symbol, &[Value])]` map handed to the native
- [x] Change `NativeFn` signature to
      `fn(args: &[Value], refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError>`
- [x] Update `natives.rs` registration to declare refinements per native
- [x] Reimplement existing hard-coded `/part`/`/only`-style behaviors on
      `copy`, `find`, `append` using the new mechanism
- [x] Lex/parse refinement words (`/foo`) — new `TokenKind::Refinement` and
      `Value::Refinement { sym, span }`; mold back to `/foo`
- [x] Support refinement words as values (`'foo`-style use, `/foo` as lit)
- [x] Inline `#[test]`: `copy/part [1 2 3] 2` → `[1 2]`
- [x] Inline `#[test]`: `find/case [a A b] 'A` returns positioned series
- [x] Inline `#[test]`: user `func [x /only][...]` callable with and without `/only`
- [x] Inline `#[test]`: refinement arg passing `func [x /with y][...]`
- [x] Update golden fixtures for refinement-using programs
- [x] `cargo test --workspace` passes

## Milestone 14 — Type conversions + `make`/`to`

- [x] Implement `to-integer` (from float, string, logic, char)
- [x] Implement `to-float` (from integer, string)
- [x] Implement `to-string` (from integer, float, logic, block via `form`
      semantics, word via mold)
- [x] Implement `to-block` (from string via `load`, from word → `[word]`)
- [x] Implement `to-word`, `to-set-word`, `to-get-word`, `to-lit-word` (from
      string/word)
- [x] Implement `to-logic` (from any value via truthiness rule)
- [x] Implement `to-integer`/`to-float` error on unparseable string with span
- [x] Implement `make <type-spec> <value>` general dispatcher:
      - `make integer! 3.5` → 3 (truncates)
      - `make string! 5` → `"     "` (or empty per Red; pick documented behavior)
      - `make block! 3` → `[]` with capacity hint
- [x] Implement `to <type> <value>` as alias covering `to-*` family
- [x] Implement `form` (human-readable, space-joined, no delimiters) distinct
      from `mold` (reparseable)
- [x] Inline `#[test]`: each `to-*` round-trips for in-range inputs
- [x] Inline `#[test]`: `make` constructor for each supported type
- [x] Inline `#[test]`: `form [1 2 3]` → `"1 2 3"` (string, not block)
- [x] Add golden fixtures for conversions
- [x] `cargo test --workspace` passes

## Milestone 15 — String manipulation natives

- [ ] Implement `rejoin` (reduce block, concatenate molded results to string)
- [ ] Implement `reform` (reduce + form)
- [ ] Implement `join` (binary `join a b` shortcut)
- [ ] Implement `+` over strings (concatenation, both operands string)
- [ ] Implement `split` (and `/with` refinement for delimiter)
- [ ] Implement `trim` (and `/auto`/`/with`/`/lines`/`/all` refinements)
- [ ] Implement `replace` (and `/all` refinement)
- [ ] Extend `find` for string input: substring search, `/case` refinement,
      `/any` (wildcard) deferred
- [ ] Extend `copy` for strings including `/part`
- [ ] Implement `uppercase`/`lowercase` (and `/part`)
- [ ] Implement `suffix?` (file extension)
- [ ] Inline `#[test]`: `rejoin ["a" 1 "b"]` → `"a1b"`
- [ ] Inline `#[test]`: `"abc" + "def"` → `"abcdef"`
- [ ] Inline `#[test]`: `split "a,b,c" ","` → `["a" "b" "c"]`
- [ ] Inline `#[test]`: `trim "  hi  "` → `"hi"`
- [ ] Inline `#[test]`: `replace/all "a-a" "a" "b"` → `"b-b"`
- [ ] Inline `#[test]`: `find "hello" "ll"` returns index/position
- [ ] Add golden fixtures for string-processing programs
- [ ] `cargo test --workspace` passes

## Milestone 16 — Control flow expansion

- [ ] Implement `switch` (and `/default`, `/case` refinements)
- [ ] Implement `case` (and `/default`, `/all`)
- [ ] Implement `default 'word value` (set if unset/none)
- [ ] Implement `all [block]` (short-circuit, returns last truthy or `none`)
- [ ] Implement `any [block]` (short-circuit, returns first truthy or `none`)
- [ ] Implement `try [block]` (catch any error → error value or none)
- [ ] Implement `attempt [block]` (alias/variant of `try` returning none on error)
- [ ] Implement `catch [block]` / `throw value` pair
- [ ] Implement `cause-error` placeholder (until error values exist; map to
      `EvalError::Native`)
- [ ] Implement `function` (auto-locals) variant distinct from `func`:
      `function [x][local: 5 ...]` declares `local` as a local word
- [ ] Implement `comment` (skip block/string arg)
- [ ] Implement `exit`/`quit` from script (already in REPL; extend to script
      exit code)
- [ ] Inline `#[test]`: `switch 2 [1 ["a"] 2 ["b"]]` → "b"
- [ ] Inline `#[test]`: `case [1 > 2 ["a"] 2 > 1 ["b"]]` → "b"
- [ ] Inline `#[test]`: `all [true 1 2]` → 2; `all [true false]` → none
- [ ] Inline `#[test]`: `any [false 5 6]` → 5
- [ ] Inline `#[test]`: `try [1 + "a"]` returns error value, doesn't propagate
- [ ] Inline `#[test]`: `catch [throw 42]` → 42
- [ ] Add golden fixtures
- [ ] `cargo test --workspace` passes

## Milestone 17 — Math + bitwise

- [ ] Implement `//` (modulo) for integers and floats
- [ ] Implement `abs`, `negate`, `add`/`subtract`/`multiply`/`divide` as word
      aliases for `+ - * /`
- [ ] Implement `min`/`max` (any-orderable values: int/float/char/string)
- [ ] Implement `round` (and `/to`/`/even` refinements)
- [ ] Implement `random` (and `/seed`/`/only`/`/secure`)
- [ ] Implement `power` (`**`); defer trig (`sin`/`cos`/`tan`/`sqrt`/`log`) to
      a later plan
- [ ] Implement `even?`/`odd?`
- [ ] Implement bitwise on integers: `and`/`or`/`xor`/`complement`/
      `shift-left`/`shift-right` (separate from logic `and`/`or` via type dispatch)
- [ ] Resolve `and`/`or` ambiguity: logic operands → logic op; integer operands → bitwise
- [ ] Implement `to-integer` from `#"a"` char (deferred until char! exists; stub for now)
- [ ] Inline `#[test]`: `7 // 3` → 1, `7.0 // 3.0` → 1.0
- [ ] Inline `#[test]`: `min 3 5` → 3
- [ ] Inline `#[test]`: `round 3.6` → 4, `round/to 3.14159 0.01` → 3.14
- [ ] Inline `#[test]`: `random 100` returns int in `0..100`
- [ ] Inline `#[test]`: `5 and 3` → 1 (bitwise); `true and false` → false (logic)
- [ ] Add golden fixtures
- [ ] `cargo test --workspace` passes

## Milestone 18 — Objects & contexts

- [ ] Extend `Value` with `Object(Rc<RefCell<ObjectDef>>)` variant
- [ ] Define `ObjectDef { words: Vec<Symbol>, values: Vec<RefCell<Value>>,
      parent: Option<Rc<RefCell<ObjectDef>>>, self_word: Symbol }`
- [ ] Implement `make object! [spec-block]`:
      - New context, bind spec words to it, eval body with `self` bound
- [ ] Implement `object` keyword (alias for `make object!`)
- [ ] Implement `context` keyword (alias)
- [ ] Implement prototype inheritance: child object inherits parent's words,
      writes create new slots in child
- [ ] Implement `in object 'word` (deferred from Milestone 9) returning a
      bound-word value pointing into the object's slot
- [ ] Implement `set [words] [values]` and `get [words]` returning block
- [ ] Implement `words-of`/`values-of` for objects and contexts
- [ ] Implement `reflect object 'words` / `'values`
- [ ] Implement `bind` of a block to an object (existing native extended)
- [ ] Implement object mold: `make object! [a: 1 b: 2]`
- [ ] Support `self` reference inside object spec body
- [ ] Implement `object?` predicate
- [ ] Implement `same?`/`not-same?` (reference identity) for objects
- [ ] Inline `#[test]`: `o: make object! [a: 5] o/a` → 5 (via `select` or path
      stub until M19)
- [ ] Inline `#[test]`: object method calling self:
      `o: make object! [n: 0 inc: does [n: n + 1]] o/inc o/n` → 1
- [ ] Inline `#[test]`: inheritance: child `make object! parent [...]` sees parent words
- [ ] Inline `#[test]`: `in o 'a` returns a usable bound word
- [ ] Inline `#[test]`: `words-of o` → `[a n inc]`
- [ ] Add golden fixtures for object-oriented programs
- [ ] `cargo test --workspace` passes

## Milestone 19 — Real paths

- [x] Replace `Value::Path(Vec<Value>)` with
      `Value::Path { parts: Vec<Value>, span: Span }`
- [ ] Lex/parse paths: `foo/bar/baz`, `:foo/bar` (get-path), `'foo/bar` (lit-path)
- [ ] Add `GetPath` and `LitPath` value variants
- [ ] Mold paths back including nested `foo/(a+b)/bar` parens
- [ ] Implement path evaluation:
      - `object/word` → object slot lookup (depends on M18)
      - `block/integer` → `pick` by 1-based index
      - `string/integer` → char pick (deferred until char!; stub error)
      - `map/word` → map lookup (deferred until map!)
      - `context/word` → context slot
      - `func/refinement` → bound refinement reference (deferred)
- [ ] Implement `set-path` evaluation: `obj/field: value` writes into object slot
- [ ] Implement path-of-path chaining: `obj/a/b`
- [ ] Implement `path?`/`get-path?`/`lit-path?` predicates
- [ ] Implement `to-path`/`to-get-path`/`to-lit-path`
- [ ] Implement `in object 'word` returning a path value as alternative form
- [ ] Inline `#[test]`: `o: make object! [a: 1] o/a` → 1 (ties M18 + M19)
- [ ] Inline `#[test]`: `b: [10 20 30] b/2` → 20
- [ ] Inline `#[test]`: `o/a: 5 o/a` → 5
- [ ] Inline `#[test]`: nested path `obj/inner/x` resolves through object graph
- [ ] Update golden fixtures: replace `select`-on-block idioms with paths where natural
- [ ] `cargo test --workspace` passes

## Milestone 20 — File & shell I/O

- [ ] Add `Value::File(Rc<str>, span)` variant (lexer: `%foo/bar`)
- [ ] Add `Value::Url(Rc<str>, span)` variant (lexer: `http://...` etc., scheme detection)
- [ ] Mold `File` as `%"...escaped..."`, `Url` as the raw string
- [ ] Implement `file?`/`url?` predicates
- [ ] Implement `to-file`/`to-url`
- [ ] Implement `read` (file/url → string) with `/lines` refinement
- [ ] Implement `read/binary` returning String8 (deferred until String8 supported; stub)
- [ ] Implement `write` (file + string) and `write/lines`/`append`/`binary` refinements
- [ ] Implement `load` from file path (currently only source string)
- [ ] Implement `save` (mold value to file)
- [ ] Implement `open`/`close`/`read`/`write` over ports (skip full port model;
      expose `read`/`write` directly)
- [ ] Implement `exists?`/`size?`/`modified?` for files
- [ ] Implement `dir?`/`make-dir`/`delete`/`rename`/`change-dir`/`what-dir`
- [ ] Implement `call`/`shell` for running external commands (gated behind a
      `--allow-shell` CLI flag for safety in test fixtures)
- [ ] Implement `now` (current date/time as a deferred `date!`-like struct or
      tuple of integers pending `date!` type)
- [ ] Implement `wait` (sleep seconds)
- [ ] Implement `env`/`get-env`/`set-env`
- [ ] Expose `system/options/args` for script access to CLI args beyond the script path
- [ ] Update CLI to accept multiple file args and trailing args; pass via `system/options`
- [ ] Inline `#[test]`: `read %fixtures/hello.txt` returns expected contents
      (use `tempfile` dev-dep for write tests)
- [ ] Inline `#[test]`: `write` then `read` round-trips
- [ ] Inline `#[test]`: `exists? %nonexistent` → false
- [ ] Inline `#[test]`: `now` returns a value with year/month/day fields accessible
- [ ] Add golden fixtures: file-copying program, line-count program
- [ ] Document sandbox policy in `project-brief.md`: no shell by default in tests
- [ ] `cargo test --workspace` passes

## Milestone 21 — Polish & v0.2.0 release

- [ ] Audit `EvalError` rendering for new error sources (refinement arity,
      path resolution, object slot missing, file not found)
- [ ] Add spans to all new value variants (`File`, `Url`, `Path`, `Object`)
- [ ] Extend `LineMap` to cover `read`-time file errors (separate source buffer)
- [ ] Golden fixture per new error case
- [ ] Property test: extend `mold(parse(mold(v)))` to cover `Path`, `GetPath`,
      `LitPath`, `File`, `Url`; skip `Object` (not source-origin)
- [ ] Update `red-core/tests/golden/` to cover all new literals
- [ ] Expand `red-eval/tests/programs/` to 30-40 fixtures
- [ ] Run clippy + `cargo fmt --all --check`; fix
- [ ] Update `project-brief.md` and `architecture.md`:
      - Add `Object`/`File`/`Url`/`Path`/`GetPath`/`LitPath` to value model
      - Document refinement dispatch in architecture
      - Document path resolution rules
      - Note `char!`/`map!`/`date!` still deferred to v0.3
- [ ] Add `README.md` with quickstart, supported features, known gaps
- [ ] Final `cargo test --workspace` green
- [ ] Tag release `v0.2.0`
