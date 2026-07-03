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
   M113 closes the synchronous subset: a `port!` value, a `net/` protocol
   facade (per `rust-networking-protocol-crate-recommendation.md`), and
   HTTP/HTTPS GET via the **existing** `ureq` dep (TLS is on by default in
   ureq 2.x — no new dependency).

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
- For M113: WebSocket; non-HTTP protocols (FTP/SMTP/POP3/NNTP/DNS/TCP/
  UDP/WHOIS/Finger/Daytime — reserved as `PortScheme` variants that error in
  v0.9, land in v0.10+); HTTP methods beyond GET; request headers/cookies/
  auth; redirect control; `write http://` (POST/PUT); and the async port
  model. A minimal `read http://host/path` and `read https://host/path`
  GET **is** in scope; a full HTTP client is **not**.

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
  `Value::Port` variant in `value.rs` and no `open`/`close`/`create`
  natives. **`read url!` for `http://`/`https://` already works today**
  (`io.rs:119-121` dispatches to `fetch_url` at `io.rs:144-163`, which calls
  `ureq::get(url).call()` and slurps the body as a `string!`). The test at
  `io.rs:1058` (`read_url_wrong_scheme_errors`) only covers non-http/https
  schemes (e.g. `ftp://`), **not** all URL reads. So M113's actual gaps are:
  (a) no `port!` value type or `open`/`close`/`create` natives, (b)
  `read url!` is a whole-body slurp with no streaming / no lazy body,
  (c) **no network capability gate** — `read http://...` works today with no
  `--allow-network` gate, unlike `call`/`shell` which `allow_shell` gates
  (a sandbox hole M113 closes), and (d) no `net/` facade for future
  protocols.
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

