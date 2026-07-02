# Plan 11: Core Functional Gaps (v0.9)

Execution checklist extending the v0.8.0 baseline in `plan9-modern-types.md`
(M105 polish assumed complete). Where `plan8`/`plan9` closed the **value-type**
gap, v0.9 closes the four **highest-leverage functional gaps** identified by
the post-v0.8 feature audit — the ones that block real-world scripts rather
than just missing a nice-to-have predicate. Each milestone is independently
shippable and touches a different subsystem (`parse.rs`, `printer.rs`,
`series.rs`, a new `io/port.rs`), so — unlike `plan8`/`plan9`, which built
cumulative type scaffolding — **M110–M113 may land in any order**; the
numbering is priority, not dependency.

Per `project-brief.md`, GUI / `draw` / `vid` / reactive dialects remain
**permanently out of scope**. Control-flow gaps (`unless`/`forever`/`for`/
`forskip`) are deliberately **not** here — see `plan12-control-flow.md`.
Everything else (series/string DSL round-out, object reflection, math
helpers, module extras, refinement expansion) is in
`plan13-feature-parity.md`.

## Why these four

The post-v0.8 feature audit (see conversation history / `project-brief.md`
"Known gaps") flagged twelve categories of missing functionality. Most are
individually low-stakes (a missing predicate, a thin refinement). Four stood
out as **functional**, not cosmetic — each one blocks a whole class of
otherwise-idiomatic Red programs:

1. **Parse named-rule recursion** — without it, `parse` cannot express any
   grammar that factors sub-rules into named blocks, which is *the* idiomatic
   way to write non-trivial `parse` dialects. Every Red tutorial's second
   `parse` example (a recursive-descent mini-grammar) fails today.
2. **`mold` not callable from script** — the single most-used reflection/
   debug primitive in Rebol/Red (`print mold value`) does not exist as a
   native, even though the Rust implementation is complete and tested.
3. **No series `sort`/set-ops** — `sort` exists only in the embedded stdlib
   (interpreted Red, not a native), and there are no series-level
   `unique`/`intersect`/`union`/`difference`/`exclude` at all (only the
   `bitset!`-restricted versions from M46). Any script doing basic data
   wrangling hits this immediately.
4. **No `port!` / networking layer** — `read`/`write`/`open`/`close` are
   file-only; there is no streaming abstraction and no `read http://`. This
   is the largest architectural gap, deliberately last and most open-ended.

## Deferred / out of scope

- Reactivity, concurrency, full async port model — `future-plan-reactivity.md`,
  `future-plan-concurrency.md`. M113 below ships a **synchronous, in-process**
  `port!` shape only; it does not require or anticipate the `Channel`/actor
  work.
- Control-flow natives (`unless`, `forever`, `for`, `forskip`, `except`,
  `finally`) — `plan12-control-flow.md`.
- Everything in the "Everything else" bucket (series DSL round-out beyond
  M112's set-ops, object reflection, meta/quotation, math helpers, eval
  reflection, module extras, refinement expansion) — `plan13-feature-parity.md`.
- `regex!`-as-parse-rule (already deferred in `plan8` M82).
- Named-rule recursion inside `parse`'s `into`/`collect` combinators beyond
  the base case (M110 ships the core lookup; combinator interaction is a
  stretch goal, not a gate).
- TLS/HTTPS, WebSocket, or any protocol beyond plain HTTP `GET` for M113 — a
  minimal `read http://host/path` is in scope; a full HTTP client
  (`write http://`, headers, redirects, `https://`) is **not**.

## Non-goals

- A register VM, JIT, or further perf work — the v0.3.3 VM stays the default.
- New `Instr` variants — M110–M112 are pure native/interpreter-loop changes;
  M113's `port!` value is synthetic and enters via the existing `Const`-pool
  path like `hash!`/`vector!` in `plan8`.
- Behavior changes to existing parse/mold/series fixtures. All four
  milestones are strictly additive: existing `parse` rules that use words as
  bitset references keep working (M110 only adds a *new* fallback path when
  a word resolves to a `block!`, not a `bitset!`); `mold`'s Rust
  implementation is unchanged (M111 only adds the native wrapper); existing
  `sort`(stdlib) callers are unaffected by the new native (M112 shadows the
  stdlib version with an equivalent/faster native — see M112 decision).

