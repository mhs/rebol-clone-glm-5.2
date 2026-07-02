# Plan 13: Feature-Parity Round-Out (v0.10)

Execution checklist extending the v0.9.2 baseline in `plan12-control-flow.md`
(M124 polish assumed complete). Where `plan11` closed the four highest-
leverage **functional** gaps and `plan12` closed the **control-flow**
vocabulary gap, v0.10 is the cleanup pass: it lands **everything else** the
post-v0.8 feature audit flagged as missing — series/string DSL round-out,
object/context reflection, meta & quotation primitives, math helper natives,
eval-time reflection & error cataloging, module extras, and refinement
expansion on existing natives. None of these individually blocks a whole
class of programs the way `plan11`'s four items did; collectively they close
the remaining gap between this POC and a "no obvious missing native" Red
clone.

Per `project-brief.md`, GUI / `draw` / `vid` / reactive dialects remain
**permanently out of scope**. Parse recursion, `mold`-as-native, series
`sort`/set-ops, and the port model are in `plan11-functional-gaps.md`.
`unless`/`forever`/`for`/`forskip`/`except`/`finally`/`does-not` are in
`plan12-control-flow.md`. **Do not duplicate those here** — this plan's
milestones are additive to both.

## What's in scope for v0.10

Seven milestones, grouped by subsystem (independent — land in any order
after the M130 template is proven):

- **M130 — Series/string DSL round-out:** `map-each`, `remove-each`,
  `collect` (as a general native, distinct from `plan8`'s parse-only
  `collect` keyword), `checksum`, `compress`/`decompress`, `enbase`/
  `debase`, `encode`/`decode`.
- **M131 — Object/context reflection:** `set?`, `bound?`, `bind?`,
  `bind-of`, `context-of`, `context?`, `spec-of`, `body-of`, `resolve`,
  `protect`/`unprotect`/`protect-system`, `has`, `extend`.
- **M132 — Meta & quotation:** `quote`, `to-lit-word` companion review,
  `meta`/`to-meta-word`, `unset` as a general-purpose eval-time construct
  (distinct from `plan8` M86's `Value::Unset` *type*, which already
  exists — this milestone is about the *native surface* around it:
  `uneval`, `eval-set`).
- **M133 — Math helper natives:** `floor`, `ceiling`, `truncate`, `zero?`,
  `positive?`, `negative?`, `sign?`/`sign-of` (promote from stdlib),
  `gcd`/`lcm` (promote from stdlib), `sinh`/`cosh`/`tanh`, `square-root`/
  `absolute` (aliases), the `math` evaluation-order mode.
- **M134 — Eval reflection & error cataloging:** `trace` (user-level,
  distinct from the existing `--trace` CLI VM-instruction dump), `dump`,
  `stop?`, an `errors` catalog native.
- **M135 — Module extras:** `load-module`, `exports-of`.
- **M136 — Refinement expansion:** widen `find`/`append`/`copy`/`replace`/
  `round`/`parse` to Red's full refinement surface (each currently exposes
  only a thin subset — see the table in the ground-truth section).
- **M137 — Polish & v0.10.0 release.**

## Deferred / out of scope

- Everything already covered by `plan11`/`plan12` (see header above).
- Reactivity, concurrency, full port/async model — `future-plan-reactivity.md`,
  `future-plan-concurrency.md`.
- `typeset!` algebra (`union`/`intersect`/`complement` of typesets) —
  `plan8` M89 deferral, still open.
- Named timezones (`chrono-tz`) — `plan5` open-q #5, still open.
- A central package registry server — `plan7` open deferral, still open.
- New value types of any kind — this plan is pure native/behavior surface
  on top of the `Value` enum as it stands after `plan9`/`plan11`'s `Port`
  addition. If a milestone below turns out to need a new variant (unlikely,
  but flag if so — e.g. if `errors` catalog (M134) wants a first-class
  "error kind" enum value rather than a string), treat that as a plan
  deviation requiring sign-off, not a silent scope-creep.

