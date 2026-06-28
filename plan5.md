# Plan 5: Language Completeness (v0.4)

Execution checklist extending the v0.3.0/v0.3.3 baseline in `plan3.md` /
`plan4.md`. v0.4 closes the **"Known gaps"** list in `README.md` by landing the
deferred value types and the missing pieces of the `parse` dialect. The
language surface — frozen at v0.2 for the v0.3 performance release —
re-opens for v0.4: new value variants, new natives, new literals, new
predicates, and a real error model.

Per `project-brief.md`, GUI/draw/VID/reactive dialects remain **permanently
out of scope**.

Deferred to v0.5+ (acknowledged, not built here): modules / `import` /
`export`, closures (`closure!`), full port model, `any*?` family beyond what
ships here, `tag!`, `ref!`, `image!`, `vector!`, `hash!`, `regex!`,
`logic!`/`bitset!` advanced ops, `object!` `on-change` reactive slots,
`routine!` FFI. v0.4 is a **language-completeness release**: it lands the
value types the README promised; modules and closures remain the two
conspicuous v0.5 candidates.

Non-goal: a register VM, JIT, or further perf work. The v0.3.3 VM stays the
default evaluator; new types and natives compile through it via the existing
`Const`-pool + `Call(native_idx, argc)` path. New `Instr` variants are added
only when a new construct cannot be expressed as a native call (e.g. `MakeMap`
may warrant a dedicated instr if profiling shows the `Const(Value::Map(...))`
path is hot — deferred). The golden parity harness (`tests/parity.rs`) and
`cargo test --workspace --features force-walk` remain the regression gates.

## Design summary

Three themes, in priority order:

1. **Type completeness** — land the value variants the README lists as gaps:
   `char!`, real `binary!` (promoting the dead `String8` stub), `map!`,
   `pair!`, `tuple!`, `date!`/`time!`, and a first-class `error!` with the
   full Red field set. Each follows the `Value::File` end-to-end template
   (enum variant → lexer → parser → mold/form → walker arm → VM const-pool →
   predicates → converters → property test → golden fixtures).
2. **`parse` dialect completion** — `collect`/`keep`/`match`/`into`/`fail`/
   `break`/`if`/`not`/`??`/`accept`/`reject`/`ahead`/`behind` + `/case`
   refinement + `bitset!`-backed charset matching. Closes the most-limited
   core feature.
3. **Native-surface fill-in** — `compose`, trig/transcendentals, missing
   type predicates (`integer?`/`float?`/`string?`/`number?`/`none?`/`logic?`/
   `word?`/`error?`/etc.), and `read/binary`/`write/binary` de-stubbed.

Non-goal: behavior changes to existing v0.2/v0.3 features. Every new
construct is additive. The v0.2 parity contract (`tests/parity.rs:14`) holds:
existing golden fixtures must produce byte-identical output under both `Vm`
and `force-walk` modes after every milestone.

---

## Milestone 38 — `char!` type

Smallest lift; unblocks ~6 existing stub-error sites. Adds `Value::Char`
variant, `#"a"` / `#"^-"` / `#"^(NN)"` lexer rule, `char?` predicate, and
real string char pick/poke replacing the integer-cast stubs.

- [x] Add `Value::Char { c: char, span: Span }` variant in
      `crates/red-core/src/value.rs` (struct-variant, source-origin)
- [x] Add `Value::char(c)` constructor shorthand + `span()` arm returning
      `Some(span)`
- [x] Extend `Lexer` (`crates/red-core/src/lexer.rs`):
  - [x] New `TokenKind::Char(char)` variant
  - [x] `scan_char` after `%`-file/`#`-binary dispatch: `#"..."` form
  - [x] Support `#"a"` (single char), `#"^-"` (caret escape: `^-` tab,
        `^/` newline, `^@` null, `^M-C` meta), `#"^(41)"` (codepoint hex)
  - [x] Error `InvalidChar` on unterminated `#"` or bad escape
- [x] Extend `Parser` (`crates/red-core/src/parser.rs`):
      `TokenKind::Char(c) => Value::Char { c, span }`