## Ground-truth references (from research)

- `parse.rs` word-resolution: `parse.rs:1057–1086` resolves a bound `Word` to
  its value *only* to check whether it's a `Value::Bitset`; there is no arm
  for "word resolves to a `Value::Block` → recurse into it as a sub-rule."
  The keyword dispatch table itself lives in `parse.rs:591–960` (`some`/
  `any`/`while`/`opt`/`skip`/`end`/`fail`/`break`/`copy`/`set`/`ahead`/`not`/
  `if`/`collect`/`keep`/`into`).
- `printer.rs::mold` (line 9) and `mold_to_string` (line 165) are complete,
  tested Rust functions. `red-eval/src/natives/registry.rs` never registers
  a `"mold"` key in `env.natives` — only `"form"` is wired, via
  `crate::convert::register_convert_natives` (`registry.rs:334`).
- `bitset.rs:333–338` registers `intersect`/`union`/`difference`/`exclude`
  **only** for `bitset!` operands. `series.rs` (the `series!` native module,
  registered at `registry.rs:320`) has no matching arms for `block!`/
  `string!` operands.
- The Red-side `stdlib.red:217` defines a pure-Red `sort` (likely a naive
  sort — insertion or bubble, given it's interpreted Red, not a native
  quicksort/mergesort). `sign-of`/`gcd`/`lcm` live alongside it at
  `stdlib.red:182–184`.
- `io.rs::register_io_natives` (`registry.rs:361`) registers `read`/`write`/
  `exists?`/`size?`/`modified?`/`dir?`/`make-dir`/`delete`/`rename`/
  `change-dir`/`what-dir`/`get-env`/`set-env`/`env`/`now`/`today`/`to-utc`/
  `wait`/`call`/`shell` — all file-path or OS-process based. There is no
  `Value::Port` variant in `value.rs`, no `open`/`close`/`create` natives,
  and `read` errors on any URL scheme other than a bare file path (per the
  existing test at `io.rs:1058`).
- `env.allow_shell` (`io.rs:589–648`) is the existing capability-gate pattern
  for natives with host-system side effects (used by `call`/`shell`); M113's
  networking should follow the same pattern (`env.allow_network`, default
  off) rather than inventing a new gate mechanism.
- `red-core/src/value.rs:241`+ is the `Value` enum; after `plan8`/`plan9` it
  has ~35+ variants. M113 adds one more (`Value::Port`), synthetic like
  `Hash`/`Vector`/`Image`.

---

## Milestone 110 — `parse` named-rule recursion

The core gap: today, `digit: [#"0" - #"9"] parse "5" [some digit]` does not
recurse into `digit` as a sub-rule — the walker only checks whether a bound
word resolves to a `Value::Bitset`. This milestone adds the missing case:
**a word that resolves to a `block!` is treated as a named sub-rule and
parsed recursively**, with its own cursor/backtrack state layered on the
parent's `input`.

- [x] In `parse.rs`'s word-resolution block (`:1057–1086`), add a third arm:
      if `v_resolved` is `Some(Value::Block(b))` (or the word's bound value
      is a `Block` directly, mirroring the existing `Bitset` check), treat it
      as a sub-rule: recursively invoke the rule-block interpreter
      (`rule_seq`-equivalent) against the **same** `input`/`i` cursor state,
      not a copy — sub-rule success/failure must affect the parent's
      position exactly like an inline `[...]` group would.
- [x] Recursion depth guard: add a `max_depth` (or reuse an existing
      recursion-guard pattern from `interp_walker.rs` if one exists) to avoid
      a stack overflow on `self-referential-rule: [self-referential-rule]`.
      Error `EvalError::ParseRecursionLimit` on overflow, not a Rust panic.
