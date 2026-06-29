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
- [x] Inline `#[test]`: poke a char into a string round-trips
      **(unblocked: M38 follow-up "Lexer: integer SetPath" landed —
      `s/2: #"X"` now works in both VM and walker modes)**
- [x] Inline `#[test]`: `char? #"a"` → true; `char? 5` → false
- [x] Inline `#[test]`: `#"a" + 1` → `#"b"`; `#"b" - #"a"` → `1`
- [x] Add golden fixtures: `char_literal` (lex round-trip), `char_pick`,
      `char_arith`, `char_poke` (unblocked by M38 follow-up)
- [x] Add `programs_errors/char_bad_escape.red` for unterminated `#"`
- [x] Update `red-core/tests/property.rs` to include `Char` in the
      round-trip proptest (mold → parse → mold)
- [x] `cargo test --workspace` green; `--features force-walk` green;
      `cargo clippy --workspace --all-targets -- -D warnings` clean

### M38 follow-up tasks (done)

- [x] **Lexer: integer SetPath** — `scan_number` now emits `Integer(n)` +
      `SetWord("n")` (overlapping spans) when a digit run is immediately
      followed by a single `:`. The parser's existing span-overlap SetPath
      detection folds the run into a `SetPath` with an `Integer` final part.
      Also fixed a pre-existing compiler bug: `Instr::Const(path_idx)` was
      never emitted in the `SetPath` arm (only object-headed SetPaths worked
      because they route through the walker via `needs_rebind`). Unblocks
      `b/2: 99` and `s/2: #"X"`.
- [x] **`append`/`insert` accept `string!` and `char!`** — `append` on a
      `string!` builds a new string (char → push codepoint; string →
      concatenate; block → splice chars/strings). `insert` on a `string!`
      inserts at the head (no cursor in POC). Documented limitation: strings
      are immutable `Rc<str>` so the mutation is not visible to aliases —
      use `s: append s value` to update.

## Milestone 39 — `compose` + missing type predicates

Quick wins. `compose` parallels `rejoin` (`strings.rs:74`) and reuses the
`dispatch_block_reduce` infrastructure. The missing predicates are trivial —
each is a one-line match arm in `natives/words.rs`.

- [x] Implement `compose` native in `crates/red-eval/src/strings.rs`:
  - [x] Walks a block, evaluates only `(...)` paren expressions, leaves
        literals verbatim, returns a new block
  - [x] `compose/deep` refinement recurses into nested blocks
  - [x] `compose/only` refinement wraps non-paren results as a single value
        (not spread)
  - [x] Register in `strings.rs` registration block (~line 386)
- [x] Add type predicates to `natives/words.rs` (alongside `function?`/
      `value?`):
  - [x] `integer?`, `float?`, `number?` (int or float), `string?`, `logic?`,
        `none?`, `char?` (lands in M38 but register here if not already),
        `binary?` (lands in M41 — register here as a forward-declared
        always-false stub until M41)
  - [x] `word?`, `set-word?`, `get-word?`, `lit-word?`, `refinement?`,
        `path?` (already exists — confirm), `any-word?`, `any-path?`
  - [x] `error?` (forward-stub until M42 — checks the `Value::Error`
        variant), `object?` (already exists — confirm), `any-object?`
  - [x] `type?` of value (returns type word — Red's `type?` native, distinct
        from the `?` predicates)
- [x] Implement `types-of` returning a block of type words a value matches
      (e.g. `types-of 5` → `[integer! number!]`)
- [x] Inline `#[test]`: `compose [a (1 + 2) b]` → `[a 3 b]`
- [x] Inline `#[test]`: `compose/deep [a [(1 + 2)] b]` → `[a [3] b]`
- [x] Inline `#[test]`: `compose [() (1) ()]` → `[none 1 none]`
- [x] Inline `#[test]`: `integer? 5` → true; `integer? 5.0` → false
- [x] Inline `#[test]`: `number? 5`, `number? 5.0` → both true
- [x] Inline `#[test]`: `type? #"a"` → `char!`
- [x] Inline `#[test]`: `any-word? 'foo` → true; `any-word? 5` → false
- [x] Add golden fixtures: `compose_basic`, `compose_deep`, `type_predicates`
- [x] `cargo test --workspace` green; `--features force-walk` green

