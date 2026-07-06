# Plan 14: `duration!` Type (v0.11)

Execution checklist extending the v0.10.0 baseline in `plan13-feature-parity.md`
(M137 polish assumed complete). v0.11 lands a single new value type —
`duration!` — a signed span-of-time scalar that the POC has been missing.
Today the POC folds Red's `time!` into `date!` (per `value.rs:437`), and
`date - date` returns a bare `integer!` day count (`math.rs:465`). v0.11
introduces a proper first-class duration that interoperates with `date!`,
mirrors the "modern general-purpose language" expectation (Rust/Go/Swift all
ship a Duration type), and replaces the stopgap integer-day-count with a
typed value.

Per `project-brief.md`, GUI / `draw` / `vid` / reactive dialects remain
**permanently out of scope**. v0.11 is a **single-type additive release** —
one new `Value` variant, its lexer/parser/mold/convert/arithmetic surface,
and the `date!` integration. No new VM hot-path instrs; every new construct
is additive through the existing `Const`-pool + native-call path.

## What's in scope for v0.11

- **M140 — `duration!` value + lexer + mold.** The new `Value::Duration`
  variant backed by `chrono::Duration` (signed i64 nanoseconds, ~292-year
  range), the unit-suffix float literal (`30s`/`1.5h`/`250ms`/`5m`/`2h`/
  `1d`/`500us`/`100ns`), **compound literals** (`1d1h`/`1h30m45s`/
  `1.5h30m` — strict descending unit order, no repeats, sub-component
  overflow rejected), mold/form, predicates, and `make duration!`/
  `to-duration` constructors.
- **M141 — `date!` ↔ `duration!` arithmetic integration.**
  `date + duration → date`, `date - duration → date`,
  `duration + duration → duration`, `duration - duration → duration`,
  `duration * scalar → duration`, `duration / scalar → duration`.
  **Behavior change:** `date - date → duration!` (replaces today's
  `integer!` day count — see "The date-subtraction transition" below).
- **M142 — Duration accessors + decomposition.** Path access
  (`duration/seconds`, `/nanos`, `/hours`, `/minutes`, `/days`,
  `/total-seconds`), `to-integer`/`to-float` on duration (total seconds),
  `absolute <duration>`, `negate <duration>`.
- **M143 — Polish & v0.11.0 release.**

## Deferred / out of scope

- Calendar durations (`PnYnMnD` — months/years are calendar-bound, not
  fixed-length; cannot be represented as nanoseconds). A future `period!`
  type could cover these; `duration!` is strictly the fixed-length
  physical-time subset (days and below).
- Named timezones (`chrono-tz`) — `plan5` open-q #5, still open.
  `duration!` is zone-agnostic.
- Reactivity, concurrency, full port/async model —
  `future-plan-reactivity.md`, `future-plan-concurrency.md`.
- Other modern types from the `plan9` exploration not yet landed
  (`rope!`/`buffer!`, `grapheme!`, `sorted-map!`, `stream!`, `units!`,
  `enum!`/`sum-type!`, `slice!`/`span!`) — each is a plausible v0.12+
  candidate; none lands here.
- A `wait <duration>` overload — `wait` already accepts `integer!`/
  `float!` seconds (`io.rs:661`); M141 extends `wait` to accept
  `duration!` but no new scheduling semantics.

## Non-goals

- A register VM, JIT, or further perf work — the v0.3.3 VM stays the
  default evaluator.
- New `Instr` variants — `duration!` literals enter via the existing
  `Const`-pool; every duration operation is a native call.
- Behavior changes to existing v0.2–v0.10 features **other than** the
  documented `date - date` transition in M141. The parity contract holds:
  existing golden fixtures (excluding the ones M141 explicitly updates)
  produce byte-identical output under both `Vm` and `force-walk` modes.
- Mixing `duration!` with `integer!`/`float!` in arithmetic
  (`30s + 5` → error; require explicit `to-duration`). `duration!` is
  strict-typed against the bare numerics — only `date!`, `duration!`, and
  scalar `*`/`/` interoperate.

## Ground-truth references (from research)

- `Value` enum lives in `crates/red-core/src/value.rs:241`; after v0.10 it
  has ~36 variants. New `Duration` variant slots after `Date { .. }`
  (value.rs:444) — they're the same conceptual family.
- `type_name` (`crates/red-eval/src/natives/mod.rs:135`) is the single
  `&'static str` switch driving `type?` and error messages — one new arm.
- `Lexer::scan_number` (`lexer.rs:765`) already has the suffix-extension
  pattern: the `%` branch at `lexer.rs:831` turns a digit run into a
  `Percent` token. The `duration!` suffix branch mirrors this exactly
  (digit run + unit-suffix).
- `date_add` / `date_subtract` in `crates/red-eval/src/math.rs:426` /
  `:465` are the existing date-arithmetic dispatchers; M141 extends both.
- `num_binop` (`math.rs:489`) is the cross-numeric dispatcher — M141 does
  **not** route through it (duration is strict-typed, not a `Num`).
- `convert.rs::make_native` (`convert.rs:406`) is the `make` dispatcher;
  `make_money`/`make_percent` (`convert.rs:557`/`:522`) are the closest
  templates. `to-*` converters live alongside.