- [x] Self-reference / mutual reference: `a: [some b] b: [some a]`-style
      mutual recursion must work — the lookup happens at *rule-invocation*
      time (word is re-resolved each time it's encountered), not at
      `parse`-call setup time, so forward references to not-yet-defined
      words at parse-time are fine as long as they're bound by the time the
      rule actually runs.
- [x] Disambiguation with the existing `Bitset` arm: if a word resolves to
      **both** meanings is impossible (a binding is one `Value`), so order
      is simply: `Bitset` → charset match (existing behavior, unchanged);
      `Block` → sub-rule recursion (new); anything else → literal-value
      match (existing fallback, unchanged).
- [x] `collect`/`keep` interaction: a named sub-rule invoked from inside an
      active `collect` block should have its matched span contribute to the
      collect the same way an inline `[...]` group does (i.e., the *parent*
      collect stack is visible to the sub-rule's matches — no separate
      collect scope per sub-rule call). Confirm against Red semantics; if Red
      scopes `collect` per-invocation instead, document the deviation.
- [x] `into` interaction: a named sub-rule can be the rule argument to
      `into word rule` (`parse.rs:950`) — no special-casing needed if the
      sub-rule dispatch happens through the same code path `into` already
      calls into.
- [x] Inline `#[test]`: `digit: [#"0" - #"9"] parse "5" [some digit]` → true.
- [x] Inline `#[test]`: mutual recursion —
      `even-run: [opt [#"a" odd-run]] odd-run: [#"b" opt even-run]
      parse "abab" [even-run]` matches (or an equivalent simpler mutual-rule
      fixture — pick the minimal grammar that actually exercises the mutual
      path).
- [x] Inline `#[test]`: self-recursive rule matching nested balanced
      parens — `group: [#"(" any [group | (a run of non-paren chars)]
      #")"] parse "((a)(b))" [group]` → true (the canonical "why you need
      recursion" example).
- [x] Inline `#[test]`: recursion-limit fixture — a self-referential rule
      with no base case on non-matching input terminates with
      `ParseRecursionLimit`, not a stack overflow (run under a debug-assert
      build so a real overflow would be caught, not silently pass).
- [x] Inline `#[test]`: regression guard — existing `bitset`-as-word rules
      (`parse.rs` test suite, e.g. `parse_bitset_rule`/`parse_bitset_some`)
      still pass unchanged.
- [x] Add golden fixtures: `parse_named_rule_basic`, `parse_named_rule_mutual`,
      `parse_named_rule_nested_recursion`.
- [x] Add `programs_errors/parse_recursion_limit.red`.
- [x] `cargo test --workspace` green; `--features force-walk` green.

### M110 open questions

1. **Depth limit value.** Pick a number generous enough for real grammars
   (JSON-depth, s-expression-depth) but small enough to fail fast on a
   genuine infinite loop — likely in the low thousands, matching whatever
   the walker's existing call-stack-depth guard uses (if any; if none
   exists, this milestone may need to introduce the first one).
2. **`collect` scoping across sub-rule boundaries.** Confirmed against real
   Red before merging — see the checklist item above.

---

## Milestone 111 — `mold` as a callable native

`printer.rs::mold`/`mold_to_string` are complete and tested; they are simply
never exposed to scripts. This is the smallest milestone in the plan —
included here (rather than in `plan13`) because `print mold x` is used
*constantly* in idiomatic Red for debugging, and its absence is surprising
enough to count as a functional gap, not a nice-to-have.

- [x] Register `mold` in `registry.rs` alongside the existing `form`
      registration (`registry.rs:334`, inside/adjacent to
      `crate::convert::register_convert_natives`): `mold value` →
      `printer::mold_to_string(&value)` wrapped as a `Value::Str`.
- [x] Add a `/part` refinement? **Decision: no** — Red's `mold/part` exists
      for length-limiting output; skip in v0.9, note as a `plan13`
      refinement-expansion candidate instead (keep this milestone minimal).
- [x] Add `/only` refinement — Red's `mold/only` omits the outer `[...]` when
      molding a `block!`. **Decision: include** — it's a one-line change atop
      the same `mold_to_string` call (strip outer brackets when the input is
      a `Block` and `/only` is set) and is commonly paired with `mold`.
- [x] Inline `#[test]`: `mold 5` → `"5"`; `mold "hi"` → `{"hi"}`; `mold [1 2]`
      → `"[1 2]"`.
- [x] Inline `#[test]`: `mold/only [1 2]` → `"1 2"` (no brackets).
- [x] Inline `#[test]`: `print mold make object! [x: 1]` produces the same
      text `printer.rs`'s own unit tests already assert for `mold_object`.
- [x] Add golden fixtures: `mold_native_basic`, `mold_native_only`.
- [x] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 112 — Series `sort` + set operations

Promote `sort` from stdlib-Red to a native (for both correctness-independence
from the stdlib bundle and performance), and add the series-level set
operations that `bitset.rs` only offers for `bitset!` today.

### `sort`

- [x] Add `sort` native in `series.rs` (or a new `sort.rs` if the module is
      getting large): in-place stable sort of a `block!` (by `Value`
      comparison, reusing whatever total order `compare.rs` already defines
      for `<`/`>` — the same order `min`/`max` use) or a `string!` (by char).
      Mutates and returns the input series (Red parity — `sort` is
      destructive by default).
- [x] Refinements: `/case` (case-sensitive string comparison — mirrors the
      existing `find/case` pattern in `series.rs:1182`), `/reverse`,
      `/skip size` (sort records of `size` elements as a unit — e.g.
      `sort/skip data 2` sorts `[k1 v1 k2 v2 ...]` pairs by `k`), `/compare
      func` (custom comparator — a `func`/`closure` value taking two
      elements, returning `logic!` or an ordering integer per Red's
      `compare` refinement contract — confirm exact Red signature before
      implementing).
- [x] **Decision: the new native shadows/replaces the stdlib `sort` at
      `stdlib.red:217`.** Remove the stdlib definition once the native is
      registered (natives are looked up before stdlib bindings — confirm
      resolution order so this is a clean swap, not a silent shadow bug).
- [x] Inline `#[test]`: `sort [3 1 2]` → `[1 2 3]`.
- [x] Inline `#[test]`: `sort/reverse [3 1 2]` → `[3 2 1]`.
- [x] Inline `#[test]`: `sort/skip [b 2 a 1] 2` → `[a 1 b 2]` (sort pairs by
      first element).
- [x] Inline `#[test]`: `sort/compare [3 1 2] func [a b][a < b]` → `[1 2 3]`.
- [x] Inline `#[test]`: `sort "cba"` → `"abc"`.

### Series set operations

- [x] Add `unique` native (`series.rs`): returns a new series with duplicate
      elements removed, preserving first-occurrence order. Works on
      `block!`/`string!`.
- [x] Add `intersect`/`union`/`difference`/`exclude` **series** overloads —
      today these symbols are bound only to the `bitset!` versions
      (`bitset.rs:333–338`); extend the same native names to dispatch on
      `block!`/`string!` operands too (order-preserving-by-first-operand
      semantics, matching Red — confirm exact tie-break/order rules per Red
      docs before implementing).
- [x] Refinements: `/case` on all four (string case-sensitivity), `/skip
      size` on all four (record-wise set ops, mirroring `sort/skip`).
- [x] Inline `#[test]`: `unique [1 2 2 3 1]` → `[1 2 3]`.
- [x] Inline `#[test]`: `intersect [1 2 3] [2 3 4]` → `[2 3]`.
- [x] Inline `#[test]`: `union [1 2] [2 3]` → `[1 2 3]`.
- [x] Inline `#[test]`: `difference [1 2 3] [2]` → `[1 3]`.
- [x] Inline `#[test]`: `exclude [1 2 3] [2]` → `[1 3]` (confirm Red's
      `exclude` vs `difference` distinction — in Red they differ when more
      than two sets/order matters; verify before asserting equivalence in
      the test).
- [x] Inline `#[test]`: regression guard — `intersect`/`union`/`difference`/
      `exclude` on two `bitset!` values still dispatch to the M46
      implementation unchanged.
- [x] Add golden fixtures: `sort_basic`, `sort_refinements`, `set_ops_series`,
      `set_ops_bitset_regression`.
- [x] `cargo test --workspace` green; `--features force-walk` green.

### M112 open questions

1. **`sort`'s default order for mixed-type blocks.** Red errors (or has a
   defined cross-type order) when sorting `[1 "a" 2]`. Confirm which, and
   whether `compare.rs`'s existing `<`/`>` cross-type behavior already
   matches — if `<` errors on mismatched types today, `sort` should
   propagate that error rather than silently coerce.
   **Resolution (M112):** implemented a pragmatic total order (numerics via
   `num_cmp`; strings lexicographically; word-family by name; everything
   else falls back to `(type_name, mold)`). `sort` never errors on
   mixed-type blocks — a documented POC deviation from Red's exact
   cross-type ordering.