## Non-goals

- A register VM, JIT, or further perf work — the v0.3.3 VM stays the
  default evaluator.
- New `Instr` variants — every native below is a `fixed_native`/
  `variadic_native`/`reg_refined` registration over existing eval
  primitives, following the exact pattern `registry.rs` already uses
  throughout (see `plan8`/`plan9`/`plan11`/`plan12` ground-truth sections
  for prior art — this plan doesn't repeat the mechanism, just applies it).
- Behavior changes to any existing native. Every milestone is either a new
  native (M130–M132, M134–M135) or a **widening** of an existing native's
  refinement surface (M133 promotes stdlib functions to natives with
  identical observable behavior; M136 adds refinements without changing
  default/no-refinement behavior).

## Ground-truth references (from research)

- `strings.rs:461–485` registers `rejoin`/`reform`/`join`/`suffix?`/`split`/
  `trim`/`replace`/`uppercase`/`lowercase`/`compose`. No `checksum`/
  `compress`/`decompress`/`enbase`/`debase`/`encode`/`decode` anywhere in
  the crate (confirmed by grep — zero matches).
- `series.rs:1104–1196` has no `map-each`/`remove-each`. `collect`
  (`natives/mod.rs`? — confirm exact home) exists **only** inside `parse.rs`
  as a parse-keyword (`parse.rs:858,866,906`) — M130's `collect` is a
  general block-transforming native (`collect [...]` evaluates a body,
  gathering values passed to an inline `keep`-equivalent), a different
  construct sharing only the name.
- `object.rs:403–430` has `object?`/`same?`/`not-same?`/`words-of`/
  `values-of`/`reflect`/`in`/`object`/`context`. `natives/words.rs:585–636`
  has `get`/`set`/`value?`/`char?`/`use`/`bind`. Neither module has `set?`/
  `bound?`/`bind?`/`bind-of`/`context-of`/`context?`/`spec-of`/`body-of`/
  `resolve`/`protect`/`unprotect`/`has`/`extend` (confirmed by grep — zero
  matches for all of these symbol names as native registrations).
- `value.rs` has no `Value::Struct`-style "spec" accessor for `FuncDef` —
  `spec-of`/`body-of` (M131) will need to read `FuncDef.params`/whatever
  body-`Series` field already backs `func`/`closure` (`func.rs:30,68,252`)
  and re-mold it as a `block!`, not add new storage.
- `Value::Unset` already exists as of `plan8` M86 (`value.rs`, gated behind
  `--unset-on-unbound`). M132 is scoped to the **native surface** around
  quotation/meta (`quote`/`meta`/`uneval`/`eval-set`), not to re-litigating
  the M86 gate decision.
- `math.rs:1320–1412` (`register_math_natives`) and `:1417–1450`
  (`register_transcendental_natives`) — `round`/`random`/`power`/`min`/
  `max`/`abs`/`negate`/`complement`/`even?`/`odd?`/`sin`/`cos`/`tan`/`asin`/
  `acos`/`atan`/`atan2`/`sqrt`/`exp`/`log-e`/`log-10`/`log-2`/`degrees`/
  `radians` all present. `stdlib.red:182–184` has pure-Red `sign-of`/`gcd`/
  `lcm` — M133 promotes these to natives (same treatment `plan11` M112 gave
  `sort`). No `floor`/`ceiling`/`truncate`/`zero?`/`positive?`/`negative?`/
  `sinh`/`cosh`/`tanh`/`square-root`/`absolute` anywhere (confirmed by
  grep).
- The CLI `--trace` flag (`red-cli/src/main.rs:32`) drives a **VM-
  instruction** trace to stderr — this is a debugging tool for the
  implementation, not a user-callable native. M134's `trace` is a
  *different* thing: a script-level native for tracing evaluation of a
  specific expression/block, analogous to Rebol's `trace` word. Do not
  confuse the two or reuse the CLI flag's plumbing beyond possibly sharing
  an output-formatting helper.
- `module.rs:595–619` registers `module`/`export`/`module?`/`import`. No
  `load-module` (Red's lower-level module-loading primitive, distinct from
  `import`'s higher-level search-path behavior) or `exports-of`
  introspection native.
- Refinement gaps (current state, confirmed by reading each registration
  site):
  | Native | Current refinements | Registration site |
  |---|---|---|
  | `find` | `/case` only | `series.rs:1182` |
  | `append` | `/only` only | `series.rs:1185` |
  | `copy` | `/part` only | `series.rs:1191` |
  | `replace` | `/all` only | `strings.rs:476` |
  | `round` | `/to`, `/even` | `math.rs:1381` |
  | `parse` | `/case` only | `registry.rs:325` |
- `reg_refined` (`registry.rs:83`) is the shared refinement-registration
  helper; M136 adds refinement entries to its existing call sites rather
  than introducing a new mechanism.

---

## Milestone 130 — Series/string DSL round-out

### Series transforms

- [ ] Add `map-each word series body` native (`series.rs`) — evaluates
      `body` once per element of `series` with `word` bound to the element,
      collecting the body's return value into a new output series. (Distinct
      from `foreach`, which discards the body's return value.)
- [ ] Add `remove-each word series body` native — evaluates `body` once per
      element; removes elements from `series` in place where `body`
      evaluates truthy. Returns the mutated series (Red parity — confirm
      exact return value).
- [ ] Add `collect body` native (general form, NOT the parse-keyword) —
      evaluates `body`, which may call an inline `keep value` word bound
      only within `collect`'s dynamic scope, gathering `keep`'d values into
      a `block!` that `collect` returns. Confirm the exact Red mechanism for
      how `keep` becomes available inside `body` (a dynamically-bound
      function injected for the duration of the call, most likely) before
      implementing — this is the trickiest native in the milestone because
      it needs a temporary binding, not just argument evaluation.
- [ ] Inline `#[test]`: `map-each x [1 2 3] [x * 2]` → `[2 4 6]`.
- [ ] Inline `#[test]`: `a: [1 2 3 4] remove-each x a [even? x] a` → `[1 3]`.
- [ ] Inline `#[test]`: `collect [keep 1 keep 2]` → `[1 2]`.
- [ ] Inline `#[test]`: `collect [repeat i 3 [keep i]]` → `[1 2 3]` (keep
      works inside nested control flow, not just top-level statements).
- [ ] Add golden fixtures: `map_each_basic`, `remove_each_basic`,
      `collect_basic`, `collect_nested`.

### Checksums, compression, encoding

- [ ] Add `checksum data` native with `/method` refinement (at minimum
      `crc32`; add `sha1`/`sha256` if a lightweight no-async crate is
      available without pulling in a large dependency tree — confirm crate
      choice before implementing, following the `plan8`/`plan9` pattern of
      justifying each new `Cargo.toml` dependency).
- [ ] Add `compress data` / `decompress data` natives — backed by a
      pure-Rust `flate2` (or equivalent) dependency; confirm crate choice
      and document the size/complexity tradeoff the way `plan8` M82
      documented the `regex` crate choice.
- [ ] Add `enbase data` / `debase data` natives — base64 encode/decode
      (Red's default `enbase` base is 64; confirm whether `/base 16`/`/base
      2` refinements are in scope for v0.10 or a stretch goal).
- [ ] Add `encode`/`decode` natives — Red's generic encode/decode dispatches
      on a format word (e.g. `encode 'url string`); scope the v0.10 format
      set to what's actually needed (at minimum `url`-encoding, given
      M113's HTTP work in `plan11` may want it — confirm cross-plan
      dependency before assuming).
- [ ] Inline `#[test]`: `checksum "abc"` produces a stable, documented value
      (assert against a known CRC32/SHA reference value, not just
      "doesn't crash").
- [ ] Inline `#[test]`: `decompress compress "hello world"` → `"hello
      world"` (round-trip).
- [ ] Inline `#[test]`: `debase enbase "hello"` → `"hello"` (round-trip).
- [ ] Inline `#[test]`: `encode 'url "a b"` → `"a%20b"` (or Red's exact
      escaping convention — confirm before asserting).
- [ ] Add golden fixtures: `checksum_basic`, `compress_roundtrip`,
      `enbase_roundtrip`, `encode_url_basic`.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M130 open questions

1. **`collect`/`keep` scoping mechanism.** The trickiest implementation
   question in the milestone — resolve before coding, not during.
2. **Crate choices for checksum/compression.** Follow the `plan8` M82
   precedent: state the tradeoff, pick the smallest crate that covers the
   need, confirm in this milestone rather than defaulting silently.

---

## Milestone 131 — Object/context reflection

- [ ] Add `set? word` predicate — true if `word` is bound to something other
      than `Value::Unset`/unbound (distinct from `value?`, which
      `natives/words.rs:585` already provides — confirm the exact
      distinction between `set?` and the existing `value?` before
      implementing; they may already be equivalent, in which case `set?`
      is a one-line alias).
- [ ] Add `bound? word` predicate — true if `word` has *any* binding
      (context association), independent of whether that binding currently
      holds a set value.
- [ ] Add `bind? word` — Red's `bind?` (confirm exact contract vs `bound?`
      — in some Rebol dialects these are synonyms, in others `bind?`
      returns the context itself rather than a `logic!`; verify before
      implementing, since getting this wrong silently breaks scripts that
      rely on the distinction).
- [ ] Add `bind-of word` / `context-of word` — return the `context!`/
      `object!` a word is bound into. (Confirm whether these are two names
      for the same operation or genuinely distinct in Red — the audit
      flagged both names, but they may collapse to one native.)
- [ ] Add `context? value` predicate.
- [ ] Add `spec-of func-value` — returns the `block!` spec a `func`/
      `closure`/`function` was defined with (read from `FuncDef`, re-mold as
      a block — no new storage, per the ground-truth note above).
- [ ] Add `body-of func-value` — returns the `block!` body (same sourcing
      strategy as `spec-of`).
- [ ] Add `resolve target source` — copies bindings/values from `source`
      into `target` (an object-merging primitive; confirm exact Red
      semantics for conflict resolution — does `resolve` overwrite existing
      target slots, or only fill unset ones? Verify before implementing).
- [ ] Add `protect value` / `unprotect value` — marks an object/series
      immutable/mutable again; subsequent mutating natives (`append`/
      `poke`/`set-path` etc.) must check the protect flag and error cleanly
      instead of panicking. This is the one item in the milestone that
      touches **existing** mutation code paths (every mutating native needs
      a protect-check) — budget more review time for this than the other
      one-off additions.
- [ ] Add `protect-system` — protects the root `system` object specifically
      (a thin wrapper over `protect` applied to `env`'s system object).
- [ ] Add `has object word` — Red's field-existence check (distinct from
      `select`/`in`, which look up the *value*; `has` only checks presence).
- [ ] Add `extend object spec` — adds new fields to an existing object in
      place (mutates, unlike `make object!` which copies).
- [ ] Inline `#[test]` per predicate/accessor: `set?`/`bound?`/`bind?`/
      `context-of`/`context?`/`spec-of`/`body-of`/`has`/`extend` each get at
      least a true-case and false-case (where applicable) fixture.
- [ ] Inline `#[test]`: `protect` — `o: make object! [x: 1] protect o
      o/x: 2` errors cleanly (no panic); `unprotect o o/x: 2` then succeeds.
- [ ] Inline `#[test]`: `resolve` — merging two objects produces the
      documented conflict-resolution behavior (confirmed above).
- [ ] Add golden fixtures: one per new native (roughly a dozen — batch as
      `object_reflection_*`).
- [ ] Add `programs_errors/protect_mutation_denied.red`.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M131 open questions

1. **`set?` vs `value?` overlap.** Resolve before implementing — likely a
   one-line alias, but confirm.
2. **`bind?` vs `bind-of`/`context-of` overlap.** Same caution — the audit
   flagged all three by name; Red may only have two distinct operations
   under three names, or genuinely three. Confirm before building three
   separate implementations that turn out to be duplicates.
3. **`protect` scope of enforcement.** Confirm every mutating native that
   needs the check — an incomplete audit here means `protect` silently
   fails to protect in some code path, which is worse than not having the
   feature at all (it creates a false sense of safety).

---

## Milestone 132 — Meta & quotation

- [ ] Add `quote value` native — Rebol/Red's `quote` prevents evaluation of
      its argument (distinct from the existing lit-word (`'word`) literal
      syntax — `quote` is the *functional* form, usable on already-
      constructed values, e.g. inside a `map-each` body). Confirm exact
      Red semantics (is `quote` a native at all in Red, or purely a parse
      convention via `'`? If Red has no `quote` native and only the lexer
      form, **document that and drop this item** rather than inventing a
      native Red doesn't have).
- [ ] Add `meta value` / `to-meta-word` — Red's meta-word concept (if
      present in the target Red version this POC tracks — confirm against
      whatever Red version `project-brief.md` cites as the parity target;
      meta-words are a newer/less-universal Red feature and may not be
      worth building if the parity target predates them).
- [ ] Add `uneval value` — produces a form of `value` that evaluates back to
      itself unchanged when `do`'d (Rebol's classic "make this self-
      quoting" primitive — closely related to `mold`+`load` round-tripping,
      but a direct in-memory operation without the string round-trip).