- `red-core` already depends on `chrono` (`Cargo.toml:22`) —
  `chrono::Duration` is the backing type, **no new crate dep**.
- The POC's `wait` native (`io.rs:661`) takes `integer!`/`float!` seconds
  and calls `std::thread::sleep(Duration::from_secs_f64(secs))`; M141 adds
  a `Value::Duration` arm.
- `printer.rs` mold for `Date` is the closest template for the `Duration`
  mold (numeric + unit assembly).
- `compare.rs::values_equal` is the cross-type equality switch — one new
  arm.
- `DateValue` (`value.rs:1797`) is `NaiveDateTime + Option<i32>`;
  `+ chrono::Duration` is already used internally (`value.rs:1842`,
  `:1881`, `:1905`). M141 exposes this externally.

---

## Milestone 140 — `duration!` value + lexer + mold

The headline milestone. One new `Value` variant, its literal form, mold/form,
predicates, and constructors.

### Dependencies

- [ ] **No new crate dep.** `chrono::Duration` (re-exported from `chrono`
      already in `Cargo.toml:22`) is the backing type. Re-export
      `chrono::Duration` from `red-core/src/lib.rs` alongside the existing
      `NaiveDate`/`NaiveDateTime`/`NaiveTime` re-exports (`lib.rs:27`).

### `Value::Duration` variant

- [ ] Add `Value::Duration { d: chrono::Duration, span: Span }` in
      `value.rs` (immediately after `Date { dt, span }` at line 444 —
      they're the same conceptual family).
- [ ] Add `Value::duration(d: chrono::Duration) -> Value` constructor
      (uses `Span::default()` — most durations are synthetic or
      runtime-constructed).
- [ ] Add `Value::duration_from_nanos(nanos: i128) -> Result<Value, EvalError>`
      — saturating constructor (clamps to `chrono::Duration::max_value()`/
      `min_value()` rather than wrapping; documents the ~292-year range).
- [ ] Document the invariant: `chrono::Duration` is signed; negative
      durations are first-class (distinct from `std::time::Duration`,
      which is unsigned). `chrono::Duration::nanoseconds(i64)` is the
      canonical constructor.
- [ ] Extend `interp_walker.rs` `eval_prefix` self-evaluating arm with
      `Value::Duration { .. }`.
- [ ] Extend `vm/compiler.rs` const-pool arm with `Value::Duration { .. }`.

### Lexer: unit-suffix float literal (single & compound)

The literal form mirrors `percent!`'s suffix model (`lexer.rs:831`): a digit
run immediately followed by a unit suffix becomes a `Duration` token. **Both
single-unit (`30s`) and compound (`1d1h`, `1h30m45s`) forms are accepted.**
Forms accepted:

- `30s` / `1.5s` — seconds (integer or float digit run + `s`)
- `5m` — minutes
- `2h` — hours
- `1d` — days (fixed 86,400 seconds; no calendar days — see "Deferred")
- `250ms` — milliseconds
- `500us` — microseconds
- `100ns` — nanoseconds
- **Compound (strict descending unit order):** `1d1h`, `1h30m45s`,
  `1.5h30m`, `1d2s`, `2h30m`. Units must appear in descending magnitude
  order (`d` > `h` > `m` > `s` > `ms` > `us` > `ns`); `1h1d` is a lex
  error. Repeated units (`1h1h`) are a lex error. Sub-component overflow
  is rejected: a non-leading component's contribution may not equal or
  exceed the next-larger unit's factor (e.g. `1h70m`, `1d25h`, `1m60s`
  are errors — otherwise the no-repeats/descending rule would be
  undermined, since `1h60m` and `1h1h` would have identical sums).
- Negative: `-30s` (the leading `-` is consumed by the main loop's number
  branch, exactly like `-$10.00` in M80 — `scan_money` at `lexer.rs:898`
  documents this pattern; the duration scanner applies the sign itself).
  **The leading sign negates the whole (possibly compound) literal;**
  per-component negatives (`1d-1h`) are not allowed — a `-` after a digit
  run is a delimiter / next-token sign, not a compound continuation.

- [ ] Add `TokenKind::Duration(chrono::Duration)` to the `TokenKind` enum
      (after `Percent`/`Money`).