## Milestone 40 — Trig & transcendental math

Greenfield module. No type dep — operates on `Integer` (promotes to `Float`)
and `Float`. Adds `pi` as a context-stored constant (alongside `true`/
`false`/`none`/`newline`).

- [x] Extend `crates/red-eval/src/math.rs` with a trig + transcendentals
      section (decision: extend `math.rs` rather than create a new
      `transcendentals.rs` module — keeps the private `as_number`/
      `num_type_err`/`arity_err`/`native_err` helpers in scope without
      re-exports)
- [x] Implement `sin`, `cos`, `tan` (radians)
- [x] Implement `asin`, `acos`, `atan` (radians)
- [x] Implement `atan2` (2-arg: y, x)
- [x] Implement `sqrt`, `exp`, `log-e`/`ln`, `log-10`, `log-2`
- [x] Implement `degrees`/`radians` conversion natives
- [x] Install `pi` and `e` constants in `install_constants`
      (`natives/registry.rs` alongside `true`/`false`)
- [x] Register all in a sibling `register_transcendental_natives`
      called from `register_natives` (alongside `register_math_natives`)
- [x] Error on `sqrt` of negative, `log` of non-positive (return
      `EvalError::Native` with span)
- [x] Promote `Integer` arg to `Float` for all trig ops (result always
      `Float`) — `as_float_arg` helper centralizes the promotion
- [x] Inline `#[test]`: `sin 0` → `0.0`; `cos 0` → `1.0`
- [x] Inline `#[test]`: `sin pi / 2` → `1.0` (within float tolerance)
- [x] Inline `#[test]`: `sqrt 16` → `4.0`; `sqrt -1` errors
- [x] Inline `#[test]`: `log-e e` → `1.0`; `log-10 1000` → `3.0`
- [x] Inline `#[test]`: `atan2 1 1` → `pi / 4` (within tolerance)
- [x] Inline `#[test]`: `degrees pi` → `180.0`; `radians 180` → `pi`
- [x] Add golden fixtures: `trig_basic`, `trig_log`, `trig_constants`
- [x] `cargo test --workspace` green; `--features force-walk` green;
      `cargo clippy --workspace --all-targets -- -D warnings` clean

## Milestone 41 — Real `binary!` (`String8` promotion)

The `String8(Vec<u8>)` variant exists (`value.rs:270`) but is unreachable.
This milestone wires it up end-to-end and de-stubs `read/binary`/`write/
binary`.

- [x] Add `Value::binary(bytes: Vec<u8>)` constructor shorthand
- [x] Add `Value::string8` as alias (keep old name for back-compat with
      any test code at `value.rs:893`)
- [x] Promote `Value::String8(Vec<u8>)` → `Value::String8 { bytes, span }`
      (struct-with-span, matching the M38 `Char` template — `#{hex}` is now
      source-origin and errors localize to the literal)
- [x] Extend lexer to parse `#{hex}` literals into `TokenKind::Binary`:
  - [x] `scan_binary` after `#`-char dispatch: `#{...}` form
  - [x] Accept even/odd hex digit count (odd → high nibble zero-padded)
  - [x] Allow whitespace inside `{}` (Red behavior) — skipped (plan decision)
  - [x] Error `InvalidBinary` on non-hex chars or unterminated `#}`
- [x] Extend parser: `TokenKind::Binary(Rc<[u8]>)` → `Value::String8`
- [x] Confirm `mold` arm (`printer.rs:18-26`) emits `#{HEX}` uppercase,
      no separators — matches Red
- [x] Confirm `form` arm (`printer.rs:116-123`) emits same `#{HEX}` form
- [x] Add `binary?` predicate (was forward-stubbed in M39 — replaced stub;
      predicate was already real, just unreachable)
