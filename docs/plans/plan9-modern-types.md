# Plan 9: Modern General-Purpose Types (v0.8)

Execution checklist extending the v0.7.0 baseline in `plan8-missing-types.md`
(M90 polish assumed complete). v0.8 lands **nine modern value types** that
match the defaults of contemporary general-purpose languages (Python, JS,
Rust, Swift) — the "every modern language has these" set. The POC stays a
faithful Red clone at its core; v0.8 adds types that the modern Red/Rebol
inventory lacks but which a 2026-era general-purpose language user expects
out of the box.

Per `../../project-brief.md`, GUI / `draw` / `vid` / reactive dialects remain
**permanently out of scope**. v0.8 is a **modern-ergonomics release**, in
the spirit of v0.4/v0.7 but focused on two themes: **numeric exactness** and
**identity & concurrency primitives**. No new VM hot-path instrs; every new
construct is additive through the existing `Const`-pool + native-call path.

## Deferred to v0.9+ (acknowledged, not built here)

- Reactivity (`object!` `on-change` slots — `future-plan-reactivity.md`).
- Concurrency (`Value::Channel` + actor model, OS threads —
  `future-plan-concurrency.md`). v0.8 lands `promise!`/`atomic!` as
  *single-threaded* value shapes; the concurrency release activates them.
- Full port model (`port!` I/O abstraction backed by `Channel`).
- Shared-cell closures (SetWord capture) — `plan6` open-q #1. v0.8's `cell!`
  is the user-side workaround; the deep `bind_pass`/`Context::set` refactor
  remains a v0.9+ candidate.
- `unimport` — `plan6` M62.
- Named timezones (`chrono-tz`) — `plan5` open-q #5.
- `tag!`-algebra (`union`/`intersect`/`complement` of typesets) — `plan8`
  M89 deferral.
- The remaining "modern" types from the exploration not picked for v0.8:
  `rope!`/`buffer!`, `grapheme!`, `sorted-map!`/`btree-set!`, `stream!`,
  `ring-buffer!`, `matrix!`/`tensor!`, `units!`, `enum!`/`sum-type!`,
  `slice!`/`span!`, `c-string!`, `mime-type!`, `color!` (HSL/Lab). Each is
  a plausible v0.9 candidate; none lands here.

## What's in scope for v0.8

Nine new `Value` variants, grouped by theme:

- **M100 — Numeric exactness** (`bigint!`, `decimal!`, `rational!`,
  `complex!`). Four value types backed by the `num-*` crate family. Lands
  together because they share the `num-traits` infrastructure and the
  cross-numeric promotion table.
- **M101 — `uuid!`** (standalone, trivial). The `uuid` crate.
- **M102 — `cell!`** (standalone, high-leverage). The user-visible mutable
  container that fixes the snapshot-closure gap from `plan6` open-q #1
  without the deep `bind_pass`/`Context::set` refactor.
- **M103 — `weak-ref!`** (standalone, small). Non-owning reference for
  cycle-breaking and liveness observation.
- **M104 — `promise!` + `atomic!`** (concurrency-forwarding primitives).
  Land the value shapes in *single-threaded* mode now; the concurrency
  release (v0.9+) activates their thread semantics. `promise!` is a thunk
  / lazy value in v0.8; `atomic!` is a `Cell`-backed mutable cell with
  `compare-and-swap!` ergonomics.
- **M105 — Polish & v0.8.0 release.**

## Non-goals

- A register VM, JIT, or further perf work — the v0.3.3 VM stays the
  default evaluator.
- New `Instr` variants unless a construct provably cannot be a native call
  (none of M100–M104 require one — every new constructor is a `make`
  native, every new predicate is a native, every new literal enters via
  the `Const`-pool).
- Behavior changes to existing v0.2–v0.7 features. The parity contract
  holds: existing golden fixtures produce byte-identical output under both
  `Vm` and `force-walk` modes after every milestone.
- Auto-promotion of `float` to `decimal` or `integer` to `bigint` on
  *operations* — only `bigint` auto-promotes on *overflow at lex time*
  (Python parity). All other numeric exactness is opt-in via constructors
  / explicit conversions.
- Adding sugar for `cell!` auto-deref on closure-captured words — see
  M102's "no-sugar" decision.

## Ground-truth references (from research)

- `Value` enum lives in `crates/red-core/src/value.rs:241`; after v0.7 (plan8)
  it has ~35 variants.
- `type_name` (`crates/red-eval/src/natives/mod.rs:134`) is the single
  `&'static str` switch driving `type?` and error messages. New variants
  add arms here.
- Lexer dispatch lives in `crates/red-core/src/lexer.rs`; the main scan loop
  keys off the first byte. Numeric promotion lives in
  `crates/red-eval/src/math.rs` (`as_number`/`as_float_arg` helpers).
- `compare.rs::values_equal` is the cross-type equality switch; new
  variants need arms (value types compare by contents; reference types by
  `Rc::ptr_eq` for `same?`, deep-equal for `=`).
- `convert.rs::make_value` is the `make` dispatcher; `to-*` converters
  live alongside.
- `red-core` already depends on `indexmap` (M43) and `chrono` (M45); v0.8
  adds `num-bigint`/`num-rational`/`num-complex`/`num-traits` (M100) and
  `uuid` (M101). All are std-ecosystem crates with no async/proc-macro
  surface.
- The snapshot-closure gap: `plan6` open-q #1 documents that SetWord inside
  a `closure` body is treated as a local by the binding pass (not a
  freevar capture). `counter.red` uses block-as-state (`poke`) as a
  workaround. M102's `cell!` is the ergonomic alternative.
- The POC's `Env` is `!Send` (`Rc`/`RefCell`-laden). M104's `atomic!` is
  single-threaded in v0.8 (`Cell`-backed); the `Send`-constraint + real
  atomic semantics land with the concurrency release.

---

## Milestone 100 — Numeric exactness: `bigint!` / `decimal!` / `rational!` / `complex!`

The headline milestone. Four value types backed by the `num-*` crate family,
plus a unified cross-numeric promotion table. Lands together because the
crate infrastructure is shared and the promotion table must be consistent
across all four.

### Dependencies

- [ ] Add to `crates/red-core/Cargo.toml [dependencies]`:
      `num-bigint = "4"`, `num-rational = "0.4"` (pulls `num-bigint`),
      `num-complex = "0.4"`, `num-traits = "0.2"`. (No `rust_decimal`
      for M100 — see "decimal! backing" decision below.)
- [ ] `rust_decimal` considered and **declined** for v0.8 —
      `num-bigint` + `num-rational` give exact arbitrary-precision decimals
      via `BigRational` with a power-of-10 denominator. `decimal!` is
      modeled as `BigRational` constrained to denominator = 10^n at
      construction time. Trade-off: more flexible than `rust_decimal`'s
      fixed 96-bit, slower for tight loops. **Decision: use
      `BigRational`** — keeps the dep surface to the `num-*` family alone,
      and the perf-sensitive decimal use case (`money!` from plan8) is
      already on a fixed `i64` cents representation. `decimal!` is the
      general-purpose exact-decimal type; `money!` is the
      currency-aware fixed-point type; they coexist.

