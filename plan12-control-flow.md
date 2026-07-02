# Plan 12: Control-Flow Completeness (v0.9.x)

Execution checklist extending the v0.9.0 baseline in
`plan11-functional-gaps.md` (M114 polish assumed complete). This is a small,
focused release: it lands the **seven missing control-flow natives** the
post-v0.8 feature audit identified — `unless`, `forever`, `for`, `forskip`,
`does-not`, `except`, `finally` — plus `recurse`/`recur` as a bonus if time
allows (see M124's open question). Every one of these is a thin wrapper
around evaluation machinery that already exists (`if`/`either`, `loop`,
`while`, `foreach`/`forall`, `try`/`catch`); none requires a new `Value`
variant or VM instruction.

Per `project-brief.md`, GUI / `draw` / `vid` / reactive dialects remain
**permanently out of scope**. This plan does not touch parse, mold, series
set-ops, or the port model — see `plan11-functional-gaps.md` for those, and
`plan13-feature-parity.md` for everything else (reflection, math helpers,
module extras, refinements).

## Why these seven

`natives/control.rs` and `registry.rs` implement `if`/`either`/`case`/
`switch`/`default`/`all`/`any`/`loop`/`repeat`/`until`/`while`/`break`/
`continue`/`try`/`attempt`/`catch`/`throw`/`cause-error`/`comment`/`exit`/
`quit`/`does`/`func`/`function`/`closure`/`return` — a solid core. Grepping
the registry and `control.rs` for the rest of Red's control-flow vocabulary
turns up nothing for:

- **`unless`** — the inverse of `if` (`unless cond body` ≡
  `if not cond body`). Trivial, but its absence is the single most-noticed
  gap in casual scripts (`if not x [...]` where Red idiom is `unless x [...]`).
- **`forever`** — an unconditional infinite loop (`while [true] [...]` today
  works as a substitute, but `forever [...]` is the idiomatic, more-
  efficient form — no condition re-check every iteration).
- **`for`** — the classic counted loop with a step
  (`for word start end step body`), distinct from `repeat` (which only
  counts up by 1 through a series or to a number) and `loop` (which takes no
  loop variable at all).
- **`forskip`** — iterate a series in fixed-size skips (record-wise
  iteration), used constantly alongside `hash!`/`map!`-style flat
  key/value blocks.
- **`does-not`** — Red's negated-does sugar (rare, but cheap to add
  alongside `does` in the same milestone).
- **`except`** — the "catch a specific error type" companion to `try`
  (today only blanket `try`/`attempt` exist — no way to branch on *which*
  error occurred without manually inspecting `error-type` after the fact).
- **`finally`** — guaranteed cleanup after a `try`/`except` chain, regardless
  of whether an error occurred.

## Deferred / out of scope

- `unimport` — `plan6` M62 (unrelated subsystem, not control-flow proper).
- Parse-dialect control keywords (`accept`/`reject`/`behind`) — those are
  `parse`-internal, tracked in `plan11` M110 as parse-recursion follow-ups,
  not general control flow.
- Coroutines/generators/`yield` — no such construct exists in Red either;
  not in scope for this or any planned milestone.
- Full exception hierarchies / typed custom exceptions beyond what
  `make error!` + `except`'s type-matching already provides (see M123) —
  a `plan13`-or-later candidate if real usage demands it.

## Non-goals

- A register VM, JIT, or further perf work — the v0.3.3 VM stays the
  default evaluator.
- New `Instr` variants — every native in M120–M123 is expressible as a
  native-call wrapper over existing eval primitives (`eval_block`,
  the existing `try`/`catch` unwind machinery, the existing loop-body
  evaluation helper `control.rs` already factors out for `while`/`until`).
  Confirm this holds during implementation; if any construct genuinely needs
  a new `Instr` (e.g. if `for`'s step-and-compare can't be expressed as a
  native loop without a VM-level counter), flag it as a plan deviation
  rather than silently expanding VM surface.
- Behavior changes to `if`/`while`/`try`/`catch`/`does`. All new natives are
  additive; none redefines or shadows an existing symbol.

## Ground-truth references (from research)

- `natives/control.rs` line map (pre-v0.9): `if` (`:24`), `either` (`:41`),
  `loop` (`:64`), `repeat` (`:93`), `until` (`:143`), `while` (`:176`),
  `break` (`:223`), `continue` (`:233`), `switch` (`:253`), `case` (`:304`),
  `default` (`:359`), `all` (`:397`), `any` (`:423`), `try` (`:453`),
  `attempt` (`:483`), `catch` (`:505`), `throw` (`:520`), `cause-error`
  (`:541`, variadic), `comment` (`:777`), `exit`/`quit` (`:787`).