- [x] Add `to-binary` converter (`convert.rs`):
  - [x] From string → UTF-8 bytes
  - [x] From integer → big-endian 8 bytes
  - [x] From block of integers → byte vec (each int mod 256)
- [x] Add `make binary! <value>` to the `make` dispatcher (also accepts
      char!/string!/binary! elements in a block spec)
- [x] Add `to-string` from `binary!` (UTF-8 decode; error on invalid UTF-8)
- [x] Implement `length?` on `binary!` (byte count)
- [x] Implement `pick`/`poke`/`copy`/`find`/`append`/`insert` on `binary!`
      (byte-indexed; `pick` returns `Integer` 0-255; value semantics —
      `poke`/`append`/`insert` return a new binary, aliases don't see
      updates, mirroring the existing `String` behavior)
- [x] De-stub `read/binary` (`io.rs:86-91`): read file bytes as `binary!`
- [x] De-stub `write/binary` (`io.rs:159-163`): write `binary!` to file
      (also accepts `string!`); `/append` supported
- [x] Update `type_name` (`natives/mod.rs:75`) — already returns
      `"binary!"`, confirmed
- [x] Extend `vm/compiler.rs:630` const-pool arm (already includes
      `String8` — confirmed; pattern updated to struct form)
- [x] Add `String8` equality arm to `compare.rs:values_equal` (was
      catch-all `_ => false`, so `#{00} = #{00}` was wrongly `false`)
- [x] Inline `#[test]`: `#{48656C6C6F}` molds back to `#{48656C6C6F}`
- [x] Inline `#[test]`: `to-binary "hi"` → `#{6869}`
- [x] Inline `#[test]`: `read/binary` round-trips with `write/binary`
      (uses `tempfile` dev-dep)
- [x] Inline `#[test]`: `length? #{0102}` → `2`
- [x] Inline `#[test]`: `pick #{4142} 2` → `66` (`'B'` as integer)
- [x] Add golden fixtures: `binary_literal`, `binary_io`, `binary_convert`,
      `binary_series`
- [x] Add `programs_errors/binary_bad_hex.red` for non-hex in `#{...}`
- [x] Update `property.rs` to include `String8` in the normal
      `mold(parse(mold(v)))` round-trip proptest (now that `#{hex}` reparses,
      the byte-equality-only approach was unnecessary)
- [x] `cargo test --workspace` green; `--features force-walk` green;
      `cargo clippy --workspace --all-targets -- -D warnings` clean (both
      modes); `cargo fmt --all --check` clean

## Milestone 42 — First-class `error!` values

Extend `ErrorValue` (`value.rs:285`) from message-only to the full Red
field set. Rewrites `try`/`attempt`/`catch`/`throw`/`cause-error` and
updates mold/form/equality/same.

- [x] Extend `ErrorValue` struct in `crates/red-core/src/value.rs:285-287`:
  - [x] `code: Option<i64>` (numeric error code; `None` for user-thrown)
  - [x] `type: Option<Symbol>` (category word: `'math`/`'syntax`/`'script`/
        `'user`/`'access`/`'reference`/`'io`)
  - [x] `message: String` (kept; derived from template if `code` present)
  - [x] `args: Vec<Value>` (values referenced by the message template)
  - [x] `near: Option<Value>` (block/expression nearest the error —
        typically the call site block)
  - [x] `where: Option<Symbol>` (function/frame name where raised)
  - [x] `by: Option<Symbol>` (actor — calling function name)
- [x] Keep `Value::Error(Rc<ErrorValue>)` variant (immutable shared payload)
- [x] Add `Value::error(msg)` convenience constructor that fills the new
      fields with `None`/empty defaults (back-compat with existing
      `try`/`attempt` return values)
- [x] Add `Value::error_structed(code, type, msg, args, near, where, by)`
      constructor for structured construction
- [x] Update `printer.rs:67-73` `mold` arm:
  - [x] For message-only errors: keep `make error! "msg"` form
  - [x] For structured errors: `make error! [code: 42 type: 'math args: [x y] message: "..."]`