### Promotion table (the load-bearing design)

The unified promotion rules across all numeric types. New types integrate
here; existing `Integer`/`Float` behavior is **unchanged** (back-compat).

| Left `\` Right | Integer | Float | Bigint | Decimal | Rational | Complex |
|---|---|---|---|---|---|---|
| Integer | int | float | bigint | decimal | rational | complex |
| Float | float | float | **err** | **err** | **err** | complex |
| Bigint | bigint | **err** | bigint | decimal | rational | complex |
| Decimal | decimal | **err** | decimal | decimal | rational | complex |
| Rational | rational | **err** | rational | rational | rational | complex |
| Complex | complex | complex | complex | complex | complex | complex |

Rules:
- `Integer` promotes freely (it's the smallest type).
- `Float` is **lossy** — it does NOT promote *to* exact types (would lie
  about precision). `float + bigint` errors with "lossy promotion; use
  `to-bigint` explicitly". `float + complex` is the one exception (complex
  carries floats, no loss).
- `Complex` absorbs everything (its components are `Float`).
- All exact types interoperate through `Rational` (the most general exact
  type): `bigint + decimal` → `rational` (then molded back to the simpler
  form if denominator is 1 or a power of 10 — see mold rules per type).
- `Float` operand + `Complex` operand → complex (complex's real/imag are
  floats; the float operand stays float, no loss).

- [ ] Implement `promote_numeric(l: &Value, r: &Value) -> Result<(Numeric, Numeric), EvalError>`
      in a new `crates/red-eval/src/numeric.rs` module (extracted from
      `math.rs`'s existing `as_number` helper). All `+`/`-`/`*`/`/`/`**`
      arms route through this. Error variant:
      `EvalError::LossyPromotion { from: &'static str, to: &'static str, span }`.
- [ ] Add `EvalError::LossyPromotion` to `crates/red-core/src/env.rs`.
- [ ] Add a `render_error` arm in `crates/red-core/src/error.rs`:
      `*** Error: [loc: ]type error: lossy promotion from <from> to <to>; use to-<to> explicitly`.

### `bigint!`

Arbitrary-precision integers. The "Python default int" — never overflows.

- [ ] Add `Value::Bigint { n: Rc<BigInt>, span: Span }` in `value.rs`
      (after `Integer` — they're the same conceptual type at different
      precisions).
- [ ] Add `Value::bigint(n: BigInt) -> Value` constructor.
- [ ] Extend `Lexer`:
  - [ ] In `scan_number`, after parsing an integer run, if the value
        overflows `i64::MAX`/`i64::MIN`, parse as `BigInt` and emit
        `TokenKind::Bigint(Rc::new(n))`. **Auto-promote on overflow**
        (Python parity).
  - [ ] Also accept explicit `123n` suffix (JS BigInt style) — force
        `BigInt` even if the value fits in `i64`. The `n` suffix is a
        hint, not a requirement.
  - [ ] Disambiguation: `n` after a digit run is the suffix; `n` after a
        word-char is part of the word (no collision — the lexer's
        digit-run branch wins by order).
- [ ] Extend `Parser`: `TokenKind::Bigint(n) => Value::Bigint { n, span }`.
- [ ] Extend `printer.rs`:
  - [ ] `mold`: bare digits (no suffix) — `12345678901234567890`. Round-trip
        works because the lexer auto-promotes on reparse.
  - [ ] `form`: same as mold.
- [ ] Extend `interp_walker.rs` `eval_prefix` self-evaluating arm with
      `Value::Bigint { .. }`.
- [ ] Extend `vm/compiler.rs` const-pool arm with `Value::Bigint { .. }`.
- [ ] Add `bigint?` predicate.
- [ ] Add `to-bigint` converter:
  - [ ] From integer → bigint (free promotion).
  - [ ] From float → **error** (lossy); require explicit `to-bigint` with
        a `/truncate` or `/round` refinement. Default `/truncate`.
  - [ ] From string → parse (decimal or hex `0x...`).
  - [ ] From decimal → error if non-integer; else extract numerator.
- [ ] Add `make bigint! <value>` (same forms as `to-bigint`).
- [ ] Arithmetic: per the promotion table. `bigint + integer` → bigint;
      `bigint + float` → error (lossy). `bigint / bigint` → rational if
      non-divisible? **Decision: stays bigint (truncating division, matches
      `integer / integer`); use `rational!` for exact division.**
- [ ] Bitwise ops (`and`/`or`/`xor`/`complement`/`shift-left`/`shift-right`)
      extended to `bigint` (via `num-bigint`'s `BitAnd` etc.).
- [ ] Comparison (`=`/`<>`/`<`/`>`/`<=`/`>=`) across `integer`/`bigint`
      (cross-type compare by numeric value).
- [ ] Update `type_name` → `"bigint!"`.
- [ ] Update `compare.rs::values_equal` with a `Bigint` arm (cross-type
      with `Integer`: `5 = 5n` → true).
- [ ] Update `types-of`: a bigint is `[bigint! number!]` (NOT `integer!` —
      bigint is its own type word).
- [ ] Inline `#[test]`: `99999999999999999999999` lexes to `Bigint` (auto-
        promote on overflow).
- [ ] Inline `#[test]`: `123n` lexes to `Bigint(123)` even though it fits
        i64 (suffix forces).
- [ ] Inline `#[test]`: `5n + 3` → `8n` (bigint); `5n + 3.0` → error
        (lossy).
- [ ] Inline `#[test]`: `mold 5n` → `"5"` (no suffix on mold).
- [ ] Inline `#[test]`: `5 = 5n` → true (cross-type equality).
- [ ] Inline `#[test]`: `bigint? 5n` → true; `bigint? 5` → false.
- [ ] Add golden fixtures: `bigint_literal`, `bigint_arith`,
        `bigint_overflow`, `bigint_bitwise`.

### `decimal!`

Exact base-10 decimal. The general case of `money!` (plan8 M80) — no
currency, no fixed precision. Backed by `BigRational` constrained to a
power-of-10 denominator.

- [ ] Add `struct DecimalValue { rat: BigRational }` in `value.rs`
      (invariant: denominator is a power of 10; enforced at construction).
- [ ] Add `Value::Decimal { d: Rc<DecimalValue>, span: Span }` variant
      (after `Float`).