- `func.rs`: `does` (`:91`), `func` (`:68`), `function` (`:30`), `closure`
  (`:252`), `return` (`:220`, variadic).
- `registry.rs:171–253` is where all of the above get inserted into
  `env.natives` — new M120–M123 natives are inserted in this same block,
  following the existing `fixed_native(fn as NativeFn, arity)` /
  `variadic_native(fn as NativeFn)` / `reg_refined(...)` call patterns
  already used for `switch`/`case` (`registry.rs:202–215`).
- `series.rs:1104–1196` holds `foreach`/`forall` — the record-wise iteration
  pattern `forskip` needs (M121) is closest in spirit to `forall`'s cursor-
  advance loop, so `forskip`'s implementation should live in `series.rs`
  near `forall`, not in `control.rs`, despite being introduced in this
  control-flow-themed plan. (Note the module split explicitly here so the
  milestone below doesn't get implemented in the wrong file.)
- `EvalError` (wherever it's defined — the type `catch_native`/`throw_native`
  unwind through) needs a way to carry a **typed** error tag for `except`
  (M123) to pattern-match on; confirm whether `error-type`/`error-code`
  (`convert.rs:1027–1030`) already expose enough structure, or whether
  `except` needs a new `EvalError` variant/field.
- `try_native`/`catch_native`/`throw_native` (`control.rs:453,505,520`) are
  the existing unwind primitives `except`/`finally` build on top of — no new
  unwind mechanism should be introduced; `except`/`finally` are sugar over
  the same `Result`-propagation `try`/`catch` already use.

---

## Milestone 120 — `unless` and `does-not`

The two purely-syntactic-sugar natives. Land first to prove the milestone
template before touching loop machinery.

### `unless`

- [ ] Add `unless_native(cond, body)` in `control.rs`, implemented as the
      logical inverse of `if_native` (`control.rs:24`) — same signature,
      same "body must be a `block!`, evaluated only when the condition is
      falsy" contract, same return-value semantics (Red's `unless` returns
      `none!` when the condition is truthy and the body doesn't run — mirror
      whatever `if`'s "condition true, no else" return value is today).
- [ ] Register `unless` in `registry.rs` alongside `if`/`either`
      (`registry.rs:172–175`), `fixed_native(unless_native as NativeFn, 2)`.
- [ ] Inline `#[test]`: `unless false [1]` → `1`.
- [ ] Inline `#[test]`: `unless true [1]` → `none` (or whatever `if`'s
      analogous no-branch-taken value is — match it exactly).
- [ ] Inline `#[test]`: `unless (1 = 1) [print "no"]` prints nothing.
- [ ] Add golden fixture: `unless_basic`.

### `does-not`

- [ ] Confirm exact Red semantics before implementing — `does-not` is rare
      enough that its contract should be verified against Red docs/source,
      not assumed. (Working hypothesis: a `does`-like zero-arg function
      wrapper whose body's truthiness is negated — `does-not [cond]`
      produces a thunk that returns `not cond` when called. If Red doesn't
      actually define this as documented, **drop it from the milestone**
      rather than inventing semantics.)
- [ ] If confirmed: add `does_not_native` in `func.rs` next to `does`
      (`:91`), register in `registry.rs` next to `does` (`:271–273`).
- [ ] Inline `#[test]`: per confirmed semantics.
- [ ] Add golden fixture: `does_not_basic` (only if implemented).
- [ ] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 121 — `forever`, `for`, `forskip`

The three loop-shape gaps. `forever` is trivial; `for` and `forskip` need
careful attention to step direction and off-by-one semantics (both are
classic sources of subtle bugs in loop natives).

### `forever`

- [ ] Add `forever_native(body)` in `control.rs` next to `while_native`
      (`:176`) — an unconditional loop: evaluate `body` repeatedly until a
      `break` unwinds it. Reuse `while_native`'s body-evaluation/`break`-
      catching inner loop, just drop the condition check.
- [ ] Register `forever` in `registry.rs`: `fixed_native(forever_native as
      NativeFn, 1)`.
- [ ] Inline `#[test]`: `i: 0 forever [i: i + 1 if i = 5 [break]] i` → `5`.
- [ ] Inline `#[test]`: `forever [break]` returns cleanly (single-iteration
      guard against an off-by-one in the break-catch wiring).
- [ ] Add golden fixture: `forever_basic`.

### `for`

- [ ] Add `for_native(word, start, end, step, body)` in `control.rs`. Red's
      `for` signature: `for word start end bump body` — binds `word` to
      `start`, evaluates `body`, adds `bump` to `word`, repeats while
      `word` hasn't passed `end` (direction-aware: if `bump` is positive,
      loop while `word <= end`; if negative, loop while `word >= end`).
      Confirm this direction-aware comparison exactly against Red before
      implementing — it's the one subtle part of an otherwise-simple native.