Ships a **synchronous** `port!` value type and a protocol facade
(`red-eval/src/net/`) layered over `ureq` for HTTP/HTTPS. Note that
`read url!` for `http://`/`https://` **already works today** via an ad-hoc
`fetch_url` helper in `io.rs:144-163` (slurp-mode, no capability gate);
M113 formalizes that path into a proper `port!` abstraction with streaming,
a network capability gate, and an extensible facade for the v0.10+ protocol
breadth. Other protocols from the project's crate-recommendation research
(`rust-networking-protocol-crate-recommendation.md`) are **reserved as
`PortScheme` enum variants but not implemented in v0.9** — they error with a
clear "not supported in v0.9" message rather than silently misbehaving.
This is explicitly **not** the async/`Channel`-backed port model from
`future-plan-concurrency.md` — it's the synchronous subset that makes
`port!` exist as a value, unifies file I/O under the same `open`/`close`/
`read`/`write`/`create` verbs Red scripts expect, gates network access
(closing today's sandbox hole where `read http://` works unconditionally),
and adds streaming so `read open url!` doesn't slurp the whole body.

**Crate strategy (per the recommendation doc):** compose small, mature
crates behind a uniform facade instead of hand-rolling protocol parsers.
v0.9 reuses the **existing** `ureq = "2"` dependency already pulled in by
`red-eval` for `read url!` — `ureq` 2.x ships TLS **on by default**
(`rustls` + `webpki-roots` are already in `Cargo.lock`), so HTTPS works
with **zero `Cargo.toml` changes**. FTP/SMTP/DNS/TCP-socket/WHOIS/Finger/
Daytime land in v0.10+ by adding `suppaftp`/`lettre`/`domain` and sibling
files under `net/`; the facade is designed now to make that a pure
addition, not a rework.

- [x] **New `crates/red-eval/src/net/` module** (facade, per the
      recommendation doc's example layout):
      `mod.rs` (public API: `open`, `read`, `write`, `close`),
      `protocol.rs` (`PortScheme` enum), `request.rs`/`response.rs`
      (uniform request/response model — `NetworkRequest`/
      `NetworkResponse`/`NetworkOptions`/`NetworkStatus`),
      `error.rs` (`NetError`), `http.rs` (`ureq`-backed HTTP/HTTPS GET).
      Future `ftp.rs`/`smtp.rs`/`dns.rs`/`whois.rs`/`finger.rs`/
      `daytime.rs`/`tcp.rs`/`udp.rs` are stubbed with a "not supported in
      v0.9" `NetError::UnsupportedInV09(scheme)` so the file tree matches
      the planned v0.10+ surface and the dispatch table is obviously
      extensible.
- [x] `PortScheme` enum in `net/protocol.rs`:
      `File`, `Http` (covers both `http://` and `https://` — TLS is a
      `NetworkOptions.tls` flag, not a separate scheme, matching `ureq`'s
      URL-driven model), `Ftp`, `Smtp`, `Pop3`, `Nntp`, `Dns`, `Tcp`,
      `Udp`, `Whois`, `Finger`, `Daytime`. Only `File` and `Http` have
      live dispatch; the rest return `NetError::UnsupportedInV09(scheme)`.
- [x] `struct PortDef { scheme: PortScheme, target: Rc<str>, state:
      RefCell<PortState> }` in `value.rs`. `PortState` tracks open/closed,
      an in-process buffered cursor for streaming reads, and (for HTTP)
      the `ureq::Response` body `Read` handle held across multiple
      `read port` calls so the body is not slurped at `open` time.
- [x] Add `Value::Port(Rc<PortDef>)` variant (synthetic, no span — built by
      `open`, not the lexer).
- [x] Add `port?` predicate.
- [x] Add `open <file!|url!>` native: dispatches by URL scheme to
      `net::open`. For a `file!` argument, wraps the existing file-handle
      logic from `io.rs`'s `read`/`write` into a `Port` (read/write
      semantics unchanged). For a `url!` with scheme `http://` or
      `https://`, issues a `ureq` GET **at `open` time** (not `read`
      time) — the response status + headers are materialized immediately
      (so `open` can fail fast on DNS/connection/HTTP errors), but the
      **body is read lazily** on subsequent `read port` calls. This
      matches Red's `open` semantics and the recommendation doc's
      `net::open("http://…")` example.
- [x] Add `close port` native — drops the `PortState`, releasing the
      `ureq` body `Read` handle or file handle.
- [x] Add `create <file!>` native — Red's `create` opens-or-truncates;
      for v0.9, alias it to today's `write`-with-truncate path if that's
      already the `write` default, otherwise implement the create-only
      semantics explicitly.
- [x] Extend `read`: when passed a `url!` with scheme `http`/`https`,
      route through the new `net/` facade (`net::open` + immediate
      streaming `read`) instead of the ad-hoc `fetch_url` helper at
      `io.rs:144-163`. The one-shot `read url!` ergonomics are preserved
      (equivalent to `read open url!` as a single call), and the existing
      `/binary`/`/lines` refinements still apply to the materialized body.
      **Migrate `fetch_url`'s call sites to `net::http`** and delete the
      `fetch_url` helper once `read url!` routes through the facade — the
      `io.rs:1058` `read_url_wrong_scheme_errors` test should be updated to
      assert the new `NetError::UnsupportedInV09` (or scheme-mismatch)
      message rather than the old `"not supported in POC"` string.
- [x] Extend `read`: when passed an already-open `Port` (not a
      `file!`/`url!`), read from the port's current position — streaming
      for HTTP ports (read-what's-available, not a whole-body slurp) and
      whole-file for file ports (matching today's `read <file!>` behavior).
- [x] Extend `write`: accept an open `Port` as a destination. HTTP ports
      **error** (`NetError::UnsupportedInV09("http-write")` — GET-only is
      v0.9's stance); file ports write through to the underlying file
      handle.
- [x] **Capability gate:** add `env.allow_network: bool` (default **off**,
      mirroring `env.allow_shell`'s pattern at `io.rs:589`). All
      networking natives (`open` on a URL, `read` on a URL/HTTP-port)
      check this gate and error with a clear "network access disabled"
      message when off. New CLI flag `--allow-network` mirrors
      `--allow-shell` exactly (same parsing path in `main.rs`).
      **Backwards-compat note:** today `read url!` works with no gate; v0.9
      makes it gated by default. Any existing script (or `#[ignore]`-marked
      url-read test in `io.rs`) that relies on ungated network access must
      either set `env.allow_network = true` in Rust tests or be invoked
      with `--allow-network` from the CLI. Audit the existing `#[ignore]`
      url tests in `io.rs` and update them to set the flag.
- [x] **Verify `ureq` TLS availability (no `Cargo.toml` change expected):**
      confirm `ureq::get("https://...").call()` works against the existing
      `ureq = "2"` pin — `rustls` + `webpki-roots` are already in
      `Cargo.lock` from ureq's default features. If a future ureq minor
      bumps TLS off default, add `features = ["tls"]` to the dep line;
      otherwise no manifest edit is required.
- [x] Extend `printer.rs`: `mold`/`form` of `Port` →
      `#[port <scheme>://<target>]` (non-reparseable synthetic form,
      matching the `#[regex ...]`/`#[handle ...]` placeholder style from
      `plan8`).
- [x] Update `type_name` → `"port!"`.
- [x] Update `compare.rs`: `same?`/`not-same?` via `Rc::ptr_eq`; no
      structural `equal?` beyond identity (ports are stateful handles,
      not values).
- [x] **Explicitly out of scope for v0.9** (document via
      `NetError::UnsupportedInV09`, don't build): FTP/SMTP/POP3/NNTP/DNS/
      TCP/UDP/WHOIS/Finger/Daytime; HTTP methods beyond GET; request
      headers/cookies/auth; redirect control (ureq follows redirects by
      default — we do not expose redirect tuning); chunked transfer-encoding
      hand-rolling; `wait`-based async reads; `write http://` (POST/PUT).
      A script hitting any of these gets the clear error, not a silent
      wrong result.
- [x] Inline `#[test]`: file-port round-trip —
      `p: open %test.txt write p "hi" close p read %test.txt` → `"hi"`.
- [x] Inline `#[test]`: with `--allow-network`, `read http://127.0.0.1:<port>/`
      against an in-process `std::net::TcpListener` test server returns the
      expected body.
- [x] Inline `#[test]`: with `--allow-network`, `read https://127.0.0.1:<port>/`
      against the in-process test server with a self-signed cert (use
      `ureq::AgentBuilder::tls_config` with a `rustls::ClientConfig` that
      trusts the test cert; if awkward, mark `#[ignore]` with a documented
      manual run rather than skipping the HTTPS path entirely).
- [x] Inline `#[test]`: without `--allow-network`, `read http://example.com`
      errors with the capability-gate message — assert via a mock flag on
      `Env`, **no real network call attempted**.
- [x] Inline `#[test]`: `port? open %test.txt` → true; `close` then a
      second `read` on the same port errors cleanly (no panic on a closed
      handle).
- [x] Inline `#[test]`: unsupported-scheme dispatch —
      `open ftp://...` / `open whois://...` → `NetError::UnsupportedInV09`
      with the scheme name in the message.
- [x] Inline `#[test]`: streaming — `p: open http://<test-server/large-body>`
      then two `read p` calls return distinct chunks (assert the body is
      not slurped on `open`).
- [x] Add golden fixtures: `port_file_roundtrip`, `port_predicate`,
      `port_http_read` (the HTTP golden is guarded by `--allow-network` +
      in-process server — if the golden-runner infra can't spin a server,
      keep it as an inline `#[test]` instead and drop the golden).
- [x] Add `programs_errors/port_network_disabled.red`,
      `programs_errors/port_read_after_close.red`,
      `programs_errors/port_unsupported_scheme.red`.
- [x] Add a stable-string property test for `Port` (non-reparseable
      `mold`).
- [x] `cargo test --workspace` green; `--features force-walk` green.
- [x] Networking tests must not depend on live internet access — use an
      in-process `std::net::TcpListener` on `127.0.0.1:0` only; CI stays
      hermetic.

### M113 open questions

1. **Scope of "minimal networking."** *Resolved:* GET-only HTTP+HTTPS via
   the existing `ureq = "2"` dep (TLS on by default in ureq 2.x; no new
   dependency). Other protocols from the recommendation doc (`ftp`/`smtp`/
   `pop3`/`nntp`/`dns`/`tcp`/`udp`/`whois`/`finger`/`daytime`) are
   reserved as `PortScheme` enum variants that error in v0.9, landing in
   v0.10+ via `suppaftp`/`lettre`/`domain` and direct `std::net` code.
   The original "hand-rolled HTTP/1.1 over `TcpStream`, no TLS" stance
   was abandoned because `ureq` was already a `red-eval` dependency and
   HTTPS comes free with it.
2. **Is `port!` a `series!`?** Red's `port!` is *not* itself a `series!`
   type in the traditional sense (no `pick`/`length?`), but it supports
   `read`/`write`/`copy` idioms similar to series. Confirm the exact Red
   surface before over- or under-building series ops onto `Port`.
   **Recommendation:** ship no series ops on `Port` in v0.9; defer to
   v0.10+ alongside the broader protocol surface.
3. **Relationship to `plan9`'s `promise!`.** *Resolved:* strictly
   deferred — M113's `read http://` blocks synchronously; no `promise!`
   interop in this milestone.
4. **Streaming-read buffer size** (new). The HTTP-port `read` returns
   "what's available" — pick a chunk size (e.g. 8 KiB) for a single
   `read port` call, and document that `read port` may return an empty
   string/block at EOF (unlike file `read` which slurps). Confirm
   against Red's `read port` semantics before locking the buffer
   contract.
5. **TLS test-cert strategy** (new). `https://` is in scope; the
   in-process test server needs a self-signed cert + a `ureq::Agent`
   configured to trust it (or skip verification in tests via
   `AgentBuilder::tls_config`). Confirm `ureq` 2.x's test affordances
   before writing the HTTPS fixture; fall back to `#[ignore]` with a
   documented manual run if it's awkward.

---

## Milestone 114 — Polish & v0.6.0 release

- [x] Audit `EvalError` rendering for all new error sources: `ParseRecursionLimit`
      (M110), sort/set-op type-mismatch errors (M112), port capability-gate
      and closed-port errors (M113), and `NetError::UnsupportedInV09` for
      reserved-but-unimplemented schemes (M113).
- [x] **Backwards-compat audit for the network gate (M113).** Beforev0.6,
      `read url!` worked with no gate. v0.9 gates it behind
      `env.allow_network` (default off). Audit every existing `read url!`
      call site — in tests (`io.rs`'s `#[ignore]` url tests, any golden
      fixture), in `examples/`, and in `stdlib.red` — and either set the
      flag (Rust tests) or pass `--allow-network` (CLI/golden). Confirm no
      existing test silently starts erroring "network access disabled"
      without being explicitly updated.
- [x] Golden fixture per new error case introduced across M110–M113.
- [x] Run `cargo bench --bench eval`; confirm `parse` recursion (M110) adds no
      measurable regression to the existing non-recursive parse benchmarks
      (the new word-resolution check should be a single extra match arm on
      the already-taken "resolve word" path, not a new pass).
- [x] Run `cargo clippy --workspace --all-targets -- -D warnings`; fix.
- [x] Run `cargo fmt --all --check`; fix.
- [x] **Verify `ureq` TLS still on by default** (M113 dependency strategy):
      confirm `cargo tree -p red-eval` still pulls `rustls` + `webpki-roots`
      transitively from `ureq = "2"`; confirm `ureq::get("https://...")`
      compiles and resolves in the HTTPS fixture. No `Cargo.toml` change is
      expected — if a ureq minor bump flips TLS off default, add
      `features = ["tls"]` to the `ureq` dep line and re-run the HTTPS
      fixture.
- [x] **Confirm no new top-level dependency was added** (M113 was supposed
      to reuse the existing `ureq` pin, not pull `reqwest`/`hyper`/etc.).
      `cargo tree -p red-eval` should show no new direct deps vs. the v0.8
      baseline; `suppaftp`/`lettre`/`domain` are explicitly v0.10+ and must
      not appear in the v0.9 lockfile.
- [x] Update `project-brief.md`:
  - [x] Add a "Core Functional Gaps (v0.9)" subsection: parse recursion,
        `mold` native, series `sort`/set-ops, `port!` + minimal HTTP/HTTPS
        GET (via the existing `ureq` dep — TLS on by default).
  - [x] Update "Known gaps" — remove parse-recursion/`mold`-native/series-
        sort items; replace the networking gap statement. **TLS/HTTPS is no
        longer a gap** (ureq 2.x ships TLS on by default). The new gap
        statement lists: non-HTTP protocols (FTP/SMTP/POP3/NNTP/DNS/TCP/
        UDP/WHOIS/Finger/Daytime — reserved as `PortScheme` variants that
        error in v0.9), HTTP methods beyond GET, request headers/cookies/
        auth, redirect control, `write http://` (POST/PUT), and the async
        port model.
  - [x] Reference `rust-networking-protocol-crate-recommendation.md` as
        the source for the composed-facade decision (so the rationale is
        discoverable from the brief, not just conversation history).
- [x] Update `architecture.md`:
  - [x] `PortDef`/`PortScheme`/`PortState` struct definitions.
  - [x] The `crates/red-eval/src/net/` facade module tree (`mod.rs`/
        `protocol.rs`/`request.rs`/`response.rs`/`error.rs`/`http.rs`),
        with a note that v0.10+ adds sibling protocol files (`ftp.rs`/
        `smtp.rs`/`dns.rs`/`whois.rs`/`finger.rs`/`daytime.rs`/`tcp.rs`/
        `udp.rs`) without changing the `port!` value shape or the public
        `open`/`read`/`write`/`close` surface.
  - [x] The full `PortScheme` enum (12 variants: `File`/`Http` live in
        v0.9; `Ftp`/`Smtp`/`Pop3`/`Nntp`/`Dns`/`Tcp`/`Udp`/`Whois`/
        `Finger`/`Daytime` reserved, erroring with
        `NetError::UnsupportedInV09`).
  - [x] The `env.allow_network` capability gate (alongside `allow_shell`).
  - [x] Parse's sub-rule recursion path in the `parse.rs` design section.
- [x] Update `README.md`:
  - [x] Bump version to v0.9.0.
  - [x] Add `mold`/`sort`/`unique`/`intersect`(series)/`union`(series)/
        `difference`(series)/`exclude`(series)/`open`/`close`/`create`/
        `port?` to the natives list.
  - [x] Add a "Networking" subsection documenting the `port!` surface:
        `open`/`close`/`create`/`read port`/`read url!` (file + HTTP/HTTPS
        GET), the `--allow-network` capability gate, and the explicit
        v0.9 boundary (no non-HTTP protocols, no POST/PUT, no headers).
  - [x] Add `--allow-network` to the CLI section.
  - [x] Update "Known gaps" per the project-brief change above.
- [x] Final `cargo test --workspace` green.
- [x] Final `cargo test --workspace --features force-walk` green.
- [x] Final `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [x] Tag release `v0.6.0`.

---

## Milestone 138 — `parse` integer-count rules

The gap surfaced by `digit: charset "0123456789"` then `parse "2026-07-03"
[4 digit "-" 2 digit "-" 2 digit]` returning `false`. Per Red's parse
dialect spec (§3.6.1 "Iteration count"), an `integer!` in a rule block is
a **count prefix**: `n rule` matches `rule` exactly `n` times, and
`n m rule` matches `rule` between `n` and `m` times (inclusive range,
lower ≤ upper). The count may be an `integer!` literal or a `word!`
referring to a non-negative `integer!` (`THX: 1138 … 0 thx [rule]`).

The pre-M138 implementation treated `integer!` rules as literal-value
matches against block input (so `parse [1 2 3] [1 2 3]` returned `true`),
which is the *opposite* of Red's disambiguation. M138 corrects this to
full Red parity.

### Implementation (in `crates/red-eval/src/parse.rs`)

- [x] `resolve_count(v, env) -> Result<Option<usize>, EvalError>`: returns
      `Ok(Some(n))` for an `integer!` literal (n ≥ 0) or a `word!`/
      `lit-word!` whose bound value is an `integer!`; `Ok(None)` for
      anything else (so a word resolving to a `bitset!`/`block!` still
      dispatches correctly); `Err(EvalError::Native)` for a negative
      count.
- [x] `rule_extent` is now env-aware (takes `&Env`): at its top, if the
      element resolves to a count, peek the next element — if also a
      count, return `2 + extent(i+2)` (range form); else
      `1 + extent(i+1)` (exact form). The pre-M138 body was factored
      into `extent_keyword`. All 6 external call sites (`:712`/`:799`/
      `:815`/`:828`/`:846`/`:872`) and all internal recursive calls now
      thread `env`. This is required so `any`/`some`/`opt`/`while`/
      `ahead`/`behind`/`not` advance past a counted inner rule correctly
      (e.g. `some [2 digit "-"]`).
- [x] `run_counted(input, rules, inner_start, min, max, …)`: mirrors
      `run_repetition`; loops up to `max` iterations, restores cursor on
      a failed iteration and breaks, returns `count >= min`. Reuses
      `run_repetition`'s no-progress guard (count once, then break) to
      avoid infinite loops on no-op inner rules — a documented slight
      deviation from Red's pure-possessive count semantics. Forwards
      `collect_stack` so `collect`/`keep`/`copy`/`set`/`into` work
      inside counted rules.
- [x] Count dispatch at the top of `rule_one` (before the keyword
      check): on `resolve_count` → `Some(n1)`, advance `*i`, peek the
      next element — if also a count `Some(n2)`, range form (error if
      `n2 < n1`); else exact form (`min == max == n1`). Call
      `run_counted`, then advance `*i` past the inner rule via
      `rule_extent` (env-aware, so nested counts work).

### Disambiguation decisions (confirmed before implementation)

1. **Full Red parity** — every `integer!` in a rule block is a count
   prefix. `parse [1 2 3] [1 2 3]` no longer matches the literal
   integers (it parses as "range `1 2` applied to rule `3`"). The
   existing `parse_block_match` inline test + golden fixture were
   updated to use lit-word/string/`match` forms for literal matching.
2. **Word-resolved counts** — a `word!`/`lit-word!` in count position
   resolving to an `integer!` is the count (`THX: 1138 … 0 thx [rule]`).
   A word resolving to a non-integer returns `Ok(None)` from
   `resolve_count` and falls through to existing bitset/block/literal
   dispatch (no regression).
3. **Error model** — reuse `EvalError::Native { message, span }` for
   negative-count and inverted-range errors (avoids touching the
   specific-error pass-through list in `natives/mod.rs` and the `span()`
   match in `env.rs`). Messages: `"parse count cannot be negative: N"`
   and `"parse range lower bound N exceeds upper bound M"`.
4. **No-operand count** — `parse "" [3]` (count with no following rule)
   returns `false`, mirroring `any`/`some`'s no-operand handling at
   `parse.rs:695`.

### Tests & fixtures

- [x] Inline `#[test]` `parse_count_exact` — `4 digit` on `"2026"` →
      true; `"202"` → false; `"20260"` → false (trailing input).
- [x] Inline `#[test]` `parse_count_user_case` — the issue's exact
      `parse "2026-07-03" [4 digit "-" 2 digit "-" 2 digit]` → true.
- [x] Inline `#[test]` `parse_count_range` — `2 5 digit` on `"123"` →
      true; `"1"` → false (below min); `"123456"` → false (above max);
      `"12345"` → true (exact upper bound).
- [x] Inline `#[test]` `parse_count_zero` — `0 skip` always succeeds
      (no advance); `0 3 skip`; degenerate `0 0 skip`.
- [x] Inline `#[test]` `parse_count_word` — `n: 4` then `[n digit]`;
      `lo: 2 hi: 5` then `[lo hi digit]`.
- [x] Inline `#[test]` `parse_count_with_subrule` — `2 [#"a" #"b"]` on
      `"abab"`; count applied to sub-rule group then literal tail.
- [x] Inline `#[test]` `parse_count_inside_some` —
      `some [2 digit "-"] 2 digit` on `"12-34-56"` (verifies env-aware
      `rule_extent`).
- [x] Inline `#[test]` `parse_count_collect` — `collect w 3 digit`
      captures `[#"1" #"2" #"3"]`.
- [x] Inline `#[test]` `parse_count_negative_error` — `[-1 skip]` →
      `Err` with "negative".
- [x] Inline `#[test]` `parse_count_range_inverted_error` —
      `[3 1 skip]` → `Err` with "exceeds".
- [x] Inline `#[test]` `parse_count_no_following_rule` —
      `parse "" [3]` and `parse "" [2 3]` → false.
- [x] Updated inline `parse_block_match` and its golden fixture for the
      new disambiguation (lit-word/string/`match` forms).
- [x] Golden fixtures: `parse_count_exact`, `parse_count_range`,
      `parse_count_word`, `parse_count_zero`.
- [x] `examples/parse.red` updated (block-match line uses lit-words;
      added an integer-count demo block).
- [x] `README.md` parse inventory documents `n rule` / `n m rule` +
      the integer-is-always-a-count disambiguation.
- [x] `cargo test -p red-eval` green (676 lib + 11 integration + 4
      golden harness tests).
- [x] `cargo test --workspace --features force-walk` green (parity
      preserved between walker and VM).

### M138 deviations / out of scope

- **`quote` keyword** — Red's literal-match escape (needed to match a
  literal `integer!` in block input now that integers are always
  counts). Not implemented; belongs in a future "parse direct-matching
  parity" milestone alongside `datatype!`/`typeset!` matching. The
  `match` keyword and lit-word/string forms cover the same ground.
- **No-progress guard** — `run_counted` breaks after one no-advance
  iteration rather than looping `max` times (mirrors `run_repetition`).
  Slight deviation from Red's pure-possessive count semantics; safer
  (no infinite loops on no-op inner rules).
- **`break`/`reject` inside a counted rule** — pre-existing deviation
  (exits the whole parse, not just the repetition); unchanged by M138.
- **`/all` and `/part` parse refinements** — already covered by M136.

(End of plan11-functional-gaps.md)