- [ ] Add `eval-set` — confirm exact Red semantics before implementing (the
      audit flagged this by name from the exploration pass; verify it's a
      real Red primitive and not a misread of `set`+`do` composition before
      building anything).
- [ ] Inline `#[test]` per confirmed-real primitive (skip tests for any
      item dropped after the confirmation step above).
- [ ] Add golden fixtures for whichever primitives survive confirmation.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M132 open questions

1. **Which of these four are real Red primitives vs. audit
   misidentification.** This entire milestone is gated on confirming each
   item against actual Red documentation/source before writing any code.
   **Do not implement speculatively** — a wrong quotation-semantics native
   is worse than a missing one, since scripts will silently rely on
   incorrect behavior.

---

## Milestone 133 — Math helper natives

- [ ] Add `floor value` / `ceiling value` / `truncate value` natives
      (`math.rs`, next to `round` at `:1381`) — standard rounding-mode
      variants; `round` already exists with `/to`/`/even`, these three are
      the fixed-mode shortcuts Red provides as separate words.
- [ ] Add `zero? value` / `positive? value` / `negative? value` predicates.
- [ ] Promote `sign-of` from `stdlib.red:184` to a native `math.rs`
      registration (same treatment `plan11` M112 gave `sort` — confirm
      resolution order so the native cleanly shadows/replaces the stdlib
      version) and add `sign?` if Red has both names, or just `sign-of` if
      that's the only real one — confirm before adding both.