- [ ] Add `Value::decimal(rat: BigRational) -> Result<Value, ...>`
      constructor (validates the denominator constraint; returns
      `EvalError::Native` if the denominator isn't a power of 10).
- [ ] Extend `Lexer`:
  - [ ] `1.23d` suffix (mirrors plan8's `1.23m` for money). `d` is free
        (no existing float suffix).
  - [ ] Also accept `1d` (integer-valued decimal) and `0.0001d` (many
        decimal places).
  - [ ] Error `InvalidDecimal` on malformed forms.
- [ ] Extend `Parser`: `TokenKind::Decimal(d) => Value::Decimal { d, span }`.
- [ ] Extend `printer.rs`:
  - [ ] `mold`: `1.23d` form (suffix required for reparse — otherwise
        `1.23` is float). Trim trailing zeros after the decimal point
        (`1.50d` → `1.5d`).
  - [ ] `form`: same as mold.
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [ ] Add `decimal?` predicate.
- [ ] Add `to-decimal` converter:
  - [ ] From float → **error** (lossy); require explicit `to-decimal` with
        a `/precision <n>` refinement (round to n decimal places).
  - [ ] From integer → decimal (free promotion; denominator = 1).
  - [ ] From string → parse (`"1.23"` → `1.23d`).
  - [ ] From rational → error if denominator isn't a power of 10; else
        convert.
  - [ ] From money → strip currency, keep the decimal value.
- [ ] Add `make decimal! <value>` (same forms).
- [ ] Arithmetic: per the promotion table. `decimal + decimal` → decimal
      (exact); `decimal + float` → error (lossy); `decimal + integer` →
      decimal; `decimal + bigint` → rational (then re-narrow to decimal
      if the denominator is still a power of 10 — which it always is for
      decimal ± bigint, since bigint's denominator is 1).
- [ ] Comparison across `integer`/`bigint`/`decimal`/`rational` (by
      numeric value).
- [ ] Update `type_name` → `"decimal!"`.
- [ ] Update `compare.rs` with a `Decimal` arm (cross-type with `Integer`/
        `Bigint`/`Rational`).
- [ ] Update `types-of`: decimal is `[decimal! number!]` (NOT `float!`).
- [ ] Inline `#[test]`: `1.23d` lexes to `Decimal`.
- [ ] Inline `#[test]`: `0.1d + 0.2d` → `0.3d` exactly (the headline
        decimal correctness test).
- [ ] Inline `#[test]`: `0.1 + 0.2` (float) → `0.30000000000000004`
        (float — unchanged; decimal is opt-in).
- [ ] Inline `#[test]`: `1.5d + 1.5d` → `3d` (mold trims trailing zero).
- [ ] Inline `#[test]`: `1.5d + 0.5` → error (lossy).
- [ ] Inline `#[test]`: `decimal? 1.23d` → true; `decimal? 1.23` → false.
- [ ] Inline `#[test]`: `1.5d = 1.5` → true (cross-type by value —
        decimal and float with the same value are equal).
- [ ] Add golden fixtures: `decimal_literal`, `decimal_arith`,
        `decimal_exact`, `decimal_convert`.

### `rational!`

Exact fractions. **No literal form** — `/` is the path delimiter; `1/2`
would break path parsing. Constructor-only.

- [ ] Add `Value::Rational { r: Rc<BigRational>, span: Span }` variant
      (after `Decimal`; rational is the more general exact type).
- [ ] Add `Value::rational(r: BigRational) -> Value` constructor
      (auto-reduces via `BigRational`'s default).
- [ ] Extend `Lexer`: **no new token**. Rationals are constructed at
      runtime only.
- [ ] Extend `Parser`: no new arm.
- [ ] Extend `printer.rs`:
  - [ ] `mold`: `make rational! [<num> <den>]` form (unambiguous,
        reparseable, no lexer collision). Example: `make rational! [1 2]`.
  - [ ] `form`: same as mold.
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [ ] Add `rational?` predicate.
- [ ] Add `to-rational` converter:
  - [ ] From integer → rational (denominator = 1).
  - [ ] From bigint → rational.
  - [ ] From decimal → rational (extract the `BigRational`).
  - [ ] From float → **error** (lossy); require explicit `to-rational`
        with a `/limit <n>` refinement (continued-fraction approximation
        to n terms).
  - [ ] From string → parse `"1/2"` form (the `/` here is in the string
        content, not the lexer — a string parse, not source parse).
- [ ] Add `make rational! <spec>`:
  - [ ] From block `[num den]` → rational.
  - [ ] From integer → rational (denominator = 1).
  - [ ] From decimal → rational.
- [ ] Arithmetic: per the promotion table. `rational + rational` →
      rational (auto-reduces); `rational + float` → error; `rational +
      integer` → rational; `rational + bigint` → rational; `rational +
      decimal` → rational.
- [ ] **Auto-promote `integer / integer` to rational on non-divisible
      division?** Decision: **no** — stays float (back-compat with
      existing `/` semantics). Rationale: changing `1 / 2` from `0.5`
      (float) to `make rational! [1 2]` would break every existing
      fixture using `/`. Rational is opt-in via `to-rational`/`make
      rational!`.
- [ ] Special accessors: `numerator <rational>` → bigint; `denominator
      <rational>` → bigint.
- [ ] Comparison across all exact numeric types (by value).
- [ ] Update `type_name` → `"rational!"`.
- [ ] Update `compare.rs` with a `Rational` arm (cross-type with all
      exact numerics).
- [ ] Update `types-of`: rational is `[rational! number!]`.
- [ ] Inline `#[test]`: `make rational! [1 2]` molds back to
        `make rational! [1 2]`.
- [ ] Inline `#[test]`: `make rational! [1 2] + make rational! [1 3]` →
        `make rational! [5 6]` (auto-reduced).
- [ ] Inline `#[test]`: `numerator make rational! [3 9]` → `1` (reduced).
- [ ] Inline `#[test]`: `1 / 2` → `0.5` (float, unchanged — regression
        guard).
- [ ] Inline `#[test]`: `to-rational 0.5` → error (lossy); `to-rational/limit 5 0.5` → `make rational! [1 2]`.
- [ ] Inline `#[test]`: `rational? make rational! [1 2]` → true;
        `rational? 0.5` → false.
- [ ] Add golden fixtures: `rational_construct`, `rational_arith`,
        `rational_convert`.

### `complex!`

Complex numbers. `1i2` = 1+2i. Mirrors `pair!`'s `NxM` lexer form.

- [ ] Add `struct ComplexValue { re: BigRational, im: BigRational }` in
      `value.rs` (real/imag as `BigRational` so a complex can hold any
      exact numeric; `complex + decimal` promotes the decimal to
      `BigRational`).
      - **Alternative considered:** `num_complex::Complex64` (f64
        components). **Decision: `BigRational` components** — keeps
        cross-exact-type arithmetic lossless; the f64 case is covered by
        `float + complex` promoting the float to a rational (lossy at the
        rational step? No — floats that aren't exact rationals error on
        the promotion, per the table). **Revisit:** this is the one
        promotion-table entry that's subtle. `float + complex` per the
        table is allowed (complex absorbs float). But if complex's
        components are `BigRational`, the float must convert to
        `BigRational`, which is lossy for non-exact floats (e.g. `0.1`).
        **Resolution: `complex!` components are `f64`** (use
        `num_complex::Complex64`), matching Python's `complex` (which is
        f64-backed). Cross-type arithmetic with exact types (bigint/
        decimal/rational + complex) promotes the exact value to f64
        (lossy) and errors if the exact value can't be represented —
        **or** always succeeds with potential loss. Decision: **always
        succeeds with potential loss** (matches Python; users who want
        exactness use `rational!` and avoid `complex!`). Update the
        promotion table: `complex` absorbs everything but the result is
        always f64-backed complex.
- [ ] Add `Value::Complex { c: num_complex::Complex64, span: Span }`
      variant (after `Rational`).
- [ ] Add `Value::complex(re: f64, im: f64) -> Value` constructor.
- [ ] Extend `Lexer`:
  - [ ] `scan_complex`: `NiM` form where N/M are integers or floats (e.g.
        `1i2`, `1.5i2.5`). The `i` separator between digit-led routes
        here (mirrors `pair!`'s `x` separator in M44's
        `detect_pair_tuple`).
  - [ ] Disambiguation from a word starting with `i` (e.g. `if`): the
        `i` must be between two digit runs to be a complex separator.
        No collision (confirm by re-running M44's `detect_pair_tuple`
        audit pattern).
  - [ ] Error `InvalidComplex` on malformed forms.
- [ ] Extend `Parser`: `TokenKind::Complex(c) => Value::Complex { c, span }`.
- [ ] Extend `printer.rs`:
  - [ ] `mold`: `1i2` form (mirrors pair's mold; no spaces around `i`).
  - [ ] `form`: same as mold.
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [ ] Add `complex?` predicate.
- [ ] Add `to-complex` converter (from float → complex with im=0; from
      integer → complex; from pair → complex where pair/x = re, pair/y
      = im).
- [ ] Add `make complex! <spec>`:
  - [ ] From block `[re im]` → complex.
  - [ ] From float/integer → complex with im=0.
  - [ ] From pair → complex.
- [ ] Arithmetic: `complex + complex` → complex; `complex + float` →
      complex (float promotes to re, im=0); `complex * complex` →
      complex. `complex / complex` → complex (error on divide-by-zero —
      both components zero).
- [ ] Math natives on complex: `abs` (magnitude), `conjugate`,
      `real-part`/`imag-part`, `arg` (phase in radians).
- [ ] Trig: `sin`/`cos`/`tan`/`exp`/`log-e` extended to complex (via
      `num_complex`).
- [ ] Comparison: `=`/`<>` only (no ordering — complex is not ordered).
- [ ] Update `type_name` → `"complex!"`.
- [ ] Update `compare.rs` with a `Complex` arm (componentwise equality;
        cross-type with `float`/`integer` via promotion).
- [ ] Update `types-of`: complex is `[complex! number!]`.
- [ ] Inline `#[test]`: `1i2` lexes to `Complex { re: 1.0, im: 2.0 }`.
- [ ] Inline `#[test]`: `1i2 + 3i4` → `4i6`.
- [ ] Inline `#[test]`: `1i2 * 1i2` → `-3i4` (i² = -1).
- [ ] Inline `#[test]`: `abs 3i4` → `5.0` (Pythagorean).
- [ ] Inline `#[test]`: `complex? 1i2` → true; `complex? 1x2` → false
        (pair, not complex).
- [ ] Inline `#[test]`: `1i2 = 1.0i2.0` → true (componentwise).
- [ ] Inline `#[test]`: `1i2 < 3i4` → error (no ordering on complex).
- [ ] Add golden fixtures: `complex_literal`, `complex_arith`,
        `complex_math`, `complex_convert`.
- [ ] Add `programs_errors/complex_no_ordering.red` (`<` on complex).
- [ ] Update `property.rs` for `Bigint`/`Decimal`/`Rational`/`Complex`
      round-trip (Bigint auto-promotes on reparse; Decimal needs the `d`
      suffix; Rational uses the `make rational! [...]` form; Complex uses
      `NiM`).

### M100 closeout

- [ ] **Promotion table audit** — every `+`/`-`/`*`/`/`/`**` arm in
      `math.rs` routes through `promote_numeric`. Confirm no direct
      `Integer`/`Float` arithmetic remains outside the table.
- [ ] **Lossy-promotion error messages** — each error names the from/to
      types and suggests the explicit `to-<type>` conversion.
- [ ] `cargo test --workspace` green; `--features force-walk` green.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `cargo fmt --all --check` clean.

### M100 open questions

1. **`decimal!` backing: `BigRational` vs `rust_decimal`.** Plan ships
   `BigRational` (constrained to power-of-10 denominators) to keep the
   dep surface to `num-*` alone. `rust_decimal` would be faster for tight
   loops but adds a non-`num-*` dep and is fixed-precision (96-bit).
   Confirm before implementing. Recommendation: `BigRational`.
2. **`bigint!` auto-promote on overflow.** Python does this; JS requires
   the `n` suffix. Auto-promote is the "modern general-purpose" default.
   Confirm. Recommendation: auto-promote.
3. **`rational!` no literal.** The `/` collision is real. The
   `make rational! [1 2]` mold form is unambiguous but verbose.
   Alternative: a `1r2` suffix (mirrors `1i2` complex). Decision: **no
   `r` suffix** — `r` is too common a word-start char and the
   digit-then-`r`-then-digit detection would surprise users reading
   `1red` as a word. Stick with constructor-only.
4. **`complex!` components: `f64` vs `BigRational`.** Plan ships `f64`
   (matches Python). `BigRational` components would make
   `complex + decimal` lossless but make `complex` much heavier and
   break the "complex is the f64-backed numeric" intuition. Confirm
   `f64`. Recommendation: `f64`.
5. **`integer / integer` staying float.** Back-compat. Confirm.
   Recommendation: stay float; rational is opt-in.
6. **`bigint!` and `integer!` cross-type equality.** `5 = 5n` → true
   (by value). `types-of 5n` → `[bigint! number!]` (NOT `integer!`).
   Confirm — this is the subtle case. Recommendation: as stated.

---

## Milestone 101 — `uuid!`

Standalone, trivial. The `uuid` crate. No lexer form — UUIDs aren't source
literals in any modern language.

- [ ] Add `uuid = "1"` to `crates/red-core/Cargo.toml [dependencies]`.
- [ ] Add `Value::Uuid { u: uuid::Uuid, span: Span }` variant (synthetic
      by default — `make uuid!` produces `Span::default()`; a future
      `#uuid"..."` literal form is deferred).
- [ ] Add `Value::uuid(u: uuid::Uuid) -> Value` constructor.
- [ ] Extend `printer.rs`:
  - [ ] `mold`: standard `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` lowercase
        form. **Reparseable** via `make uuid! "<string>"`.
  - [ ] `form`: same as mold.
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [ ] Add `uuid?` predicate.
- [ ] Add `to-uuid` converter (from string parse — error on malformed;
      from binary → first 16 bytes).
- [ ] Add `make uuid! <spec>`:
  - [ ] From string → parse.
  - [ ] From binary → first 16 bytes (error if < 16).
  - [ ] From `none` or no arg → `random-uuid` (v4). **Decision: `make
        uuid!` with no args = v4 random; `make uuid! <value>` = parse.**
- [ ] Add `random-uuid` native (0-arity; returns a v4 UUID). The headline
      constructor.
- [ ] Path access: **none** — UUIDs are opaque scalars (mirrors `issue!`).
- [ ] Equality: by 128-bit content (two UUIDs with the same bytes are
      `equal?`); `same?` is `Rc::ptr_eq` (but UUIDs are `Rc<Uuid>` only if
      we box them — **decision: store `uuid::Uuid` inline, not `Rc`**;
      `Uuid` is 16 bytes, cheaper to copy than to `Rc`. `same?` on UUIDs
      = `equal?` by value).
- [ ] Update `type_name` → `"uuid!"`.
- [ ] Update `compare.rs` with a `Uuid` arm (byte compare).
- [ ] Update `types-of`: uuid is `[uuid!]` (NOT `number!`/`series!` —
      it's an opaque scalar).
- [ ] Inline `#[test]`: `random-uuid` returns a `uuid!` value.
- [ ] Inline `#[test]`: `make uuid! "550e8400-e29b-41d4-a716-446655440000"` →
        the parsed UUID; molds back identically.
- [ ] Inline `#[test]`: `make uuid! "not-a-uuid"` → error.
- [ ] Inline `#[test]`: `make uuid!` (no args) → v4 random (same as
        `random-uuid`).
- [ ] Inline `#[test]`: `uuid? random-uuid` → true; `uuid? "..."` → false.
- [ ] Inline `#[test]`: two `random-uuid` calls return different values
        (with overwhelming probability — run 1000×, assert all distinct).
- [ ] Inline `#[test]`: `equal?` of two UUIDs from the same string → true.
- [ ] Add golden fixtures: `uuid_construct`, `uuid_random`,
        `uuid_convert`.
- [ ] Add `programs_errors/uuid_bad_string.red`.
- [ ] Update `property.rs` for `Uuid` round-trip (mold → parse → mold).
- [ ] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 102 — `cell!`

The user-visible mutable container. The most leveraged type in v0.8 —
fixes the snapshot-closure gap (`plan6` open-q #1) without the deep
`bind_pass`/`Context::set` refactor that shared-cell closures would need.

### The closure-gap fix

Today, SetWord inside a `closure` body is treated as a local by the
binding pass (not a freevar capture). `counter.red` uses block-as-state
(`poke`) as a workaround. With `cell!`:

```red
make-counter: func [n][
    c: cell n
    closure [][
        c: set c (get c) + 1   ; explicit get/set through the cell
        get c
    ]
]
```

The closure captures the *cell* (a `Value::Cell`), not the integer. Reads
via `get c`, writes via `set c`. The cell's `Rc<RefCell<Value>>` is
shared across invocations of the same closure (the snapshot captures the
`Rc`).

- [ ] Add `struct CellDef { value: RefCell<Value> }` in `value.rs`.
- [ ] Add `Value::Cell(Rc<RefCell<CellDef>>)` variant (synthetic, no span).
      - **Wait:** `CellDef` wraps `RefCell<Value>`, and the variant is
        `Rc<RefCell<CellDef>>` — that's a double `RefCell` (one on the
        outer `Rc<RefCell<...>>`, one on the inner `CellDef.value`).
        **Simplify:** `Value::Cell(Rc<RefCell<Value>>)` directly — no
        `CellDef` struct. The variant IS the cell. Confirm.
- [ ] Add `Value::cell(v: Value) -> Value` constructor.
- [ ] Extend `printer.rs`:
  - [ ] `mold`/`form`: `#[cell <mold-of-current-value>]` (debug form,
        non-reparseable — like `#[function]`). Example: `#[cell 5]`.
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms
      (a cell value is data — returned as-is, not unwrapped).
- [ ] Add `cell?` predicate.
- [ ] Add `make cell! <value>` (or `cell <value>` — **decision: `make
      cell!` only**, no bareword `cell` native, to avoid colliding with a
      user word named `cell`).
- [ ] Add `get` extension: `get` today resolves a word to its value; for
      a `cell!` operand, `get <cell>` returns the cell's current value.
      The existing `get` native gains a `Value::Cell` arm.
- [ ] Add `set` extension: `set <cell> <value>` updates the cell's value
      and **returns the cell itself** (for chaining: `c: set c (get c) + 1`).
      The existing `set` native gains a `Value::Cell` arm.
- [ ] Add `swap <cell> <value>` — alias for `set` (returns the cell).
      Distinct name because some languages' `swap!` exchanges two values;
      here it's one-way (set the cell, return it). **Decision: don't add
      `swap`; `set` is enough.** (Revisit if users want two-cell swap.)
- [ ] Add `cell-reset <cell>` — set the cell to `none` (convenience).
      **Decision: skip — `set <cell> none` works.**
- [ ] **No auto-deref on closure-captured cells.** Users write `get c`/`set c`.
      Rationale: auto-deref would require the binding pass to recognize
      cell-typed words and rewrite their `Binding` to a "cell-deref"
      variant — a deep change. The explicit form is verbose but
      transparent. Document the pattern in `examples/cell-closure.red`.
- [ ] Update `type_name` → `"cell!"`.
- [ ] Update `compare.rs`: two cells are `equal?` iff their current
      values are `equal?` (deep — mirrors `map!`/`object!`); `same?` is
      `Rc::ptr_eq` (two cells are the same cell iff they share the
      underlying `Rc<RefCell<Value>>`).
- [ ] Update `types-of`: cell is `[cell!]` (NOT `series!` — a cell is a
      single-slot container, not indexable).
- [ ] Inline `#[test]`: `c: make cell! 5  get c` → `5`.
- [ ] Inline `#[test]`: `set c 10  get c` → `10`.
- [ ] Inline `#[test]`: `set c (get c) + 1  get c` → `11` (the counter
        pattern).
- [ ] Inline `#[test]`: closure capture — `f: func [][c: make cell! 0
        closure [][set c (get c) + 1  get c]]  g: f  g  g` → `1`, `2`
        (the closure's cell persists across calls).
- [ ] Inline `#[test]`: `cell? make cell! 0` → true; `cell? 0` → false.
- [ ] Inline `#[test]`: `mold make cell! 5` → `"#[cell 5]"`.
- [ ] Inline `#[test]`: `same?` on the same cell → true; on two cells
        with the same value → false (identity, not value).
- [ ] Add golden fixtures: `cell_basic`, `cell_closure_counter`,
        `cell_shared`.
- [ ] Add `examples/cell-closure.red` — the canonical counter pattern,
        documented as the replacement for snapshot-closure mutation.
- [ ] Add a stable-string property test for `Cell` (`mold cell == "#[cell <mold-v>]"`).
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M102 open questions

1. **`CellDef` struct vs `Rc<RefCell<Value>>` directly.** Decision: the
   latter (simpler). Confirm.
2. **`set <cell> <value>` return value.** Returns the cell (for
   chaining) vs. returns `none` (side-effect-only). Recommendation:
   return the cell. Confirm.
3. **`cell!` and `object!` overlap.** An object is a multi-slot
   container; a cell is a single-slot. Should `cell!` be a degenerate
   `object!`? Decision: no — distinct types, distinct ergonomics (a
   cell is captured by closures; an object is pathed). Confirm.

---

## Milestone 103 — `weak-ref!`

Non-owning reference. Breaks reference cycles (object graphs, caches),
observes liveness without preventing GC.

- [ ] Add `struct WeakRefDef { weak: RefCell<Weak<Value>> }` in `value.rs`.
      - `Weak<Value>` requires `Value: ?Sized` — `Value` is `Sized`, so
        `Weak<Value>` works. But `Weak` needs the strong ref to be an
        `Rc<Value>`; the POC's `Rc`s are inside variants (`Rc<RefCell<
        ObjectDef>>` etc.), not `Rc<Value>` directly. **Resolution: a
        `weak-ref!` holds a `Weak<...>` to the *inner* `Rc` of whatever
        variant it was created from.** The `WeakRefDef` stores an enum
        of `Weak<RefCell<ObjectDef>>`/`Weak<RefCell<MapDef>>`/etc. — one
        per `Rc`-backed variant. For non-`Rc` variants (Integer, Float,
        etc.), `weak-ref!` errors ("cannot weak-ref a value type") OR
        wraps the value in an `Rc<Value>` internally. **Decision: error
        on value types** — `weak-ref!` is for reference types
        (object!/map!/module!/closure!/cell!); value types are copied,
        not referenced, so a weak-ref to them is meaningless. Document.
- [ ] Add `enum WeakTarget { Object(Weak<RefCell<ObjectDef>>), Map(Weak<RefCell<MapDef>>), Module(Weak<RefCell<ModuleDef>>), Closure(Weak<ClosureDef>), Cell(Weak<RefCell<Value>>), Bitset(Weak<RefCell<BitsetDef>>), Hash(Weak<RefCell<HashDef>>), Vector(Weak<RefCell<VectorDef>>), Image(Weak<RefCell<ImageDef>>) }`
      in `value.rs` (one variant per `Rc`-backed `Value` variant).
- [ ] Add `Value::WeakRef(Rc<WeakRefDef>)` variant (synthetic, no span).
- [ ] Add `Value::weak_ref(target: &Value) -> Result<Value, EvalError>`
      constructor — matches the value's variant, downgrades the inner
      `Rc`, stores in the appropriate `WeakTarget` arm. Errors on value
      types.
- [ ] Extend `printer.rs`:
  - [ ] `mold`/`form`: `#[weak-ref <alive|dead>]` (non-reparseable;
        status shown for debug).
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [ ] Add `weak-ref?` predicate.
- [ ] Add `weak-ref <value>` native (the constructor — distinct from
      `make weak-ref!` because the latter suggests parsing a spec; the
      operand is an existing value).
- [ ] Add `make weak-ref! <value>` (alias for `weak-ref <value>`).
- [ ] Add `deref <weak-ref>` native — returns the strong value if alive,
      `none` if dead. (Upgrades the `Weak` to `Rc`; if `Weak::upgrade()`
      returns `None`, returns `Value::None`.)
- [ ] Add `weak-ref-alive? <weak-ref>` predicate — true if the target is
      still alive (doesn't deref; just checks `Weak::strong_count() > 0`).
- [ ] No path access (opaque).
- [ ] Update `type_name` → `"weak-ref!"`.
- [ ] Update `compare.rs`: two weak-refs are `equal?` iff they target
      the same allocation (`Weak::as_ptr` equality); `same?` is `Rc::ptr_eq`
      on the `WeakRefDef` itself.
- [ ] Update `types-of`: weak-ref is `[weak-ref!]`.
- [ ] Inline `#[test]`: `o: make object! [x: 1]  w: weak-ref o  deref w` →
        the object.
- [ ] Inline `#[test]`: `weak-ref-alive? w` → true; then drop `o`; → false.
  - *(Single-threaded, `Rc` — the strong ref drops when `o`'s slot is
        cleared. Test by setting `o: none` and forcing a collection
        pass — `Rc` has no GC, so the strong ref drops immediately when
        the last `Rc` goes out of scope. The test sets `o: none`,
        which drops the user_ctx's `Rc`, and `deref w` returns `none`.)*
- [ ] Inline `#[test]`: `weak-ref 5` → error ("cannot weak-ref a value
        type").
- [ ] Inline `#[test]`: `weak-ref? weak-ref (make object! [])` → true.
- [ ] Inline `#[test]`: `mold (weak-ref (make object! []))` →
        `"#[weak-ref alive]"` (or `dead` if the temp already dropped).
- [ ] Add golden fixtures: `weak_ref_basic`, `weak_ref_cycle_break`.
- [ ] Add `programs_errors/weak_ref_value_type.red` (`weak-ref 5`).
- [ ] Add a stable-string property test for `WeakRef`.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M103 open questions

1. **Value-type weak-refs.** Plan errors. Alternative: wrap the value in
   an `Rc<Value>` internally so any value can be weak-ref'd (the weak-ref
   dies when the *wrapper* drops, which is immediately unless the user
   also keeps a strong ref). Decision: error — keep the semantics clear
   (weak-refs are for reference types). Confirm.
2. **`deref` naming.** `deref` vs `resolve` vs `weak-ref/value`. Red
   has no `deref`; this is a modern addition. Recommendation: `deref`
   (Rust parity; clear). Confirm.
3. **Cycle-breaking in practice.** A test demonstrating an object graph
   cycle (A → B → A) where `weak-ref!` on one direction prevents the
   leak. Add as a golden fixture. Confirm.

---

## Milestone 104 — `promise!` + `atomic!` (single-threaded)

Concurrency-forwarding primitives. Land the value shapes in *single-threaded*
mode now; the concurrency release (v0.9+) activates their thread semantics.

### `promise!`

A single-assignment future. In v0.8, a thunk / lazy value. Full thread
integration lands with the concurrency release.

- [ ] Add `struct PromiseDef { state: RefCell<PromiseState> }` in `value.rs`.
- [ ] Add `enum PromiseState { Pending, Fulfilled(Value), Rejected(Rc<ErrorValue>) }`.
- [ ] Add `Value::Promise(Rc<RefCell<PromiseDef>>)` variant (synthetic).
- [ ] Add `Value::promise() -> Value` constructor (creates a pending
      promise).
- [ ] Extend `printer.rs`:
  - [ ] `mold`/`form`: `#[promise <pending|fulfilled|rejected>]` (debug
        form, non-reparseable).
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [ ] Add `promise?` predicate.
- [ ] Add `make promise!` (no args → pending promise; the constructor).
- [ ] Add `fulfill <promise> <value>` native — transitions
      `Pending → Fulfilled(value)`. Errors if already fulfilled/rejected.
- [ ] Add `reject <promise> <error>` native — transitions
      `Pending → Rejected(error)`.
- [ ] Add `await <promise>` native — **in v0.8, single-threaded**:
  - [ ] If `Fulfilled(v)`, returns `v`.
  - [ ] If `Rejected(e)`, raises `e` (as a `Value::Error`).
  - [ ] If `Pending`, **panics** with "deadlock: await on an unfulfilled
        promise in single-threaded mode" (deadlock detection — no other
        thread can fulfill it).
  - [ ] Document the v0.9+ behavior: `await` blocks the current thread
        until fulfilled; in the M:N scheduler, `await` parks the actor
        and yields to the scheduler.
- [ ] Add `promise-state <promise>` → returns `'pending`/`'fulfilled`/
      `'rejected` (a `word!`).
- [ ] Add `promise/then`? **Decision: no `.then` chaining in v0.8** —
      that's a combinator that belongs with the concurrency release
      (needs scheduler integration). v0.8 is the primitive; v0.9 adds
      combinators.
- [ ] No path access (opaque).
- [ ] Update `type_name` → `"promise!"`.
- [ ] Update `compare.cs`: two promises are `equal?` iff they're the same
      promise (`Rc::ptr_eq`); `same?` is also `Rc::ptr_eq` (a promise IS
      its identity).
- [ ] Update `types-of`: promise is `[promise!]`.
- [ ] Inline `#[test]`: `p: make promise!  fulfill p 42  await p` → `42`.
- [ ] Inline `#[test]`: `await (make promise!)` → panic (deadlock
        detection — wrap in `std::panic::catch_unwind`).
- [ ] Inline `#[test]`: `p: make promise!  reject p (make error! "boom")
        await p` → raises the error.
- [ ] Inline `#[test]`: `fulfill p 1  fulfill p 2` → error (already
        fulfilled).
- [ ] Inline `#[test]`: `promise-state (make promise!)` → `'pending`;
        after `fulfill` → `'fulfilled`.
- [ ] Inline `#[test]`: `promise? make promise!` → true.
- [ ] Inline `#[test]`: `mold make promise!` → `"#[promise pending]"`.
- [ ] Add golden fixtures: `promise_fulfill`, `promise_reject`,
        `promise_deadlock`.
- [ ] Add `programs_errors/promise_double_fulfill.red`,
        `programs_errors/promise_deadlock.red`.
- [ ] Add a stable-string property test for `Promise`.
- [ ] Document the deadlock caveat in `../../project-brief.md` and `../../README.md`.

### `atomic!`

A mutable shared cell with `compare-and-swap!` semantics. In v0.8,
single-threaded (`Cell`-backed); v0.9+ makes it `Send`-safe with real
atomics.

- [ ] Add `struct AtomicDef { value: RefCell<Value> }` in `value.rs`
      (single-threaded; will become `AtomicCell` or `Arc<AtomicU64>` +
      payload in v0.9+).
- [ ] Add `Value::Atomic(Rc<RefCell<AtomicDef>>)` variant (synthetic).
      - **Naming collision:** `Value::Cell` (M102) and `Value::Atomic`
        are both single-slot mutable containers. Distinct because
        `atomic!` has CAS semantics and will be `Send`-restricted; `cell!`
        is unsynchronized. Confirm the distinction is worth two types.
- [ ] Add `Value::atomic(v: Value) -> Value` constructor.
- [ ] Extend `printer.rs`:
  - [ ] `mold`/`form`: `#[atomic <mold-of-current-value>]` (debug form).
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [ ] Add `atomic?` predicate.
- [ ] Add `make atomic! <value>`.
- [ ] Add `atomic-get <atomic>` (alias `get` extension — `get` on an
      atomic returns the value; same arm as `cell!`).
- [ ] Add `atomic-set <atomic> <value>` (alias `set` extension — `set`
      on an atomic updates and returns the atomic).
- [ ] Add `swap! <atomic> <value>` native — atomically sets the value,
      returns the *old* value. (Distinct from `set` which returns the
      atomic itself.)
- [ ] Add `compare-and-swap! <atomic> <expected> <new>` native — if the
      current value equals `expected`, set to `new` and return `true`;
      else return `false`. (The CAS primitive.)
  - [ ] In v0.8 (single-threaded), CAS is trivially atomic (no
        contention). In v0.9+, it uses real `AtomicU64` compare-exchange.
  - [ ] Equality for `<expected>`: structural (`equal?`), not identity.
        Document.
- [ ] Update `type_name` → `"atomic!"`.
- [ ] Update `compare.rs`: `equal?` on atomics is by current value
      (deep); `same?` is `Rc::ptr_eq`.
- [ ] Update `types-of`: atomic is `[atomic!]`.
- [ ] Inline `#[test]`: `a: make atomic! 5  atomic-get a` → `5`.
- [ ] Inline `#[test]`: `swap! a 10` → `5` (old); `atomic-get a` → `10`.
- [ ] Inline `#[test]`: `compare-and-swap! a 10 20` → `true`;
        `atomic-get a` → `20`.
- [ ] Inline `#[test]`: `compare-and-swap! a 10 30` → `false` (expected
        mismatch); `atomic-get a` → `20` (unchanged).
- [ ] Inline `#[test]`: `atomic? make atomic! 0` → true; `atomic? make
        cell! 0` → false (distinct types).
- [ ] Inline `#[test]`: `mold make atomic! 5` → `"#[atomic 5]"`.
- [ ] Add golden fixtures: `atomic_basic`, `atomic_cas`.
- [ ] Add a stable-string property test for `Atomic`.
- [ ] Document the v0.9+ activation (real atomics, `Send`-restriction)
      in `../../project-brief.md` and `../../README.md`.

### M104 closeout

- [ ] Document the **single-threaded caveat** for both `promise!` and
      `atomic!` in `../../project-brief.md` and `../../README.md`: "v0.8 lands the
      value shapes; the concurrency release (v0.9+) activates thread
      semantics. `await` on a pending promise panics (deadlock detection);
      `atomic!` is `Cell`-backed (no real concurrency until v0.9+)."
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M104 open questions

1. **`promise!` before threads.** Worth landing as a thunk, or wait for
   the concurrency release? Plan lands now (the value shape is stable;
   v0.9+ adds the scheduler integration). The deadlock-on-await caveat
   is documented. Confirm.
2. **`atomic!` vs `cell!` distinction.** Both are single-slot mutable
   containers. `atomic!` has CAS + will be `Send`; `cell!` is
   unsynchronized + never `Send`. Is the distinction worth two types?
   Recommendation: yes — they have different contracts (CAS is the
   concurrency primitive; `cell!` is the closure-capture container).
   Confirm.
3. **`swap!` return value.** Returns the old value (Rust `Atomic*::swap`
   parity) vs. returns the atomic (for chaining). Recommendation: old
   value (the CAS-family ergonomics). Confirm.
4. **`compare-and-swap!` equality.** Structural (`equal?`) vs. strict
   (`==`/`same?`). Recommendation: structural — matches the intuition
   ("is the current value the value I expect?"). Document that
   `compare-and-swap!` on a `cell!`-backed atomic with a `Rc`-payload
   compares by value, not identity; use `same?`-flavored CAS if needed
   (a `/same` refinement? Deferred to v0.9+).

---

## Milestone 105 — Polish & v0.8.0 release

- [ ] Audit `EvalError` rendering for all new error sources:
  - [ ] `InvalidDecimal` / `InvalidComplex` (M100 lexer errors).
  - [ ] `LossyPromotion` (M100 — the new error variant for float→exact
        and exact→float attempts).
  - [ ] `InvalidUuid` (M101).
  - [ ] `WeakRefValueType` (M103 — "cannot weak-ref a value type").
  - [ ] `PromiseDeadlock` (M104 — panic message; consider promoting to
        an `EvalError` variant for catchability via `try`).
  - [ ] `PromiseAlreadyFulfilled`/`PromiseAlreadyRejected` (M104).
- [ ] Add spans to all source-origin new variants (`Bigint`/`Decimal`/
      `Complex` already struct-with-span; confirm synthetic variants
      use `Span::default()`).
- [ ] Golden fixture per new error case (one per error kind added in
      M100–M104).
- [ ] Property test: extend `mold(parse(mold(v)))` to cover `Bigint`/
      `Decimal`/`Complex`/`Uuid` (the reparseable ones). `Rational`/
      `Cell`/`WeakRef`/`Promise`/`Atomic` get stable-string assertions
      instead (their mold forms are `#[...]` placeholders or
      `make rational! [...]` which IS reparseable — include `Rational`
      in the round-trip set after all).
- [ ] Extend `red-core/tests/golden/` to cover all new literals.
- [ ] Expand `red-eval/tests/programs/` to 25+ new fixtures (one per new
      type × positive + error case).
- [ ] Run `cargo bench --bench eval`; record in `../../BENCHMARKS.md` under
      "v0.8.0".
  - [ ] Expected neutral on existing benches (no new hot-path work).
  - [ ] The M100 promotion table routes every `+`/`-`/`*`/`/` through
        `promote_numeric` — a single match. If any bench regresses >5%,
        investigate the match arm count (the table is 6×6 = 36 entries,
        but the common `Integer`/`Float` paths short-circuit early).
  - [ ] Add a `bigint_arith` bench (Bigint × Bigint addition) — expected
        slower than `Integer` but the use case is correctness, not speed.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`; fix.
- [ ] Run `cargo fmt --all --check`; fix.
- [ ] Update `../../project-brief.md`:
  - [ ] Add a "Modern Types (v0.8)" subsection under "Value model": list
        the nine new variants, the `num-*` and `uuid` crate deps, the
        promotion table, the `cell!` closure-gap fix, the single-
        threaded caveat for `promise!`/`atomic!`.
  - [ ] Update the value-model code block (add `Bigint`/`Decimal`/
        `Rational`/`Complex`/`Uuid`/`Cell`/`WeakRef`/`Promise`/
        `Atomic`).
  - [ ] Update "Deferred" — remove the v0.8 items; add v0.9+ candidates
        (reactivity, concurrency/port model, routine! FFI binding
        layer, typeset algebra, shared-cell closures, the remaining
        modern types from the v0.8 exploration not picked here).
- [ ] Update `../../architecture.md`:
  - [ ] New value variants in the value-model section.
  - [ ] `DecimalValue`/`ComplexValue`/`WeakRefDef`/`WeakTarget`/
        `PromiseDef`/`PromiseState`/`AtomicDef` struct definitions.
  - [ ] The `promote_numeric` table and the `LossyPromotion` error.
  - [ ] The `cell!` closure-capture pattern (with the `examples/cell-
        closure.red` reference).
  - [ ] The `weak-ref!` cycle-breaking pattern.
  - [ ] The `promise!`/`atomic!` single-threaded caveat.
- [ ] Update `../../README.md`:
  - [ ] Bump version to v0.8.0.
  - [ ] Add the nine new types to the "Value types" list.
  - [ ] Add `bigint?`/`decimal?`/`rational?`/`complex?`/`uuid?`/`cell?`/
        `weak-ref?`/`promise?`/`atomic?` to the type predicates list.
  - [ ] Add `to-bigint`/`to-decimal`/`to-rational`/`to-complex`/
        `to-uuid` to the conversions list.
  - [ ] Add `random-uuid`/`fulfill`/`reject`/`await`/`promise-state`/
        `swap!`/`compare-and-swap!`/`weak-ref`/`deref`/`weak-ref-alive?`
        to the natives list.
  - [ ] Note the `LossyPromotion` error and the explicit-conversion
        requirement (float → exact needs `to-bigint`/`to-decimal`/etc.).
  - [ ] Note the `promise!`/`atomic!` single-threaded caveat.
  - [ ] Update "Known gaps" with the new deferrals (reactivity,
        concurrency, port model, routine! FFI, typeset algebra,
        shared-cell closures, the remaining modern types).
- [ ] Final `cargo test --workspace` green.
- [ ] Final `cargo test --workspace --features force-walk` green.
- [ ] Final `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] Tag release `v0.8.0`.

---

## Open questions (plan-wide)

1. **`decimal!` backing (`num-rational` vs `rust_decimal`).** M100 open-q
   #1. Recommendation: `BigRational` (constrained). Confirm before
   implementing.
2. **`bigint!` auto-promote on overflow.** M100 open-q #2. Recommendation:
   auto-promote (Python parity). Confirm.
3. **`complex!` components (`f64` vs `BigRational`).** M100 open-q #4.
   Recommendation: `f64` (Python parity). Confirm.
4. **`integer / integer` staying float.** M100 open-q #5. Recommendation:
   stay float (back-compat); rational is opt-in. Confirm.
5. **`bigint!` / `integer!` cross-type equality.** M100 open-q #6. `5 = 5n`
   → true; `types-of 5n` → `[bigint! number!]` (not `integer!`). Confirm.
6. **`rational!` no literal.** M100 open-q #3. `make rational! [1 2]` mold
   form; no `r` suffix (collision risk). Confirm.
7. **`cell!` no-sugar.** M102 open-q: explicit `get`/`set` vs auto-deref
   on closure-captured cells. Recommendation: explicit (transparent,
   avoids a binding-pass change). Confirm.
8. **`weak-ref!` on value types.** M103 open-q #1. Recommendation: error
   (weak-refs are for reference types). Confirm.
9. **`promise!` before threads.** M104 open-q #1. Recommendation: land now
   (value shape is stable; v0.9+ activates scheduler). Confirm.
10. **`atomic!` vs `cell!` distinction.** M104 open-q #2. Recommendation:
    two types (CAS contract vs. closure-capture container). Confirm.
11. **`swap!` return value.** M104 open-q #3. Recommendation: old value
    (CAS-family ergonomics). Confirm.
12. **`compare-and-swap!` equality.** M104 open-q #4. Recommendation:
    structural (`equal?`). Confirm.

## Dependency summary

| Crate | Added in | Used by | Purpose |
|---|---|---|---|
| `num-bigint = "4"` | M100 | red-core | `bigint!`, `rational!` backing |
| `num-rational = "0.4"` | M100 | red-core | `rational!`, `decimal!` backing |
| `num-complex = "0.4"` | M100 | red-core | `complex!` |
| `num-traits = "0.2"` | M100 | red-core | cross-numeric trait bounds |
| `uuid = "1"` | M101 | red-core | `uuid!` |

All are std-ecosystem, no async, no proc-macros. `red-core`'s dep count
rises from 2 (`indexmap`, `chrono`) to 7 after v0.8. `red-eval` and
`red-cli` add no new deps in v0.8.

## Sequencing recap

- **M100** is the biggest milestone (four types + the promotion table).
  Land it first; the promotion table is load-bearing for the other three.
- **M101** (`uuid!`) is trivial — land second as a palette-cleanser.
- **M102** (`cell!`) is the highest-leverage — land third so the closure-
  gap fix is available for users writing against v0.8.
- **M103** (`weak-ref!`) is small — land fourth.
- **M104** (`promise!` + `atomic!`) lands the concurrency-forwarding
  primitives — land fifth, after the data types are stable.
- **M105** polish + release.

(End of plan9-modern-types.md)