- [ ] Add a duration-suffix branch in `scan_number` (`lexer.rs:765`),
      **after** the `%` branch (line 831) and **before** the final
      `kind = if is_float { ... }` assembly. The branch is a **component
      loop** (not a single-suffix consume):
  - [ ] Maintain loop state: `total_nanos: i128` (accumulate), `seen:`
        a 7-bit set of already-consumed unit flags, `prev_unit_rank:
        Option<u8>` (the magnitude rank of the last consumed unit, where
        `d=6 > h=5 > m=4 > s=3 > ms=2 > us=1 > ns=0`).
  - [ ] **Per iteration:**
        1. Scan the digit run (int or float) into `f64 magnitude`.
        2. Match the **longest** unit suffix at the cursor: `ms`/`us`/
           `ns` (2 chars) before single-char `s`/`m`/`h`/`d`. If no
           suffix matches, break out of the loop (the digit run is
           emitted as `Integer`/`Float` per the existing assembly, and
           any suffix chars start a fresh Word — this is the collision
           guard).
        3. **Collision guard (commit check):** the suffix is consumed
           only if the char **after** the suffix is a non-word char
           (delimiter, EOF, operator, **or a digit** — a digit signals
           compound continuation, also a valid commit). If the char after
           is a word char (`[A-Za-z_]` and any char that can extend a
           word per `is_delimiter`), do **not** commit this suffix:
           treat the digit run as `Integer`/`Float`, leave the suffix
           chars in the cursor to start a fresh Word. `30stuff` stays
           `Integer 30` + `Word "tuff"`; `30s` becomes `Duration`;
           `30s x` becomes `Duration` + `Word "x"`; `1d1hx` becomes
           `Duration(1d1h)` + `Word "x"` (the second `1h` commits
           because after `h` is a delimiter `x`... wait — `x` is a word
           char. Re-examine: `1d1h` then EOF/delimiter commits; `1d1hx`
           — after the `h` is `x`, a word char, so the `h` suffix is
           **not** committed, and `1d1` lexes as... `1d` commits (next
           char `1` is a digit, valid), then `1` + `hx` → `Integer 1` +
           `Word "hx"`. **Confirm this edge case via test.**)
        4. **Descending check:** if `prev_unit_rank` is `Some(r)` and the
           current unit's rank is `>= r`, emit `InvalidDuration { span,
           chars }` (non-descending or repeat).
        5. **Sub-component overflow check:** if `prev_unit_rank` is
           `Some(r)`, the current component's contribution
           `magnitude * unit_factor[current]` must be strictly less
           than `unit_factor[prev_unit]` (the next-larger unit's
           factor). E.g. after consuming `1h`, the `m` component must
           be `< 60`; `1h70m` → error. Fractional mantissa in a
           non-trailing component also trips this: `1h60.5m` → error
           (60.5m ≥ 1h). **Trailing component exempt** (it has no
           smaller unit to overflow into).
        6. Accumulate: `total_nanos += (magnitude * unit_factor[unit])
           as i128`.
        7. Record `seen[unit] = true`; `prev_unit_rank = Some(rank)`.
        8. Peek the next char: digit → continue loop; delimiter/EOF/
           operator → break and emit `Duration(total_nanos)`; word char
           → impossible here (step 3 already handled it by not
           committing the suffix).
  - [ ] Compute the unit factors (nanoseconds): `s = 1e9`,
        `ms = 1e6`, `us = 1e3`, `ns = 1`, `m = 60e9`, `h = 3600e9`,
        `d = 86400e9`. Convert `total_nanos` (i128) to `i64` via
        `as_i64` (saturating), construct
        `chrono::Duration::nanoseconds(ns)`.
  - [ ] Apply the leading sign: if the caller passed `negative: true`
        (the main loop consumed a leading `-`), negate the duration. Add
        a `negative: bool` parameter to `scan_number` (the main loop's
        number branch already knows whether it consumed a `-` — wire it
        through). The sign applies to the whole accumulated literal.
  - [ ] Error `InvalidDuration { span, chars }` on:
        (a) non-descending or repeated unit (step 4),
        (b) sub-component overflow (step 5),
        (c) parser-level overflow (the i64-nanos cast saturates; if the
        saturation clamps, that's acceptable — the ~292-year range
        covers realistic use; alternatively error on values > ~292
        years; **decision: saturate, document** — matches
        `i64::MAX`-nanos behavior, no error path needed for realistic
        inputs).
- [ ] Extend `Parser`: `TokenKind::Duration(d) => Value::Duration { d, span }`.
      The parser is agnostic to compound-ness — the lexer hands a fully
      accumulated `chrono::Duration` to the token.

### Mold / form

- [ ] Extend `printer.rs` with `mold_duration(d: chrono::Duration, out: &mut String)`.
- [ ] **Mold strategy:** decompose into the largest whole unit, then append
      sub-units. Pick the canonical, reparseable form:
  - [ ] If the duration is whole days → `<N>d` (e.g. `2d`).
  - [ ] Else if whole hours → `<N>h`.
  - [ ] Else if whole minutes → `<N>m`.
  - [ ] Else if whole seconds (no sub-second) → `<N>s`.
  - [ ] Else if whole milliseconds → `<N>ms`.
  - [ ] Else if whole microseconds → `<N>us`.
  - [ ] Else → `<N>ns`.
  - [ ] **Negative:** `-<N>s` (sign on the whole token).
  - [ ] **Mixed decomposition decision:** `90s` vs `1m30s`? **Decision:
        single-unit form only** (mold as `90s`). Rationale: simpler,
        unambiguous reparse, mirrors how `money!` molds a single scalar.
        A future `/long` refinement could emit `1h30m`; deferred.
        **Compound literals are a lexical convenience only — `1d1h`
        molds as `30h`, not `1d6h`.** This preserves the existing
        round-trip contract (value-equal, not text-equal).
  - [ ] **Fractional mantissa:** `1.5h` molds as `1.5h` (not `90m` or
        `1h30m`). The mold picks the unit that the value was constructed
        with? **No — `chrono::Duration` doesn't track the original unit.**
        Mold by magnitude: pick the largest unit where the value is a whole
        multiple, falling back to fractional if needed. `90m` → `90m`
        (whole minutes). `5400s` → `5400s` (whole seconds — but `90m` is
        also whole minutes; pick the **largest** whole unit, so `5400s`
        molds as `90m`). `1.5h = 5400s = 90m` — same value, molds as `90m`.
        **Document:** mold picks the largest unit that yields a whole-number
        representation; fractional mantissa only when no unit yields a whole
        number (e.g. `1.5s = 1500ms` → `1500ms`).
  - [ ] **Round-trip:** `mold` then `reparse` yields the same
        `chrono::Duration` value (value-equal, not necessarily same-unit,
        and **not necessarily text-equal** — a compound literal
        `1d1h` molds as `30h`). Confirm via property test.
- [ ] `form`: same as mold (no separate form).

### Predicates + constructors

- [ ] Add `duration?` predicate native.
- [ ] Add `make duration! <spec>` (`make_duration` in `convert.rs`,
      mirroring `make_money` at line 557):
  - [ ] From `integer!` → duration of N seconds (`make duration! 30` →
        `30s`).
  - [ ] From `float!` → duration of N seconds (fractional;
        `make duration! 1.5` → `1.5s`).
  - [ ] From `string!` → parse the unit-suffix form (`"30s"`, `"1.5h"`,
        `"250ms"`, `"-5m"`, **`"1d1h"` compound**). Error on malformed
        (including non-descending / repeated / sub-component-overflow
        compounds — reuse the same component loop and the same
        `InvalidDuration` errors as the lexer). Leading sign negates the
        whole literal; `"−1d1h"` accepted; per-component negatives
        rejected.
  - [ ] From `block!` → `[h m s]`, `[h m s ms]`, or `[d h m s ms]`
        (positional; missing trailing components default to 0). Reject
        blocks with wrong arity.
  - [ ] From `duration!` → identity.
- [ ] Add `to-duration` converter (alias for `make duration!` for the
      non-block forms; same dispatcher).
- [ ] Register both in `register_conversions` (`convert.rs:1319`).
- [ ] Extend `make_native` (`convert.rs:406`) with the `duration!`
      type-word arm.

### Type-system integration

- [ ] Update `type_name` (`natives/mod.rs:135`) → `"duration!"`.
- [ ] Update `compare.rs::values_equal` with a `Duration` arm (compare by
      `chrono::Duration` numeric value; `=` is by nanos, not by original
      unit — `30s = 30000ms` → true).
- [ ] Update `compare.rs` ordering: durations are totally ordered (by
      nanos); extend `<`/`>`/`<=`/`>=`.
- [ ] Update `types-of`: duration is `[duration!]` (NOT `number!` —
      duration is a scalar, not a numeric; arithmetic is strict-typed).
      **Decision: duration is its own type word, not a `number!`.** Confirm
      — this is the subtle case. Recommendation: not `number!` (matches
      `money!`'s precedent — `money!` is also not `number!` despite
      arithmetic).
- [ ] Update `property.rs` for `Duration` round-trip (mold → parse →
      value-equal).
- [ ] Inline `#[test]`: `30s` lexes to `Duration::nanoseconds(30_000_000_000)`.
- [ ] Inline `#[test]`: `1.5h` lexes to `Duration::nanoseconds(5_400_000_000_000)`.
- [ ] Inline `#[test]`: `250ms` lexes to `Duration::nanoseconds(250_000_000)`.
- [ ] Inline `#[test]`: `100ns` lexes to `Duration::nanoseconds(100)`.
- [ ] Inline `#[test]`: `-30s` lexes to `Duration::nanoseconds(-30_000_000_000)`.
- [ ] Inline `#[test]`: `1d1h` lexes to
        `Duration::nanoseconds((86400 + 3600) * 1_000_000_000)`.