- [ ] Promote `gcd`/`lcm` from `stdlib.red:182–183` similarly.
- [ ] Add `sinh`/`cosh`/`tanh` natives (`math.rs`, next to the existing
      `sin`/`cos`/`tan` transcendentals at `register_transcendental_natives`,
      `:1417–1450`) — pull from Rust's `f64` stdlib methods directly (no new
      crate needed).
- [ ] Add `square-root` (alias of `sqrt`) and `absolute` (alias of `abs`) —
      confirm whether Red actually has both long and short forms as
      distinct words (common in Rebol-family languages) before adding what
      would otherwise be redundant aliases.
- [ ] Investigate the `math` **evaluation-order mode** (Red's optional
      strict left-to-right arithmetic evaluation, distinct from the default
      operator-precedence evaluation) — confirm exact scope: is this a
      per-block dialect (`math [...]`) or a global eval mode? This is the
      most open-ended item in the milestone; if it requires eval-loop
      changes beyond a native wrapper, treat it as a **candidate for
      demotion to a future plan** rather than force-fitting it into v0.10's
      "additive native" non-goal constraint.
- [ ] Inline `#[test]`: `floor 3.7` → `3.0`; `ceiling 3.2` → `4.0`;
      `truncate -3.7` → `-3.0` (confirm truncate's sign behavior — toward
      zero, not toward negative infinity, matching most languages'
      `truncate`).