- [x] Extend `printer.rs` `mold`/`form`:
  - [x] `mold` emits `#"a"` form with escapes (`"`, `\`, `^`, control chars)
  - [x] `form` emits the raw character
- [x] Extend `interp_walker.rs` `eval_prefix` self-evaluating arm with
      `Value::Char { .. }`
- [x] Extend `vm/compiler.rs` const-pool arm with `Value::Char { .. }`
- [x] Add `char?` predicate in `crates/red-eval/src/natives/words.rs`
- [x] Add `type_name` arm returning `"char!"` (`natives/mod.rs:90`)
- [x] Add `to-char` converter in `crates/red-eval/src/convert.rs` (from
      integer codepoint, single-char string, logic)
- [x] Add `make char! <value>` to the `make` dispatcher (integer → truncate
      to u32 codepoint, string → first char)
- [x] Replace stub-error sites with real `char!` returns:
  - [x] `interp_walker.rs:629-630` (string char pick) — return `Value::Char`
        instead of `Integer` codepoint
  - [x] `interp_walker.rs:653-657` (POC char pick comment)
  - [x] `interp_walker.rs:769` (string char poke) — accept `Value::Char`
        (implementation in `set_path_value` via `poke_string_char`; the
        `write_path_slot` arm still errors because immutable `Rc<str>` is
        rebuilt at the `set_path_value` layer, not the slot writer)
  - [x] `interp_walker.rs:835` (string char poke) — accept `Value::Char`
        (stub message updated to reflect the new path)
  - [x] `interp_runner.rs:584` (same note)
  - [x] `convert.rs:100` (char → integer conversion stub)
- [x] Update `+`/`-` on `char!` (Red: char + int → char, char - char → int,
      char + char → int) in `math.rs`
- [x] Update comparison ops (`=`/`<>`/`<`/etc.) to order `char!` by codepoint
- [x] Update `min`/`max` to accept `char!`
- [x] Extend `Value::span()` test in `value.rs` to cover `Char`
- [x] Inline `#[test]`: `#"a"` lexes to `TokenKind::Char('a')`
- [x] Inline `#[test]`: `#"^-"` → tab, `#"^(41)"` → `'A'`
- [x] Inline `#[test]`: `mold(CHAR_A) == "#\"a\""`
- [x] Inline `#[test]`: `"hello"/1` → `Value::Char('e')` (was `Integer(101)`)
- [ ] Inline `#[test]`: poke a char into a string round-trips
      **(blocked: `s/2: #"X"` set-path with integer index is unreachable
      from source — lexer can't tokenize `2:`. See M38 follow-up task
      "Lexer: integer SetPath" below.)**
- [x] Inline `#[test]`: `char? #"a"` → true; `char? 5` → false
- [x] Inline `#[test]`: `#"a" + 1` → `#"b"`; `#"b" - #"a"` → `1`
- [x] Add golden fixtures: `char_literal` (lex round-trip), `char_pick`,
      `char_arith` (char_poke deferred — same lexer gap)
- [x] Add `programs_errors/char_bad_escape.red` for unterminated `#"`
- [x] Update `red-core/tests/property.rs` to include `Char` in the
      round-trip proptest (mold → parse → mold)
- [x] `cargo test --workspace` green; `--features force-walk` green;
      `cargo clippy --workspace --all-targets -- -D warnings` clean

### M38 follow-up tasks (deferred)

- [ ] **Lexer: integer SetPath** — extend `scan_refinement` (or
      `scan_number`) so `2:` tokenizes as `Integer(2)` + `SetWord("2")`
      (or a new `TokenKind::SetInteger`). Unblocks `b/2: 99` (block-integer
      set-path) and `s/2: #"X"` (string char poke via set-path). Currently
      `2:` lexes as a single `Integer` token and the trailing `:` is a
      separate word-classification error. The `poke_string_char` helper
      and `set_path_value` intercept are already implemented in
      `interp_walker.rs` and will activate once the lexer emits the right
      tokens. (Architecture notes the block-integer SetPath gap; the char
      poke case is a new wrinkle surfaced by M38.)
- [ ] **`append`/`insert` accept `string!` and `char!`** — currently
      `append "foo" "bar"` errors with `expected series!, found string!`
      because `append`'s `extract_series` only accepts `Block`/`Paren`.
      Extend `append` (and `insert`/`change`) to accept a `string!` series:
      append a `string!` (concatenate) or a `char!` (push codepoint). Mirror
      Red's behavior: `append "foo" "bar"` → `"foobar"`;
      `append "foo" #"s"` → `"foos"`. May need a positioned-string series
      type or a value-rebuild (strings are immutable `Rc<str>`).

## Milestone 39 — `compose` + missing type predicates

Quick wins. `compose` parallels `rejoin` (`strings.rs:74`) and reuses the
`dispatch_block_reduce` infrastructure. The missing predicates are trivial —
each is a one-line match arm in `natives/words.rs`.

- [ ] Implement `compose` native in `crates/red-eval/src/strings.rs`:
  - [ ] Walks a block, evaluates only `(...)` paren expressions, leaves
        literals verbatim, returns a new block
  - [ ] `compose/deep` refinement recurses into nested blocks
  - [ ] `compose/only` refinement wraps non-paren results as a single value
        (not spread)
  - [ ] Register in `strings.rs` registration block (~line 386)
- [ ] Add type predicates to `natives/words.rs` (alongside `function?`/
      `value?`):
  - [ ] `integer?`, `float?`, `number?` (int or float), `string?`, `logic?`,
        `none?`, `char?` (lands in M38 but register here if not already),
        `binary?` (lands in M41 — register here as a forward-declared
        always-false stub until M41)
  - [ ] `word?`, `set-word?`, `get-word?`, `lit-word?`, `refinement?`,
        `path?` (already exists — confirm), `any-word?`, `any-path?`
  - [ ] `error?` (forward-stub until M42 — checks the `Value::Error`
        variant), `object?` (already exists — confirm), `any-object?`
  - [ ] `type?` of value (returns type word — Red's `type?` native, distinct
        from the `?` predicates)
- [ ] Implement `types-of` returning a block of type words a value matches
      (e.g. `types-of 5` → `[integer! number!]`)
- [ ] Inline `#[test]`: `compose [a (1 + 2) b]` → `[a 3 b]`
- [ ] Inline `#[test]`: `compose/deep [a [(1 + 2)] b]` → `[a [3] b]`
- [ ] Inline `#[test]`: `compose [() (1) ()]` → `[none 1 none]`
- [ ] Inline `#[test]`: `integer? 5` → true; `integer? 5.0` → false
- [ ] Inline `#[test]`: `number? 5`, `number? 5.0` → both true
- [ ] Inline `#[test]`: `type? #"a"` → `char!`
- [ ] Inline `#[test]`: `any-word? 'foo` → true; `any-word? 5` → false
- [ ] Add golden fixtures: `compose_basic`, `compose_deep`, `type_predicates`
- [ ] `cargo test --workspace` green; `--features force-walk` green

## Milestone 40 — Trig & transcendental math

Greenfield module. No type dep — operates on `Integer` (promotes to `Float`)
and `Float`. Adds `pi` as a context-stored constant (alongside `true`/
`false`/`none`/`newline`).

- [ ] Create `crates/red-eval/src/transcendentals.rs`
- [ ] Implement `sin`, `cos`, `tan` (radians)
- [ ] Implement `asin`, `acos`, `atan` (radians)
- [ ] Implement `atan2` (2-arg: y, x)
- [ ] Implement `sqrt`, `exp`, `log-e`/`ln`, `log-10`, `log-2`
- [ ] Implement `degrees`/`radians` conversion natives
- [ ] Install `pi` and `e` constants in `install_constants`
      (`natives/registry.rs` alongside `true`/`false`)
- [ ] Register all in `register_math_natives` (or a sibling
      `register_transcendental_natives` called from `register_natives`)
- [ ] Error on `sqrt` of negative, `log` of non-positive (return
      `EvalError::Native` with span)
- [ ] Promote `Integer` arg to `Float` for all trig ops (result always
      `Float`)
- [ ] Inline `#[test]`: `sin 0` → `0.0`; `cos 0` → `1.0`
- [ ] Inline `#[test]`: `sin pi / 2` → `1.0` (within float tolerance)
- [ ] Inline `#[test]`: `sqrt 16` → `4.0`; `sqrt -1` errors
- [ ] Inline `#[test]`: `log-e e` → `1.0`; `log-10 1000` → `3.0`
- [ ] Inline `#[test]`: `atan2 1 1` → `pi / 4` (within tolerance)
- [ ] Inline `#[test]`: `degrees pi` → `180.0`; `radians 180` → `pi`
- [ ] Add golden fixtures: `trig_basic`, `trig_log`, `trig_constants`
- [ ] `cargo test --workspace` green

## Milestone 41 — Real `binary!` (`String8` promotion)

The `String8(Vec<u8>)` variant exists (`value.rs:270`) but is unreachable.
This milestone wires it up end-to-end and de-stubs `read/binary`/`write/
binary`.

- [ ] Add `Value::binary(bytes: Vec<u8>)` constructor shorthand
- [ ] Add `Value::string8` as alias (keep old name for back-compat with
      any test code at `value.rs:893`)
- [ ] Extend lexer to parse `#{hex}` literals into `Value::String8`:
  - [ ] `scan_binary` after `#`-char dispatch: `#{...}` form
  - [ ] Accept even/odd hex digit count (odd → high nibble zero-padded)
  - [ ] Allow whitespace inside `{}` (Red behavior) — skipped
  - [ ] Error `InvalidBinary` on non-hex chars or unterminated `#}`
- [ ] Extend parser: `TokenKind::Binary(Vec<u8>)` → `Value::String8`
- [ ] Confirm `mold` arm (`printer.rs:18-26`) emits `#{HEX}` uppercase,
      no separators — matches Red
- [ ] Confirm `form` arm (`printer.rs:116-123`) emits same `#{HEX}` form
- [ ] Add `binary?` predicate (was forward-stubbed in M39 — replace stub)
- [ ] Add `to-binary` converter (`convert.rs`):
  - [ ] From string → UTF-8 bytes
  - [ ] From integer → big-endian 8 bytes
  - [ ] From block of integers → byte vec (each int mod 256)
- [ ] Add `make binary! <value>` to the `make` dispatcher
- [ ] Add `to-string` from `binary!` (UTF-8 decode; error on invalid UTF-8)
- [ ] Implement `length?` on `binary!` (byte count)
- [ ] Implement `pick`/`poke`/`copy`/`find`/`append`/`insert` on `binary!`
      (byte-indexed; returns `Integer` 0-255)
- [ ] De-stub `read/binary` (`io.rs:86-91`): read file bytes as `binary!`
- [ ] De-stub `write/binary` (`io.rs:159-163`): write `binary!` to file
- [ ] Update `type_name` (`natives/mod.rs:75`) — already returns
      `"binary!"`, confirm
- [ ] Extend `vm/compiler.rs:630` const-pool arm (already includes
      `String8` — confirm)
- [ ] Inline `#[test]`: `#{48656C6C6F}` molds back to `#{48656C6C6F}`
- [ ] Inline `#[test]`: `to-binary "hi"` → `#{6869}`
- [ ] Inline `#[test]`: `read/binary %fixtures/binary.dat` round-trips with
      `write/binary` (use `tempfile` dev-dep already present)
- [ ] Inline `#[test]`: `length? #{0102}` → `2`
- [ ] Inline `#[test]`: `pick #{41 42} 2` → `66` (`'B'` as integer)
- [ ] Add golden fixtures: `binary_literal`, `binary_io`, `binary_convert`
- [ ] Add `programs_errors/binary_bad_hex.red` for non-hex in `#{...}`
- [ ] Update `property.rs` to include `String8` round-trip (already noted
      as not reparseable — adjust proptest to skip the mold-reparse step
      for `binary!` and instead assert byte-equality)
- [ ] `cargo test --workspace` green; `--features force-walk` green

## Milestone 42 — First-class `error!` values

Extend `ErrorValue` (`value.rs:285`) from message-only to the full Red
field set. Rewrites `try`/`attempt`/`catch`/`throw`/`cause-error` and
updates mold/form/equality/same.

- [ ] Extend `ErrorValue` struct in `crates/red-core/src/value.rs:285-287`:
  - [ ] `code: Option<i64>` (numeric error code; `None` for user-thrown)
  - [ ] `type: Option<Symbol>` (category word: `'math`/`'syntax`/`'script`/
        `'user`/`'access`/`'reference`/`'io`)
  - [ ] `message: String` (kept; derived from template if `code` present)
  - [ ] `args: Vec<Value>` (values referenced by the message template)
  - [ ] `near: Option<Value>` (block/expression nearest the error —
        typically the call site block)
  - [ ] `where: Option<Symbol>` (function/frame name where raised)
  - [ ] `by: Option<Symbol>` (actor — calling function name)
- [ ] Keep `Value::Error(Rc<ErrorValue>)` variant (immutable shared payload)
- [ ] Add `Value::error(msg)` convenience constructor that fills the new
      fields with `None`/empty defaults (back-compat with existing
      `try`/`attempt` return values)
- [ ] Add `Value::error_structed(code, type, msg, args, near, where, by)`
      constructor for structured construction
- [ ] Update `printer.rs:67-73` `mold` arm:
  - [ ] For message-only errors: keep `make error! "msg"` form
  - [ ] For structured errors: `make error! [code: 42 type: 'math args: [x y] message: "..."]`
- [ ] Update `printer.rs:141` `form` arm: still emits `message` (Red
      behavior — `form` of an error is just the message text)
- [ ] Update `natives/compare.rs:26` equality: compare all fields, not just
      `message`
- [ ] Update `object.rs:187` `same?`: keep `Rc::ptr_eq` (identity)
- [ ] Rewrite `cause-error` (`natives/control.rs:528-543`):
  - [ ] Accept `type word message string args block` keyword form, or
        `code integer` form, or `type word` short form
  - [ ] Build a structured `ErrorValue` and raise `EvalError::Native` with
        the value attached (extend `EvalError::Native` to carry an optional
        `Value::Error` payload)
- [ ] Add `make error!` to the `make` dispatcher (from block of
      keyword/value pairs, from string → message-only)
- [ ] Add `to-error` converter
- [ ] Rewrite `try` (`natives/control.rs:460-477`): on caught
      `EvalError::Native` carrying an `Error` payload, return that value;
      otherwise synthesize an `ErrorValue` with `type: 'script`,
      `where: <native name>`, `message: <rendered>`
- [ ] Rewrite `attempt`: same as `try` but returns `none` instead of an
      error value
- [ ] Extend `catch` (`natives/control.rs:502-513`) to also catch
      `Value::Error` propagated errors (currently catches only `Throw`)
- [ ] Add `error?` predicate (was forward-stubbed in M39 — replace stub)
- [ ] Add `error-type`/`error-code`/`error-args`/`error-near` accessors
- [ ] Add `attempted?` predicate (true if value is an `error!`)
- [ ] Wire structured error capture in the VM: when `Instr::Call` raises
      `EvalError::Native`, attach the call's span to `near` and the native
      name to `where`
- [ ] Wire structured error capture in the walker: same for
      `eval_expression`'s native-call path
- [ ] Inline `#[test]`: `make error! "boom"` molds back to
        `make error! "boom"`
- [ ] Inline `#[test]`: `make error! [code: 42 type: 'math message: "x"]`
        molds with all fields
- [ ] Inline `#[test]`: `try [1 / 0]` returns an error with `type: 'math`
- [ ] Inline `#[test]`: `try [1 + "a"]` returns an error with `type: 'script`
- [ ] Inline `#[test]`: `cause-error 'user "boom"` returns an error with
        `type: 'user`
- [ ] Inline `#[test]`: `error? try [1 / 0]` → true
- [ ] Inline `#[test]`: `error-code (try [1 / 0])` → numeric
- [ ] Inline `#[test]`: structured equality — two errors with same fields
        are `equal?`
- [ ] Add golden fixtures: `error_construct`, `error_catch`, `error_fields`,
        `error_try_type`
- [ ] Add `programs_errors/cause_error_bad_type.red`
- [ ] Audit `EvalError` rendering (`error.rs`): structured errors render
        `file:line:col: <type> error: <message>` instead of the current
        generic `*** Error: <message>`
- [ ] Update existing error golden fixtures in `programs_errors/` — expect
        output format change; update `.expected` files to match the new
        rendering
- [ ] `cargo test --workspace` green; `--features force-walk` green

## Milestone 43 — `map!` type

New `MapDef` struct (don't reuse `ObjectDef` — needs heterogeneous keys).
Adds `indexmap` dep for insertion-order preservation.

- [ ] Add `indexmap = "2"` to `crates/red-eval/Cargo.toml [dependencies]`
      (red-eval only; red-core stays zero-dep by keeping `MapDef` in red-eval
      — or add `indexmap` to red-core if the `Value` variant must live there;
      **decision: `Value::Map` lives in red-core (the enum is there), so
      `indexmap` joins red-core's deps** — first non-std dep for red-core)
- [ ] Add `indexmap = "2"` to `crates/red-core/Cargo.toml [dependencies]`
- [ ] Define `MapDef` in `crates/red-core/src/value.rs`:
  - [ ] `pub struct MapDef { entries: RefCell<IndexMap<MapKey, Value>> }`
  - [ ] `MapKey` enum: `Sym(Symbol)`/`Int(i64)`/`Str(Rc<str>)`/`Char(char)`/
        `Bool(bool)`/`None` — the set of hashable, non-container Red values
  - [ ] `MapKey::from_value(&Value) -> Option<MapKey>` (returns `None` for
        unhashable types like `Block`/`Object`/`Func`)
  - [ ] `MapKey::to_value() -> Value`
  - [ ] `MapDef::new()`, `get(&MapKey)`, `set(MapKey, Value)`,
        `remove(&MapKey)`, `len()`, `keys()`, `values()`
- [ ] Add `Value::Map(Rc<RefCell<MapDef>>)` variant (struct-tuple, synthetic
      — no span)
- [ ] Add `Value::map()` constructor shorthand
- [ ] Implement `Hash` for the `MapKey` enum (derive; all variants are
      hashable)
- [ ] Update `printer.rs`:
  - [ ] `mold` arm: `make map! [key1 val1 key2 val2 ...]` form, one entry
        per line for multi-entry maps (matches Red)
  - [ ] `form` arm: same as mold
- [ ] Extend `interp_walker.rs` `eval_prefix` self-evaluating arm with
      `Value::Map(_)`
- [ ] Extend `vm/compiler.rs:630` const-pool arm with `Value::Map(_)`
- [ ] Add `map?` predicate
- [ ] Add `to-map` converter (from block of key/value pairs, from object →
        map of word/value)
- [ ] Add `make map! <spec>` to the `make` dispatcher:
  - [ ] From block: `make map! [a: 1 b: 2]` → key=`'a` (word), val=`1`
  - [ ] From object: extract word/value pairs
  - [ ] From block of pairs: `make map! [[a 1] [b 2]]`
- [ ] Implement path resolution for `map!`:
  - [ ] `map/word` → lookup by `MapKey::Sym` (also try as string if word
        not found)
  - [ ] `map/integer` → lookup by `MapKey::Int`
  - [ ] `map/string` → lookup by `MapKey::Str`
  - [ ] `map/char` → lookup by `MapKey::Char`
  - [ ] Set-path `map/word: value` → `MapDef::set`
- [ ] Update `interp_walker.rs` path resolver (`eval_get_path`/
      `set_path_value`) and `vm/vm.rs` `GetPath`/`SetPath` arms
- [ ] Implement `select` on `map!` (return value or `none`)
- [ ] Implement `find` on `map!` (return key or `none`)
- [ ] Implement `put`/`extend`/`copy` on `map!`
- [ ] Implement `keys-of`/`values-of` on `map!` (already exist for
        objects — extend)
- [ ] Implement `length?`/`empty?`/`clear` on `map!`
- [ ] Update `same?`/`not-same?` for `map!` (Rc identity)
- [ ] Update equality (`compare.rs`): deep equality on entries
- [ ] Update `type_name` to return `"map!"`
- [ ] Inline `#[test]`: `make map! [a: 1 b: 2]` molds back identically
- [ ] Inline `#[test]`: `m: make map! [a: 1] m/a` → `1`
- [ ] Inline `#[test]`: `m/b: 2 m/b` → `2`
- [ ] Inline `#[test]`: heterogeneous keys `m: make map! [a 1 2 "two" #"c" 3]`
        round-trips
- [ ] Inline `#[test]`: `map? make map! []` → true; `map? []` → false
- [ ] Inline `#[test]`: `length? make map! [a 1 b 2]` → `2`
- [ ] Inline `#[test]`: insertion order preserved: `keys-of m` → `[a 2 #"c"]`
- [ ] Add golden fixtures: `map_construct`, `map_paths`, `map_hetero_keys`,
        `map_convert`
- [ ] Add `programs_errors/map_unhashable_key.red` (e.g. using a block as
        a key)
- [ ] Update `property.rs` to include `Map` round-trip
- [ ] `cargo test --workspace` green; `--features force-walk` green

## Milestone 44 — `pair!` and `tuple!`

Geometric types. `pair!` = 2D point (`x`/`y` integers or floats); `tuple!` =
RGB color (`r`/`g`/`b` bytes, optionally `a` alpha). Both are value types
(immutable, copy-semantics).

- [ ] Add `Value::Pair { x: Rc<Value>, y: Rc<Value>, span: Span }` variant
      (x/y are `Value` so a pair can hold int/int, int/float, float/float)
- [ ] Add `Value::Tuple { bytes: [u8; 3], span: Span }` variant (RGB; alpha
      via a separate `Value::TupleA { bytes: [u8; 4], span }` or a length
      flag — **decision: single `Tuple { bytes: Rc<[u8]>, span }` variant
      supporting 3 or 4 bytes** to avoid variant sprawl)
- [ ] Add `Value::pair(x, y)` / `Value::tuple(bytes)` constructors
- [ ] Extend lexer:
  - [ ] `scan_pair`: `NxM` where N/M are integers or floats (e.g. `100x200`,
        `1.5x2.5`)
  - [ ] `scan_tuple`: `R.G.B` where R/G/B are 0-255 integers (e.g. `255.0.0`,
        `128.64.32.128` for RGBA)
  - [ ] Disambiguate from float (`1.5`) by counting dots — 1 dot = float,
        2 dots = tuple, `x` separator = pair
  - [ ] Error `InvalidPair`/`InvalidTuple` on malformed forms
- [ ] Extend parser with `TokenKind::Pair`/`TokenKind::Tuple` → `Value`
- [ ] Update `printer.rs`:
  - [ ] `mold` pair: `100x200` (no spaces around `x`)
  - [ ] `mold` tuple: `255.0.0` (dots, no spaces)
  - [ ] `form` same as mold
- [ ] Add `pair?`/`tuple?` predicates
- [ ] Add `to-pair`/`to-tuple` converters
- [ ] Add `make pair!`/`make tuple!` to the `make` dispatcher:
  - [ ] `make pair! [100 200]` → pair
  - [ ] `make tuple! [255 0 0]` → tuple
  - [ ] `make tuple! 3` → `0.0.0` (all-zero tuple of given component count)
- [ ] Implement arithmetic on `pair!`:
  - [ ] `pair + pair` → pair (componentwise)
  - [ ] `pair + int` → pair (scalar to both components)
  - [ ] `pair - pair`, `pair * pair`, `pair * int`
  - [ ] `pair / int`
- [ ] Implement arithmetic on `tuple!`:
  - [ ] `tuple + tuple` → tuple (clamped to 0-255)
  - [ ] `tuple - tuple` → tuple (clamped)
  - [ ] `tuple * float` → tuple (scaled, clamped)
- [ ] Implement `pair/x`/`pair/y` path access
- [ ] Implement `tuple/r`/`tuple/g`/`tuple/b`/`tuple/a` path access
- [ ] Implement `set-path` writes for pair/tuple components (returns a new
      value since these are immutable — or make them `Rc<RefCell<...>>`
      like objects; **decision: immutable, set-path returns a new value
      and updates the binding**)
- [ ] Implement `negate`/`abs` on `pair!`
- [ ] Update `min`/`max` on `pair!` (componentwise)
- [ ] Update comparison (`=`/`<>` only; no ordering) for both types
- [ ] Update `same?`/`not-same?` (value identity = equality for immutable
      types)
- [ ] Update `type_name` → `"pair!"` / `"tuple!"`
- [ ] Extend `interp_walker.rs` `eval_prefix` self-evaluating arm
- [ ] Extend `vm/compiler.rs` const-pool arm
- [ ] Inline `#[test]`: `100x200` lexes to `Pair`
- [ ] Inline `#[test]`: `255.0.0` lexes to `Tuple`
- [ ] Inline `#[test]`: `100x200 + 50x50` → `150x250`
- [ ] Inline `#[test]`: `255.0.0 + 0.10.0` → `255.10.0`
- [ ] Inline `#[test]`: `255.0.0/r` → `255`; `255.0.0/g` → `0`
- [ ] Inline `#[test]`: `pair? 1x2` → true; `tuple? 1.2.3` → true
- [ ] Add golden fixtures: `pair_arith`, `tuple_arith`, `pair_paths`,
        `tuple_construct`
- [ ] Add `programs_errors/pair_bad_form.red`, `tuple_out_of_range.red`
- [ ] Update `property.rs` for `Pair`/`Tuple` round-trip
- [ ] `cargo test --workspace` green; `--features force-walk` green

## Milestone 45 — `date!` / `time!` / `now`

Adds `chrono` dep to red-core. Lexer for `29-Jun-2024` and `12:30:00`.
Replaces the `modified?` epoch-seconds stub (`io.rs:313`).

- [ ] Add `chrono = { version = "0.4", default-features = false, features = ["clock"] }`
      to `crates/red-core/Cargo.toml [dependencies]`
- [ ] Define `DateValue` in `crates/red-core/src/value.rs`:
  - [ ] `pub struct DateValue { date: chrono::NaiveDate, time: Option<chrono::NaiveTime>, zone: Option<i32> }`
  - [ ] Or store as a single `chrono::NaiveDateTime` + optional offset
        — **decision: `NaiveDateTime` + `Option<FixedOffset>`**
- [ ] Add `Value::Date { dt: Rc<DateValue>, span: Span }` variant
      (single variant covers both date-only and date+time)
- [ ] Add `Value::date(dt)` constructor
- [ ] Extend lexer:
  - [ ] `scan_date`: `DD-Mon-YYYY` (e.g. `29-Jun-2024`), `DD/MM/YYYY`,
        `YYYY-MM-DD`
  - [ ] `scan_time`: `HH:MM:SS`, `HH:MM:SS.mmm`
  - [ ] Combined `DD-Mon-YYYY/HH:MM:SS` (date/time separator `/`)
  - [ ] Error `InvalidDate` on bad date (e.g. `31-Feb-2024`)
- [ ] Extend parser with `TokenKind::Date`/`TokenKind::Time` → `Value`
- [ ] Update `printer.rs`:
  - [ ] `mold` date-only: `29-Jun-2024`
  - [ ] `mold` date+time: `29-Jun-2024/12:30:00`
  - [ ] `form` same as mold
- [ ] Add `date?`/`time?` predicates
- [ ] Add `now` native: returns `Value::Date` with current local time
- [ ] Add `today` native: returns date-only (midnight)
- [ ] Implement date arithmetic:
  - [ ] `date + integer` → date + N days
  - [ ] `date - date` → integer (day difference)
  - [ ] `date + time` → date+time
- [ ] Implement date accessors: `date/year`/`month`/`day`/`time`/`weekday`/
        `yearday`/`week` paths
- [ ] Implement `to-date` (from string parse, from block `[year month day]`,
        from integer epoch)
- [ ] Add `make date!` to the `make` dispatcher
- [ ] Implement `now`/`today`/`date?`/`time?` registration
- [ ] Replace `io.rs:313` `modified?` epoch-seconds stub: return `Value::Date`
- [ ] Implement `wait` (already exists — confirm; may already use
        `std::time::Duration`, no change needed)
- [ ] Update `type_name` → `"date!"`
- [ ] Extend `interp_walker.rs` `eval_prefix` self-evaluating arm
- [ ] Extend `vm/compiler.rs` const-pool arm
- [ ] Inline `#[test]`: `29-Jun-2024` lexes to `Date`
- [ ] Inline `#[test]`: `12:30:00` lexes to `Date` (time-only, epoch date)
- [ ] Inline `#[test]`: `29-Jun-2024 + 1` → `30-Jun-2024`
- [ ] Inline `#[test]`: `30-Jun-2024 - 29-Jun-2024` → `1`
- [ ] Inline `#[test]`: `now` returns a date with year ≥ 2024
- [ ] Inline `#[test]`: `modified? %file` returns a `date!`
- [ ] Inline `#[test]`: `date/year (29-Jun-2024)` → `2024`
- [ ] Add golden fixtures: `date_literal`, `date_arith`, `now_basic`,
        `date_paths`
- [ ] Add `programs_errors/bad_date.red`
- [ ] Update `property.rs` for `Date` round-trip
- [ ] `cargo test --workspace` green; `--features force-walk` green

## Milestone 46 — `parse` dialect completion + `bitset!`

Closes the most-limited core feature. Adds the missing rule words, the
`/case` refinement, and `bitset!`-backed charset matching.

### `bitset!` type

- [ ] Define `BitsetDef` in `crates/red-core/src/value.rs`:
  - [ ] `pub struct BitsetDef { bits: RefCell<Vec<u64>>, len: usize }`
        (bit-packed; `len` = bit count)
- [ ] Add `Value::Bitset(Rc<RefCell<BitsetDef>>)` variant
- [ ] Add `Value::bitset()` constructor
- [ ] Implement `BitsetDef::new(len)`, `set(byte)`, `clear(byte)`,
        `test(byte)`, `union(&other)`, `intersect(&other)`,
        `difference(&other)`, `complement()`, `from_chars(&str)`,
        `from_range(byte, byte)`
- [ ] Extend lexer to parse `#{...}` already taken by `binary!` —
      **bitset literals use `make bitset! [...]` or `charset "ABC"` form**
      (no new lexer token)
- [ ] Add `charset` native: `charset "ABC"` → bitset of those chars
- [ ] Add `make bitset!` to the `make` dispatcher:
  - [ ] From string → bits for each char
  - [ ] From block: `make bitset! [#"a" - #"z"]` (ranges), `["abc" "XYZ"]`
        (unions)
- [ ] Update `printer.rs`:
  - [ ] `mold` bitset: `make bitset! #{010203...}` (the internal bit pattern
        as binary) or `make bitset! "ABC"` form (reconstruct a string form
        when possible) — **decision: use `make bitset! "..."` form listing
        set chars; fall back to `#{hex}` for sparse bitsets**
- [ ] Add `bitset?` predicate
- [ ] Add `to-bitset` converter
- [ ] Implement bitset ops: `union`/`intersect`/`difference`/`complement`/
        `extract?` (membership test)
- [ ] Update `type_name` → `"bitset!"`
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` for `Value::Bitset(_)`

### `parse` rule additions

- [ ] Implement `/case` refinement on `parse` itself (currently `_refs` at
      `parse.rs:278`): case-sensitive vs case-insensitive string matching
- [ ] Implement `bitset!` as a rule: matches any char in the set, advances
      cursor by 1
- [ ] Implement `collect` rule:
  - [ ] `collect 'word rule` — accumulate matched values into a block,
        bind word
  - [ ] `collect into 'word rule` — append to existing block
  - [ ] `collect [...]` — collect rules in a block
- [ ] Implement `keep` rule:
  - [ ] `keep value` — push value into the current collect target
  - [ ] `keep 'word` — push word's value
  - [ ] `keep (expr)` — evaluate Red expr, push result
- [ ] Implement `match` rule: `match value` — like literal match but
      returns the matched value (not just true/false)
- [ ] Implement `into 'word rule` — parse a sub-series, bind result
- [ ] Implement `fail` rule: always fails (opposite of `none`)
- [ ] Implement `break` rule: exit the current `parse` entirely (return
      true)
- [ ] Implement `if (expr)` rule: succeeds iff expr is truthy (no advance)
- [ ] Implement `not rule` — succeeds iff sub-rule fails (no advance)
- [ ] Implement `??` debug rule: prints current input position to stderr
- [ ] Implement `accept value` — succeed immediately, return value
- [ ] Implement `reject` — fail immediately
- [ ] Implement `ahead rule` — lookahead; succeed/fail without advancing
- [ ] Implement `behind rule` — reverse lookahead
- [ ] Update `rule_extent` (`parse.rs:648-684`) to count args for each new
      rule word
- [ ] Extend `rule_one` (`parse.rs:406-514`) with a keyword arm per new rule
- [ ] Inline `#[test]`: `parse "abc" [collect w some [skip]]` → true,
        `w == [#"a" #"b" #"c"]` (block of chars, post-M38)
- [ ] Inline `#[test]`: `parse "a1b2" [collect w some [match #"a" | match #"b" | skip]]`
        → true, `w == [#"a" #"b"]`
- [ ] Inline `#[test]`: `parse/case "Abc" ["A" "b" "c"]` → false (case
        sensitive); without `/case` → true
- [ ] Inline `#[test]`: `parse "xyz" [charset "abc" charset "xyz" "z"]` →
        true (bitset matches `x` then `y`)
- [ ] Inline `#[test]`: `parse "abc" [ahead "a" "b"]` → true, cursor didn't
        advance past `a`
- [ ] Inline `#[test]`: `parse "abc" [not "z" "a" "b" "c"]` → true
- [ ] Inline `#[test]`: `parse "abc" [fail]` → false
- [ ] Inline `#[test]`: `parse "abc" [if (1 < 2) "a" "b" "c"]` → true
- [ ] Add golden fixtures: `parse_collect`, `parse_keep`, `parse_bitset`,
        `parse_case`, `parse_lookahead`, `parse_match`
- [ ] `cargo test --workspace` green; `--features force-walk` green

## Milestone 47 — Polish & v0.4.0 release

- [ ] Audit `EvalError` rendering for all new error sources (char/binary/
        map/pair/tuple/date/bitset/parse errors, structured error fields)
- [ ] Add spans to all new value variants (already struct-style with `span`)
- [ ] Golden fixture per new error case (one per error kind added in
        M38–M46)
- [ ] Property test: extend `mold(parse(mold(v)))` to cover `Char`/`Pair`/
        `Tuple`/`Date`/`Map` (skip `Bitset` — mold form may not reparse
        cleanly; assert byte-equivalence instead)
- [ ] Extend `red-core/tests/golden/` to cover all new literals
- [ ] Expand `red-eval/tests/programs/` to 50+ new fixtures (one per new
        feature × positive + error case)
- [ ] Run `cargo bench --bench eval` and record numbers in `BENCHMARKS.md`
      under a new "v0.4.0" header — confirm no regression vs v0.3.3 (the
      new types add const-pool entries but no new hot-path instrs)
- [ ] Run clippy + `cargo fmt --all --check`; fix
- [ ] Update `project-brief.md`:
  - [ ] Add `Char`/`Pair`/`Tuple`/`Date`/`Map`/`Bitset`/real `String8` to
        the value model section
  - [ ] Document the full error model (`code`/`type`/`args`/`near`/
        `where`/`by`)
  - [ ] Document `parse` rule additions
  - [   ] Note `chrono`/`indexmap` deps
- [ ] Update `architecture.md`:
  - [ ] New value variants in the value-model section
  - [   ] `MapDef`/`BitsetDef`/`DateValue`/`MapKey` struct definitions
  - [   ] Path resolution rules for `map!`
  - [ ] Trig/transcendental native list
  - [ ] `parse` rule inventory
- [ ] Update `README.md`:
  - [   ] Bump version to v0.4.0
  - [ ] Remove closed items from "Known gaps" (char/map/pair/tuple/date/
        bitset/binary/compose/trig/parse rules/error fields)
  - [ ] Add new "Known gaps" entries for anything still deferred
        (modules, closures)
  - [   ] Update feature list
- [ ] Final `cargo test --workspace` green
- [ ] Final `cargo test --workspace --features force-walk` green
- [ ] Final `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] Tag release `v0.4.0`

---

## Open questions

1. **`red-core` dependencies (M43, M45):** v0.3 kept `red-core` zero-dep
   (only `std`). v0.4 adds `indexmap` (for `map!`) and `chrono` (for
   `date!`) to `red-core` because the `Value` enum and its payload structs
   live there. Alternatives:
   - (a) Accept the two new deps (matches the plan above).
   - (b) Move `Value`/`MapDef`/`DateValue` into `red-eval` and keep
     `red-core` pure — large refactor, breaks the existing crate split.
   - (c) Hand-roll an ordered map and a date struct without `indexmap`/
     `chrono` — more code, no date arithmetic/timezone support.
   Recommendation: (a). The zero-dep constraint was never documented as a
   design goal; `red-eval` already pulls `ureq`. Proceed with `indexmap` +
   `chrono` in `red-core`.
2. **`pair!`/`tuple!` mutability (M44):** plan says immutable (set-path
   returns a new value). Real Red treats them as immutable too, so this
   matches. Confirm before implementing — if mutability is needed for
   perf (e.g. graphics loops, even though draw is out of scope), the
   `Rc<RefCell<...>>` form is available.
3. **Error model `near` field (M42):** capturing the source block at the
   error site requires threading the current `Series`/`Value` through
   `EvalError::Native`. The plan extends `EvalError::Native` to carry an
   optional `Value::Error` payload. Alternative: a separate
   `EvalError::Structured { error: Value }` variant. The plan's approach
   keeps the variant count stable — confirm before implementing.
4. **`bitset!` mold form (M46):** `make bitset! "ABC"` form requires
   reconstructing a char list from the bitset at mold time, which is
   ambiguous (control chars, gaps). Alternative: always mold as
   `make bitset! #{hex}` (the raw bit pattern). The plan prefers the
   string form when all set bits are printable ASCII; falls back to
   `#{hex}` otherwise. Confirm before implementing.
5. **`date!`/`time!` as separate types or one (M45):** the plan folds them
   into a single `Value::Date` variant with an optional time component
   (matches Red's `date!` which can be date-only or date+time). Real Red
   has no separate `time!` type — `time?` is a predicate on `date!`
   values that have a time component. Confirm before implementing.
6. **M48 (modules) deferred to v0.5:** confirmed per user decision.
   Modules / `import` / `export` will be the headline v0.5 feature
   alongside closures (`closure!`).

(End of plan5.md)