- [ ] Inline `#[test]`: `1d2s` lexes to
        `Duration::nanoseconds((86400 + 2) * 1_000_000_000)`.
- [ ] Inline `#[test]`: `1h30m45s` lexes to
        `Duration::nanoseconds((3600 + 1800 + 45) * 1_000_000_000)`.
- [ ] Inline `#[test]`: `1.5h30m` lexes to
        `Duration::nanoseconds((5400 + 1800) * 1_000_000_000)` (= 2h).
- [ ] Inline `#[test]`: `1h30.5m` lexes to
        `Duration::nanoseconds((3600 + 1830) * 1_000_000_000)` (= 90.5m).
- [ ] Inline `#[test]`: `-1d1h` lexes to
        `Duration::nanoseconds(-((86400 + 3600) * 1_000_000_000))`
        (leading sign negates whole).
- [ ] Inline `#[test]`: `1h1h` → InvalidDuration error (repeated unit).
- [ ] Inline `#[test]`: `1h1d` → InvalidDuration error (non-descending).
- [ ] Inline `#[test]`: `1h70m` → InvalidDuration error (sub-component
        overflow: 70m ≥ 1h).
- [ ] Inline `#[test]`: `1d25h` → InvalidDuration error (25h ≥ 1d).
- [ ] Inline `#[test]`: `1m60s` → InvalidDuration error (60s ≥ 1m).
- [ ] Inline `#[test]`: `1h60.5m` → InvalidDuration error (fractional
        sub-component overflow).
