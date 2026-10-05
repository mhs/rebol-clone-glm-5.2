# Plan: OTP-Style Supervisor Library POC

Extends the v0.6 actor primitives (M40–M47) with an Erlang/OTP-style
supervisor library. Built as a separate `import 'supervisor` module on
top of the existing `spawn-actor`/`send-actor`/`link`/`run-actors`
natives. One small Rust-level addition (`self` native) + a Red-level
module + an example.

## Background

The current `spawn-supervisor` native is a pure alias for `spawn-actor`
— there is no supervisor machinery. The `actor_supervisor.red` example
manually handles restart logic in the handler using globals. A proper
supervisor library should encapsulate child specs, restart strategies,
crash recovery, and id-based message routing.

## Key constraint: the `self` problem

Actor handlers run via `call_user_func`, which pushes a `CallFrame`
with the func's own `ctx` and evaluates the body on the walker. The
handler has no built-in reference to its own actor object. Without a
`self` primitive, the handler can't read its own state (`children`,
`specs`, `strategy`).

Alternatives considered:

- **Globals** — `sup: spawn-supervisor [func [msg] [foreach c sup/children ...]]`
  works (sup resolves from `user_ctx` at runtime) but breaks encapsulation:
  can't have two supervisors (they'd share the same global name).
- **`closure` capture** — chicken-and-egg: the closure captures `sup`
  *before* `spawn-supervisor` returns, so it captures `none`. Can't
  self-reference.
- **Inject `self` as handler arg** — would require modifying the scheduler
  to pass the actor as an extra arg. Changes the handler contract for ALL
  actors, not just supervisors.

**Decision: add a `self` native.** It's the fundamental actor identity
primitive (mirrors Erlang's `self()`), works for all actors, and has no
name collision with `make object! [self: ...]` — the object's `self`
slot takes binding priority via `Binding::Local`; the `self` native only
resolves when `self` is `Binding::Unbound` (inside a func handler, not
an object spec).

---

## Phase 1: Rust-level changes

### 1.1 Add `self` native

**File:** `crates/red-eval/src/natives/concurrency.rs`

```rust
/// `self` (arity 0): returns the current actor object (the one whose
/// handler is being dispatched), or `none` if outside an actor handler.
/// Mirrors Erlang's `self()`. Inside `make object! [self: ...]`, the
/// object's own `self` slot takes binding priority; this native only
/// resolves when `self` is `Binding::Unbound` (inside a func handler).
pub(crate) fn self_native(
    _args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    match &env.current_actor {
        Some(actor) => Ok(Value::Object(Rc::clone(actor))),
        None => Ok(Value::None),
    }
}
```

Register as `"self"` (arity 0) in `register_concurrency_natives`.

### 1.2 Embed the supervisor module

**File:** `crates/red-eval/src/stdlib.rs`

- Add `const SUPERVISOR_SRC: &str = include_str!("../stdlib/supervisor.red");`
- Add `pub fn ensure_supervisor_module(env: &mut Env) -> Result<(), EvalError>`:
  - Parses + evaluates `SUPERVISOR_SRC` as a module body
  - Caches the resulting `ModuleDef` in `env.modules` under the name
    `'supervisor` (so `import 'supervisor` works)
  - Idempotent (skips if already cached)
- Call `ensure_supervisor_module(env)` from `run_series_inner_opts`
  alongside `ensure_stdlib` (unless `--no-stdlib` is set)

---

## Phase 2: Red-level supervisor module

**File:** `crates/red-eval/stdlib/supervisor.red`

### Module structure

```red
module 'supervisor [

    ; ---- Child spec helpers ----

    ; make-child-spec 'id handler-func
    ;   → make object! [id: <id> handler: <func> restart: 'permanent]

    ; ---- Supervisor creation ----

    ; supervise [child-spec1 child-spec2 ...]
    ;   1. Creates a supervisor actor whose handler manages the children.
    ;   2. Spawns each child from its spec, links self to each.
    ;   3. Stores specs + children blocks on self (via `self` native).
    ;   4. Returns the supervisor actor value.

    ; ---- Message routing ----

    ; supervisor-send sup 'child-id msg
    ;   Sends [child-id msg] to the supervisor's mailbox.

    ; ---- Supervisor handler logic ----

    ; On "exit":
    ;   - Scan self/children for alive? = false
    ;   - Find the spec for the dead child (by matching id)
    ;   - Spawn a new child from the spec, link self to it
    ;   - Replace the dead child in self/children
    ;   - Increment self/restart-count; if > max-restarts, die
    ;
    ; On [child-id msg]:
    ;   - Find child in self/children by id
    ;   - send-actor child msg

    export [supervise supervisor-send]
]
```

### Child spec shape

```red
make object! [
    id: 'worker1
    handler: func [msg] [...]
    restart: 'permanent
]
```

### Restart strategy

`'one-for-one` only (POC). On child death, restart just that child.
No cascade.

### Max restarts

Simple counter (`self/restart-count`). After N restarts (default 3),
the supervisor sets its own `alive?` to false (cascading failure).

### Exit message format

Bare string `"exit"` (current limitation from M47: actor objects can't
cross the Send boundary because they contain `Func` handlers). The
supervisor scans `self/children` for `alive? = false` to find the dead
child. O(n) but fine for a POC.

---

## Phase 3: Example

**File:** `examples/otp_supervisor.red`

```red
Red []
import 'supervisor

worker1: make object! [id: 'w1 handler: func [msg] [
    either msg = 'crash [print "w1: crashing" 1 / 0] [print "w1: ok"]
]]
worker2: make object! [id: 'w2 handler: func [msg] [print "w2: ok"]]

sup: supervise [worker1 worker2]

supervisor-send sup 'w1 "hello"
supervisor-send sup 'w2 "world"
run-actors

supervisor-send sup 'w1 'crash
run-actors

supervisor-send sup 'w1 "still here"
run-actors
```

### Expected output

```
w1: ok
w2: ok
w1: crashing
supervisor: w1 died, restarting
w1: ok
```

---

## Limitations (POC gaps)

- **One-for-one only** — `one-for-all` and `rest-for-one` deferred
- **No max-restart intensity/period** — simple counter (N restarts then
  give up); no time-window tracking
- **Exit message is bare string** — supervisor scans children for
  `alive? = false` (O(n)); structured exit payloads deferred
- **No graceful shutdown** — no `exit actor reason` native, no
  `trap-exit?` flag
- **No child shutdown ordering** — children are killed in arbitrary
  order when the supervisor dies

---

## Verification

- `cargo build --workspace` passes
- `cargo test --workspace` passes (`self` native + supervisor module
  don't break existing tests)
- `./target/.../red-cli examples/otp_supervisor.red` produces expected
  output

---

## Files

| File | Action |
|---|---|
| `crates/red-eval/src/natives/concurrency.rs` | Add `self` native + register |
| `crates/red-eval/src/stdlib.rs` | Add `SUPERVISOR_SRC` embed + `ensure_supervisor_module` |
| `crates/red-eval/stdlib/supervisor.red` | **Create** — supervisor library module |
| `examples/otp_supervisor.red` | **Create** — demo with 2 children, crash + restart |