- [x] Update `printer.rs:141` `form` arm: still emits `message` (Red
      behavior — `form` of an error is just the message text)
- [x] Update `natives/compare.rs:26` equality: compare all fields, not just
      `message`
- [x] Update `object.rs:187` `same?`: keep `Rc::ptr_eq` (identity)
- [x] Rewrite `cause-error` (`natives/control.rs:528-543`):
  - [x] Accept `type word message string args block` keyword form, or
        `code integer` form, or `type word` short form
  - [x] Build a structured `ErrorValue` and raise `EvalError::Native` with
        the value attached (extend `EvalError::Native` to carry an optional
        `Value::Error` payload)
- [x] Add `make error!` to the `make` dispatcher (from block of
      keyword/value pairs, from string → message-only)
- [x] Add `to-error` converter
- [x] Rewrite `try` (`natives/control.rs:460-477`): on caught
      `EvalError::Native` carrying an `Error` payload, return that value;
      otherwise synthesize an `ErrorValue` with `type: 'script`,
      `where: <native name>`, `message: <rendered>`
- [x] Rewrite `attempt`: same as `try` but returns `none` instead of an
      error value
- [x] Extend `catch` (`natives/control.rs:502-513`) to also catch
      `Value::Error` propagated errors (currently catches only `Throw`)
- [x] Add `error?` predicate (was forward-stubbed in M39 — replace stub)
- [x] Add `error-type`/`error-code`/`error-args`/`error-near` accessors
- [x] Add `attempted?` predicate (true if value is an `error!`)
- [x] Wire structured error capture in the VM: when `Instr::Call` raises
      `EvalError::Native`, attach the call's span to `near` and the native
      name to `where`
- [x] Wire structured error capture in the walker: same for
      `eval_expression`'s native-call path
- [x] Inline `#[test]`: `make error! "boom"` molds back to
        `make error! "boom"`
- [x] Inline `#[test]`: `make error! [code: 42 type: 'math message: "x"]`
        molds with all fields
- [x] Inline `#[test]`: `try [1 / 0]` returns an error with `type: 'math`
- [x] Inline `#[test]`: `try [1 + "a"]` returns an error with `type: 'script`
- [x] Inline `#[test]`: `cause-error 'user "boom"` returns an error with
        `type: 'user`
- [x] Inline `#[test]`: `error? try [1 / 0]` → true
- [x] Inline `#[test]`: `error-code (try [1 / 0])` → numeric
- [x] Inline `#[test]`: structured equality — two errors with same fields
        are `equal?`
- [x] Add golden fixtures: `error_construct`, `error_catch`, `error_fields`,
        `error_try_type`
- [x] Add `programs_errors/cause_error_bad_type.red`
- [x] Audit `EvalError` rendering (`error.rs`): structured errors render
        `file:line:col: <type> error: <message>` instead of the current
        generic `*** Error: <message>`
- [x] Update existing error golden fixtures in `programs_errors/` — expect
        output format change; update `.expected` files to match the new
        rendering
- [x] `cargo test --workspace` green; `--features force-walk` green

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

## Milestone 45 — `date!` / `time!` / `now` (with timezone support)

Adds `chrono` dep to red-core. Lexer for `29-Jun-2024`, `12:30:00`, and the
timezone offset suffix `±HH:MM`. Replaces the `modified?` epoch-seconds stub
(`io.rs:313`). Timezone model matches Red parity: **fixed UTC offsets only**
(no named zones, no DST). Internal representation: `Option<i32>` minutes,
mirroring Red's `date!/zone` field exactly.

- [ ] Add `chrono = { version = "0.4", default-features = false, features = ["clock"] }`
      to `crates/red-core/Cargo.toml [dependencies]`
