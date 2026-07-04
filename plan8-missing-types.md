# Plan 8: Missing Value Types (v0.7)

Execution checklist extending the v0.5.0 baseline in `plan6-closures-modules.md`
(M65 polish assumed complete) and the v0.6.0 baseline in `plan7-package-manager.md`
(M74 polish assumed complete). v0.7 closes the **remaining type-gap** between
the POC's `Value` enum and the Red/Rebol value type inventory by landing every
missing variant the user-supplied canonical list calls out, plus `regex!`
(already documented as a gap).

Per `project-brief.md`, GUI / `draw` / `vid` / reactive dialects remain
**permanently out of scope**. v0.7 is a **type-completeness release**, in the
spirit of v0.4 (plan5) but smaller: it lands ten new value types and their
end-to-end scaffolding (lexer → parser → mold/form → walker arm → VM const-pool
→ predicates → converters → golden fixtures). No new VM hot-path instrs; every
new construct is additive through the existing `Const`-pool + native-call path.

## Deferred to v0.8+ (acknowledged, not built here)

- Reactivity (`object!` `on-change` slots — `future-plan-reactivity.md`).
- Concurrency (`Value::Channel` + actor model — `future-plan-concurrency.md`).
- Full port model (a real `port!` I/O abstraction backed by `Channel` — deferred
  to the post-concurrency release).
- Shared-cell closures (SetWord capture) — `plan6` open-q #1.
- `unimport` — `plan6` M62.
- Named timezones (`chrono-tz`) — `plan5` open-q #5.
- Advanced `bitset!`/`logic!` ops beyond membership.
- A central package registry server — `plan7` ships git/path sources only.

## What's in scope for v0.7

Ten new `Value` variants, grouped by risk:

- **M80 — Easy four (lexer rule + thin variant):** `percent!`, `money!`,
  `issue!`, `email!`. All are source-origin scalars with a small lexical form
  and trivial mold/form. Land first; unblocks no other work.
- **M81 — `tag!`:** HTML/XML-style tag literal. Standalone lexer rule; no
  collisions (delimiters `<`/`>` are reserved today).
- **M82 — `regex!`:** a compiled-regex value backed by the `regex` crate.
  First new runtime dep since `chrono`/`indexmap`. Powers a future `parse`
  extension and a `regex!`-as-`parse`-rule form.
- **M83 — `hash!`:** an insert-ordered key→value table backed by a real hash
  map (not `indexmap`). Distinct from `map!` in iteration order semantics and
  in being a `series!` (indexable, sliceable) — see "hash! vs map!" below.
- **M84 — `vector!`:** a packed numeric vector (`i8`/`i16`/`i32`/`i64`/`f32`/
  `f64` element kind). The first "container with a typed payload" type.
- **M85 — `image!`:** a 2D pixel buffer (RGBA8). Heavy by itself; lands last
  among the data types because it overlaps conceptually with `vector!` (both
  are packed-array types). No GUI/draw — pure data.
- **M86 — `unset!`:** a distinct "no value" sentinel, separate from `none!`.
  Touches the binding/eval model: unbound words can now optionally evaluate to
  `unset!` rather than error. The one milestone that is **not purely additive**
  — see "unset! semantics" below.
- **M87 — `native!` / `op!` split:** promote the existing `FuncDef.native` /
  `FuncDef.infix` flags into distinct `Value` variants (or keep as flags —
  **decision: flags stay, but `type?`/`native?`/`op?` predicates report them as
  distinct types** — see "native!/op! decision" below).