- [ ] Register `for` in `registry.rs`: `fixed_native(for_native as
      NativeFn, 5)`.
- [ ] Support both `integer!` and `decimal!`/`float!` start/end/bump values
      (Red's `for` works over any numeric type the `+`/`<=`/`>=` operators
      already support — reuse `math.rs`'s existing numeric-comparison
      helpers rather than hand-rolling a new comparator).
- [ ] Inline `#[test]`: `total: 0 for i 1 5 1 [total: total + i] total` → `15`.
- [ ] Inline `#[test]`: `for i 5 1 -1 [prin i]` prints `54321` (descending
      step, the direction-aware branch).
- [ ] Inline `#[test]`: `for i 1 1 1 [prin "x"]` prints `x` exactly once
      (start == end, inclusive bound — a common off-by-one trap).
- [ ] Inline `#[test]`: `for i 1 0 1 [prin "x"]` prints nothing (start past
      end with a positive step — the loop body never runs, doesn't error).
- [ ] Inline `#[test]`: `break` inside a `for` body exits cleanly.
- [ ] Add golden fixtures: `for_ascending`, `for_descending`,
      `for_single_iteration`, `for_empty_range`.

### `forskip`

- [ ] Add `forskip_native(word, series, skip_size, body)` in `series.rs`
      near `forall` (`:1049,1195`) — binds `word` to successive positions of
      `series`, advancing `skip_size` elements each iteration (not 1, unlike
      `forall`), evaluating `body` each time. Stops when fewer than
      `skip_size` elements remain (Red parity — confirm exact boundary
      behavior: does a short trailing partial-record still get one final
      iteration, or is it skipped? Check Red source/docs, don't assume).
- [ ] Register `forskip` in `registry.rs` next to `foreach`/`forall`'s
      registration inside `crate::series::register_series_natives`
      (`registry.rs:320`).
- [ ] Inline `#[test]`: `out: copy [] forskip s: [1 2 3 4] 2 [append out
      first s] out` → `[1 3]` (visits every-other element, the flat
      key/value walking pattern).
- [ ] Inline `#[test]`: `forskip` over an odd-length series with a trailing
      partial record — behavior matches whatever was confirmed above.
- [ ] Inline `#[test]`: `break` inside a `forskip` body exits cleanly.
- [ ] Add golden fixtures: `forskip_basic`, `forskip_partial_trailing`.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 122 — Polish & v0.9.1 release (loop + sugar batch)

Ship M120–M121 as a self-contained point release before tackling
`except`/`finally` (M123), since the loop/sugar natives are lower-risk and
fully independent of the exception-handling work.

- [ ] Golden fixture audit: every new native from M120–M121 has at least one
      positive and one edge-case fixture (empty range, single iteration,
      `break` mid-loop).
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `cargo fmt --all --check` clean.
- [ ] Update `README.md`: add `unless`/`forever`/`for`/`forskip`(+`does-not`
      if shipped) to the natives list; bump version to v0.9.1.
- [ ] Final `cargo test --workspace` green; `--features force-walk` green.
- [ ] Tag release `v0.9.1`.

---

## Milestone 123 — `except` and `finally`

The one milestone in this plan that's genuinely new *semantics*, not pure
sugar: `except` needs to inspect *what kind* of error unwound through `try`,
and `finally` needs to run cleanup code on **both** the success and error
paths of a `try`/`except` chain — something the current `try`/`catch`
primitives don't need to do (they're binary: caught or not).

- [ ] Confirm Red's exact `try`/`except`/`finally` grammar before
      implementing. Working hypothesis (verify against Red docs): a
      dialect roughly like —
      ```
      try/except body [error-type-word] handler
      ```
      or a block-based chain where `except` and `finally` are refinements of
      `try` (`try/except`, `try/finally`) rather than standalone natives.
      **This shapes the whole milestone — do not start implementation until
      confirmed**, since guessing wrong means redoing the arg-parsing.
- [ ] Once confirmed, add whichever of the following the real Red grammar
      calls for:
  - [ ] `try/except` refinement on the existing `try` native
        (`control.rs:453`) — reuse `reg_refined` (the pattern `switch`/`case`
        already use at `registry.rs:202–215`) rather than a new standalone
        `except` native, if Red's grammar is refinement-shaped.
  - [ ] `try/finally` refinement similarly — cleanup block runs after the
        `try` body regardless of error, and (critically) **before** the
        error (if any) is re-raised or the `except` handler runs — confirm
        exact ordering against Red.
  - [ ] Type-matching in `except`: the handler should be able to
        discriminate by error type (using whatever `error-type`/`error-code`
        already expose per `convert.rs:1027–1030`) — confirm whether Red's
        `except` takes a type-filter block (`except [network!] [...]`) or
        always catches everything and leaves filtering to the handler body.
- [ ] Ensure `except`/`finally` compose with the existing `catch`/`throw`
      pair (`control.rs:505,520`) without double-unwinding or swallowing a
      `throw` that isn't meant for this `try` (i.e. `except` should only
      intercept *errors* raised via the error path, not values passed to
      `throw`/`catch`, which is a separate mechanism in Red — confirm this
      distinction is preserved, not accidentally merged).
