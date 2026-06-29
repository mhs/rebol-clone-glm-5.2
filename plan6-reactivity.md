# Plan 6: Reactive Dialect (v0.5)

Execution checklist extending the v0.4.0 baseline in `plan5.md`. v0.5 lands a
**reactive dialect** matching upstream Red's `react`/`on-change` semantics —
data-flow reactions over object fields, with no GUI dependency. Reverses the
"permanently out of scope" stance documented in `README.md`,
`project-brief.md`, and `plan2/3/4/5.md` (see "Scope reversal" below).

Per upstream Red, **reactions are object-field-scoped data-flow**, not
GUI redraw signals: a `react [body]` re-runs `body` whenever an object
field *read* by the body is *written*. No `draw`/`vid`/GUI is required, and
none is added. `draw`/`vid` remain permanently out of scope.

Deferred to v0.6+ (acknowledged, not built here): modules / `import` /
`export`, closures (`closure!`), full port model, `tag!`, `ref!`, `image!`,
`vector!`, `hash!`, `regex!`, `logic!`/`bitset!` advanced ops, `routine!`
FFI. v0.5 is a **reactivity release**: it lands the dialect, the registry,
the read/write hooks, the batching semantics, and the GC. Modules and
closures remain the two conspicuous v0.6 candidates (deferred from v0.5's
original scope per `plan5.md:756`).

Non-goal: a register VM, JIT, or further perf work beyond the M54 hot-path
audit. The v0.3.3 VM stays the default evaluator; reaction bodies compile
through it via the existing `dispatch_block` path (M27 cache). New `Instr`
variants are added only if profiling proves the native-`Call` path is hot
on fire — currently not anticipated (reactions fire on writes, not reads).
The golden parity harness (`tests/parity.rs`) and
`cargo test --workspace --features force-walk` remain the regression
gates.

## Design summary

Three themes, in priority order:

1. **Reaction registry & lifecycle** — a first-class `ReactionRegistry` on
   `Env` (new file `crates/red-eval/src/react.rs`) holds `Reaction` records
   keyed by stable `ReactionId`. Each reaction stores its body `Series`,
   its definition `Rc<Context>`, the set of `(ctx, slot)` source pairs it
   read during its last run (the "dependency set"), an optional
   `/with`-condition `Series`, and liveness flags. Reactions fire when a
   watched write occurs; the registry coalesces, dedups, and guards against
   cycles.
2. **Read/write hooks** — `Context` gains an optional `WatchTable`
   (`RefCell<Option<Box<WatchTable>>>`); `None` for the vast majority of
   contexts (user ctx, function-local ctxs) so the hot path is a single
   `is_none` load. `Some` only for object contexts that a reaction has
   read. `slot_value`/`slot_value_unchecked` push reads onto the active
   observer; `set_slot`/`set_slot_unchecked` schedule fires. Both hooks
   route through one method each, so every existing call site (the 14+
   `set_slot` sites cataloged in `natives/words.rs`, `natives/control.rs`,
   `parse.rs`, `object.rs`, `series.rs`, `interp_walker.rs`, `vm/vm.rs`)
   gets reactivity for free.
3. **Dialect surface** — `react`/`react/later`/`react/with`/`react/into`/
   `unreact`/`unreact/all`/`clear-reactions`/`is-reactive?`/`react?`/
   `on-change`/`cause-reaction`/`update`/`react/gc`. No new value types
   (reactions are not first-class values in Red; they live in the
   registry). No new lexer/parser/printer changes.

Non-goal: behavior changes to existing v0.2/v0.3/v0.4 features. Every new
construct is additive. The v0.2 parity contract (`tests/parity.rs:14`)
holds: existing golden fixtures must produce byte-identical output under
both `Vm` and `force-walk` modes after every milestone. The one caveat is
the M54 audit: if the `Option<WatchTable>` branch measurably regresses
`fib 30` or `sum_loop`, the entire mechanism is gated behind
`#[cfg(feature = "react")]` (default-on in dev/test, opt-in for
`--release` perf builds) — see M54.

## Scope reversal (documentation)

The "permanently out of scope" coupling of reactive↔GUI in the existing
docs is a Red-world assumption (reactive's main upstream use is GUI
redraws). The dialect itself is pure data-flow over object fields and
needs no GUI. This plan reverses the stance:

- [ ] `README.md:241` — change "**GUI / `draw` / `vid` / reactive dialects
      are permanently out of scope.**" to "**GUI / `draw` / `vid` are
      permanently out of scope.**" and add a "Reactive dialect" bullet
      under "What's implemented" once M50 lands.