- [ ] Inline `#[test]`: `zero? 0` → true; `positive? 5` → true;
      `negative? -5` → true; each false-case too.
- [ ] Inline `#[test]`: `sign-of -5` → `-1`; `sign-of 0` → `0`;
      `sign-of 5` → `1`.
- [ ] Inline `#[test]`: `gcd 12 18` → `6`; `lcm 4 6` → `12`.
- [ ] Inline `#[test]`: `sinh 0` → `0.0`; round-trip check `cosh x * cosh x
      - sinh x * sinh x` ≈ `1.0` for a sample `x` (hyperbolic identity,
      cheap correctness check beyond a single reference value).
- [ ] Add golden fixtures: `math_floor_ceiling_truncate`, `math_sign_predicates`,
      `math_gcd_lcm`, `math_hyperbolic`.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M133 open questions

1. **`math` evaluation-order mode scope.** Resolve scope before committing
   to this milestone's non-goals constraint (no new `Instr`s) — if it
   can't be done as a pure native, demote it to a future plan rather than
   breaking the constraint silently.

---

## Milestone 134 — Eval reflection & error cataloging

- [ ] Add a user-level `trace` native — distinct from the CLI `--trace`
      VM-instruction dump (`red-cli/src/main.rs:32`). Confirm exact Red
      semantics (does `trace on`/`trace off` toggle a global tracing mode
      that prints each evaluated expression, or is it `trace [body]`
      wrapping a specific block? Check before implementing — this shapes
      whether it's a stateful toggle native or a scoped wrapper native).
- [ ] Add `dump value` — Red's `dump` prints a value's *label + mold* pair
      for debugging (`dump x` prints something like `x: 5`), distinct from
      both `print`/`probe` (which print the value alone) — confirm exact
      output format against Red before implementing.
- [ ] Add `stop? value` — confirm this is a real Red primitive (the audit
      flagged it; verify against docs/source — if it doesn't exist in the
      target Red version, drop it, matching the M132 caution).
- [ ] Add an `errors` catalog native — Red's built-in table of known error
      types/messages, queryable at runtime (e.g. `errors` returns a
      `block!`/`object!` enumerating the error catalog). Confirm exact
      shape against Red before implementing; this may already be partially
      covered by whatever `make error!`'s internal type table looks like
      (`convert.rs::make_error`, referenced in `plan8`'s ground-truth
      section) — reuse that table rather than duplicating it if so.
- [ ] Inline `#[test]` per confirmed-real primitive.
- [ ] Add golden fixtures for whichever primitives survive confirmation.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M134 open questions

1. **`trace` toggle-vs-scoped shape.** Resolve before implementing.
2. **`stop?` existence.** Confirm against Red docs/source; drop if not
   real, per the same caution as M132.

---

## Milestone 135 — Module extras

- [ ] Add `load-module spec` — the lower-level module-construction
      primitive `import` (`module.rs:595–619`) builds on top of internally;
      confirm whether exposing it separately adds real value over
      `make module!` (`module.rs:489–580`, already exposed) — if
      `load-module` would be a near-duplicate of `make module!`, document
      that finding and consider dropping the item rather than adding a
      confusing near-alias.
- [ ] Add `exports-of module-value` — returns the `block!` of exported
      word-symbols for a given module (read from whatever internal
      `exports:` field `make module!`'s spec-parsing already populates,
      per `module.rs:489–580` — no new storage).
- [ ] Inline `#[test]`: `exports-of import 'stdlib` returns a non-empty
      block containing at least a few known stdlib export names.
- [ ] Inline `#[test]`: `load-module` (if kept, per the confirmation above)
      round-trips against an equivalent `make module!` call.
- [ ] Add golden fixtures: `exports_of_basic`, `load_module_basic` (if kept).
- [ ] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 136 — Refinement expansion

Widen the six natives flagged in the ground-truth table to (most of) Red's
full refinement surface. Each sub-item below is independent — land them in
any order, one PR per native is reasonable given the low coupling.

### `find`

- [ ] Add `/part length` — limit the search to the first `length` elements.
- [ ] Add `/only` — for a `block!` haystack, match the *needle* as a single
      element even if it's itself a `block!` (vs. today's implicit
      sub-sequence search) — confirm exact Red semantics before
      implementing (this refinement is easy to get backwards).