- [ ] Inline `#[test]`: `try/except [1 / 0] [print "caught"]` prints
      "caught" (or whatever the confirmed grammar's minimal form is).
- [ ] Inline `#[test]`: `try/finally [1 + 1] [print "cleanup"]` prints
      "cleanup" even though no error occurred (finally runs on the success
      path too).
- [ ] Inline `#[test]`: `try/finally [1 / 0] [print "cleanup"]` prints
      "cleanup" **and** the error still propagates/is reported afterward
      (finally doesn't swallow the error).
- [ ] Inline `#[test]`: nested `try/except` — an inner `try` that doesn't
      match its type-filter re-raises to an outer `try/except` (only if the
      confirmed grammar supports type-filtering; skip this test otherwise).
- [ ] Inline `#[test]`: regression guard — all existing `try`/`attempt`/
      `catch`/`throw` fixtures unchanged (the new refinements/natives are
      strictly additive to the arg surface).
- [ ] Add golden fixtures: `except_basic`, `finally_success_path`,
      `finally_error_path`, `except_finally_combined`.
- [ ] Add `programs_errors/except_unmatched_type.red` (if type-filtering is
      part of the confirmed grammar).
- [ ] `cargo test --workspace` green; `--features force-walk` green.

### M123 open questions

1. **Exact grammar.** This is the blocking open question for the entire
   milestone — see the checklist's first item. Do not proceed past the
   grammar-confirmation step without it.
2. **Interaction with `throw`/`catch`.** Confirm `except` does not
   accidentally become a second way to catch `throw`n values — Red keeps
   error-handling (`try`/`except`) and value-passing unwind (`catch`/
   `throw`) as two distinct mechanisms; the implementation must preserve
   that separation.

---

## Milestone 124 — Polish & v0.9.2 release (exception batch)

- [ ] Audit `EvalError` rendering for any new error-carrying state `except`
      needed to add (M123).
- [ ] Golden fixture audit for M123's success/error/finally paths.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`; fix.
- [ ] Run `cargo fmt --all --check`; fix.
- [ ] Update `project-brief.md`: add a "Control-Flow Completeness (v0.9.x)"
      subsection listing all seven (or eight, if `does-not` shipped) new
      natives; remove them from "Known gaps."
- [ ] Update `README.md`: add `except`/`finally` (or `try/except`/
      `try/finally` refinements, per whatever grammar was confirmed) to the
      natives/refinements list; bump version to v0.9.2.
- [ ] Final `cargo test --workspace` green; `--features force-walk` green.
- [ ] Final `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] Tag release `v0.9.2`.

### Open question (plan-wide)

1. **`recurse`/`recur`.** Not in the original seven-item gap list, but a
   natural companion (self-reference without naming the enclosing function —
   useful in anonymous `func`/`closure` bodies). **Decision: stretch goal
   only** — attempt after M123 if time remains; do not let it block the
   v0.9.2 tag. If deferred, note it explicitly in `project-brief.md`'s
   "Known gaps" rather than letting it silently disappear.

(End of plan12-control-flow.md)