- [ ] Define `DateValue` in `crates/red-core/src/value.rs`:
  - [ ] `pub struct DateValue { dt: chrono::NaiveDateTime, zone: Option<i32> }`
        (`zone` = minutes east of UTC; `None` = zone-naive; matches Red's
        internal `date!/zone` representation)
  - [ ] `to_offset_utc() -> DateTime<Utc>` (apply `zone` to produce an
        absolute instant; `None` treated as UTC for arithmetic)
  - [ ] `from_local(dt, zone_minutes) -> Self` constructor
  - [ ] **decision: `Option<i32>` minutes, not `Option<FixedOffset>`** —
        matches Red's internal model and the `date/zone` accessor shape;
        `FixedOffset` is used transiently during `now`/parse/mold only
- [ ] Add `Value::Date { dt: Rc<DateValue>, span: Span }` variant (single
      variant covers date-only, date+time, and date+time+zone)
- [ ] Add `Value::date(dt)` constructor
- [ ] Extend lexer:
  - [ ] `scan_date`: `DD-Mon-YYYY` (e.g. `29-Jun-2024`), `DD/MM/YYYY`,
        `YYYY-MM-DD`
  - [ ] `scan_time`: `HH:MM:SS`, `HH:MM:SS.mmm`
  - [ ] Combined `DD-Mon-YYYY/HH:MM:SS` (date/time separator `/`)
  - [ ] **Zone offset suffix**: `+HH:MM`, `-HH:MM`, `+HHMM`, `-HHMM`,
        `+HH`, and `Z` (alias for `+00:00`); attachable to any date+time
        form (e.g. `29-Jun-2024/12:30:00+5:30`, `2024-06-29T12:30:00Z`,
        `12:30:00-04:00`)
  - [ ] Error `InvalidDate` on bad date (e.g. `31-Feb-2024`)
  - [ ] Error `InvalidZone` on out-of-range offset (|minutes| > 14*60) or
        malformed suffix
- [ ] Extend parser with `TokenKind::Date`/`TokenKind::Time` → `Value`
- [ ] Update `printer.rs`:
  - [ ] `mold` date-only: `29-Jun-2024` (no zone emitted)
  - [ ] `mold` date+time, zone-naive: `29-Jun-2024/12:30:00`
  - [ ] `mold` date+time, zone UTC: `29-Jun-2024/12:30:00+00:00`
        (always emit `+HH:MM` two-digit form, never `Z`)
  - [ ] `mold` date+time, non-UTC zone: `29-Jun-2024/12:30:00-04:00`
  - [ ] `form` same as mold
- [ ] Add `date?`/`time?` predicates (`time?` = date with a `time` component
      and `zone != None`; matches Red)
- [ ] Add `now` native: returns `Value::Date` with current **local** time
      and the system's **local UTC offset** attached (uses `chrono::Local`;
      offset may differ between calls during DST transitions — that's the
      system's behavior, not a Red-parity issue since Red only supports
      fixed offsets anyway)
- [ ] Add `today` native: returns date-only at local midnight, `zone: None`
- [ ] Implement date arithmetic:
  - [ ] `date + integer` → date + N days (zone preserved)
  - [ ] `date - date` → integer (day difference, computed on the absolute
        instant — zone-adjusted so two dates in different zones compare by
        wall-clock day, not raw instant)
  - [ ] `date + time` → date+time
  - [ ] `date + date` errors