- [ ] Inline `#[test]`: `0d0h` lexes to `Duration::nanoseconds(0)`
        (zero components allowed; molds as `0s`).
- [ ] Inline `#[test]`: `30stuff` lexes as `Integer 30` + `Word "tuff"`
        (collision guard).
- [ ] Inline `#[test]`: `30s x` lexes as `Duration` + `Word "x"`.
- [ ] Inline `#[test]`: `1d1hx` lexes as `Duration(1d1h)` ... wait,
        confirm per the loop step 3: after `1d` commits (next char `1`
        is a digit), the second `1h` sees `x` next — a word char — so
        the `h` is not committed and the run collapses to `Integer 1` +
        `Word "hx"`. **Final expected: `Duration(1d)` + `Integer 1` +
        `Word "hx"`? Or `Integer 1` + `Integer 1` + `Word "hx"`?** The
        latter — when the suffix is not committed, the digit run is
        emitted as a standalone number and the suffix chars start a
        fresh Word. But the first `1d` **did** commit. So:
        `Duration(1d)` + `Integer 1` + `Word "hx"`. Pin this in the
        test.
- [ ] Inline `#[test]`: `mold 30s` → `"30s"`; `mold 90m` → `"90m"`;
        `mold 1.5h` → `"90m"` (largest whole unit);
        `mold 1d1h` → `"30h"` (compound molds single-unit, largest
        whole — round-trip is value-equal, not text-equal).
- [ ] Inline `#[test]`: `30s = 30000ms` → true (value equality across units).
- [ ] Inline `#[test]`: `1d1h = 30h` → true (compound equal to
        single-unit of same value).
- [ ] Inline `#[test]`: `duration? 30s` → true; `duration? 30` → false.
- [ ] Inline `#[test]`: `duration? 1d1h` → true.
- [ ] Inline `#[test]`: `make duration! 30` → `30s`;
        `make duration! "1.5h"` → `90m`; `make duration! [1 30 0]` → `90m`
        (1h 30m 0s); `make duration! "1d1h"` → `30h`
        (compound string accepted by `make`).
- [ ] Inline `#[test]`: `make duration! "not-a-duration"` → error;
        `make duration! "1h1h"` → InvalidDuration (repeated);
        `make duration! "1h70m"` → InvalidDuration (overflow).
- [ ] Add golden fixtures: `duration_literal`, `duration_mold`,
        `duration_construct`, `duration_negative`,
        `duration_compound_literal`, `duration_compound_errors`.

### M140 open questions

1. **`duration!` and `number!` membership.** Plan ships `[duration!]` only
   (not `number!`), mirroring `money!`'s precedent. `30s + 5` errors
   (strict-typed); require `30s + to-duration 5`. Confirm.
2. **Mold: single-unit vs mixed (`90m` vs `1h30m`).** Plan ships
   single-unit (largest whole). Mixed form is a future `/long` refinement.
   **Compound literals (`1d1h`) mold single-unit (`30h`), not as compound
   — the round-trip contract is value-equal, not text-equal.** Confirm.
3. **Saturation vs error on overflow.** `chrono::Duration` caps at ~292
   years. Plan saturates silently. Alternative: error on values exceeding
   the range. Recommendation: saturate (realistic inputs never hit it;
   erroring adds a path for no practical benefit). Confirm.
4. **`d`/`h`/`m`/`s` collision with words.** `2d` alone → duration;
   `2d-array` → integer + word. The suffix is consumed only when followed
   by a non-word char (or a digit, for compound continuation). Confirm the
   guard is sufficient (audit `is_delimiter` for `-`). **Compound edge
   case `1d1hx`: confirm the documented collapse to
   `Duration(1d)` + `Integer 1` + `Word "hx"`** (the second `1h`'s suffix
   is not committed because `x` follows; the digit run `1` is emitted as a
   standalone integer and `hx` starts a fresh Word).
5. **Sub-component overflow rejection.** Reject `1h70m` / `1d25h` /
   `1m60s` (component contribution ≥ next-larger unit factor)?
   Recommendation: **yes** — otherwise the "no repeats, descending
   only" rule is undermined (`1h60m` and `1h1h` would have identical
   sums but opposite accept/reject outcomes). Fractional mantissa in a
   non-trailing component also trips this (`1h60.5m` → error). The
   trailing (smallest) component is exempt. Confirm.
6. **Leading sign vs per-component sign.** Only the leading sign of a
   compound literal is honored; `1d-1h` is two tokens
   (`Duration(1d)` + `Integer -1h`... actually `Integer -1` + `Word "h"`).
   Recommendation: whole-literal sign only. Confirm.
7. **Zero-component compounds (`0d0h`).** Allowed (sums to `0s`,
   molds as `0s`) or rejected as silly? Recommendation: allow (cheap to
   permit; consistent with `make duration! 0` → `0s`). Confirm.