- [ ] Add `/any` — wildcard matching (glob-style `*`/`?`) for `string!`
      searches.
- [ ] Add `/with wildcards` — custom wildcard character set, paired with
      `/any`.
- [ ] Add `/last` — search backward from the tail.
- [ ] Add `/tail` — return the position *after* the match instead of at it.
- [ ] Add `/match` — anchor the match at the current position only (no
      scanning forward).
- [ ] Add `/skip size` — record-wise search (mirrors `sort/skip` from
      `plan11` M112 — reuse the same skip-iteration helper if one was
      factored out there).
- [ ] Inline `#[test]` per new refinement (positive case at minimum;
      negative/no-match case for the trickier ones — `/only`, `/match`).

### `append`

- [ ] Add `/part length` — append only the first `length` elements of the
      argument series (when the argument is itself a series).
- [ ] Add `/dup count` — append `count` copies of the value.
- [ ] Add `/line` — mark the appended value with a "new line" mold hint
      (Red's line-break-preservation metadata — confirm whether this POC's
      `Series`/mold model tracks per-element line hints at all; if not,
      this refinement may require a small `Series` model extension, which
      would make it the one refinement in this milestone that isn't purely
      additive at the native layer — flag if so).
- [ ] Inline `#[test]` per new refinement.

### `copy`