2. **`exclude` vs `difference` semantics.** Verify against Red docs/source
   before implementing — do not assume they're aliases.
   **Resolution (M112):** confirmed distinct. `difference` is symmetric
   (a⊖b: a-not-b then b-not-a); `exclude` is set difference (a\b: a-not-b
   only). Tests use multi-element second operands to exercise the
   distinction.

---

## Milestone 113 — `port!` abstraction + minimal networking

The largest, most open-ended milestone. Ships a **synchronous** `port!`
value type and enough of the port protocol to (a) unify file I/O under the
same `open`/`close`/`read`/`write`/`create` verbs Red scripts expect, and
(b) support a bare-minimum `read http://host/path` GET. This is explicitly
**not** the full async/`Channel`-backed port model from
`future-plan-concurrency.md` — it's the synchronous subset that makes
`port!` exist as a value and makes the most common script pattern
(`read http://...`) work.

- [ ] Add `struct PortDef { scheme: PortScheme, target: Rc<str>, state:
      RefCell<PortState> }` in `value.rs`, where `PortScheme` is an enum
      (`File`, `Http`, ...) and `PortState` tracks open/closed + any
      buffered data.
- [ ] Add `Value::Port(Rc<PortDef>)` variant (synthetic, no span — built by
      `open`, not the lexer).