---

## Milestone 141 — `date!` ↔ `duration!` arithmetic integration

The cross-type arithmetic. **Contains the one behavior change in v0.11.**

### `date ± duration → date`

- [ ] Extend `date_add` (`math.rs:426`) with a `Value::Duration` arm:
      `date + duration → date` (apply `dt.dt + duration` via the existing
      `DateValue + chrono::Duration` pattern at `value.rs:1842`/`:1905`;
      preserve the zone).
- [ ] Extend `date_subtract` (`math.rs:465`) with a `Value::Duration` arm:
      `date - duration → date`.
- [ ] Commutative: `duration + date → date` (mirror the
      `integer + date` arm at `math.rs:440`).
- [ ] **Not** commutative for subtraction: `duration - date` → TypeError.

### `duration ± duration → duration`

- [ ] Add a `duration_binop` dispatcher in `math.rs` (sibling to
      `num_binop`/`percent_binop`). Handles `+`/`-`/`*`/`/`:
  - [ ] `duration + duration → duration` (chrono `Duration + Duration`).
  - [ ] `duration - duration → duration`.
  - [ ] `duration * integer → duration` (scaled).
  - [ ] `duration * float → duration` (scaled; saturate on overflow).
  - [ ] `duration / integer → duration`.
  - [ ] `duration / float → duration`.
  - [ ] `duration / duration → float` (the ratio — e.g.
        `90m / 60m → 1.5`). **The one non-duration-producing op** — useful
        for "how many times does this fit". Confirm.
  - [ ] `duration * duration` → TypeError (duration² is meaningless).
- [ ] Add the `duration_binop` dispatch in the main `+`/`-`/`*`/`/`
      native arms (the existing arms check `date_add`/`num_binop`/
      `percent_binop` first; add `duration_binop` alongside).

### The `date - date` transition (behavior change)

Today, `date - date → integer!` (day count, zone-adjusted — `math.rs:465`).
v0.11 changes this to `date - date → duration!` (full nanosecond precision,
zone-adjusted). This is the **one breaking change** in v0.11.

**Rationale:** the integer-day-count was always a stopgap (documented
inline at `math.rs:465`); it loses the time component, so
`2024-06-29/12:00:00 - 2024-06-29/06:00:00` returns `0` (same day) instead
of `6h`. The duration form is strictly more correct.

- [ ] Update `date_subtract` (`math.rs:465`): for `date - date`, compute
      `a_utc - b_utc` as a `chrono::Duration` (full precision,
      zone-adjusted) and return `Value::Duration`.
- [ ] **Migration path for existing code:** `to-integer date - date` still
      yields the day count (M142 adds a `to-integer` arm on `duration!`
      that returns total seconds truncated — for day count,
      `to-integer (date - date) / 86400`). Document the migration in the
      release notes.
- [ ] **Golden fixture updates:** audit `red-eval/tests/programs/` for any
      fixture using `date - date` and expecting an `integer!`. Update each
      to expect a `duration!` (molded as e.g. `6h` or `0s` for same-day).
      **Decision: update in place** — no deprecation period, no feature
      flag. The POC's audience is small; a clean break is simpler than
      carrying a flag. Confirm this is acceptable (it's the one
      parity-contract exception in v0.11).
- [ ] Add `programs_errors/date_date_sub_type.red` documenting the new
      return type.

### `wait` extension

- [ ] Extend `wait` (`io.rs:661`) with a `Value::Duration` arm:
      `wait 30s` sleeps for the duration (convert via
      `chrono::Duration::to_std` — handle negative by treating as 0,
      mirroring the existing `if secs > 0.0` guard).

### Tests

- [ ] Inline `#[test]`: `2024-06-29/12:00:00 + 1h` → `2024-06-29/13:00:00`.
- [ ] Inline `#[test]`: `2024-06-29/12:00:00 + 1d1h` → `2024-06-30/13:00:00`
      (compound duration through `date_add`).
- [ ] Inline `#[test]`: `2024-06-29/12:00:00 - 30m` → `2024-06-29/11:30:00`.
- [ ] Inline `#[test]`: `30s + 1m` → `90s` (molds as `90s`).
- [ ] Inline `#[test]`: `1h - 30m` → `30m`.
- [ ] Inline `#[test]`: `30s * 2` → `60s`.
- [ ] Inline `#[test]`: `90m / 60m` → `1.5` (float ratio).
- [ ] Inline `#[test]`: `30s + 5` → TypeError (strict-typed).
- [ ] Inline `#[test]`: `2024-06-29/12:00:00 - 2024-06-29/06:00:00` → `6h`
      (the headline correctness fix).
- [ ] Inline `#[test]`: `2024-06-29 - 2024-06-28` → `1d` (date-only
      subtraction preserves day granularity).
- [ ] Inline `#[test]`: `wait 10ms` returns `none` (no-op for testing; the
      sleep is real but trivial).
- [ ] Add golden fixtures: `duration_date_add`, `duration_date_sub`,
      `duration_arith`, `duration_date_date_sub` (the transition fixture),
      `duration_compound_date_add` (compound-through-date_add coverage).
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M141 open questions