- [ ] Add `/deep` — deep-copy nested blocks (today's `copy` is shallow for
      nested series — confirm exact current behavior before asserting the
      gap, since `/part`'s existing implementation may already be doing
      something deep-adjacent for a different reason).
- [ ] Add `/types typeset` — copy only elements matching a typeset (ties
      into `plan8` M89's `typeset!` — confirm that milestone's `TypesetDef`
      is reusable here without modification).
- [ ] Inline `#[test]` per new refinement.

### `replace`

- [ ] Add `/case` — case-sensitive matching (mirrors `find/case`).
- [ ] Add `/part length` — limit the search-and-replace scope.
- [ ] Inline `#[test]` per new refinement.

### `round`

- [ ] Add `/down`, `/up`, `/floor`, `/ceiling` — explicit rounding-direction
      refinements (distinct from and complementary to M133's standalone
      `floor`/`ceiling`/`truncate` natives — confirm `round`'s refinements
      and the standalone natives don't diverge in behavior for the same
      input, since users may reasonably expect `round/floor x` ==
      `floor x`).
- [ ] Add `/half-down`, `/half-up`, `/half-to-even` — tie-breaking modes for
      exact-half values (today only `/even` exists, which is presumably
      `/half-to-even` under a shorter name — confirm and consolidate rather
      than adding a duplicate).
- [ ] Inline `#[test]` per new refinement, focused on exact-half inputs
      (`2.5`, `-2.5`) where the different tie-breaking modes actually
      diverge.

### `parse`

- [ ] Add `/all` — Red's parse-all-the-way-through-input strictness mode
      (fails if the whole input isn't consumed, vs. today's presumed
      partial-match-allowed default — confirm current default behavior
      before framing `/all` as strictly additive).
- [ ] Add `/part length` — limit parsing to the first `length` elements/
      chars of the input.
- [ ] Do **not** add `/trace` here — parse tracing overlaps with M134's
      `trace` native; if both land, confirm they share an implementation
      rather than diverging (cross-reference the two milestones during
      implementation).