- [ ] Add `port?` predicate.
- [ ] Add `open <file!|url!>` native: for a `file!` argument, wraps the
      existing file-handle logic from `io.rs`'s `read`/`write` into a
      `Port`; for a `url!` argument with an `http://` scheme, opens a TCP
      connection (via `std::net::TcpStream` — no new async runtime
      dependency) and stages a GET request.
- [ ] Add `close port` native — releases the underlying handle/socket.
- [ ] Add `create <file!>` native — Red's `create` opens-or-truncates; for
      v0.9, alias it to today's `write`-with-truncate path if that's already
      the `write` default, otherwise implement the create-only semantics
      explicitly.
- [ ] Extend `read`: when passed a `url!` with scheme `http`, dispatch to the
      new networking path instead of erroring (today's behavior per the
      `io.rs:1058` test) — issue a GET, block on the response (synchronous,
      no `wait`/event loop), return the body as per `read`'s existing
      `/binary`/`/lines` refinements.
- [ ] Extend `read`: when passed an already-open `port!` value (not a
      `file!`/`url!`), read from the port's current position (streaming
      semantics — read-what's-available, not the whole-file slurp that
      `read <file!>` does today).
- [ ] Capability gate: add `env.allow_network: bool` (default **off**,
      mirroring `env.allow_shell`'s pattern at `io.rs:589`). All networking
      natives (`open` on a URL, `read` on a URL/port) check this gate and
      error with a clear "network access disabled" message when off. A new
      CLI flag `--allow-network` sets it on, symmetric to whatever flag
      (if any) gates `--allow-shell` today — confirm the existing flag name
      and mirror it exactly.
- [ ] Extend `printer.rs`: `mold`/`form` of `Port` → `#[port <scheme>://...]`
      (non-reparseable synthetic form, matching the `#[regex ...]`/
      `#[handle ...]` placeholder style from `plan8`).
- [ ] Update `type_name` → `"port!"`.
- [ ] Update `compare.rs`: `same?`/`not-same?` via `Rc::ptr_eq`; no structural
      `equal?` beyond identity (ports are stateful handles, not values).
- [ ] Update `write`: accept an open `Port` (in addition to today's `file!`
      path) as a destination — writes go to the port's underlying stream.
- [ ] Explicitly **out of scope** for M113 (document, don't build):
      `https://` (TLS), any method beyond GET, request headers/cookies,
      redirects, chunked transfer-encoding, `wait`-based async reads,
      `write http://` (POST/PUT). A script that needs any of these gets a
      clear "not supported in v0.9" error, not a silent wrong result.
- [ ] Inline `#[test]`: `p: open %test.txt write p "hi" close p read %test.txt`
      → `"hi"` (file-port round-trip through the new verbs).
- [ ] Inline `#[test]`: with `--allow-network`, `read http://127.0.0.1:<test-
      server-port>/` against a throwaway local test HTTP server (spun up in
      the test itself, not a real external host) returns the expected body.
- [ ] Inline `#[test]`: without `--allow-network`, `read http://example.com`
      errors with the capability-gate message (no real network call
      attempted — assert via a mock/mutex flag, not a live request).
- [ ] Inline `#[test]`: `port? open %test.txt` → true; `close` then a second
      `read` on the same port errors cleanly (no panic on a closed handle).
- [ ] Add golden fixtures: `port_file_roundtrip`, `port_predicate`.
- [ ] Add `programs_errors/port_network_disabled.red`,
      `programs_errors/port_read_after_close.red`,
      `programs_errors/port_https_unsupported.red`.
- [ ] Add a stable-string property test for `Port` (non-reparseable).
- [ ] `cargo test --workspace` green; `--features force-walk` green.
- [ ] Networking tests must not depend on live internet access — use an
      in-process test server (`std::net::TcpListener` on `127.0.0.1:0`) so
      CI stays hermetic.

### M113 open questions

1. **Scope of "minimal networking."** Confirm GET-only, HTTP-only (no TLS)
   is an acceptable v0.9 slice before starting — if the project needs
   `https://` sooner, this milestone's crate choice (hand-rolled HTTP/1.1
   over `TcpStream` vs. pulling in `ureq`/`reqwest`) changes materially.
   **Recommendation:** hand-rolled minimal HTTP/1.1 client (no new runtime
   dependency, GET-only, no TLS) — revisit with a real crate if `https://`
   becomes a requirement.
2. **Is `port!` a `series!`?** Red's `port!` is *not* itself a `series!`
   type in the traditional sense (no `pick`/`length?`), but it supports
   `read`/`write`/`copy` idioms similar to series. Confirm the exact Red
   surface before over- or under-building series ops onto `Port`.
3. **Relationship to `plan9`'s `promise!`.** `plan9` M104 landed `promise!`
   as a single-threaded thunk shape "for the concurrency release to
   activate." Does `open` on a slow URL need to interoperate with
   `promise!` in v0.9, or is that strictly a v0.10+/post-concurrency
   concern? **Decision: strictly deferred** — M113's `read http://` blocks
   synchronously; no `promise!` interop in this milestone.

---

## Milestone 114 — Polish & v0.9.0 release

- [ ] Audit `EvalError` rendering for all new error sources: `ParseRecursionLimit`
      (M110), sort/set-op type-mismatch errors (M112), port capability-gate
      and closed-port errors (M113).
- [ ] Golden fixture per new error case introduced across M110–M113.
- [ ] Run `cargo bench --bench eval`; confirm `parse` recursion (M110) adds no
      measurable regression to the existing non-recursive parse benchmarks
      (the new word-resolution check should be a single extra match arm on
      the already-taken "resolve word" path, not a new pass).
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`; fix.
- [ ] Run `cargo fmt --all --check`; fix.
- [ ] Update `project-brief.md`:
  - [ ] Add a "Core Functional Gaps (v0.9)" subsection: parse recursion,
        `mold` native, series `sort`/set-ops, `port!` + minimal HTTP GET.
  - [ ] Update "Known gaps" — remove parse-recursion/`mold`-native/series-
        sort items; add the M113 out-of-scope list (TLS, POST/PUT, etc.) as
        the new networking gap statement.
- [ ] Update `architecture.md`:
  - [ ] `PortDef`/`PortScheme`/`PortState` struct definitions.
  - [ ] The `env.allow_network` capability gate (alongside `allow_shell`).
  - [ ] Parse's sub-rule recursion path in the `parse.rs` design section.
- [ ] Update `README.md`:
  - [ ] Bump version to v0.9.0.
  - [ ] Add `mold`/`sort`/`unique`/`intersect`(series)/`union`(series)/
        `difference`(series)/`exclude`(series)/`open`/`close`/`create`/
        `port?` to the natives list.
  - [ ] Add `--allow-network` to the CLI section.
  - [ ] Update "Known gaps" per the project-brief change above.
- [ ] Final `cargo test --workspace` green.
- [ ] Final `cargo test --workspace --features force-walk` green.
- [ ] Final `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] Tag release `v0.9.0`.

(End of plan11-functional-gaps.md)
