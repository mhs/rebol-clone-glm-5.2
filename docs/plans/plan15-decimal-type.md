# Plan 15: `decimal!` Type (v0.11)

Execution checklist extending the v0.10.0 baseline in `plan13-feature-parity.md`
(M137 polish assumed complete) and the v0.11 `duration!` work in
`plan14-duration.md`. v0.11 lands a second new value type — `decimal!` — a
true fixed-decimal numeric that solves the IEEE-754 float surprises
(`0.1 + 0.2 ≠ 0.3`) the POC inherits from its f64-backed `float!`.

Today the POC has only `Value::Float(f64)` for non-integer numerics.
`0.1 + 0.2` returns `0.30000000000000004`; `1.0 / 0.0` yields `inf`
silently; `0.0 / 0.0` yields `NaN` and breaks `sort`/`<` invariants. The
existing `money!` type (fixed-point cents, 2 decimals) sidesteps this for
the narrow currency case but isn't a general numeric. v0.11 introduces a
proper `decimal!` backed by [`rust_decimal`](https://crates.io/crates/rust_decimal)
(28-digit precision, 96-bit mantissa, no NaN/Inf) that interoperates with
`integer!`/`float!`/`money!`/`percent!` and lets users opt in per-literal
via the `3.14dec` suffix.

Per `../../project-brief.md`, GUI / `draw` / `vid` / reactive dialects remain
**permanently out of scope**. v0.11 is an **additive release** for the
decimal work — one new `Value` variant, its lexer/parser/mold/convert/
arithmetic surface, and the mixed-type promotion rules. No new VM
hot-path instrs; every new construct is additive through the existing
`Const`-pool + native-call path. The `float!` type is **unchanged** —
`decimal!` coexists with it, never replaces it.

## What's in scope for v0.11 (decimal work)

- **M150 — `decimal!` value + lexer + mold.** The new `Value::Decimal`
  variant backed by `rust_decimal::Decimal` (28-digit precision, 96-bit
  mantissa, no NaN/Inf), the `3.14dec` word-suffix literal (lexed in
  `scan_number` after the duration check, before float assembly),
  mold/form, predicates (`decimal?`), and `make decimal!`/`to-decimal`
  constructors.
- **M151 — Arithmetic + mixed-type promotion.** `+`/`-`/`*`/`/`/`//`/`**`
  on `decimal!` (exact, software u96 arithmetic via `rust_decimal`).
  Promotion rules: `decimal + integer → decimal` (safe, precision-
  preserving); `decimal + float → float` (Float wins on mix — Decimal
  converts to f64, NaN/Inf can't round-trip back); `decimal + percent →
  decimal`; `decimal + money → error` (require explicit `to-decimal`).
  `abs`/`negate`/`min`/`max`/`round` on `decimal!`.
- **M152 — Transcendentals + accessors.** Trig/log/roots (`sin`/`cos`/
  `tan`/`asin`/`acos`/`atan`/`atan2`/`sqrt`/`exp`/`log-e`/`ln`/`log-10`/
  `log-2`) on `decimal!` auto-convert to f64 internally and return
  `float!` (rust_decimal has no transcendental ops; the result would be
  f64-precision anyway — returning `float!` is honest about that).
  `to-integer`/`to-float` on `decimal!` (truncating/explicit conversion).
  `floor`/`ceiling`/`truncate`/`square-root`/`absolute` (the v0.10 math
  helpers) extended to `decimal!`.
- **M153 — Type system + parity polish.** `decimal?` predicate;
  `type?` returns `decimal!`; `types-of` includes it; `typeset!` accepts
  `decimal!` as a known type word; `number!` group accepts `decimal!`;
  `to-decimal`/`make decimal!` from every numeric + parseable string.
  Golden fixtures for the new type; VM/walker parity preserved. v0.11.0
  release.
- **M154 — Polish & v0.11.0 release** (combined with M143 for duration
  if both land together).

## Deferred / out of scope

- **Replacing `float!` with `decimal!`.** `float!` stays as f64 — it
  retains NaN/Inf, transcendentals, and CPU-native speed. Users opt into
  `decimal!` per-literal or via `to-decimal`. A future "rename only"
  (option A from the design discussion — `decimal!` as an alias word for
  `float!`) could layer on top later; not landed here.
- **A literal suffix that collides with duration.** `3.14m` (C# style)
  was rejected — `m` collides with the duration minute suffix
  (`try_scan_duration` at `lexer.rs:990` matches `m` as a 1-char unit
  for minutes, and accepts float magnitudes per `lexer.rs:873`). `dec` is
  collision-free.
- **`decimal!` transcendental ops returning `decimal!`.** `rust_decimal`
  has no `sin`/`cos`/`log`/`sqrt`/`exp` — these would compute via f64
  internally anyway. Returning `float!` is honest about that precision.
  A future "fake-precision" mode that rounds back to `decimal!` is
  possible but misleading; deferred.
- **Arbitrary-precision `BigDecimal`.** Considered (via `bigdecimal`
  crate) — rejected for v0.11: slower, more allocation, unbounded
  precision is overkill for the float-surprise use case. `rust_decimal`'s
  28 digits / 96-bit is the right trade-off.
- **Mixed `decimal + money` arithmetic.** Errors — require explicit
  `to-decimal money-value` first. Money's fixed-2-decimal + currency
  semantics don't compose cleanly with decimal's 28-digit precision.
- **`sort/compare` on mixed `decimal!`/`float!` blocks.** Sort dispatches
  on `partial_cmp`; mixed-type comparison falls through to the `compare`
  native's promotion rules (Decimal→Float). Works but the result block
  will contain both types — users should homogenize first.
- Reactivity, concurrency, full port/async model —
  `future-plan-reactivity.md`, `future-plan-concurrency.md`.
- GUI / `draw` / `vid` dialects — permanently out of scope.

## Non-goals

- A register VM, JIT, or further perf work — the v0.3.3 VM stays the
  default evaluator.
- New `Instr` variants — `decimal!` literals enter via the existing
  `Const`-pool; every decimal operation is a native call.
- Behavior changes to existing v0.2–v0.10 features. The parity contract
  holds: existing golden fixtures (excluding the ones M153 explicitly
  adds for decimal) produce byte-identical output under both `Vm` and
  `force-walk` modes. `float!` arithmetic is unchanged — `0.1 + 0.2`
  still returns `0.30000000000000004` on `float!`; only `0.1dec + 0.2dec`
  returns `0.3`.
- Implicit conversion from `float!` to `decimal!` in arithmetic
  (`3.14 + 0.1dec → error`; require explicit `to-decimal 3.14` first).
  The asymmetry — Decimal+Float promotes to Float, but Float+Decimal
  also promotes to Float — is intentional: Float "wins" because it's
  the wider type (has NaN/Inf, transcendentals).

## Ground-truth references (from research)

- `Value` enum lives in `crates/red-core/src/value.rs:249`. After v0.10 it
  has ~36 variants. `Float { f: f64, span: Span }` at `value.rs:267`;
  `Money { amount: Rc<MoneyValue>, span: Span }` at `value.rs:279`;
  `Percent { value: f64, span: Span }` at `value.rs:273`. New `Decimal`
  variant slots after `Float` (value.rs:267) — they're the same numeric
  family. `Money` is the closest precedent for a non-f64 numeric type:
  it's `Rc`-wrapped because `rust_decimal::Decimal` is 16 bytes (vs f64's
  8); `Decimal` itself is `Copy` so no `Rc` needed, matching `Float`'s
  shape.
- `type_name_for` (`crates/red-core/src/value.rs:1452`) is the red-core
  canonical `&'static str` switch driving `type?` and error messages —
  one new arm returning `"decimal!"`. `TYPE_WORDS` (`value.rs:1510`) and
  the `NUMBER` group (`value.rs:1569`) both need `"decimal!"` added.
- `Lexer::scan_number` (`crates/red-core/src/lexer.rs:783`) already has
  the suffix-extension pattern: percent (`%`) is checked at `:866`,
  duration (`try_scan_duration`) at `:878`, then float/integer assembly
  at `:882`. The `dec` suffix check slots in **after** the duration
  check and **before** float assembly — if `dec` follows the digit run,
  parse as f64 then construct `rust_decimal::Decimal::from_str`.
- `TokenKind` (`crates/red-core/src/lexer.rs:17` lists `Float(f64)`;
  `Money` at `:1282`, `Percent` at `:866`, `Duration` at `:90`). Add
  `Decimal(rust_decimal::Decimal)` near `Float`.
- `mold_float` (`crates/red-core/src/printer.rs:291`) uses `{:?}` for
  shortest round-trippable form; `mold_money` (`printer.rs:320`) is the
  precedent for a non-f64 numeric. `mold_decimal` should use
  `rust_decimal::Decimal`'s `to_string()` (which is already round-trip-
  safe and never produces `NaN`/`inf`). The `mold` arms at `printer.rs:23`
  (Float) and `printer.rs:25` (Money) each gain a Decimal sibling.
- `equal` (`crates/red-eval/src/natives/compare.rs:164`) handles Float at
  `:20` and Money at `:28`. `Num` enum (`compare.rs:330-339`) extracts
  numeric values for ordering; add a `Dec(rust_decimal::Decimal)` arm.
  Integer/Float mixed equality at `compare.rs:35-36` is the precedent for
  cross-type promotion — Decimal/Integer and Decimal/Float follow the
  same pattern.
- `as_f64` (`crates/red-eval/src/math.rs:74`) on the `Num` enum is the
  arithmetic dispatch helper. `+`/`-`/`*`/`/` go through `as_f64`
  (`math.rs:110`, `:129`); Decimal needs a separate path that preserves
  precision (use `rust_decimal`'s native `+`/`-`/`*`/`/` when both sides
  are Decimal-or-Integer, fall back to f64 when Float is involved).
- `to_float` (`crates/red-eval/src/convert.rs:187`) and
  `to_integer` (`convert.rs:94`) are the conversion natives; register at
  `convert.rs:1448-1451`. Add `to_decimal` alongside.
- `float?` predicate (`crates/red-eval/src/natives/words.rs:281`);
  `money?` at `:301`; `types_of` at `:575`; register calls at `:684-686`.
  Add `decimal?` sibling.
- `TypesetDef::is_known_type_word` (`crates/red-eval/src/typeset.rs:98`)
  gates which type-words `typeset!` accepts — add `"decimal!"`.
- VM const pool (`crates/red-eval/src/vm/pool.rs`) stores `Value` directly
  (untyped) — Decimal works with no pool change. Compiler const emission
  (`crates/red-eval/src/vm/compiler.rs:675`) lists `Float` in the const-
  literal arm; add `Decimal` there.
- Tree-walker (`crates/red-eval/src/interp_walker.rs:372`) lists `Float`
  in the "literal value, no further eval" arm; add `Decimal`.
- `rust_decimal` is a pure-Rust crate, no native deps, no async. Adds
  ~20KB to the binary. The `Decimal` type is `Copy` (16 bytes), `Eq`,
  `Hash`, and `Display`-formatted.

## M150 — `decimal!` value + lexer + mold

### Value model (`crates/red-core/src/value.rs`)

- [ ] Add `rust_decimal` to `crates/red-core/Cargo.toml`:
      `rust_decimal = "1.36"` (or current latest stable; pure-Rust, no
      features needed). Add `use rust_decimal::Decimal as RDecimal;` at
      the top of `value.rs` (aliased to avoid clash with our `Value::Decimal`
      variant name).
- [ ] Add `Value::Decimal { d: RDecimal, span: Span }` variant after
      `Float { f: f64, span: Span }` at `value.rs:267`. Use `RDecimal`
      (it's `Copy`, 16 bytes — no `Rc` needed, unlike `Money`).
- [ ] Constructor helper `Value::decimal(d: impl Into<RDecimal>) -> Self`
      near `Value::float` (search for `pub fn float`).
- [ ] `type_name_for` (`value.rs:1452`): add `Value::Decimal { .. } =>
      "decimal!"` arm.
- [ ] `TYPE_WORDS` (`value.rs:1510`): add `"decimal!"` to the array.
- [ ] `NUMBER` group (`value.rs:1569`): add `"decimal!"` so `number?`
      and `typeset!`'s `number!` group accept it.
- [ ] `is_truthy`: Decimal is falsy only when `== RDecimal::ZERO`. Find
      the `is_truthy` impl (search `fn is_truthy` in value.rs) and add
      the arm.
- [ ] `Hash` impl on `Value` (if present): `RDecimal` implements `Hash`;
      add the arm mirroring the `Float` arm.
- [ ] `MapKey` extraction (`value.rs:698` area): decimals can be map
      keys (RDecimal implements `Eq`+`Hash`). Add a `MapKey::Decimal`
      variant or convert to string-key — match the `Integer` precedent.

### Lexer (`crates/red-core/src/lexer.rs`)

- [ ] Add `Decimal(rust_decimal::Decimal)` to the `TokenKind` enum near
      `Float(f64)` at `:17`.
- [ ] In `scan_number` (`lexer.rs:783`), after the duration check at
      `:878` and before float/integer assembly at `:882`, add a `dec`
      suffix check:
      ```rust
      // M150: a digit run immediately followed by `dec` is a decimal!
      // literal (`3.14dec`/`100dec`). Collision-free — duration unit
      // suffixes are 1-2 chars (`s`/`m`/`h`/`d`/`ms`/`us`/`ns`), never
      // `dec`. The suffix must be followed by a delimiter/EOF (not a
      // word-extending char) to commit — `3.14decal` stays a float +
      // word.
      if end + 3 <= bytes.len() && &bytes[end..end+3] == b"dec" {
          let after = end + 3;
          let committed = after >= bytes.len()
              || is_delimiter(bytes[after])
              || bytes[after].is_ascii_digit();
          if committed {
              let text = &src[start..end];
              let d = text.parse::<RDecimal>().map_err(|_| LexError::InvalidNumber {
                  span: Span::new(start, after),
                  chars: src[start..after].to_string(),
              })?;
              return Ok((after, TokenKind::Decimal(d)));
          }
      }
      ```
      Note: `is_float` (`text` contains `.`/`e`) doesn't matter — both
      `3.14dec` and `100dec` are valid. `1e9dec` is also valid
      (`rust_decimal` parses scientific notation).
- [ ] Add `LexError::InvalidDecimal { span, chars }` variant mirroring
      `InvalidPercent`/`InvalidMoney` (`lexer.rs:112-130` area), or
      reuse `InvalidNumber` (simpler — recommended for v0.11).
- [ ] Update `TokenKind` `span()` and `Display` impls to handle `Decimal`.
- [ ] Lexer tests at `lexer.rs:2413` (the `scan_number` tests): add
      `3.14dec`, `0dec`, `100dec`, `1e9dec`, `3.14decx` (should NOT
      commit — lexes as float `3.14` + word `decx`).

### Parser (`crates/red-core/src/parser.rs`)

- [ ] Find `TokenKind::Float(f)` arm (`parser.rs:142` per research) and
      add `TokenKind::Decimal(d) => Value::decimal(d)` alongside.

### Printer (`crates/red-core/src/printer.rs`)

- [ ] Add `mold_decimal(d: RDecimal, out: &mut String)` helper near
      `mold_float` (`printer.rs:291`). Use `d.to_string()` (rust_decimal's
      `Display` is round-trip-safe and never produces `NaN`/`inf`). For
      integer-valued decimals, append `.0` to match the `mold_float`
      convention (so `100dec` molds as `100.0dec`, not `100dec`).
- [ ] Add `Value::Decimal { d, .. } => mold_decimal(*d, out)` arms in
      both `mold` (`:23` area) and `form` (`:211` area).
- [ ] Printer tests at `printer.rs:922` area: add `mold_decimal` test
      covering `3.14dec`, `100dec` (→ `100.0dec`), negative, zero.

### Predicate + constructors

- [ ] `decimal?` predicate in `crates/red-eval/src/natives/words.rs`
      near `float?` (`:281`): `pred1(args, "decimal?", |v| matches!(v,
      Value::Decimal { .. }))`. Register at `:684` area.
- [ ] `to-decimal` native in `crates/red-eval/src/convert.rs` near
      `to_float` (`:187`):
      - `Integer n` → `Decimal::from(n)`
      - `Float f` → `Decimal::try_from(f)` (may fail on NaN/Inf — return
        EvalError in that case)
      - `Percent p` → `Decimal::try_from(p)`
      - `Decimal d` → `d` (identity)
      - `Money m` → `Decimal::new(m.cents, 2)` (currency discarded)
      - `String s` → `s.parse::<Decimal>()` (may fail)
      - `Logic` → `1`/`0`
      Register at `:1448` area.
- [ ] `make decimal!` — ensure the `make` dispatch (`convert.rs` `make`
      native) accepts `decimal!` as a target type-word, routed to the same
      `to-decimal` logic.
- [ ] `types_of` (`words.rs:575`) and `type?`: Decimal returns
      `"decimal!"` via `type_name_for` (already handled in the value.rs
      edit above).

## M151 — Arithmetic + mixed-type promotion

### Compare (`crates/red-eval/src/natives/compare.rs`)

- [ ] `equal` (`compare.rs:164`): add `Decimal`/`Decimal` arm (`d1 == d2`,
      `RDecimal` implements `Eq`). Add `Decimal`/`Integer` and
      `Integer`/`Decimal` mixed arms (promote Integer to Decimal:
      `RDecimal::from(*n) == *d`). Add `Decimal`/`Float` and
      `Float`/`Decimal` mixed arms (convert Decimal to f64 via
      `d.try_into().unwrap_or(f64::NAN)`, compare with `==` — matches
      existing Float equality semantics).
- [ ] `Num` enum (`compare.rs:330-339`): add `Dec(RDecimal)` variant.
      Extend `as_f64` (`:74` area — actually in math.rs, see below) and
      the `partial_cmp` dispatch (`:375-380` area) to handle `Dec`.
- [ ] `compare`/ordering (`compare.rs:184` area): mixed Decimal/Float
      compares via f64 conversion (Float wins); mixed Decimal/Integer
      compares via Decimal (precision-preserving).

### Math (`crates/red-eval/src/math.rs`)

- [ ] `Num` enum (`math.rs:43`): add `Dec(RDecimal)` variant.
- [ ] `as_f64` (`math.rs:74`): add `Num::Dec(d) => d.try_into().unwrap_or(
      f64::NAN)`.
- [ ] Arithmetic dispatch (`math.rs:110` for `+`, `:129` for `-`/`*`/`/`):
      when **both** sides are `Dec` or `Int` (the "exact" path), use
      `rust_decimal`'s native `+`/`-`/`*`/`/` (promoting `Int` to `Dec`
      first). When **either** side is `Float` (or `Percent`, which is
      f64-backed), use the existing f64 path via `as_f64` (Float wins,
      result is `Value::Float`).
- [ ] Result-type rules: `Dec + Dec → Value::decimal`,
      `Dec + Int → Value::decimal`, `Dec + Float → Value::float`,
      `Dec + Percent → Value::decimal` (Percent is f64 but logically a
      ratio — convert via `Decimal::try_from(p)`).
- [ ] `//` (integer-division) and `**` (power): `Dec // Dec → Value::integer`
      (truncate the exact Decimal result). `Dec ** Int → Value::decimal`
      (repeated multiplication); `Dec ** Float → Value::float` (use
      `powf` via f64 conversion).
- [ ] `abs` (`math.rs:1293`), `negate` (`:1332`), `min`/`max`: add
      `Decimal` arms. `round` (`:1663` area, the `round_with_mode`
      helper): add Decimal support via `rust_decimal`'s
      `round_dp`/`trunc` methods.
- [ ] NaN/Inf guards: `rust_decimal` has no NaN/Inf — division by zero
      on `Decimal` returns `RDecimal::ZERO` (rust_decimal's default) but
      we should **error Red-style** (`math error: divide by zero`)
      instead, matching the existing Float divide-by-zero behavior. Add
      an explicit check in the Decimal `/` path.

### Parse dialect (`crates/red-eval/src/parse.rs`)

- [ ] `match` against a literal value (`parse.rs:327` area, the
      `Value::Integer` extraction): add `Value::Decimal { d, .. }` arm
      that compares the input cursor's numeric value against `d` (for
      block-input numeric matching). String-input `parse` with a decimal
      literal in the rule matches the literal text `3.14dec` against the
      input — verify this falls out naturally from the existing
      string-match path.

## M152 — Transcendentals + accessors

- [ ] Trig/log/roots in `math.rs`: the existing natives (`sin` at
      `:1829`, `cos` at `:1834`, `tan`, `asin`, `acos`, `atan`, `atan2`
      at `:1873`, `sqrt` at `:1881`, `exp` at `:1886`, `ln`/`log-e` at
      `:1894`, `log-10` at `:1902`, `log-2` at `:1910`) currently call
      `as_f64` and dispatch via f64. For `Decimal` args, convert to f64
      (via `Num::Dec(d) => d.try_into().unwrap_or(f64::NAN)` in `as_f64`)
      and return `Value::float`. **No new code needed** if `as_f64`
      handles `Dec` — the existing f64 path produces `Value::float`
      automatically.
- [ ] `floor`/`ceiling`/`truncate`/`square-root`/`absolute` (v0.10 math
      helpers in `math.rs`): add `Decimal` arms. `floor`/`ceiling`/
      `truncate` use `rust_decimal::Decimal::floor`/`ceil`/`trunc` and
      return `Value::decimal`. `square-root` on Decimal: convert to f64,
      `sqrt`, convert back to Decimal — but since that's f64-precision,
      just return `Value::float` (parity with the transcendental rule).
      `absolute` uses `d.abs()` and returns `Value::decimal`.
- [ ] `to-integer` (`convert.rs:94`): add `Decimal d => Value::integer(
      d.try_into().unwrap_or(0))` (truncating). `to-float` (`:187`): add
      `Decimal d => Value::float(d.try_into().unwrap_or(f64::NAN))`.

## M153 — Type system + parity polish

- [ ] `typeset!` (`crates/red-eval/src/typeset.rs:98`): add `"decimal!"`
      to the known type-words set in `is_known_type_word`. Also add it to
      any relevant type-groups (`number!` already handled in value.rs;
      check `any-number!` if distinct).
- [ ] Stdlib (`crates/red-eval/stdlib/stdlib.red`): the math utils
      (`gcd`/`lcm`/`sign-of`/`clamp`/`factorial-iter`/`block-sum`/
      `block-mean`/`mean`) — `block-sum`/`block-mean` should transparently
      work on decimal blocks (they use `+` which we've extended). Verify;
      no changes expected unless they hardcode a Float seed (in which case
      add a Decimal path).
- [ ] Golden fixtures: add `crates/red-eval/tests/programs/decimal.red`
      covering:
      - Literal + mold round-trip (`3.14dec` molds as `3.14dec`)
      - Exact arithmetic (`0.1dec + 0.2dec = 0.3dec` → `true`)
      - Mixed promotion (`3.14dec + 1 → 4.14dec`; `3.14dec + 1.0 → 4.14`
        as float)
      - `decimal?`/`type?`/`to-decimal`
      - NaN-free divide-by-zero (errors Red-style)
- [ ] Golden error fixture `crates/red-eval/tests/programs_errors/
      decimal_errors.red`: decimal divide-by-zero, `to-decimal "abc"`,
      `to-decimal 1.0 / 0.0` (NaN float → error).
- [ ] Property tests (`crates/red-eval/tests/property.rs`): add a proptest
      that `to-decimal to-float d` is within epsilon of `d` (lossy
      round-trip via f64). Add a proptest that `to-float to-decimal f`
      equals `f` when `f` has ≤ 15 significant digits (f64 precision
      bound).
- [ ] ../../README.md "Value types" section: add `Decimal` to the Scalars or
      Formatted-scalars bullet with example (`3.14dec`).
- [ ] `../../project-brief.md`: add `decimal!` to the type list.
- [ ] `../../architecture.md`: note the `rust_decimal` dep and the
      Decimal/Float promotion rules.
- [ ] `../../KNOWN_ISSUES.md`: add a note that `float!` NaN/Inf behavior is
      unchanged (still surfaces in `1.0 / 0.0` on floats); recommend
      `decimal!` for exact arithmetic.
- [ ] VM/walker parity: verify `cargo test --workspace` and
      `cargo test --workspace --features force-walk` both green. The
      Decimal path is identical in both evaluators (native-call based,
      not instr-dispatched) — parity should hold automatically.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean;
      `cargo fmt --all --check` clean.

## M154 — Polish & v0.11.0 release

- [ ] Bump crate versions to 0.11.0 in all `Cargo.toml`s.
- [ ] Update `../../BENCHMARKS.md`: note any perf delta on decimal-heavy loops
      (expected: ~3-5× slower than f64 for tight `+`/`*` loops; same as
      f64 for transcendentals since those convert to f64 anyway).
- [ ] Tag `v0.11.0`.

## Verification matrix

| Check | Command | Expectation |
|-------|---------|-------------|
| Workspace builds | `cargo build --workspace` | green |
| VM tests green | `cargo test --workspace` | green |
| Walker tests green | `cargo test --workspace --features force-walk` | green |
| Clippy clean | `cargo clippy --workspace --all-targets -- -D warnings` | no warnings |
| Fmt clean | `cargo fmt --all --check` | no diff |
| Decimal round-trip | `mold 3.14dec` → `"3.14dec"` | exact |
| Exact arithmetic | `0.1dec + 0.2dec = 0.3dec` → `true` | exact |
| Mixed promotion | `3.14dec + 1.0` → `4.14` (float) | precision lost, float result |
| Integer promotion | `3.14dec + 1` → `4.14dec` | precision preserved, decimal result |
| Divide by zero | `1dec / 0dec` | `math error: divide by zero` |
| NaN float → decimal | `to-decimal (1.0 / 0.0)` | error (NaN not representable) |
| Transcendentals | `sin 3.14dec` → float result | `Value::Float` |
| Type predicate | `decimal? 3.14dec` → `true` | green |
| Type word | `type? 3.14dec` → `decimal!` | green |
| Typeset accepts | `make typeset! [decimal!]` | no error |
| Number group | `make typeset! [number!]` accepts `3.14dec` | no error |

## Design decisions (locked)

1. **Literal: `3.14dec`** — word-suffix `dec`. Collision-free with
   duration (`d`/`h`/`m`/`s`/`ms`/`us`/`ns`) and all other suffixes (`%`,
   `$`, `#`, `x`, `e`). The suffix must be followed by a delimiter/EOF
   to commit (so `3.14decal` lexes as float `3.14` + word `decal`).
   Rejected: `3.14m` (collides with duration minutes — `try_scan_duration`
   at `lexer.rs:990` matches `m` as minutes with float magnitudes).

2. **Float wins on mix** — `Decimal + Float → Float`. Rationale: Float is
   the "wider" type (has NaN/Inf, transcendentals, CPU-native speed).
   Decimal is the "precise" type. When they meet, precision is already
   lost (the Float side has rounding), so the result is Float. This
   matches the user's preference and avoids silent precision loss in the
   Decimal result.

3. **Transcendentals return Float** — `sin`/`cos`/`log`/`sqrt`/`exp` on
   Decimal auto-convert to f64 internally and return `Value::Float`.
   Rationale: `rust_decimal` has no transcendental ops; the result is
   f64-precision anyway; returning `Float` is honest about that. A
   future "fake-precision" mode that rounds back to Decimal was
   considered and rejected as misleading.

4. **No NaN/Inf in Decimal** — `rust_decimal::Decimal` has no NaN/Inf
   representation. `1dec / 0dec` errors Red-style (`math error: divide
   by zero`) instead of producing `inf`. `to-decimal` of a NaN/Inf Float
   errors. This is arguably better than Float's silent NaN-propagation,
   but it's a behavior difference between the two numeric types.

5. **`decimal!` is a distinct variant, not a float alias** — Rebol/Red
   treat `decimal!` and `float!` as two names for the same f64 type. We
   diverge: our `decimal!` is a true fixed-decimal (precision-preserving,
   no NaN/Inf), and our `float!` stays f64 (fast, NaN/Inf, transcendentals).
   The two-type world is more honest about the precision trade-off.

6. **Plan at `plan15-decimal-type.md`** — `plan14-duration.md` is taken
   (v0.11 duration work). This plan lands under v0.11 alongside (or
   after) duration, as a separate milestone cluster (M150-M154).

## Open questions (to resolve during implementation)

- Should `percent!` interoperate with `decimal!` via `Decimal::try_from`
  or via f64 conversion? (Currently listed as `Decimal + Percent →
  Decimal` via `try_from` — verify `rust_decimal` accepts the f64 range.)
- Should `sort` on a mixed `decimal!`/`float!` block error or silently
  promote? (Currently: silent promotion via the `compare` native's
  Decimal→Float rule — may produce surprising orderings. Consider an
  explicit "mixed numeric sort" warning or error.)
- Should `to-decimal` accept `money!` (discarding currency) or error
  (requiring `to-decimal to-float money-value` for currency conversion)?
  Currently listed as accepting with currency discarded — verify this
  doesn't surprise users.