- [ ] Implement date accessors: `date/year`/`month`/`day`/`time`/`weekday`/
        `yearday`/`week`/**`zone`** paths
  - [ ] `date/zone` returns a `time!`-shaped value (date with zeroed
        date portion, `time = HH:MM:SS`, `zone = None`) representing the
        offset duration — sign carried in the time value (negative offsets
        render as e.g. `-4:00`)
  - [ ] `date/zone` on a zone-naive date returns `none`
- [ ] Implement `date/zone:` set-path: **relabels the offset only**, does
      NOT shift the wall-clock `dt` (matches Red semantics — it's a
      re-labeling, not a conversion)
- [ ] Implement `to-utc` native: returns the same instant with `zone` set
      to `0` (and `dt` recomputed accordingly) — convenience for the
      "shift and relabel" case that set-path doesn't do
- [ ] Implement `to-date` (from string parse, from block `[year month day]`,
        from block with time `[year month day hour min sec]`, from integer
        epoch — epoch is UTC, result has `zone = Some(0)`)
- [ ] Add `make date!` to the `make` dispatcher
- [ ] Implement `now`/`today`/`date?`/`time?`/`to-utc` registration
- [ ] Replace `io.rs:313` `modified?` epoch-seconds stub: return
      `Value::Date` with the file's mtime as **local time + local UTC
      offset** (uses `chrono::DateTime::<Local>::from(mtime)`); the
      resulting date is timezone-aware
- [ ] Implement `wait` (already exists — confirm; uses `std::time::Duration`,
      no change needed)
- [ ] Update `type_name` → `"date!"`
- [ ] Extend `interp_walker.rs` `eval_prefix` self-evaluating arm
- [ ] Extend `vm/compiler.rs` const-pool arm
- [ ] Inline `#[test]`: `29-Jun-2024` lexes to `Date` (zone-naive)
- [ ] Inline `#[test]`: `29-Jun-2024/12:30:00+5:30` lexes to `Date` with
        `zone == Some(330)`
- [ ] Inline `#[test]`: `2024-06-29T12:30:00Z` lexes to `Date` with
        `zone == Some(0)`
- [ ] Inline `#[test]`: `12:30:00-04:00` lexes to `Date` (time-only, epoch
        date, `zone == Some(-240)`)
- [ ] Inline `#[test]`: `mold(now-ish local date+time+zone)` round-trips
        through parse+mold byte-identically (covers the `+HH:MM` form)
- [ ] Inline `#[test]`: `29-Jun-2024 + 1` → `30-Jun-2024` (zone preserved)
- [ ] Inline `#[test]`: `30-Jun-2024 - 29-Jun-2024` → `1`
- [ ] Inline `#[test]`: `now` returns a date with `year ≥ 2024` and
        `now/zone <> none`
- [ ] Inline `#[test]`: `modified? %file` returns a timezone-aware `date!`
        (zone field is `Some`)
- [ ] Inline `#[test]`: `date/zone (29-Jun-2024/12:30:00+5:30)` → `5:30:00`
        as a `time!`-shaped date
- [ ] Inline `#[test]`: `date/zone (29-Jun-2024)` → `none`
- [ ] Inline `#[test]`: `d: 29-Jun-2024/12:30:00+5:30  d/zone: -4:00` →
        `d` is `29-Jun-2024/12:30:00-04:00` (relabel, no shift)
- [ ] Inline `#[test]`: `to-utc 29-Jun-2024/12:30:00+5:30` →
        `29-Jun-2024/07:00:00+00:00` (shift, then relabel to UTC)
- [ ] Add golden fixtures: `date_literal`, `date_arith`, `date_zone`,
        `date_zone_setpath`, `now_basic`, `date_paths`, `to_utc`
- [ ] Add `programs_errors/bad_date.red`, `programs_errors/bad_zone.red`
- [ ] Update `property.rs` for `Date` round-trip (skip the `now`-derived
        zone in the proptest — use fixed offsets only)
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
5. **`date!`/`time!` + timezone model (M45):** the plan folds date and time
   into a single `Value::Date` variant with an optional time component
   (matches Red's `date!` which can be date-only or date+time). Real Red
   has no separate `time!` type — `time?` is a predicate on `date!`
   values that have a time component. **Timezones are fixed UTC offsets
   only** (`±HH:MM`), matching Red parity — no named zones, no DST.
   Named-zone support (`chrono-tz`) is deferred to v0.5+ if ever needed;
   it would require a new `tz!` word type or string convention and adds
   ~1.2 MB to the binary. The internal zone representation is
   `Option<i32>` minutes (Red's exact internal form), not
   `Option<FixedOffset>` — `FixedOffset` is used transiently during
   parse/mold/`now` only. The `date/zone:` set-path relabels without
   shifting; `to-utc` is the shift+relabel convenience.
6. **M48 (modules) deferred to v0.5:** confirmed per user decision.
   Modules / `import` / `export` will be the headline v0.5 feature
   alongside closures (`closure!`).

(End of plan5.md)