- [ ] `project-brief.md` — remove "reactive dialects" from the "Other
      dialects (illustrative, NOT implemented)" list (`project-brief.md:
      344-346`); add a "Reactive dialect" subsection under "Dialects"
      describing `react`/`on-change` semantics.
- [ ] `plan2.md:9`, `plan3.md:10`, `plan4.md:7`, `plan5.md:10-11` — each
      says "GUI/draw/VID/reactive dialects remain **permanently out of
      scope**". Strike "reactive" from each (leave GUI/draw/vid). Add a
      forward-reference note: "Reactive dialects landed in v0.5 — see
      `plan6-reactivity.md`."
- [ ] `plan5.md:16` — strike "`object!` `on-change` reactive slots" from
      the v0.5+ deferred list (it lands here in v0.5).
- [ ] Add a "Reactive dialect" section to `architecture.md` covering the
      registry, hooks, batching, and GC (lands with M56).

## Semantic target (upstream Red parity)

```red
o: object [a: 1 b: 2 c: 0]
react [o/c: o/a + o/b]          ; runs once on registration: o/c = 3
o/a: 10                         ; fires the reaction → o/c = 12
o/b: 20                         ; fires again → o/c = 30

react/later [o/c: o/a * o/b]    ; don't run on registration
o/a: 2                          ; fires → o/c = 40

react/with [o/a > 5] [...]      ; only fire when condition is true

unreact [...]                   ; remove a reaction by body identity
unreact/all                     ; remove all reactions
clear-reactions                 ; drop every reaction (test hygiene)

is-reactive? 'o/c               ; true if any reaction reads or writes o/c
react? [...]                    ; true if a reaction with this body exists