- [ ] Inline `#[test]` per new refinement.

### M136 closeout

- [ ] Add golden fixtures: one per refinement added (roughly 20+ across the
      six natives — batch by native: `find_refinements`, `append_refinements`,
      `copy_refinements`, `replace_refinements`, `round_refinements`,
      `parse_refinements`).
- [ ] Regression guard: every existing fixture exercising these six natives
      without the new refinements is unchanged (the whole milestone is
      additive to the refinement surface, never to default behavior).
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M136 open questions

1. **`append/line`'s `Series`-model impact.** The one item in this whole
   plan that might not be a pure native addition — resolve during
   implementation, flag as a deviation if it requires touching `Series`.
2. **`round`'s `/even` vs. `/half-to-even` naming.** Confirm before adding
   a possibly-duplicate refinement name.

---

## Milestone 137 — Polish & v0.10.0 release

- [ ] Audit `EvalError` rendering for all new error sources across
      M130–M136 (protect-mutation-denied, unconfirmed-primitive drops
      documented, refinement arg-mismatch errors, etc.).
- [ ] Golden fixture audit: confirm every new native/refinement from
      M130–M136 has at least one fixture (positive + a representative edge
      case).
- [ ] Run `cargo bench --bench eval`; record in `BENCHMARKS.md` under
      "v0.10.0" — expected neutral (all additions are native-call-path,
      no hot-path VM changes) except possibly `protect`'s per-mutation
      check (M131) and `round`'s expanded dispatch (M136); investigate any
      regression >5%.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`; fix.
- [ ] Run `cargo fmt --all --check`; fix.
- [ ] Update `project-brief.md`:
  - [ ] Add a "Feature-Parity Round-Out (v0.10)" subsection summarizing
        M130–M136, explicitly noting which speculative items (M132/M134's
        unconfirmed primitives) were dropped after Red-parity confirmation
        and why.
  - [ ] Update "Known gaps" — remove everything landed; retain/add anything
        explicitly dropped during confirmation steps (M130's `collect`/
        `keep` scoping if deferred, M132/M134's dropped items, M133's
        `math`-mode if demoted, M136's `append/line` if it required a
        `Series`-model change and got deferred instead).
- [ ] Update `architecture.md`:
  - [ ] Protect-flag enforcement points across mutating natives (M131).
  - [ ] The `collect`/`keep` dynamic-binding mechanism (M130), if
        implemented — this is novel enough to warrant an architecture note.
  - [ ] Refinement surface additions (M136) in whatever table/reference
        already documents native refinements, if one exists.
- [ ] Update `README.md`:
  - [ ] Bump version to v0.10.0.
  - [ ] Add every native landed in M130–M136 to the natives list.
  - [ ] Add every new refinement (M136) to wherever refinements are
        documented.
  - [ ] Update "Known gaps" per the project-brief change above.
- [ ] Final `cargo test --workspace` green.
- [ ] Final `cargo test --workspace --features force-walk` green.
- [ ] Final `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] Tag release `v0.10.0`.

## Open questions (plan-wide)

1. **How many M132/M134 items survive Red-parity confirmation?** Both
   milestones contain items the audit flagged by name without independently
   verifying against Red source/docs (`quote` native vs. lexer-only,
   `eval-set`, `stop?`, `trace`'s exact shape). Budget time for the
   confirmation pass itself, not just the implementation — it's plausible
   1–2 of these simply don't exist in the target Red version and should be
   dropped, not built.
2. **Cross-plan dependency: M130's `encode 'url` and `plan11` M113's HTTP
   client.** If `plan11` ships first, confirm whether M113 already grew an
   ad-hoc URL-escaping helper that M130 should reuse rather than
   duplicating.
3. **Cross-plan dependency: M136's `copy/types` and `plan8` M89's
   `typeset!`.** Confirm `TypesetDef`'s public surface (as landed in
   `plan8`) is sufficient for `copy/types`'s matching without modification.

(End of plan13-feature-parity.md)