1. **`date - date` break vs feature flag.** Plan ships the clean break
   (no flag). Alternative: gate behind `--duration-date-sub` for one
   release cycle. Recommendation: clean break (small audience, stopgap was
   always documented as such). Confirm.
2. **`duration / duration → float`.** Useful ratio or type-confusing?
   Recommendation: keep (the "how many times does X fit in Y" use case is
   real; e.g. `total-time / expected-time → progress ratio`). Confirm.
3. **`duration * duration`.** Plan errors. Confirm (duration² is
   meaningless).
4. **`wait <duration>` with negative.** Plan treats as 0 (no sleep).
   Alternative: error. Recommendation: 0 (mirrors existing
   `if secs > 0.0` guard). Confirm.

---

## Milestone 142 — Duration accessors + decomposition

Path access and decomposition natives for inspecting durations.

- [ ] Extend path evaluation (`interp_walker.rs`/`vm/compiler.rs`
      path-resolution arms) with `Value::Duration` accessors:
  - [ ] `duration/seconds` → `integer!` (whole-second component, signed;
        e.g. `90s/seconds` → `90`, `1m30s/seconds` → `30` — the
        seconds-of-the-minute, not total). **Decision: total or
        component?** See open-q below.
  - [ ] `duration/total-seconds` → `float!` (total as `f64`; e.g.
        `90s/total-seconds` → `90.0`).
  - [ ] `duration/nanos` → `integer!` (sub-second nanosecond component,
        0–999,999,999).
  - [ ] `duration/hours` → `integer!` (hour component, signed).
  - [ ] `duration/minutes` → `integer!` (minute-of-hour component, 0–59).
  - [ ] `duration/days` → `integer!` (day component).
- [ ] **Component vs total decision:** `/seconds`, `/minutes`, `/hours`,
      `/days` are **components** (decomposed; e.g. `1h30m/seconds` → `30`,
      `1h30m/minutes` → `30`, `1h30m/hours` → `1`). `/total-seconds` is
      the **total** (float). This mirrors how `date!/hour`,
      `date!/minute`, `date!/second` work (components). Confirm.
- [ ] Add `to-integer <duration>` (in `convert.rs::to_integer`): total
      seconds, truncated (e.g. `to-integer 90s` → `90`;
      `to-integer 1.5s` → `1`). Documents the `date - date` migration
      path.
- [ ] Add `to-float <duration>` (in `to_float`): total seconds as `f64`.
- [ ] Add `absolute <duration>` → `duration!` (the magnitude; mirrors
      `absolute` on numbers). **If `absolute` already exists** (check
      `math.rs` for an existing `absolute` native — plan13 M133 mentions
      `square-root`/`absolute` aliases), extend it with a `Duration` arm.
- [ ] Add `negate <duration>` → `duration!` (negation). **If `negate`
      exists** (check — likely a stdlib function), extend or promote to
      native with a `Duration` arm.
- [ ] No `series!` semantics — duration is a scalar (not indexable, not
      sliceable).
- [ ] Inline `#[test]`: `90s/total-seconds` → `90.0`.
- [ ] Inline `#[test]`: `(make duration! "1d1h")/total-seconds` → `90000.0`
      (compound input — total seconds as f64; documents that components
      are decomposed off the canonical `chrono::Duration`, not the
      lexical form).
- [ ] Inline `#[test]`: `(make duration! [1 30 0])/hours` → `1`;
      `/minutes` → `30`; `/seconds` → `0`.
- [ ] Inline `#[test]`: `(make duration! "1d1h")/hours` → `1`
      (component, not total — `1d1h` decomposes to `1` day + `1` hour,
      so `/hours` is `1`, not `25`); `/days` → `1`.
- [ ] Inline `#[test]`: `to-integer 90s` → `90`; `to-integer 1.5s` → `1`.
- [ ] Inline `#[test]`: `to-float 1.5s` → `1.5`.
- [ ] Inline `#[test]`: `absolute -30s` → `30s`; `negate 30s` → `-30s`.
- [ ] Add golden fixtures: `duration_accessors`, `duration_convert`.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M142 open questions

1. **Component vs total for `/seconds` etc.** Plan ships components
   (mirrors `date!/hour`). Alternative: `/seconds` = total seconds
   (integer). Recommendation: components, with `/total-seconds` for the
   total. Confirm.