on-change [o 'word] [body]      ; per-write handler, fires synchronously
                                 ; with old/new values as block args
cause-reaction [...]             ; manual trigger (matches Red)
update 'o 'a                    ; force-fire all reactions reading o/a
```

### Firing semantics (matches upstream Red)

- **Registration fires immediately** (unless `/later`): the body runs once
  on `react`, which both produces the initial output *and* records the
  dependency set (the `(ctx, slot)` pairs read during that run).
- **Writes fire reactions that read the written slot.** A write to `o/a`
  fires every reaction whose dependency set includes `(o's ctx, a's slot)`.
- **Batching:** writes within a single outermost frame exit are coalesced.
  `o/a: 1 o/b: 2` (two statements at the same level) fires each reading
  reaction *once* after the second statement, not twice. Writes inside a
  native (`do [o/a: 1 o/b: 2]`) batch until the `do` returns.
- **Dedup:** if a reaction reads both `o/a` and `o/b` and both are written
  in one transaction, the reaction fires once (per-reaction dedup, not
  per-source-slot).
- **Order:** reactions fire in registration order within a transaction.
- **Cycle guard:** a reaction re-firing itself (directly or transitively
  within its own body) is a no-op for that edge; the registry tracks
  `firing: HashSet<ReactionId>` during a transaction and drops
  already-firing IDs. A `--trace` line is emitted.
- **Errors propagate:** a reaction body that errors unwinds to the script
  entry like any eval error; caught by `try`/`attempt` per the existing
  M16 model. Uncaught errors abort the transaction (remaining queued
  reactions in the batch are dropped — matches Red).

### `on-change` vs `react`

- `react` is **batched** and **dependency-tracked**: it auto-discovers
  sources by reading during its initial run, fires post-transaction, and
  dedups across sources.
- `on-change` is **synchronous** and **per-write**: the handler runs
  immediately on the write (not batched), receives the old and new values
  as block args (`on-change [o 'a] [old new][...]`), and does not dedup
  across multiple writes in one statement. `on-change` is the lower-level
  primitive; `react` is built on top of the same `WatchTable` but with
  batching and dependency discovery layered on.

---

## Milestone 50 — Reaction registry + `react`/`react/later` + read/write hooks

The foundational milestone. Lands the registry, the `Context` watcher
hook, the read-tracking observer stack, and the minimal `react`/`react/
later` natives. No batching (each write fires immediately), no `/with`,
no `unreact`, no `on-change`. Single-source reactions only.

### Files

- [ ] **New: `crates/red-eval/src/react.rs`** — the registry, the
      `Reaction` struct, `ReactionId` newtype, `ReactionKey = (usize,
      usize)` (ctx ptr + slot), the `observing: Vec<ReactionKey>` stack
      on `Env`, and the `schedule_fire(ctx_id, slot)` entry point. The
      registry owns `Vec<Reaction>`; IDs are indices into this vec.
- [ ] **Edit: `crates/red-core/src/context.rs`** — add
      `pub watchers: RefCell<Option<Box<WatchTable>>>` to `Context`.
      `WatchTable` is a `HashMap<usize, Vec<ReactionId>>` (slot →
      reactions reading it). `None` by default (zero-cost for non-object
      contexts). `Context::set_slot`/`set_slot_unchecked` grow one
      `if let Some(ref t) = *self.watchers.borrow() { ... }` branch that
      fires *after* the slot `RefMut` is dropped (current code already
      scopes the write in a block then `drop(slots)` — insert the fire
      call between `drop(slots)` and return). `slot_value`/
      `slot_value_unchecked` grow a symmetric read-tracking branch: if
      the ctx is watched *and* `env.observing` is non-empty, push
      `(ctx_id, slot)` onto the top observer.
- [ ] **Edit: `crates/red-core/src/env.rs`** — add
      `pub reactions: ReactionRegistry`, `pub observing: Vec<ReactionKey>`,
      `pub firing: HashSet<ReactionId>` to `Env`. All three default-empty.
      Add `Env::with_react()` constructor flag (or always-on; the
      `Option<WatchTable>` on `Context` is the real gate).
- [ ] **Edit: `crates/red-eval/src/lib.rs`** — `pub mod react;`.
- [ ] **Edit: `crates/red-eval/src/natives/registry.rs`** — call
      `react::register_react_natives(&mut env)` alongside the other
      `register_*` calls.

### Natives

- [ ] `react [body]` — arity 1 (block). Binds the body (via
      `bind_function_body` against `env.user_ctx`), pushes a fresh
      observer onto `env.observing`, runs the body once via
      `dispatch_block` (records the dependency set = the observer's
      contents), pops the observer, stores the `Reaction` in the
      registry, and installs `WatchTable` entries on each source ctx
      (lazily allocating the `WatchTable` on first read). Returns
      `Value::None`.
- [ ] `react/later [body]` — same but skips the initial run; dependency
      set is empty until the first manual fire or `update`. The reaction
      will not fire until a source is written — but since no sources are
      known, it effectively never fires unless `cause-reaction` or
      `update` triggers it. (Upstream Red's `/later` semantics: the
      reaction is registered but dormant until a dependency is
      established by a later `update` or by the user running the body
      once via `cause-reaction`.) Document this in `architecture.md`.

### Read tracking — hook placement

The read-tracking hook fires only when **both** conditions hold:
1. The `Context` being read has `watchers = Some(...)` (i.e. it's an
   object ctx that a reaction has previously read from — established
   during a reaction's initial run).
2. `env.observing` is non-empty (i.e. a reaction body is currently
   executing).

When both hold, `slot_value`/`slot_value_unchecked` push
`(Rc::as_ptr(&self) as usize, idx)` onto `env.observing.last_mut()`.
The push is a `Vec::push` of a `(usize, usize)` tuple — 16 bytes, no
allocation beyond the Vec's existing capacity (the observer Vec is
reused across reaction runs; `clear()` between runs, not `drop`).

### Write hook — hook placement

`Context::set_slot`/`set_slot_unchecked` (and `Context::set`, which
routes to `set_slot`): after the slot `RefMut` is dropped and before
return, check `*self.watchers.borrow()`. If `Some(t)` and
`t.get(&idx)` is non-empty, call `env.schedule_fire(ctx_id, idx)`.
`schedule_fire` pushes the matched `ReactionId`s onto the transaction
queue (or fires immediately if no transaction is active — M50 uses
immediate firing; M51 adds batching).

**Critical:** the fire must happen *after* the `RefMut` and `Ref`
borrows on `self.slots` are fully released. The current
`set_slot_unchecked` already scopes the write:

```rust
pub fn set_slot_unchecked(&self, idx: usize, val: Value) {
    let slots = self.slots.borrow();
    debug_assert!(idx < slots.len(), ...);
    {
        *unsafe { slots.get_unchecked(idx) }.borrow_mut() = val;
    }
    drop(slots);
}
```

The fire call goes between `drop(slots)` and the closing brace. The
reaction body runs via `dispatch_block`, which allocates a fresh `Vm`
from the pool (M30.1.C) — it cannot alias the caller's `Vm.stack` or
`Vm.frames`. The borrow-checker is satisfied because `env: &mut Env` is
passed in *after* `slots` (the `RefCell` borrow) is dropped.

### `Context::set` (the allocate-or-write path)

`Context::set` calls `slot_index` (which may allocate a new slot) then
`set_slot`. The watcher hook lives in `set_slot`, so `set` is covered
automatically. New slots have no watchers (the `WatchTable` is keyed by
slot index; a freshly-allocated slot has no entry), so the hook is a
no-op for them. Correct: a reaction that hasn't read a brand-new slot
shouldn't fire on its first write.

### Golden fixtures

- [ ] `react_basic` — `o: object [a: 1 b: 2 c: 0] react [o/c: o/a + o/b]
      print o/c` → `3`, then `o/a: 10 print o/c` → `12`.
- [ ] `react_later` — `react/later [...]` does not run on registration;
      `o/a: 10` does not fire (no deps known); `cause-reaction [...]`
      runs it once and establishes deps.
- [ ] `react_cycle_guard` — `react [o/a: o/a + 1]` (self-read + self-write)
      fires once on registration (o/a: 2), then the cycle guard prevents
      the reaction from re-firing itself on its own write.
- [ ] `react_multi_source` — a reaction reading `o/a` and `o/b`; writing
      either fires it; writing both (M50: immediate firing, so twice —
      M51 will coalesce to once).

### Tests

- [ ] Inline `#[test]` in `react.rs`: `react` runs body once on
      registration.
- [ ] Inline `#[test]`: write to a read field fires the reaction.
- [ ] Inline `#[test]`: write to an unread field does not fire.
- [ ] Inline `#[test]`: `react/later` does not run on registration.
- [ ] Inline `#[test]`: cycle guard prevents infinite loop.
- [ ] Inline `#[test]`: reaction body errors propagate (caught by
      `try`).
- [ ] `cargo test --workspace` green; `--features force-walk` green.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.

---

## Milestone 51 — Batching & transactions

Coalesce writes within a single outermost frame exit. `o/a: 1 o/b: 2`
(two statements at the same level) fires each reading reaction *once*
after the second statement, not twice. Writes inside a native (`do
[o/a: 1 o/b: 2]`) batch until the `do` returns.

### Mechanism

- [ ] `Env::transaction: Option<Vec<ReactionId>>` — when `Some`,
      `schedule_fire` pushes IDs into the vec instead of firing. When
      `None` (the default outside a transaction), fires immediately
      (M50 behavior).
- [ ] **Transaction boundaries:** the outermost `dispatch_block` call
      (script entry, `do`, `reduce`, native body, reaction body itself)
      opens a transaction on entry and drains+fires on exit. Nested
      `dispatch_block` calls do *not* open new transactions — they
      inherit the outer one. This is implemented by checking
      `env.transaction.is_some()` on entry: if already `Some`, do
      nothing (the outer boundary will drain); if `None`, set it to
      `Some(vec![])`, run, then drain.
- [ ] **Drain:** on transaction close, take the vec, dedup by
      `ReactionId` (preserve registration order — use a `Vec` +
      `HashSet<ReactionId>` seen-set, not a `HashSet` iterator which
      is unordered), and fire each reaction in order. Reactions that
      error abort the drain (remaining reactions dropped — matches Red).
- [ ] **Re-entrant fires:** a reaction body that writes to a watched
      field schedules into the *current* transaction (if one is open
      during the drain — i.e. the drain itself opens a sub-transaction
      per reaction). Implement as: each reaction fire wraps its body
      in `dispatch_block`, which opens its own transaction; writes
      during the body accumulate; on the body's exit, those fires are
      drained recursively. The `firing` set prevents cycles (a reaction
      in the firing set is dropped from the queue).

### Edge cases

- [ ] **Reaction that reads a field written by another reaction in the
      same transaction:** the second reaction fires in the drain phase
      after the first writes. Order = registration order. If the first
      reaction's write makes the second's `/with` condition false, the
      second is skipped (M52 adds `/with`; M51 just fires unconditionally
      on dep-write).
- [ ] **Reaction that errors mid-drain:** the error propagates out of
      `dispatch_block` to the transaction boundary, which unwinds to
      the script entry. Remaining queued reactions are dropped. `try`
      around the triggering write catches it.

### Golden fixtures

- [ ] `react_batched` — `o/a: 1 o/b: 2` (two statements) fires a
      reaction reading both once, not twice. Output: single `print`
      side-effect.
- [ ] `react_in_do` — `do [o/a: 1 o/b: 2]` fires once after `do`
      returns.
- [ ] `react_chain` — reaction A writes `o/b`, reaction B reads
      `o/b`; both fire in registration order, B sees A's write.

### Tests

- [ ] Inline `#[test]`: two writes to two sources of one reaction fire
      it once.
- [ ] Inline `#[test]`: writes in `do` batch until `do` returns.
- [ ] Inline `#[test]`: reaction chain fires in order.
- [ ] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 52 — `react/with`, `react/into`, `unreact`, `clear-reactions`, `is-reactive?`

Full dialect surface (minus `on-change`, which is M53).

### Natives

- [ ] `react/with [cond] [body]` — the `cond` block is evaluated before
      each fire; if it yields `false` or `none`, the reaction is
      skipped for this transaction. The `cond` is also evaluated on
      registration (immediate `react` runs the body once only if
      `cond` is true; `react/later/with` skips the initial run and
      evaluates `cond` on each fire). Stored as `when: Option<Series>`
      on `Reaction`.
- [ ] `react/into target [body]` — writes the reaction's last result
      into `target` (a word or path) after each fire. Convenience for
      `react [target: body]`. Stored as `into: Option<Value>` (a word
      or path; resolved on fire).
- [ ] `unreact [body]` — remove the reaction whose body is `Series`-
      identical to the given block. Body identity = same `Rc::as_ptr`
      (the body was stored at registration; the user passes the same
      block literal). If not found, no-op. Removes the `Reaction`'s
      entries from every `WatchTable` it appears in, marks it `alive =
      false`, and (M55) makes it eligible for GC.
- [ ] `unreact/all` — clear the entire registry. Test-hygiene escape
      hatch.
- [ ] `clear-reactions` — alias for `unreact/all` (matches Red's
      REPL convenience).
- [ ] `is-reactive? 'obj/field` — true if any reaction reads or
      writes the given field. Accepts a path (`obj/field`) or an
      `in`-bound word. Walks the registry's dependency sets.
- [ ] `react? [body]` — true if a reaction with this body is
      registered.
- [ ] `cause-reaction [body]` — manually fire a reaction by body
      identity, bypassing the dep-write mechanism. Useful for
      `react/later` reactions that need an initial run.
- [ ] `update 'obj 'word` — force-fire all reactions reading
      `(obj's ctx, word's slot)`, bypassing the dep-write mechanism.
      Does *not* change the slot value (unlike a real write); just
      triggers reactions that read it.

### Golden fixtures

- [ ] `react_with` — `react/with [o/a > 5] [...]` skips fires when
      `o/a <= 5`.
- [ ] `react_into` — `react/into result [...]` writes the result to
      `result` after each fire.
- [ ] `unreact` — register a reaction, `unreact` it, write to the
      source, confirm no fire.
- [ ] `unreact_all` — register two reactions, `unreact/all`, write to
      both sources, confirm no fire.
- [ ] `is_reactive` — `is-reactive? 'o/a` true before `unreact`, false
      after.
- [ ] `cause_reaction` — `react/later [...]` then `cause-reaction
      [...]` runs it once.
- [ ] `update` — `update 'o 'a` fires reactions reading `o/a` without
      changing `o/a`.

### Tests

- [ ] Inline `#[test]` per native (8 tests).
- [ ] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 53 — `on-change` slot handlers

Per-write synchronous handlers with old/new args. Distinct from batched
`react` reactions — `on-change` fires immediately on the write, within
the transaction, and receives the old and new values as block args.

### Mechanism

- [ ] Extend `WatchTable` to hold two kinds of entries per slot:
      `reactions: Vec<ReactionId>` (the M50 path) and `handlers:
      Vec<ReactionId>` (the M53 path). Handlers fire synchronously in
      `set_slot` *before* the batched-reaction scheduling.
- [ ] `on-change [obj 'word] [body]` — arity 3 (object, lit-word for
      the field, handler block). Registers a handler `Reaction` with
      `handler: true`. The handler body receives a 2-element block
      `[old new]` as its argument (via a synthesized `call_user_func`
      frame, or by binding `old`/`new` as locals in the handler's
      context — the latter matches Red's `on-change` convention where
      the handler is `func [old new][...]`). Decision: synthesize a
      `FuncDef` with params `[old new]` and bind the body to it; call
      it with the old/new values on each fire.
- [ ] **Firing:** in `set_slot`, after the write, if `handlers` is
      non-empty, snapshot the old value (read the slot *before* the
      write — requires reordering `set_slot` to capture old first),
      write the new value, then call each handler with `[old new]`.
      Handlers fire in registration order. A handler that errors
      unwinds immediately (no batch — `on-change` is synchronous).
      Other handlers in the chain for the same write are dropped
      (matches Red).
- [ ] **`on-change` and batching:** `on-change` handlers are *not*
      batched — they fire on the write itself, before the
      transaction's drain phase. A handler that writes to another
      watched field schedules a batched reaction (via the normal
      `schedule_fire` path), which fires in the drain phase. So:
      `on-change` = synchronous, pre-transaction-drain; `react` =
      batched, post-transaction-drain.

### `set_slot` reordering

Current `set_slot`:
```rust
pub fn set_slot(&self, idx: usize, val: Value) {
    *self.slots.borrow()[idx].borrow_mut() = val;
}
```
New `set_slot` (with handlers):
```rust
pub fn set_slot(&self, idx: usize, val: Value) {
    let old = self.slots.borrow()[idx].borrow().clone();   // capture old
    *self.slots.borrow()[idx].borrow_mut() = val;           // write new
    // fire on-change handlers with (old, val) — env passed in somehow
    // then schedule batched reactions
}
```
The "env passed in somehow" is the wrinkle: `Context::set_slot`
currently takes `&self` only (no `env`). Two options:
- **(a)** Pass `&mut Env` into `set_slot` (and `set_slot_unchecked`).
  Breaks every call site (14+ sites) — they all have `env` in scope,
  but the signature churn is significant.
- **(b)** Give `Context` a back-reference to its `Env` (a `Weak<Env>`
  or a `*mut Env` raw pointer). Avoids signature churn but adds a
  self-referential `Context` (currently `Context` is plain data).
- **(c)** Add a `Context::set_slot_tracked(&self, idx, val, env: &mut
  Env)` method; keep `set_slot`/`set_slot_unchecked` as-is for
  non-reactive contexts. The hooks live in the tracked variant only.
  Call sites with `env` available (all 14+) call `set_slot_tracked`;
  sites without `env` (none currently — `Context::set` is only called
  from eval paths that have `env`) keep the untracked path.

Decision: **(c)**. Minimal churn, no self-referential context. The
14+ call sites are audited and switched to `set_slot_tracked` where
`env` is in scope (all of them). The untracked `set_slot` remains for
`Context::set` calls outside eval (e.g. `install_constants` at boot —
those don't need reactivity).

### Golden fixtures

- [ ] `onchange_basic` — `on-change [o 'a] [old new][print [old "->" new]]
      o/a: 99` → `1 -> 99`.
- [ ] `onchange_chain` — two handlers on the same slot fire in order.
- [ ] `onchange_error` — handler that errors unwinds immediately; second
      handler does not fire.
- [ ] `onchange_and_react` — `on-change` fires synchronously, then
      `react` fires in the drain phase. Confirm ordering.
- [ ] `onchange_remove` — `unreact` on an `on-change` handler
      removes it.

### Tests

- [ ] Inline `#[test]` per fixture (5 tests).
- [ ] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 54 — Hot-path audit & `react` feature flag

The `Option<WatchTable>` branch on every `set_slot`/`set_slot_unchecked`
is the one perf risk. This milestone measures it and gates the
mechanism if needed.

### Bench

- [ ] `cargo bench` on `fib 30`, `ackermann 3 5`, `sum_loop`,
      `sum_while`, `foreach_block`, `func_call_heavy` — compare
      pre-M50 (v0.4.0 baseline) vs post-M50.
- [ ] If any fixture regresses >1%, implement the feature flag:
      - [ ] Add `features = ["react"]` to `crates/red-eval/Cargo.toml`
            (default-on).
      - [ ] Gate the `watchers` field on `Context` behind
            `#[cfg(feature = "react")]`; when off, `Context` has no
            `watchers` field at all (zero-cost — the branch is
            compiled out).
      - [ ] Gate `Env.reactions`/`observing`/`firing` behind the same
            feature.
      - [ ] Gate `react::register_react_natives` behind the feature;
            when off, `react` is an unbound word (Red-style "has no
            value" error).
      - [ ] The `set_slot`/`set_slot_unchecked` hook is compiled out
            when the feature is off — no branch at all.
      - [ ] Default: `react` on in dev/test builds (so the test suite
            exercises it). `--release` perf builds can opt out via
            `--no-default-features`.
- [ ] Add a `bench_fixtures` regression test: `vm_no_slower_than_
      v0_4_0_on_fib` (mirrors the existing
      `vm_no_slower_than_walker_on_fib` pattern in
      `tests/bench_fixtures.rs`). Catches gross regressions in
      `cargo test`.

### Decision criteria

- **0–1% regression:** keep `react` always-on, no feature flag.
  Document the measurement in `BENCHMARKS.md`.
- **1–5% regression:** feature flag, default-on in dev/test,
  default-off in `--release` perf builds. Document.
- **>5% regression:** investigate the branch. Likely cause: the
  `RefCell::borrow()` on `watchers` on every write. Mitigation: use
  `AtomicBool` for the "is watched" check (set when a `WatchTable` is
  installed) and only do the `RefCell::borrow()` if the atomic is true.
  Re-bench.

### Documentation

- [ ] Add "Reactive dialect — performance" subsection to
      `BENCHMARKS.md` with the bench numbers and the feature-flag
      decision.

---

## Milestone 55 — Reaction GC & edge cases

Reactions hold `Rc<Context>` to their source ctxs, keeping them alive.
A reaction whose every source ctx is unreferenced outside the registry
is dead weight. This milestone adds GC and hardens edge cases.

### GC

- [ ] `react/gc` — walk `registry.reactions`. For each `alive` reaction,
      check each source `Rc<Context>`: if `Rc::strong_count` is 1
      (only the registry holds it), the ctx is dead — mark the reaction
      `alive = false` and drop its `WatchTable` entries.
- [ ] **Lazy GC:** run `react/gc` every N fires (N = 64 by default;
      tunable via `Env::react_gc_interval`). The counter is bumped in
      `schedule_fire`. Avoids O(n) registry walks on every write.
- [ ] **`Env::drop`** — on env drop, the registry drops, which drops
      its `Rc<Context>`s. No leak.

### Edge cases

- [ ] **Object dropped mid-fire:** a reaction fires, writes to a
      field of an object that's been dropped (the only `Rc` was in the
      registry). The `Rc<Context>` in the `Reaction` keeps the ctx
      alive, so the write succeeds — but the *object value* (`Rc<
      RefCell<ObjectDef>>`) may be gone. Confirm the reaction body
      handles this (it reads `obj/field` via path resolution, which
      resolves `obj` as a word → if the word is unbound or holds
      `none`, the path errors gracefully).
- [ ] **Reaction registered in a function body:** the body `Series` is
      cloned into the registry (so it outlives the function call).
      The definition ctx is the function's call ctx — which is dropped
      on function return. The `Rc<Context>` in the `Reaction` keeps it
      alive, but reads/writes to it after the function returns are
      reads/writes to a now-orphaned context (no word references it).
      This matches Red: reactions registered in function bodies are
      usually bugs, but they don't crash. Document.
- [ ] **Reaction that writes to a non-object context:** the
      `WatchTable` is only installed on object ctxs (the hook in
      `slot_value` checks `watchers.is_some()` — user ctx and
      function-local ctxs have `watchers = None`). So a reaction
      body that does `user-word: 5` (a user-ctx write) does *not*
      trigger any reactions — only object-field writes do. Matches
      Red.
- [ ] **`unreact` on a dead reaction:** no-op (the reaction is
      `alive = false`; `unreact` walks alive reactions only).
- [ ] **`unreact/all` during a fire:** the drain loop holds a `Vec`
      of IDs to fire; `unreact/all` marks them all `alive = false`
      but the drain continues (the bodies still run). Matches Red:
      `unreact/all` takes effect after the current transaction
      drains. Implement by checking `alive` in the drain loop —
      skip dead reactions.

### Fuzz

- [ ] Extend `fuzz/` with a `react_fuzz` target: random `react`/write
      sequences. Invariant: the program terminates (cycle guard
      works), no panic (borrow-checker satisfied), no UB (the
      `unsafe` in `slot_value_unchecked` is sound under re-entrant
      fires). Run overnight; file issues for any failures.

### Tests

- [ ] Inline `#[test]`: `react/gc` drops dead reactions.
- [ ] Inline `#[test]`: object dropped mid-fire doesn't crash.
- [ ] Inline `#[test]`: `unreact/all` during fire drains current
      transaction.
- [ ] `cargo test --workspace` green; `--features force-walk` green.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.

---

## Milestone 56 — Documentation

- [ ] **`README.md`:**
  - [ ] Remove "reactive dialects" from "Known gaps" (line 241).
  - [ ] Add "Reactive dialect" bullet under "What's implemented →
        Dialect" (alongside `parse`).
  - [ ] Add `react`/`on-change`/`unreact`/`is-reactive?`/`react?`/
        `cause-reaction`/`update`/`clear-reactions` to the natives
        list.
  - [ ] Add `examples/reactive.red` to the examples table.
- [ ] **`architecture.md`:**
  - [ ] New "Reactive dialect" section covering the registry, the
        `Context` watcher hook, read tracking, write hook + batching,
        `on-change` vs `react`, the cycle guard, and the GC.
  - [ ] Update the "Cross-cutting" section: add "Reaction registry"
        subsection noting the `Option<WatchTable>` zero-cost-when-off
        design and the M54 feature-flag decision.
  - [ ] Update the mermaid crate-ownership diagram: `red-eval` now
        contains `react.rs`.
- [ ] **`project-brief.md`:**
  - [ ] Remove "reactive dialects" from "Other dialects (illustrative,
        NOT implemented)" (line 344-346).
  - [ ] Add "Reactive dialect" subsection under "Dialects" describing
        `react`/`on-change` semantics, batching, and the
        object-field-scoped design.
  - [ ] Update "Decisions confirmed": add "Reactive dialect: in scope
        (v0.5); `react`/`on-change` over object fields, no GUI."
- [ ] **`BENCHMARKS.md`:** add the M54 perf numbers and feature-flag
      decision.
- [ ] **`examples/reactive.red`:** a self-contained demo (counter,
      derived field, `on-change` logger, `unreact`).
- [ ] Final `cargo test --workspace` green.
- [ ] Final `cargo test --workspace --features force-walk` green.
- [ ] Final `cargo clippy --workspace --all-targets -- -D warnings`
      clean.
- [ ] Tag release `v0.5.0`.

---

## Open questions

1. **`set_slot_tracked` signature (M53):** the plan adds a separate
   `set_slot_tracked(&self, idx, val, env: &mut Env)` method (option (c)
   above) to avoid passing `env` through every existing `set_slot` call
   site. Alternative: bite the bullet and thread `&mut Env` through
   `set_slot`/`set_slot_unchecked` (option (a)) — 14+ call sites change,
   but the API is uniform (no tracked/untracked split). Recommendation:
   (c); the split is clean (untracked = boot-time `install_constants`,
   tracked = everything in eval) and the audit is one-time.
2. **Reaction body identity for `unreact`:** the plan uses
   `Rc::as_ptr` on the body `Series`'s `data` field (same ABA pattern as
   M27's `block_cache`). A user who constructs the same block twice
   (e.g. `b: [o/c: o/a] unreact b`) passes a different `Rc` — `unreact`
   won't match. Alternative: deep-structural equality on the body
   (walks the `Vec<Value>`). Recommendation: `Rc::as_ptr` for the common
   case (literal block in both `react` and `unreact`); add
   `unreact/struct` for deep equality if needed (deferred).
3. **`react` and the VM cache (M27):** reaction bodies run via
   `dispatch_block`, which keys its `block_cache` on
   `(Rc::as_ptr(&series.data), series.index)`. The registry holds the
   body `Series` alive (so the address is stable), so the cache hits on
   every fire — no recompilation. Confirm with a `--trace` test that
   shows a single `Compile` line on first fire and `Cache hit` on
   subsequent fires.
4. **`on-change` handler as `FuncDef` vs `Reaction` (M53):** the plan
   synthesizes a `FuncDef` with params `[old new]` for each `on-change`
   handler, stored in the `Reaction` record. Alternative: store the
   body as a plain `Series` and bind `old`/`new` as locals in a
   synthesized context per fire. Recommendation: `FuncDef` — reuses
   the existing `call_user_func` path (walker) and `CallUser` path
   (VM) for arg binding, no new binding machinery.
5. **Reaction firing across `EvalMode` boundaries:** if the script is
   in `Vm` mode but a reaction body is `needs_rebind`-flagged (e.g. it
   uses `use`), `dispatch_block` falls back to the walker per the
   existing M29 routing. Confirm reactions work in `--walk` mode and in
   mixed mode (VM script with walker-fallback reaction bodies). The
   parity harness (`tests/parity.rs`) covers this.
6. **`react` and `import`/modules (v0.6):** reactions are registered
   against the global `Env` registry, not per-module. When modules
   land (v0.6), reactions registered in one module will fire on writes
   to objects in another module's context. This matches Red's
   global-registry semantics. Document as a forward-compat note; no
   v0.5 action.