- **M88 — `struct!` + `handle!`:** FFI-adjacent opaque types. Land together
  because `struct!` fields can be `handle!`. Forward-looking for v0.8 FFI work
  (the `routine!` design from `plan7`'s "Relationship to `routine!`" section);
  v0.7 ships only the value shapes + mold + predicates, not the binding layer.
- **M89 — `typeset!`:** a value representing a set of types. Used in function
  spec blocks (`func [x [integer! float!]]`) for runtime type-checking. Today
  the spec block stores bare `Word`s with no check; v0.7 adds the value type
  and the `typeset?` predicate, and **optionally** wires it into `func`
  spec-eval (decision: wire it — see "typeset! scope" below).
- **M90 — Polish & v0.7.0 release.**

## Non-goals

- A register VM, JIT, or further perf work — the v0.3.3 VM stays the default.
- New `Instr` variants unless a construct provably cannot be a native call
  (none of M80–M89 require one — every new literal enters via the `Const`-pool,
  every new constructor is a `make` native, every new predicate is a native).
- Behavior changes to existing v0.2–v0.6 features **other than the `unset!`
  fallback documented in M86**. The parity contract holds.
- Lexer disambiguation changes that break existing golden fixtures — every
  new literal form is a *new* leading-character dispatch (`<` for `tag!`, `$`
  for `money!`, `#`-non-`{`/`"` for `issue!`, a digit-run-then-`@` for
  `email!`, a digit-run-then-`%` for `percent!`).

## Ground-truth references (from research)

- `Value` enum lives in `crates/red-core/src/value.rs:241`; currently 24
  variants (incl. `Module`/`Closure` from v0.5).
- `type_name` (`crates/red-eval/src/natives/mod.rs:134`) is the single
  `&'static str` switch driving `type?` and error messages. New variants add
  arms here.
- Lexer dispatch lives in `crates/red-core/src/lexer.rs`; the main scan loop
  keys off the first byte. Current `#` dispatch: `#"..."` → `Char`, `#{...}`
  → `Binary`. M80's `issue!` form `#XYZ` (any non-`"`/`{` after `#`) is the
  **one** disambiguation case to handle carefully.
- `printer.rs` mold arms are an exhaustive `match Value`; every new variant
  needs `mold` + `form` arms (property test gates on round-trip for
  reparseable variants — see M90).
- `vm/compiler.rs:630` (approx) is the const-fold match for `Value` →
  `Instr::Const(idx)`. Every new source-origin variant needs an arm.
- `interp_walker.rs` `eval_prefix` self-evaluating arm: source-origin scalars
  (`Char`/`Pair`/`Tuple`/`Date`/`String8`) return themselves. New scalars
  follow the same pattern.
- `natives/words.rs` holds the type-predicate block; one `match` per
  predicate family. New predicates are one-line arms.
- `convert.rs` `to-*` and the `make` dispatcher (`convert.rs::make_value`)
  need arms per new type.
- `compare.rs::values_equal` is the cross-type equality switch; new variants
  need arms (value types compare by contents; reference types by `Rc::ptr_eq`
  for `same?`, deep-equal for `=`).
- `red-core/tests/property.rs` excludes non-reparseable variants from the
  mold→parse→mold proptest. New variants are added to either the round-trip
  set or the "stable-string" set (like `#[function]`/`#[closure]`).
- `FuncDef.native: Option<NativeFn>` and `FuncDef.infix: bool`
  (`value.rs:155` area) — the flags M87 promotes to a type distinction.
- `FuncDef.params: Vec<Symbol>` (`value.rs`) — currently stores param *names*
  only, no types. M89's `typeset!` integration adds an optional
  `param_types: Vec<Option<TypesetDef>>` parallel vec.

---

## Milestone 80 — Easy four: `percent!` / `money!` / `issue!` / `email!`

The "no surprises" milestone. Four scalar source-origin literals, each with a
single lexer rule, a thin variant, trivial mold, and one predicate. Land first
to establish the M80–M89 template and prove the build/test gates still close
after v0.6.

### `percent!`

A `Float`-backed percentage: `50%` = 0.5 internally, molds back as `50%`.

- [x] Add `Value::Percent { value: f64, span: Span }` in
      `crates/red-core/src/value.rs` (after `Float` — they share `f64`).
- [x] Add `Value::percent(f) -> Value` constructor (rounds to 6 sig figs on
      mold to match Red's printing; the stored value is the exact float).
- [x] Extend `Lexer` (`crates/red-core/src/lexer.rs`):
  - [x] In `scan_number`, when a digit run is immediately followed by `%`,
        emit `TokenKind::Percent(parsed / 100.0)` and consume the `%`. (No
        conflict: bare `%` is the file-literal lead; a digit-run-then-`%` is
        unambiguous because `%`-files don't follow digits.)
  - [x] Error `InvalidPercent` on overflow (`f64::infinity`).
- [x] Extend `Parser`: `TokenKind::Percent(f) => Value::Percent { value: f, span }`.
- [x] Extend `printer.rs`:
  - [x] `mold`: `format!("{:.6}", value * 100.0).trim_end_matches('0')
        .trim_end_matches('.').to_string() + "%"`.
  - [x] `form`: same as mold (Red parity — `form` of `percent!` is the
        printed percent form).
- [x] Extend `interp_walker.rs` `eval_prefix` self-evaluating arm with
      `Value::Percent { .. } => Ok(v.clone())`.
- [x] Extend `vm/compiler.rs` const-pool arm with `Value::Percent { .. }`.
- [x] Add `percent?` predicate in `natives/words.rs`.
- [x] Add `to-percent` converter (from float → percent; from integer →
        percent; from string parse `"50%"`).
- [x] Add `make percent! <value>` to the `make` dispatcher (float/int/string
        as above).
- [x] Arithmetic: `percent + percent` → percent; `percent + float` → float
        (percent promotes to its float value); `percent * float` → float.
        Add arms in `math.rs` `as_number`/promotion helpers.
- [x] Update `type_name` (`natives/mod.rs:134`) → `"percent!"`.
- [x] Update `compare.rs::values_equal` with a `Percent` arm (compare `value`
        field).
- [x] Inline `#[test]`: `50%` lexes to `Percent { value: 0.5 }`.
- [x] Inline `#[test]`: `mold 50%` → `"50%"`; `mold 0.5%` → `"0.5%"`.
- [x] Inline `#[test]`: `50% + 25%` → `75%`; `50% * 2` → `1.0` (float).
- [x] Inline `#[test]`: `percent? 50%` → true; `percent? 0.5` → false.
- [x] Add golden fixtures: `percent_literal`, `percent_arith`, `percent_convert`.
- [x] Update `property.rs` to include `Percent` in the round-trip proptest.

### `money!`

A fixed-point decimal currency type: `$10.00`, `$1,234.56` (commas optional,
stripped on lex). Stored as integer cents (i64) plus a currency-code string
(default `"USD"`). No floating-point — exact arithmetic.

- [x] Add `struct MoneyValue { cents: i64, currency: Rc<str> }` in `value.rs`.
- [x] Add `Value::Money { amount: Rc<MoneyValue>, span: Span }` variant.
- [x] Add `Value::money(cents, currency)` constructor.
- [x] Extend `Lexer`:
  - [x] `scan_money` on `$` lead (today `$` is not a word-start char —
        verify; if it is, this is the only collision and the rule wins by
        order). Accept `$<digits>` and `$<digits>.<digits>` and an optional
        3-letter currency suffix `:$USD` (Red form: `$10.00:USD`).
  - [x] Strip commas between digit groups (`$1,234.56` → 123456 cents).
  - [x] Error `InvalidMoney` on malformed forms.
- [x] Extend `Parser`: `TokenKind::Money(MoneyValue) => Value::Money { ... }`.
- [x] Extend `printer.rs`:
  - [x] `mold`: `$10.00` (always two decimal places); with currency suffix
        if non-USD: `$10.00:EUR`.
  - [x] `form`: same as mold.
- [x] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [x] Add `money?` predicate.
- [x] Add `to-money` converter (from integer cents, from string parse, from
        float — float rounds to nearest cent with banker's rounding).
- [x] Add `make money! <value>` (int → cents; string parse; float rounds).
- [x] Arithmetic: `money + money` (same currency only — error on mismatch);
        `money + integer` → money (treat int as cents); `money * integer` →
        money; `money / money` → float (ratio). Add `math.rs` arms.
- [x] Comparison: `= <> < >` compare by cents; cross-currency errors.
- [x] Update `type_name` → `"money!"`.
- [x] Update `compare.rs` with a `Money` arm.
- [x] Inline `#[test]`: `$10.00` lexes to `Money { cents: 1000, "USD" }`.
- [x] Inline `#[test]`: `$1,234.56` → `123456` cents (commas stripped).
- [x] Inline `#[test]`: `$10.00 + $5.00` → `$15.00`; cross-currency errors.
- [x] Inline `#[test]`: `mold $10.00:EUR` → `"$10.00:EUR"`.
- [x] Add golden fixtures: `money_literal`, `money_arith`, `money_currency`.
- [x] Add `programs_errors/money_currency_mismatch.red`.
- [x] Update `property.rs` for `Money` round-trip.

### `issue!`

A short identifier literal: `#1234`, `#ABC`, `#FF00` (any run of non-delimiter
chars after `#` that isn't `"` (char) or `{` (binary)). Stored as a `Rc<str>`.
Distinct from `binary!` (`#{hex}`) and `char!` (`#"x"`).

- [x] Add `Value::Issue { s: Rc<str>, span: Span }` variant.
- [x] Add `Value::issue(s)` constructor (validates: non-empty, no whitespace).
- [x] Extend `Lexer`:
  - [x] In the `#`-dispatch branch, after `#"` → Char and `#{` → Binary,
        fall through to `scan_issue`: consume a run of word-chars (letters,
        digits, `-`, `_`, `.`, `?`, `!`) → `TokenKind::Issue(s)`.
  - [x] Error `InvalidIssue` on `#` followed by whitespace or delimiter.
        (This is the one M80 disambiguation case — confirm no existing
        fixture starts a word with `#` other than the two known forms; the
        `natives/mod.rs` `type_name` switch confirms none of the existing
        `Value` arms collide.)
- [x] Extend `Parser`: `TokenKind::Issue(s) => Value::Issue { s, span }`.
- [x] Extend `printer.rs`:
  - [x] `mold`: `"#" + s` (no quoting — issue chars are non-delimiter).
  - [x] `form`: same as mold.
- [x] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [x] Add `issue?` predicate.
- [x] Add `to-issue` converter (from string, from integer → `#<decimal>`).
- [x] Add `make issue! <value>` (string; integer → `#<n>`; block of ints →
        `#<concat>`).
- [x] Equality/ordering by string compare.
- [x] Update `type_name` → `"issue!"`.
- [x] Update `compare.rs` with an `Issue` arm.
- [x] Inline `#[test]`: `#1234` lexes to `Issue("1234")`.
- [x] Inline `#[test]`: `#ABC` lexes to `Issue("ABC")`.
- [x] Inline `#[test]`: `#"a"` still lexes to `Char` (regression guard).
- [x] Inline `#[test]`: `#{00FF}` still lexes to `Binary` (regression guard).
- [x] Inline `#[test]`: `mold #ABC` → `"#ABC"`.
- [x] Add golden fixtures: `issue_literal`, `issue_convert`.
- [x] Add `programs_errors/issue_bad_form.red` (e.g. `# ` with space).
- [x] Update `property.rs` for `Issue` round-trip.

### `email!`

An `user@host` literal: `foo@bar.com`. Stored as a `Rc<str>` (the whole
address). The lexer detects a word run containing a single `@` with dots on
the host side.

- [x] Add `Value::Email { addr: Rc<str>, span: Span }` variant.
- [x] Add `Value::email(addr)` constructor (validates: one `@`, non-empty
      local, non-empty host with at least one dot — Red parity; bare
      `user@localhost` is **not** an email! in Red, it's two words).
- [x] Extend `Lexer`:
  - [x] In the word-scan run, detect `@` mid-run: if the run matches
        `<word-chars>@<word-chars>.<word-chars>`, emit
        `TokenKind::Email(s)`. Otherwise, `@` ends the word (today `@` is a
        delimiter — confirm; if not, this rule wins by order).
  - [x] Error `InvalidEmail` on `@` with no dot in the host portion.
- [x] Extend `Parser`: `TokenKind::Email(s) => Value::Email { addr: s, span }`.
- [x] Extend `printer.rs`:
  - [x] `mold`: the raw address (no quoting).
  - [x] `form`: same as mold.
- [x] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [x] Add `email?` predicate.
- [x] Add `to-email` converter (from string parse, from block `[user host]`).
- [x] Add `make email! <value>`.
- [x] Path access: `email/user` → local part string; `email/host` → host
        part string (Red parity — `email!` is pathable).
- [x] Update `type_name` → `"email!"`.
- [x] Update `compare.rs` with an `Email` arm.
- [x] Inline `#[test]`: `foo@bar.com` lexes to `Email("foo@bar.com")`.
- [x] Inline `#[test]`: `user@localhost` lexes to two words (regression
        guard — bare host without a dot is not an email!).
- [x] Inline `#[test]`: `mold foo@bar.com` → `"foo@bar.com"`.
- [x] Inline `#[test]`: `foo@bar.com/user` → `"foo"`.
- [x] Add golden fixtures: `email_literal`, `email_paths`.
- [x] Add `programs_errors/email_bad_form.red` (e.g. `@bar.com`, `foo@`).
- [x] Update `property.rs` for `Email` round-trip.

### M80 closeout

- [x] `cargo test --workspace` green; `--features force-walk` green.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [x] `cargo fmt --all --check` clean.

---

## Milestone 81 — `tag!`

HTML/XML-style tag literal: `<b>`, `<img src="x">`, `</p>`, `<br/>`. Stored as
a `Rc<str>` (the raw tag text between `<` and `>`). Standalone lexer rule; `<`
and `>` are not used by any existing literal today (confirm by grepping the
lexer for `<`/`>` first-class handling — they appear only as comparison
operators, which are word-tokens, not leading-char dispatch).

- [x] Add `Value::Tag { text: Rc<str>, span: Span }` variant.
- [x] Add `Value::tag(text)` constructor.
- [x] Extend `Lexer`:
  - [x] `scan_tag` on `<` lead: consume to the matching `>` (no nesting —
        Red's `tag!` is a single tag, not a tree). Honor backslash escapes for
        `\<`/`\>` inside the tag (Red behavior). Emit `TokenKind::Tag(s)`.
  - [x] Error `UnterminatedTag` on EOF before `>`.
  - [x] Disambiguation: `<` followed by space or operator char (`=`/`<`/`>`)
        is the comparison operator, not a tag. The rule: `<` followed by a
        non-space, non-operator char starts a tag; else it's the operator
        (today's behavior). **Confirm** no existing fixture breaks — the
        parity harness gates this.
- [x] Extend `Parser`: `TokenKind::Tag(s) => Value::Tag { text: s, span }`.
- [x] Extend `printer.rs`:
  - [x] `mold`: `"<" + text + ">"`.
  - [x] `form`: same as mold.
- [x] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [x] Add `tag?` predicate.
- [x] Add `to-tag` converter (from string → `<string>`; from block →
        `<word args>`).
- [x] Add `make tag! <value>`.
- [x] Series semantics: `tag!` is **not** a `series!` in Red (it's a scalar);
        `length?`/`pick` don't apply. Confirm and document.
- [x] Update `type_name` → `"tag!"`.
- [x] Update `compare.rs` with a `Tag` arm (string compare on `text`).
- [x] Inline `#[test]`: `<b>` lexes to `Tag("b")`.
- [x] Inline `#[test]`: `<img src="x">` lexes to `Tag("img src=\"x\"")`.
- [x] Inline `#[test]`: `</p>` lexes to `Tag("/p")`.
- [x] Inline `#[test]`: `< 5` lexes to two tokens (operator + integer) —
        regression guard.
- [x] Inline `#[test]`: `mold <b>` → `"<b>"`.
- [x] Add golden fixtures: `tag_literal`, `tag_convert`.
- [x] Add `programs_errors/tag_unterminated.red`.
- [x] Update `property.rs` for `Tag` round-trip.
- [x] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 82 — `regex!`

A compiled regular expression value. First new runtime dep in `red-core` since
`chrono`/`indexmap` (M43/M45). Powers (a) a future `parse` extension
(`regex!` as a rule matching a substring), (b) `find`/`replace` with `/regex`
refinement, (c) `regex?` predicate.

- [ ] Add `regex = "1"` to `crates/red-core/Cargo.toml [dependencies]`.
- [ ] Add `struct RegexDef { re: regex::Regex, source: Rc<str> }` in `value.rs`
      (keep the source for mold round-trip — `regex::Regex` doesn't store it).
- [ ] Add `Value::Regex(Rc<RegexDef>)` variant (synthetic — no span; built by
      `make regex!`/`to-regex` at runtime, not by the lexer).
- [ ] Add `Value::regex(source)` constructor (compiles via `regex::Regex::new`;
      error on invalid pattern → `EvalError::Native`).
- [ ] Extend `printer.rs`:
  - [ ] `mold`: `#{regex}...{regex}` — **decision: a synthetic mold form**
        `#[regex "..."]` (non-reparseable, matches the `#[function]`/
        `#[closure]` placeholder style). Round-trip is *not* required for
        synthetic values; the property test gets a stable-string assertion
        instead (see M90).
  - [ ] `form`: same as mold.
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [ ] Add `regex?` predicate.
- [ ] Add `to-regex` converter (from string compile).
- [ ] Add `make regex! <string>` (compile).
- [ ] Implement matching natives:
  - [ ] `match? regex value` → logic (full-match).
  - [ ] `find/regex series regex` → position or none.
  - [ ] `replace/regex string regex replacement` → string (count-limited or
        `/all`).
- [ ] Future (deferred to v0.8): `regex!` as a `parse` rule (matches a
        substring, advances cursor by the match length). **Not in v0.7** —
        noted here for the design continuity.
- [ ] Update `type_name` → `"regex!"`.
- [ ] Update `compare.rs` with a `Regex` arm (compare by `source` string —
        two regexes are equal iff their patterns are byte-identical;
        compilation artifacts don't compare).
- [ ] Inline `#[test]`: `make regex! "a.b"` returns a `Regex` value.
- [ ] Inline `#[test]`: `match? (make regex! "a.b") "axb"` → true.
- [ ] Inline `#[test]`: `match? (make regex! "a.b") "axxb"` → false (no
        full-match).
- [ ] Inline `#[test]`: `replace/regex "a1b2" (make regex! "[0-9]") "X"` →
        `"aXbX"` (with `/all`).
- [ ] Inline `#[test]`: `regex? make regex! ""` → true; `regex? "..."` → false.
- [ ] Inline `#[test]`: `mold (make regex! "a.b")` → `"#[regex \"a.b\"]"`.
- [ ] Add golden fixtures: `regex_construct`, `regex_match`, `regex_replace`.
- [ ] Add `programs_errors/regex_bad_pattern.red` (e.g. `make regex! "(a"`)
- [ ] Add a stable-string property test (not round-trip) for `Regex`.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 83 — `hash!`

An insert-ordered key→value table backed by a real `HashMap` (not `indexmap`).
Distinct from `map!` in two ways: (1) iteration order is **unspecified**
(HashMap order, not insertion order — this is Red parity: `hash!` is the
performance table, `map!` is the ordered one); (2) `hash!` IS a `series!` —
indexable, sliceable, `foreach`-able as alternating key/value pairs.

### `hash!` vs `map!`

| | `map!` (M43) | `hash!` (M83) |
|---|---|---|
| Backing | `IndexMap` (insertion-ordered) | `HashMap` (unordered) |
| `series?` | no | yes |
| Iteration order | insertion | unspecified |
| Path access (`h/key`) | yes | yes |
| `pick`/`poke` by index | no | yes (alternating key/value) |
| Use case | ordered config / JSON-like | perf-heavy lookup |

- [x] Add `struct HashDef { entries: RefCell<HashMap<MapKey, Value>>, key_order: RefCell<Vec<MapKey>> }`
      in `value.rs` (the `key_order` vec is for `keys-of` determinism in tests
      only — not part of the value semantics; document).
- [x] Add `Value::Hash(Rc<RefCell<HashDef>>)` variant (synthetic, no span).
- [x] Add `Value::hash()` constructor.
- [x] Reuse `MapKey` from M43 (`value.rs:573`) — same hashable subset.
- [x] Extend `printer.rs`:
  - [x] `mold`: `make hash! [k1 v1 k2 v2 ...]` (alternating key/value form,
        matching Red; iteration uses `key_order` for stable output).
  - [x] `form`: same as mold.
- [x] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [x] Add `hash?` predicate.
- [x] Add `to-hash` converter (from block of pairs, from `map!` → hash).
- [x] Add `make hash! <spec>` (block of alternating key/value; or block of
        `[k v]` pairs).
- [x] Implement path resolution:
  - [x] `hash/key` (any `MapKey`-shaped value) → lookup.
  - [x] `set-path` `hash/key: value` → `HashDef::set`.
- [x] Series ops (the `hash!`-specific surface):
  - [x] `pick hash integer` → key at index 2n, value at 2n+1 (alternating).
  - [x] `poke hash integer value` — write the value at the corresponding
        slot (key slot if even index, value slot if odd).
  - [x] `length?` → `2 * entry_count`.
  - [x] `foreach [k v] hash [...]` works (series iteration).
  - [x] `select`/`find` (by key) — same as `map!`.
  - [x] `append`/`insert` (as a series — append a key/value pair).
  - [x] `clear`/`empty?`.
- [x] Update `same?`/`not-same?` (`Rc::ptr_eq`).
- [x] Update equality (`compare.rs`): deep equality on entries (order-
        independent — `hash!` equality does NOT depend on insertion order,
        unlike `map!`).
- [x] Update `type_name` → `"hash!"`.
- [x] Inline `#[test]`: `make hash! [a 1 b 2]` molds back identically.
- [x] Inline `#[test]`: `h: make hash! [a 1] h/a` → `1`.
- [x] Inline `#[test]`: `h/b: 2 h/b` → `2`.
- [x] Inline `#[test]`: `series? make hash! []` → true (the `map!` vs `hash!`
        discriminator).
- [x] Inline `#[test]`: `length? make hash! [a 1 b 2]` → `4` (alternating).
- [x] Inline `#[test]`: `pick (make hash! [a 1 b 2]) 0` → `'a`; `pick ... 1`
        → `1`.
- [x] Inline `#[test]`: two `hash!` with the same entries in different
        insertion order are `equal?` (order-independence, vs `map!`).
- [x] Add golden fixtures: `hash_construct`, `hash_series`, `hash_paths`,
        `hash_vs_map`.
- [x] Add `programs_errors/hash_unhashable_key.red`.
- [x] Update `property.rs` for `Hash` round-trip (mold form is reparseable
      via `make hash!`).
- [x] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 84 — `vector!`

A packed numeric vector with a typed element kind. The first "container with a
typed payload" type. Element kinds: `i8`/`i16`/`i32`/`i64`/`f32`/`f64`. Stored
as a single enum-of-arrays (no boxing per element).

> **Implementation note:** the packed `enum-of-arrays` wording is aspirational
> — the POC stores `Vec<Value>` of `Integer`/`Float` for native-compat (the
> existing `Series` model is `Vec<Value>` and `extract_series` returns a
> `Series`, so a packed layout would force a parallel series-extraction path
> for every native). The `kind` field drives narrow-on-write and
> `vec/integer` path access. Documented deviation; perf deferred to v0.8.

- [x] Add `enum VectorKind { I8(Vec<i8>), I16(Vec<i16>), I32(Vec<i32>), I64(Vec<i64>), F32(Vec<f32>), F64(Vec<f64>) }`
      in `value.rs`.
      *(Replaced by `VectorDef { kind: RefCell<Symbol>, elems: RefCell<Vec<Value>>, cursor: RefCell<usize> }` — see note above.)*
- [x] Add `struct VectorDef { data: RefCell<VectorKind> }`.
      *(Actual: `VectorDef { kind, elems, cursor }` — `kind` is a `Symbol`,
      `elems` is `Vec<Value>`; `cursor` mirrors Red's series cursor.)*
- [x] Add `Value::Vector(Rc<RefCell<VectorDef>>)` variant (synthetic, no span).
- [x] Add `Value::vector(kind)` constructor.
- [x] Add `VectorKind::from_block(&[Value]) -> Result<VectorKind, ...>` —
      promotes all elements to a common kind (int → i64, float → f64; mixed
      int/float → f64 with promotion).
      *(Actual: `infer_vector_kind(&[Value]) -> Result<(Symbol, Vec<Value>), String>` in `value.rs`.)*
- [x] Extend `printer.rs`:
  - [x] `mold`: `make vector! [integer! 1 2 3]` or `make vector! [float! 1.0 2.0]`
        (Red form — the first element names the kind, then the values).
  - [x] `form`: same as mold.
- [x] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [x] Add `vector?` predicate.
- [x] Add `to-vector` converter (from block of ints/floats; from `binary!`
      with a kind hint).
      *(Binary!→vector! with kind hint is deferred to v0.8 — only the
      block/int/float/identity spec forms ship in M84.)*
- [x] Add `make vector! <spec>`:
  - [x] From block: `[integer! 1 2 3]` (kind then values) or `[1 2 3]`
        (infer kind).
  - [x] From integer + kind: `make vector! 3` → 3-element zero vector
        (default `i64`).
- [x] Series ops (full `series!` model):
  - [x] `length?`, `pick`, `poke`, `first`/`last`/`next`/`back`/`at`/`skip`,
        `append`/`insert`/`change`/`remove`/`clear`/`take`/`copy`.
        *(Cursored navigation `next`/`back`/`at`/`skip`/`head`/`tail`/`index?`
        returns a positioned Block view via `extract_series` — documented
        deviation from Red, where these return a positioned series over the
        vector's storage. Mutations through the Block view's `poke` propagate
        via `Rc<RefCell<...>>` sharing; other mutations on the view do not
        propagate.)*
  - [x] `pick` returns the value as `Integer`/`Float` (not a `vector!` of
        length 1) — matches Red.
  - [x] `poke` accepts `Integer`/`Float`; narrows to the vector's kind (clamp
        on overflow for ints; round for floats).
- [x] Arithmetic: `vector + vector` (same kind, componentwise; error on
        length mismatch), `vector + scalar` (broadcast), `vector * scalar`.
        *(Full `+ - * /` shipped — int-kind `/` promotes to float-kind (Red
        parity). Componentwise `vec * vec`/`vec / vec` also supported.)*
- [x] Path access: `vec/integer` → the kind word (`'integer!`/`'float!`);
        `vec/1` → first element (path-as-pick). **Confirm** Red parity.
        *(Confirmed: `vec/integer` returns the kind word as a `word!` value;
        `vec/N` is 1-based pick; `vec/N: value` is path-as-poke.)*
- [x] Update `same?`/`not-same?` (`Rc::ptr_eq`).
- [x] Update equality (`compare.rs`): deep, kind + contents.
- [x] Update `type_name` → `"vector!"`.
- [x] Inline `#[test]`: `make vector! [integer! 1 2 3]` molds back.
- [x] Inline `#[test]`: `length? make vector! [integer! 1 2 3]` → `3`.
- [x] Inline `#[test]`: `pick (make vector! [integer! 10 20 30]) 1` → `20`.
- [x] Inline `#[test]`: `make vector! [1 2 3] + make vector! [4 5 6]` →
        `make vector! [integer! 5 7 9]`.
- [x] Inline `#[test]`: `vector? make vector! []` → true.
- [x] Inline `#[test]`: kind promotion — `make vector! [1 2.0 3]` → f64 kind.
- [x] Add golden fixtures: `vector_construct`, `vector_series`,
        `vector_arith`, `vector_kind_promote`.
        *(Plus `vector_paths` for path-access coverage.)*
- [x] Add `programs_errors/vector_kind_mismatch.red` (e.g. `poke` of a string
        into a vector).
- [x] Update `property.rs` for `Vector` round-trip.
        *(Focused `vector_mold_is_stable` proptest — mirrors `hash_mold_is_stable`.)
- [x] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 85 — `image!`

A 2D pixel buffer (RGBA8). Heavy by itself; lands after `vector!` because it
shares the "packed array" template. **No GUI/draw** — pure data; this is the
in-memory image value, not a rendering surface.

- [ ] Add `struct ImageDef { width: usize, height: usize, pixels: RefCell<Vec<[u8; 4]>> }`
      in `value.rs` (RGBA8, row-major).
- [ ] Add `Value::Image(Rc<RefCell<ImageDef>>)` variant (synthetic, no span).
- [ ] Add `Value::image(w, h, pixels)` constructor.
- [ ] Extend `printer.rs`:
  - [ ] `mold`: `make image! [width: <w> height: <h> pixels: [...]]` (a
        reparseable keyword-block form, matching `make module!`'s mold
        template).
  - [ ] `form`: same as mold.
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [ ] Add `image?` predicate.
- [ ] Add `to-image` converter (from `binary!` + width + height; from
        `vector!` of i32 ARGB).
- [ ] Add `make image! <spec>`:
  - [ ] From block: `[width: 100 height: 100 pixels: [...]]` (keyword form).
  - [ ] From block: `[100 100 [...pixel-bytes...]]` (positional form).
- [ ] Path access:
  - [ ] `image/width` → integer.
  - [ ] `image/height` → integer.
  - [ ] `image/size` → pair (`width x height`).
  - [ ] `image/x y` (pair path) → the pixel at (x, y) as a `tuple!` RGBA.
  - [ ] `set-path` writes a pixel.
- [ ] Series ops (limited — `image!` is NOT a full `series!` in Red):
  - [ ] `length?` → `width * height` (pixel count).
  - [ ] `pick image integer` → pixel at flat index as `tuple!`.
  - [ ] `poke image integer tuple` → write pixel.
  - [ ] No `append`/`insert` (size is fixed) — error.
- [ ] Update `same?`/`not-same?` (`Rc::ptr_eq`).
- [ ] Update equality (`compare.rs`): deep, width/height/pixels.
- [ ] Update `type_name` → `"image!"`.
- [ ] Inline `#[test]`: `make image! [100 100 [...]]` molds back.
- [ ] Inline `#[test]`: `width?` accessor → 100 (via `image/width` path).
  - [ ] *(Open: is `width?` a native or is `image/width` the only path?
        Decision: path only — no new predicate native; matches Red.)*
- [ ] Inline `#[test]`: `pick (make image! [2 2 [...rgba bytes...]) 0` →
        `tuple!` of the first pixel.
- [ ] Inline `#[test]`: `poke` a pixel round-trips.
- [ ] Inline `#[test]`: `image? make image! [...]` → true.
- [ ] Add golden fixtures: `image_construct`, `image_paths`, `image_pixels`.
- [ ] Add `programs_errors/image_bad_dims.red` (e.g. width × height ≠
        pixel-count).
- [ ] Update `property.rs` for `Image` round-trip.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 86 — `unset!`

A distinct "no value" sentinel, separate from `none!`. In Red, `unset!` is
the result of evaluating a word with no value, or a `do` block whose last
expression had no return. **This is the one milestone that is not purely
additive** — it touches the binding/eval model. M86 lands the value variant
and a *gated* fallback so existing error fixtures stay green.

### `unset!` semantics

- [ ] Add `Value::Unset` variant in `value.rs` (unit, no span — synthetic).
- [ ] Update `printer.rs`: `mold`/`form` of `Unset` → `""` (empty string,
        matching Red — `unset!` molds to nothing).
- [ ] Add `unset?` predicate.
- [ ] Add `unset` constant in `user_ctx` (a word evaluating to `Unset`).
- [ ] **Gated fallback** — the behavior change:
  - [ ] Today, `resolve_word` `Unbound` arm in the walker errors with
        `EvalError::UnboundWord` (M62 added a `user_ctx` fallback first, but
        truly-unbound words still error).
  - [ ] M86 adds a `--unset-on-unbound` CLI flag (default **off** —
        back-compat). When on, an unbound word evaluates to `Value::Unset`
        instead of erroring. When off (default), behavior is unchanged.
  - [ ] The VM's `LoadDynamic` arm gets the same gate (consult a new
        `Env.unset_on_unbound: bool` field, default false).
  - [ ] This is the **only** v0.7 behavior change; it's opt-in. All existing
        `unbound_word` error fixtures stay green with the flag off.
- [ ] `do` of an empty block → `Unset` (today returns `None`; **decision:
        keep `None` for empty `do` — Red parity is `unset!` but changing
        `do []` to `Unset` would break existing fixtures. Document as a
        deviation; revisit if a fixture depends on `do []` returning
        `none!`.)
- [ ] `print` of `Unset` → prints nothing (Red parity).
- [ ] Update `type_name` → `"unset!"`.
- [ ] Update `compare.rs`: `Unset = Unset` → true; `Unset = None` → false
        (they ARE distinct in Red).
- [ ] Inline `#[test]`: `unset? ()` — wait, `()` evaluates its content;
        `unset? do []` → false (do [] = none today); `unset? unset` → true.
- [ ] Inline `#[test]`: with `--unset-on-unbound`, an unbound word → `Unset`;
        without, it errors.
- [ ] Inline `#[test]`: `mold unset` → `""`.
- [ ] Inline `#[test]`: `print unset` → prints empty line.
- [ ] Inline `#[test]`: `unset = unset` → true; `unset = none` → false.
- [ ] Inline `#[test]`: regression guard — all existing `unbound_word`
        fixtures still error with the flag off.
- [ ] Add golden fixtures: `unset_value`, `unset_on_unbound` (with the flag).
- [ ] Add a stable-string property test for `Unset` (`mold unset == ""`).
- [ ] `cargo test --workspace` green (default); `--features force-walk` green;
      **plus** a new `cargo test --workspace --features unset-fallback` mode
      gating the `--unset-on-unbound` behavior.
- [ ] **Open:** add a `unset-fallback` cargo feature to `red-eval` for the
        test mode, or thread the flag purely through `Env` and the CLI.
        Decision: `Env` field + CLI flag; no cargo feature (the behavior is
        runtime-gated, not compile-gated).

---

## Milestone 87 — `native!` / `op!` split

Red distinguishes `native!` (built-in, implemented in the host language) from
`function!` (user-defined). The POC folds both into `Value::Func` with a
`FuncDef.native: Option<NativeFn>` flag. Similarly, `op!` is an infix
function — the POC uses `FuncDef.infix: bool`.

**Decision (per plan): keep the flags; add type-distinction at the predicate
layer.** This avoids a sweeping `Value` refactor (splitting `Func` into
`Native`/`Function`/`Op` would touch every match arm) while satisfying the
`type?` contract.

- [ ] Update `type_name` (`natives/mod.rs:134`):
  - [ ] `Value::Func(fd)` where `fd.native.is_some()` → `"native!"`.
  - [ ] `Value::Func(fd)` where `fd.infix` → `"op!"`.
  - [ ] `Value::Func(fd)` otherwise → `"function!"`.
  - [ ] `Value::Closure(_)` → `"closure!"` (unchanged).
- [ ] Add `native?` predicate — true on `Value::Func` with `native.is_some()`
      OR on `Value::Closure` (closures are native-ish? **decision: no —
      `native?` is false on closures**; `closure?` is the strict predicate
      and `function?` is the broad one).
- [ ] Add `op?` predicate — true on `Value::Func` with `fd.infix`.
- [ ] Update `type?` to return `native!`/`op!`/`function!`/`closure!`
      appropriately.
- [ ] Update `types-of` to include the right type words (e.g. a native is
      `[native! function!]`).
- [ ] Inline `#[test]`: `type? :+` → `op!`; `type? :print` → `native!`;
        `type? :func [x][x]` → `function!`.
- [ ] Inline `#[test]`: `native? :print` → true; `native? :+` → true
        (`+` is native AND op — `op?` is the strict op check; `native?` is
        "is it a built-in function" which includes ops. **Confirm Red
        parity**: in Red `op?` and `native?` are disjoint — an op is NOT a
        native. Decision: `native?` false on `infix` funcs; `op?` true on
        them. `function?` true on all three.)
- [ ] Inline `#[test]`: `op? :+` → true; `op? :print` → false.
- [ ] Inline `#[test]`: `function? :+`, `function? :print`,
        `function? :func [x][x]` → all true.
- [ ] Add golden fixtures: `type_split_native`, `type_split_op`.
- [ ] Audit existing fixtures: any fixture asserting `type? :foo == function!`
        for a native needs updating to `native!`/`op!`. The parity harness
        catches this.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M87 open questions

1. **Is `+` a `native!` or an `op!`?** Red says `op!` only (an infix operator
   is not a native, even though its implementation is in Rust). Confirm
   before implementing — the test above assumes this.
2. **`any-function?` predicate.** Red has `any-function?` (true on
   `function!`/`native!`/`op!`/`closure!`/`routine!`). Add it in M87 for
   completeness. **Decision: yes, add `any-function?`.**

---

## Milestone 88 — `struct!` + `handle!`

FFI-adjacent opaque types. v0.7 ships only the value shapes + mold +
predicates — the actual FFI binding layer (`routine!`, `call-foreign`,
`make struct!` field access) is **deferred to v0.8** (overlaps with plan7's
cdylib plugin design). M88 lands the types so `type?`/`struct?`/`handle?`
work and so a v0.8 `routine!` milestone has somewhere to put its results.

### `struct!`

- [ ] Add `struct StructDef { fields: Vec<(Symbol, Symbol)>, layout: Rc<[u8]> }`
      in `value.rs` (field names + type words; `layout` is the packed bytes —
      opaque to Red, only `routine!` interprets it).
- [ ] Add `Value::Struct(Rc<StructDef>)` variant (synthetic, no span).
- [ ] Extend `printer.rs`:
  - [ ] `mold`: `make struct! [field1: <type-word> field2: <type-word>]`
        (the layout bytes are NOT molded — round-trip is via the field
        spec only; **document**: two structs with the same fields but
        different layout bytes mold identically; `equal?` is by identity).
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [ ] Add `struct?` predicate.
- [ ] Add `make struct! <spec>` (block of `word: type-word` pairs — defines
      the field shape; no layout bytes yet).
- [ ] Path access (deferred to v0.8 with `routine!`): `struct/field` errors
      in v0.7 with "struct field access requires routine! FFI (v0.8)".
- [ ] Update `type_name` → `"struct!"`.
- [ ] Update `same?` (`Rc::ptr_eq`); `equal?` (deep on fields + type words;
      layout bytes don't compare — they're opaque).
- [ ] Inline `#[test]`: `make struct! [x: integer! y: float!]` molds back.
- [ ] Inline `#[test]`: `struct? make struct! []` → true.
- [ ] Inline `#[test]`: `struct/field` errors with the v0.8 deferral message.
- [ ] Add golden fixtures: `struct_construct`.
- [ ] Update `property.rs` for `Struct` round-trip.

### `handle!`

- [ ] Add `struct HandleDef { ptr: *mut std::ffi::c_void, drop: Option<extern "C" fn(*mut std::ffi::c_void)> }`
      in `value.rs` (an opaque pointer + optional finalizer; `!Send`/`!Sync`
      like the rest of `Env`).
- [ ] Add `Value::Handle(Rc<HandleDef>)` variant (synthetic, no span).
- [ ] `impl Drop for HandleDef` — calls the finalizer if present (the `Rc`
      drop triggers it on the last ref).
- [ ] Extend `printer.rs`:
  - [ ] `mold`/`form`: `#[handle 0x7f...]` (the pointer address; non-
        reparseable — stable-string property test only).
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [ ] Add `handle?` predicate.
- [ ] No `make handle!` from script (handles are only produced by
      `routine!`/`load-plugin` in v0.8+). Script-level construction errors.
- [ ] Update `type_name` → `"handle!"`.
- [ ] Update `same?` (`Rc::ptr_eq`); `equal?` (identity only — handles are
        opaque, never structurally compared).
- [ ] Inline `#[test]`: `handle? <some handle value>` → true (construct one
        from Rust in the test).
- [ ] Inline `#[test]`: `mold <handle>` → `"#[handle 0x...]"`.
- [ ] Add a stable-string property test for `Handle`.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 89 — `typeset!`

A value representing a set of types. Used in function spec blocks for
runtime type-checking. Today `FuncDef.params: Vec<Symbol>` stores names only;
M89 adds an optional parallel `param_types: Vec<Option<Rc<TypesetDef>>>` and
wires the type-check into the call path.

### `typeset!` scope

- The value variant + `make typeset!` + `typeset?` + mold: **in scope**.
- Wiring `typeset!` into `func` spec-eval (so `func [x [integer! float!]]`
  type-checks args at call time): **in scope** (the headline feature).
- The `typeset!` *algebra* (`union`/`intersect`/`complement` of typesets): **deferred to v0.8**.

- [ ] Add `struct TypesetDef { types: RefCell<HashSet<Symbol>> }` in `value.rs`
      (a set of type-word symbols like `'integer!`/`'float!`/`'string!`).
- [ ] Add `Value::Typeset(Rc<TypesetDef>)` variant (synthetic, no span).
- [ ] Add `Value::typeset(words: &[Symbol])` constructor.
- [ ] Add `TypesetDef::matches(&Value) -> bool` — checks `type_name(v)` is
      in the set (handles `any-word?`/`any-path?`/`number!` etc. by checking
      the appropriate group words).
- [ ] Extend `printer.rs`:
  - [ ] `mold`: `make typeset! [integer! float!]` (reparseable).
  - [ ] `form`: same as mold.
- [ ] Extend `interp_walker.rs`/`vm/compiler.rs` self-evaluating arms.
- [ ] Add `typeset?` predicate.
- [ ] Add `make typeset! <block-of-type-words>` constructor.
- [ ] Add `to-typeset` converter.
- [ ] Extend `FuncDef` (`value.rs`):
  - [ ] Add `pub param_types: Vec<Option<Rc<TypesetDef>>>` parallel to
        `params`. `None` = unchecked (back-compat with all existing funcs).
  - [ ] Default to `vec![None; params.len()]` in existing constructors.
- [ ] Extend `func`/`function`/`closure` natives (`natives/func.rs`):
  - [ ] When a param spec entry is a block (`[integer! float!]`), build a
        `TypesetDef` and store it in `param_types[i]`.
  - [ ] When the entry is a bare word, `param_types[i] = None` (back-compat).
- [ ] Wire the type-check into the call path:
  - [ ] **Walker** (`interp_walker.rs` call shim): before binding args, if
        `param_types[i].is_some()`, check `typeset.matches(&args[i])`; on
        failure, raise `EvalError::TypeError` with the expected typeset
        (mold the typeset for the message).
  - [ ] **VM** (`vm/vm.rs` `CallUser`/`prepare_call`): same check at frame
        push.
- [ ] Update `type_name` → `"typeset!"`.
- [ ] Update `same?` (`Rc::ptr_eq`); `equal?` (deep on the type-word sets).
- [ ] Inline `#[test]`: `make typeset! [integer! float!]` molds back.
- [ ] Inline `#[test]`: `typeset? make typeset! []` → true.
- [ ] Inline `#[test]`: a func with `[x [integer!]]` rejects a string arg.
- [ ] Inline `#[test]`: a func with `[x [integer! float!]]` accepts both.
- [ ] Inline `#[test]`: existing funcs (no type spec) still accept any
        type (back-compat regression guard).
- [ ] Add golden fixtures: `typeset_construct`, `func_typed_args`,
        `func_typed_args_error`.
- [ ] Add `programs_errors/func_bad_arg_type.red`.
- [ ] Update `property.rs` for `Typeset` round-trip.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M89 open questions

1. **Type-check cost.** A `HashSet` lookup per arg per call — negligible for
   non-typed funcs (the `None` fast path skips the lookup). Confirm with a
   bench in M90.
2. **`any-*` family in typesets.** `make typeset! [any-word!]` — does the
   typeset match all word kinds? Decision: yes — `TypesetDef::matches`
   recognizes the `any-word!`/`any-path!`/`any-object!`/`any-function!`/
   `number!`/`series!` group words by checking the appropriate sub-types.
   Add a `GROUP_TYPES` const table mapping group word → predicate fn.
3. **`type?` of a typeset.** Returns `typeset!`; `types-of` of a value
   should *not* include `typeset!` (a value is never itself a typeset).
   Confirm.

---

## Milestone 90 — Polish & v0.7.0 release

- [ ] Audit `EvalError` rendering for all new error sources:
  - [ ] `InvalidPercent` / `InvalidMoney` / `InvalidIssue` / `InvalidEmail`
        / `UnterminatedTag` / `InvalidRegex` (M80–M82 lexer errors).
  - [ ] `TypeError` messages for typed-func arg mismatches (M89) — render the
        expected `typeset!` mold in the message.
  - [ ] Money currency mismatch (M80).
  - [ ] Vector kind mismatch / image dim mismatch (M84/M85).
- [ ] Add spans to all source-origin new variants (`Percent`/`Money`/
      `Issue`/`Email`/`Tag` already struct-with-span; confirm synthetic
      variants use `Span::default()`).
- [ ] Golden fixture per new error case (one per error kind added in
      M80–M89).
- [ ] Property test: extend `mold(parse(mold(v)))` to cover `Percent`/
      `Money`/`Issue`/`Email`/`Tag`/`Hash`/`Vector`/`Image`/`Struct`/
      `Typeset` (the reparseable ones). `Regex`/`Handle`/`Unset`/`Closure`/
      `Module` get stable-string assertions instead.
- [ ] Extend `red-core/tests/golden/` to cover all new literals.
- [ ] Expand `red-eval/tests/programs/` to 30+ new fixtures (one per new
      type × positive + error case).
- [ ] Run `cargo bench --bench eval`; record in `BENCHMARKS.md` under
      "v0.7.0".
  - [ ] Expected neutral on existing benches (no new hot-path work).
  - [ ] The M89 type-check adds a per-call `Option::is_some` check; expected
        negligible. If any bench regresses >5%, investigate the
        `param_types` vec access in `prepare_call`.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`; fix.
- [ ] Run `cargo fmt --all --check`; fix.
- [ ] Update `project-brief.md`:
  - [ ] Add a "Type Completeness (v0.7)" subsection under "Value model":
        list the ten new variants, the `regex` crate dep, the `unset!`
        gated-fallback behavior change, the `native!`/`op!` split, the
        `typeset!` func-spec integration.
  - [ ] Update the value-model code block (add `Percent`/`Money`/`Issue`/
        `Email`/`Tag`/`Regex`/`Hash`/`Vector`/`Image`/`Unset`/`Struct`/
        `Handle`/`Typeset`).
  - [ ] Update "Deferred" — remove the items now landed; add v0.8 candidates
        (reactivity, concurrency, port model, routine! FFI binding layer,
        typeset algebra, shared-cell closures).
- [ ] Update `architecture.md`:
  - [ ] New value variants in the value-model section.
  - [ ] `RegexDef`/`HashDef`/`VectorDef`/`ImageDef`/`StructDef`/
        `HandleDef`/`TypesetDef` struct definitions.
  - [ ] The `unset!` fallback gate (`Env::unset_on_unbound`).
  - [ ] The `FuncDef.param_types` parallel vec and the call-time type-check.
  - [ ] Path resolution rules for `email!`/`image!`.
  - [ ] Series-model rules for `hash!`/`vector!` (which series ops apply).
- [ ] Update `README.md`:
  - [ ] Bump version to v0.7.0.
  - [ ] Remove `tag!`/`ref!`¹/`image!`/`vector!`/`hash!`/`regex!` from
        "Known gaps" (now landed).
  - [ ] Add the ten new types to the "Value types" list.
  - [ ] Add `percent?`/`money?`/`issue?`/`email?`/`tag?`/`regex?`/`hash?`/
        `vector?`/`image?`/`unset?`/`struct?`/`handle?`/`typeset?`/
        `native?`/`op?`/`any-function?` to the type predicates list.
  - [ ] Add `to-percent`/`to-money`/`to-issue`/`to-email`/`to-tag`/
        `to-regex`/`to-hash`/`to-vector`/`to-image`/`to-typeset` to the
        conversions list.
  - [ ] Add `--unset-on-unbound` to the CLI section.
  - [ ] Update "Known gaps" with the new deferrals (reactivity, concurrency,
        port model, `routine!` FFI binding, typeset algebra, shared-cell
        closures).
  - [ ] Note: `ref!` is **not** landed in v0.7 — see "ref! deferral" below.
- [ ] Final `cargo test --workspace` green.
- [ ] Final `cargo test --workspace --features force-walk` green.
- [ ] Final `cargo test --workspace` with `--unset-on-unbound` (M86 new mode)
      green.
- [ ] Final `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] Tag release `v0.7.0`.

¹ `ref!` is deliberately **not** in this plan. See "ref! deferral" below.

---

## `ref!` deferral

`ref!` appears in the user-supplied list and in `README.md:352`, but it is
**excluded from v0.7**. Rationale:

- Red's `ref!` is an internal C-level reference type used by the runtime, not
  a user-facing literal. It has no lexer form and no script-level
  constructor — it's produced only by the runtime and consumed by `routine!`
  FFI.
- The closest POC equivalent is `handle!` (M88), which lands in v0.7 as the
  opaque-pointer type.
- `ref!` would only matter if v0.8's `routine!` FFI layer needs a *typed*
  reference distinct from an opaque `handle!`. Decision: defer; revisit when
  `routine!` lands. If `routine!` needs `ref!`, it lands alongside it in
  v0.8, not here.

## Open questions (plan-wide)

1. **`regex` crate vs. hand-rolled.** `regex` adds ~500 KB to the binary and
   is the standard Rust choice. Alternative: the `regex-lite` crate (~100
   KB, smaller surface). Decision: `regex` (full) — the size cost is
   acceptable and `regex-lite`'s missing features (look-around) would
   surprise users. Confirm in M82.
2. **`hash!` iteration order.** Real Red `hash!` is unspecified-order. The
   plan stores a `key_order` vec for *test* determinism (golden fixtures
   need stable output). **Decision: mold uses `key_order`; `keys-of` uses
   `key_order`; all other iteration is unspecified.** Document as a
   deviation (Red's `keys-of hash!` is unspecified; ours is insertion-order
   for testability).
3. **`unset!` gate.** Default-off vs. default-on. Default-off preserves
   back-compat (existing unbound-word fixtures stay green). Default-on matches
   Red but breaks the POC's strict-binding contract. Decision: default-off
   + `--unset-on-unbound` flag. Revisit default in v0.8.
4. **`native!` vs. `op!` overlap.** See M87 open-q #1. Decision pending
   confirmation: `op!` and `native!` are disjoint (`+` is `op!`, not
   `native!`); `function?` covers both plus `closure!`.
5. **`image!` path access.** `image/x y` (pair path) vs. a `pick`-only
   model. Pair paths on a non-pair head are unusual. Decision: pair path
   works (mirrors Red); `pick` by flat index also works. Confirm in M85.
6. **`typeset!` and `any-*` groups.** See M89 open-q #2. The group-word
   recognition table is the fiddly part; the `GROUP_TYPES` const is the
   proposed mechanism.
7. **`vector!` kind inference.** `make vector! [1 2 3]` — i64 or i32?
   Decision: i64 (matches the POC's `Integer` being i64). `make vector!
   [1.0 2.0]` → f64. Mixed → f64 with promotion.
8. **`struct!` layout bytes.** M88 ships `layout: Rc<[u8]>` as opaque. The
   v0.8 `routine!` layer interprets the bytes per the field spec. In v0.7,
   `make struct! [x: integer!]` produces an *empty* layout (zero bytes);
   field access errors. Confirm this is acceptable (the type exists, the
   data operations are deferred).

(End of plan8-missing-types.md)