2. **`absolute`/`negate` — extend existing or new?** If plan13 M133 lands
   `absolute` as a native (it's listed), M142 extends it. If M133 doesn't
   ship before M142, M142 creates the native. Coordinate ordering with
   plan13. Confirm.

---

## Milestone 143 — Polish & v0.11.0 release

- [ ] Audit `EvalError` rendering for new error sources:
  - [ ] `InvalidDuration` (M140 lexer error — malformed unit suffix).
  - [ ] TypeError messages for strict-typed duration arithmetic
        (`30s + 5` → "expected duration!, found integer!").
- [ ] Add spans to the `Duration` variant (source-origin for literals;
      `Span::default()` for synthetic/runtime-constructed). Confirm the
      lexer wires the byte-offset span through `scan_number` →
      `TokenKind::Duration` → parser.
- [ ] Golden fixture per new error case
      (`programs_errors/duration_strict_arith.red`,
      `programs_errors/duration_bad_literal.red`,
      `programs_errors/duration_bad_block.red`,
      `programs_errors/duration_compound_bad.red`
      (non-descending / repeated / sub-component-overflow)).
- [ ] Property test: extend `mold(parse(mold(v)))` to cover `Duration`
      (the mold form is reparseable by design).
- [ ] Extend `red-core/tests/golden/` to cover the duration literal.
- [ ] Expand `red-eval/tests/programs/` to 10+ new duration fixtures
      (literal, mold, arith, date integration, accessors, error cases).
- [ ] Run `cargo bench --bench eval`; record in `BENCHMARKS.md` under
      "v0.11.0".
  - [ ] Expected neutral on existing benches (the `date - date` path
        changes return type but not hot-path cost; the duration suffix in
        `scan_number` is a single peek after the existing digit run).
  - [ ] Add a `duration_arith` bench (Duration + Duration) — expected
        comparable to `num_binop` (chrono `Duration + Duration` is a single
        i64 add).
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`; fix.
- [ ] Run `cargo fmt --all --check`; fix.
- [ ] Update `project-brief.md`:
  - [ ] Add a "`duration!` (v0.11)" subsection under "Value model": the
        new variant, the `chrono::Duration` backing (no new dep), the
        unit-suffix literal (single **and compound** — `30s` and `1d1h`),
        the compound rules (strict descending, no repeats, sub-component
        overflow rejected), the `date - date` behavior change.
  - [ ] Update the value-model code block (add `Duration`).
  - [ ] Update "Deferred" — add the calendar-`period!` candidate.
- [ ] Update `architecture.md`:
  - [ ] New `Duration` variant in the value-model section.
  - [ ] The unit-suffix lexer branch (mirrors `percent!`'s pattern);
        **the compound component loop** (descending/repeat/overflow
        guards).
  - [ ] The `date_add`/`date_subtract` extension and the
        `date - date → duration!` transition.
  - [ ] The `duration_binop` dispatcher.
- [ ] Update `README.md`:
  - [ ] Bump version to v0.11.0.
  - [ ] Add `duration!` to the "Value types" list.
  - [ ] Add `duration?` to the type predicates list.
  - [ ] Add `to-duration`/`make duration!` to the conversions list.
  - [ ] Note the `date - date → duration!` behavior change (with the
        `to-integer` migration path).
  - [ ] Update "Known gaps" with the calendar-`period!` deferral.
- [ ] Final `cargo test --workspace` green.
- [ ] Final `cargo test --workspace --features force-walk` green.
- [ ] Final `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] Tag release `v0.11.0`.

---

## Open questions (plan-wide)

1. **`duration!` and `number!` membership** (M140 #1). Recommendation: not
   `number!` (strict-typed, mirrors `money!`).
2. **Mold: single-unit vs mixed** (M140 #2). Recommendation: single-unit,
   largest-whole. **Compound literals mold single-unit (`1d1h` → `30h`);
   round-trip is value-equal, not text-equal.**
3. **Saturation on overflow** (M140 #3). Recommendation: saturate silently.
4. **`d`/`h`/`m`/`s` word-collision guard** (M140 #4). Recommendation:
   suffix consumed only when followed by a non-word char (or a digit, for
   compound continuation). Confirm the `1d1hx` collapse behavior.
5. **Sub-component overflow rejection** (M140 #5). Recommendation: reject
   `1h70m` / `1d25h` / `1m60s` / `1h60.5m` (component ≥ parent factor;
   trailing component exempt).
6. **Compound literal sign** (M140 #6). Recommendation: leading sign
   negates the whole literal; per-component negatives rejected.
7. **Zero-component compounds** (M140 #7). Recommendation: allow
   (`0d0h` → `0s`).
8. **`date - date` break vs feature flag** (M141 #1). Recommendation: clean
   break (small audience).
9. **`duration / duration → float`** (M141 #2). Recommendation: keep
   (progress-ratio use case).
10. **`duration * duration`** (M141 #3). Recommendation: error.
11. **`wait <duration>` negative** (M141 #4). Recommendation: treat as 0.
12. **Component vs total for `/seconds`** (M142 #1). Recommendation:
    components; `/total-seconds` for the total. **Compound inputs decompose
    off the canonical `chrono::Duration` (e.g. `1d1h/days` → `1`,
    `1d1h/hours` → `1`, not `25`).**
13. **`absolute`/`negate` ordering with plan13 M133** (M142 #2).
    Coordinate: if M133 lands first, M142 extends; else M142 creates.
14. **`make duration! [d h m s ms]` block arity.** Plan accepts
    `[h m s]`, `[h m s ms]`, `[d h m s ms]`. Missing trailing components
    default to 0. Reject `[d]` alone (ambiguous — is it days or hours?).
    Recommendation: require at least 3 elements, or accept `[d]`/`[h]`/
    `[m]`/`[s]`/`[ms]` single-element forms. **Decision: accept
    single-element `[N]` interpreted as seconds (mirrors
    `make duration! <integer>`); require multi-element forms to be the
    documented arities.** Confirm.
