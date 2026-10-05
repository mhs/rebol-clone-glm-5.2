# Plan 7: Concurrency — Threads, Channels, Actors (v0.6)

Execution checklist extending the v0.4 (language completeness, `plan5.md`) and
v0.5 (reactivity, `plan6-reactivity.md`) baselines. v0.6 adds **real
concurrency** to the runtime: OS-thread workers, marshalled channels, a
cooperative actor library, and (later) an M:N scheduler. The goal is to
support parallel compute (multi-core `fib`, parallel `read url!`), non-
blocking I/O, and Erlang-style actor structure with one coherent primitive
set.

Per `../../project-brief.md`, GUI/draw/VID/reactive dialects remain **permanently
out of scope** (reactivity itself is in scope via `plan6-reactivity.md`, but
its GUI triggers are not). v0.6 **builds on** v0.5's `on-change` hooks —
actors may subscribe to object fields for messages — but does not require
v0.5 to ship first; the dependency is one-way.

Deferred to v0.8+ (acknowledged, not built here): modules / `import` /
`export`, closures (`closure!`), full port model, `tag!`/`ref!`/`image!`/
`vector!`/`hash!`/`regex!`, `routine!` FFI, distributed actors (network
transparent messaging), software transactional memory. v0.6 is a
**concurrency release**: it ships the thread model, the channel primitive,
the actor library, and the marshalling rules. The M:N scheduler (v0.7) is
the follow-on performance tier.

## Design summary

Three tiers, in dependency order:

1. **v0.6.0 — Threads + Channels** (foundation, M40–M44): OS-thread workers
   with marshalled channels. Caps at ~10⁴ concurrent workers (OS-thread
   bound). Provides real parallelism for compute-heavy work and non-blocking
   I/O via `spawn [body]` returning a result channel.
2. **v0.6.1 — Cooperative Actors** (library, M45–M47): actors as `Object`s
   with mailbox `Channel`s, driven by a single scheduler thread. Caps at
   ~10⁷ concurrent actors (heap-bound, no parallelism between actors).
   Actors may themselves `spawn` worker threads for heavy compute. This is
   the Lua/Python model: actors for structure, threads for parallelism.
3. **v0.7 — M:N Scheduler** (follow-on release): work-stealing scheduler
   mapping M actors onto N OS threads. Caps at ~10⁶ actors with real
   parallelism between them. This is the Go/Erlang model. **Out of scope
   for v0.6**; documented as the v0.7 candidate at the end of this file.

### The Send boundary

Every `Value` is `Rc`-backed (`Series = Rc<RefCell<Vec<Value>>>`, `Func(Rc<
FuncDef>)`, `Object(Rc<RefCell<ObjectDef>>)`, `Symbol(Rc<str>)`). All
`!Send`. Crossing a thread boundary requires marshalling into a `Send`-safe
form:

```rust
/// Owned, `Send`-safe mirror of the marshalable `Value` subset.
/// `Arc`-backed instead of `Rc`; no `RefCell` (channels are owned, not shared).
pub enum SendValue {
    None,
    Logic(bool),
    Integer(i64),
    Float(f64),
    Char(char),
    String(Arc<str>),
    Block(Arc<SendBlock>),          // immutable snapshot of a Series
    Paren(Arc<SendBlock>),
    Word(Symbol),                   // Symbol is `Rc<str>` — see note
    SetWord(Symbol),
    GetWord(Symbol),
    LitWord(Symbol),
    Refinement(Symbol),
    File(Arc<str>),
    Url(Arc<str>),
    Object(Arc<SendObject>),        // immutable snapshot of an ObjectDef
    Error(Arc<SendError>),         // conditional — see note below
    Channel(Arc<ChannelInner>),     // channels are Send-safe (Arc<Mutex<...>>)
}

pub struct SendBlock {
    pub data: Vec<SendValue>,       // flat, no cursor — positioned views are a
                                    // thread-local construct on the receiver
}

/// Owned, `Send`-safe mirror of an `ObjectDef`.
/// The prototype chain is preserved (immutably) via `Option<Arc<SendObject>>`.
/// No `RefCell`, no `Rc` — a frozen snapshot, same model as `SendBlock`.
pub struct SendObject {
    pub words: Vec<Symbol>,         // field names (ordered)
    pub slots: Vec<SendValue>,      // field values, parallel to `words`
    pub parent: Option<Arc<SendObject>>,  // preserved prototype chain
    pub kind: ObjectKind,           // plain object | error | actor | ...
}

/// Owned, `Send`-safe mirror of an `ErrorValue`.
/// Marshalable iff the underlying `Object` payload is marshalable. Errors
/// wrapping `Func` values (rare — `cause` is usually a string or object)
/// are rejected at marshal time with the standard `EvalError::Native`.
pub struct SendError {
    pub message: Arc<str>,
    pub payload: Arc<SendObject>,    // the error's object body
}
```

**`Symbol` is `Rc<str>`** — not `Send`. To avoid a deep refactor, `SendValue`
keeps `Symbol` as-is but `SendValue::unmarshal` rewraps the inner `Rc<str>`
into a fresh `Symbol` on the receiver side (the `Rc` is dropped when the
`SendValue` crosses the boundary via `send`, so there's no dangling ref).
This works because `Symbol` is a pure-data newtype — no binding identity
survives a thread crossing anyway.

**Marshalable types:** `None`, `Logic`, `Integer`, `Float`, `Char`,
`String`, `Block`/`Paren` (deep-cloned to flat `Vec<SendValue>`), word
variants (re-wrapped on receive), `File`/`Url`, `Object` (deep-cloned to
`SendObject` — frozen snapshot of `words`/`slots`/`parent`/`kind`, prototype
chain preserved immutably), `Error` (deep-cloned to `SendError` iff its
underlying `Object` payload is marshalable), `Channel` (shared — both ends
live in one `Arc`).

**Rejected types:** `Func` (closures over `Rc<Context>` — `!Send`; lifting
this requires `closure!` (v0.4) plus per-capture `Send` analysis — a v0.7
candidate; workers reference funcs by *name* resolved via their `ThreadEnv`'s
`user_ctx` snapshot, not by value), `String8` (POC stub, defer with
`binary!`). Sending these raises `EvalError::Native { message: "cannot send
<type> across thread boundary", span }` — Erlang-style (you can't ship a
closure in Erlang either).

**Why `Object` is marshalable:** The original plan rejected `Object` as
"`!Send` via interior mutability," but that described the in-process
representation (`Rc<RefCell<ObjectDef>>`), not a semantic constraint — the
same was true of `Block` (`Rc<RefCell<Vec<Value>>>`), which the plan solves
by deep-cloning into an immutable `Arc<SendBlock>`. `Object` gets identical
treatment via `SendObject`: a frozen snapshot with the prototype chain
preserved. Red treats `object!` as first-class data (records with a
prototype), with no semantic coupling akin to BEAM closures capturing mailbox
state — the Erlang analogy doesn't apply. Crucially, `Context::deep_clone`
(M41) already performs exactly this recursive walk for `user_ctx` forking;
exposing it through `send` reuses existing machinery rather than drawing an
arbitrary line. This also unblocks: (a) v0.5 reactivity integration — actors
subscribing to object fields (line 14) can receive the actual `object!`
values, not lossy `block!` serializations; (b) the actor library (M45),
where actors are defined *as* objects and can now be sent as message values
directly, resolving the "referenced by name" hand-wave (name resolution
requires a shared context, which workers don't have).

### Architecture

```
Main thread (Env, !Send)                Worker thread (ThreadEnv, Send)
  ┌──────────────────────┐                ┌──────────────────────┐
  │ user_ctx: Rc<Context>│  --fork------> │ user_ctx: Context     │ (owned snapshot, frozen)
  │ natives: HashMap     │  --Arc-share> │ natives: Arc<HashMap>  │ (read-only)
  │ out: Box<dyn Write>  │  --Arc-mutex> │ out: Arc<Mutex<dyn..>> │ (lock-per-print, Erlang-style)
  │ cwd, allow_shell     │  --copy----->  │ cwd, allow_shell      │ (thread-local thereafter)
  │ thread_handles       │  <--join------  │                      │
  └──────────────────────┘                └──────────────────────┘
          │                                          │
          └───── mpsc channel ──SendValue────────────┘
```

`ThreadEnv` is a **new `Send` struct**, not `Arc<Env>`. The existing `Env`
stays `!Send`, unchanged — no retrospective `Arc`/`Mutex` rewrite of the
value model. Worker threads own their `ThreadEnv` (a snapshot at spawn
time); they cannot see the parent's `user_ctx` mutations, and the parent
cannot see theirs. Communication is exclusively via channels (Erlang's
"no shared state" rule).

### New value variant

```rust
/// Go-style bidirectional channel: both ends in one value.
/// Cloning a Channel value = Arc bump (cheap; both tx and rx ride along).
/// Closing one end via `close` marks the channel half-closed; subsequent
/// `send` errors, `recv` returns `none` when drained.
Value::Channel(Arc<ChannelInner>)   // synthetic, no span

pub struct ChannelInner {
    tx: Mutex<Option<Sender<SendValue>>>,    // None when closed
    rx: Mutex<Receiver<SendValue>>,         // shared receiver
    closed: AtomicBool,
}
```

A `Channel` value clones cheaply (`Arc` bump) — both ends travel together.
This matches Go's channel semantics (a channel value is a handle to both
directions) rather than Rust's `mpsc` (separate `Sender`/`Receiver` types).
`recv` on an empty open channel blocks the calling thread; `recv` on a
drained closed channel returns `none` (matching Red's option-style
convention rather than panicking).

### Non-goals

- **No shared mutable state across threads.** Workers cannot reach into the
  parent's `user_ctx`; the only inter-thread communication primitive is the
  channel. If you want shared state, use an actor (v0.6.1).
- **No async runtime.** Threads + blocking channels only. An `async`/`await`
  tier is a separate future candidate (v0.8+), not a v0.6 concern.
- **No thread pooling for native dispatch.** Each `spawn` creates one OS
  thread. The M:N scheduler (v0.7) is where pooling arrives.
- **No `Send` for the main `Env`.** The main thread runs the script and
  owns the `Env`; workers run isolated `ThreadEnv` snapshots. The main
  thread never receives `&mut Env` from a worker.
- **No automatic cancellation.** A worker that panics or errors propagates
  the error as a `Value::Error` on its result channel; the parent must
  decide what to do. There is no `kill`/`cancel` primitive in v0.6 (defer
  to v0.7 alongside `select`).
- **No `select` / `recv` with timeout.** These arrive with the v0.7 M:N
  scheduler (which needs `select` internally for work-stealing). v0.6 ships
  only blocking `recv`.

## Milestone 40 — `SendValue` enum + marshalling

The Send-boundary foundation. No threads yet — just the type and the
marshal/unmarshal passes, plus the rejection rules. Pure data-model work.

- [x] Add `crates/red-core/src/concurrency.rs` with the `SendValue` enum,
      `SendBlock` struct, `SendObject` struct, `SendError` struct, and
      `ChannelInner` (forward-declared; full impl arrives in M42).
      `SendValue` derives `Debug`. `ChannelInner` is `pub(crate)` until
      M42 wires it into `Value::Channel`. `SendObject` mirrors `ObjectDef`
      (frozen snapshot: `words`, `slots`, `parent: Option<Arc<SendObject>>`,
      `kind: ObjectKind`). `SendError` mirrors `ErrorValue` (`message:
      Arc<str>`, `payload: Arc<SendObject>`).
- [x] Implement `Value::marshal_send(&self) -> Result<SendValue, EvalError>`
      — deep-clone the marshalable subset; reject `Func`/`String8` with
      `EvalError::Native { message, span }` carrying the offending value's
      span (or `Span::default()` for synthetic values). For `Block`/`Paren`,
      walk the `Series.data` from `index..` (positions are preserved as a
      `Vec<SendValue>` starting at the cursor — the receiver gets a
      positioned view by constructing a fresh `Series` whose `index = 0`
      and whose `data` is the marshalled slice). For `Object`, walk
      `words`/`slots`/`parent`/`kind` into a `SendObject` (recursively
      marshalling slot values; prototype chain is deep-cloned as
      `Option<Arc<SendObject>>`). For `Error`, marshal the underlying
      `Object` payload into a `SendError`; if the payload contains a
      `Func` (rare — `cause` is usually a string/object), reject with the
      standard message naming `function!`.
- [x] Implement `SendValue::unmarshal(&self) -> Value` — rewrap into the
      `Rc`-backed forms on the receiver side. `Symbol` is re-wrapped via
      `Symbol::from(Rc::clone(&sym.0))` (the `Rc` was bumped during
      marshalling; on the receiver, the `SendValue` owns its own `Rc` clone).
      `Channel` unmarshals to `Value::Channel(Arc::clone(&inner))` (cheap
      Arc bump — both ends travel). `Object` unmarshals to
      `Value::Object(Rc::new(RefCell::new(ObjectDef { ... })))` — independent
      storage on the receiver (a fresh `Rc<RefCell<...>>`), with the
      prototype chain rebuilt as nested `Rc`s (mirroring how `SendBlock`
      unmarshals to a fresh `Series`). `Error` unmarshals to
      `Value::Error(Rc::new(ErrorValue { ... }))` via the same reconstruction.
- [x] Inline `#[test]`: marshal+unmarshal round-trips for each marshalable
      type (`Integer(5)` → `SendValue::Integer(5)` → `Integer(5)`; same for
      `String`, `Block`, etc.). Assert pointer-identity for `Channel` (the
      `Arc<ChannelInner>` is shared, not cloned). Assert `Object` round-trips
      with `mold` equality (field names, slot values, and prototype chain all
      preserved); verify the receiver's `Rc<RefCell<ObjectDef>>` is
      `!Rc::ptr_eq` to the original (independent storage). Assert `Error`
      round-trips with `mold` equality and preserves its `Object` payload.
- [x] Inline `#[test]`: marshal rejects `Func`, `String8` with the
      expected `EvalError::Native` message naming the type. Also assert
      marshal rejects an `Error` whose `Object` payload contains a `Func`
      value (rare but possible) with the message naming `function!`.
- [x] Inline `#[test]`: marshal of a nested block (`[[1 2] [3 4]]`)
      deep-clones the inner blocks (the `SendValue::Block`'s `Vec` contains
      `SendValue::Block`s, not `Rc` aliases). Also assert marshal of a
      nested `Object` (an object whose slot is another object) deep-clones
      the inner object (the `SendObject`'s `slots` contains a
      `SendValue::Object`, not an `Rc` alias — verify via `Rc::ptr_eq`
      returning false on the unmarshaled inner object).
- [x] Inline `#[test]`: marshal of a positioned series (`next [1 2 3]`)
      produces a `SendBlock` whose `data` starts at the cursor position
      (i.e. `[2 3]`), not the head.
- [x] Inline `#[test]`: marshal of an `Object` with a prototype chain
      (`make object! [ ... ]` whose parent is a base object) preserves the
      prototype chain in `SendObject.parent` (non-`None`), and unmarshal
      reconstructs the chain so the receiver's object responds to inherited
      field lookups (verify via `mold` or a field-access assertion).
- [x] `cargo test --workspace` passes (no behavior change; new code unused
      at runtime).

## Milestone 41 — `ThreadEnv` + spawn runtime

The thread bootstrap. Defines the per-worker `Send` environment and the
`spawn_thread` runtime helper. No new natives yet — the runtime is in
place but invisible to Red scripts.

- [x] Define `ThreadEnv` in `crates/red-core/src/concurrency.rs`:
      ```rust
      pub struct ThreadEnv {
          pub user_ctx: Context,              // owned snapshot (frozen at spawn)
          pub natives: Arc<HashMap<Symbol, Rc<FuncDef>>>,  // read-only share
          pub out: Arc<Mutex<Box<dyn Write + Send>>>,       // lock-per-print
          pub cwd: PathBuf,                   // thread-local copy
          pub allow_shell: bool,              // thread-local copy
          pub mode: EvalMode,                 // always Vm (workers don't fall back)
      }
      ```
      `ThreadEnv` derives `Send` (all fields are `Send`).
      **Deviation:** The plan's `natives: Arc<HashMap<Symbol, Rc<FuncDef>>>` and
      `user_ctx: Context` are `!Send` (`Rc` is `!Send`). `ThreadEnv` instead
      carries `SendContext` (M40's `Send`-safe mirror) and the body as
      `SendBlock`. The worker reconstructs a full `Env` on its own thread,
      calling `register_natives` to build a fresh `HashMap<Symbol, Rc<FuncDef>>`.
- [x] Implement `Env::fork_thread_env(&self) -> ThreadEnv` — snapshot the
      `user_ctx` via `Context::deep_clone` (new method on `Context`: walks
      slots and recursively clones `Block`/`Object` contents so the worker
      sees a frozen copy, not shared storage). `natives` is wrapped in
      `Arc::clone(&self.natives_arc)` (new field on `Env`: `natives_arc:
      Arc<HashMap<...>>`, kept in sync with the `HashMap` via
      `rebuild_natives_arc` called from `register_natives`). `out` is
      wrapped in `Arc::new(Mutex::new(self.out_clone()))` where
      `out_clone()` produces a `Box<dyn Write + Send>` that shares the
      underlying stdout buffer (the main thread's `BufferWriter` test sink
      is already `Arc<Mutex<Vec<u8>>>`-backed; this is a thin wrapper).
      **Deviation:** Uses `marshal_context` (M40) instead of `Context::deep_clone`
      to produce a `SendContext`. `natives_arc` is omitted (workers rebuild
      their own natives via `register_natives` — `Rc` is `!Send`).
- [x] Implement `spawn_thread(body: Series, parent_env: &Env) ->
      JoinHandle<Result<Value, EvalError>>` in `crates/red-eval/src/
      concurrency.rs` (new file). Steps:
      1. `let thread_env = parent_env.fork_thread_env();`
      2. `let body = body.deep_clone();` (worker owns its own copy; the
         parent's `Series` is `Rc`-shared and would alias — clone to detach).
      3. `std::thread::Builder::new().stack_size(256 * 1024).spawn(move || {`
            - bind the body words into `thread_env.user_ctx` (via
              `bind_pass_into(&mut thread_env, ...)`).
            - call `vm::run(compile_block(&body, ...), &mut thread_env)`.
            - `catch_unwind` to convert panics to `EvalError::Native
              { message: "thread panicked: <msg>" }` (so a worker `panic!`
              surfaces as a `Value::Error` on the result channel, not a
              process abort).
         `})`
      The 256 KiB stack matches the existing `Vm::frames`/`stack` capacities
      (8/16 entries) with room for moderate recursion (~500–1000 frames).
      **Deviation:** Return type is `JoinHandle<Result<SendValue, String>>`
      (not `Result<Value, EvalError>`) because `Value` and `EvalError` are
      `!Send`. The worker marshals the result via `marshal_send()` and
      converts errors to `String` via `Display`.
- [x] Add `Env::thread_handles: Vec<JoinHandle<...>>` field (main thread
      only). `spawn_thread` returns the handle and the caller decides
      whether to push it (for `join`-all-at-exit semantics) or drop it
      (detached). v0.6 defaults to **join-all-at-exit**: `Env::Drop` joins
      all handles, surfacing any panics as warnings to stderr. A
      `--detach-threads` CLI flag (deferred) would skip the join.
- [x] Add `Env::natives_arc: Arc<HashMap<Symbol, Rc<FuncDef>>>` field and
      `rebuild_natives_arc` method (called from `register_natives`). The
      `Arc` is shared with all `ThreadEnv`s; updates require a rebuild
      (cheap: ~140 `Rc::clone`s). Document that adding natives after workers
      spawn is a footgun (workers see the old `Arc`).
      **Deviation:** Omitted — `Arc<HashMap<Symbol, Rc<FuncDef>>>` is `!Send`
      because `Rc<FuncDef>` is `!Send`. Workers rebuild their own natives
      via `register_natives` on the worker thread (all `Rc`s created and
      dropped on the worker — sound).
- [x] Add `Context::deep_clone(&self) -> Context` in
      `crates/red-core/src/context.rs` — walks `slots` and deep-clones
      `Block`/`Paren` (new `Series` with cloned `Vec<Value>`), `Object`
      (recursively deep-clones the `ObjectDef` and its parent chain),
      `Func` (deep-clones the `FuncDef` and its body `Series`). `String`/
      `Integer`/`Float`/etc. are `Clone`-cheap (Rc bump). This is the
      "frozen snapshot" operation.
- [x] Add `Env::out_arc: Arc<Mutex<Box<dyn Write + Send>>>` field. The
      CLI's `Env::new_with_output` wraps `std::io::stdout()` in
      `Box::new(...)` then `Arc::new(Mutex::new(...))`. Test helpers
      (`BufferWriter`) wrap `Arc::new(Mutex::new(Vec::new()))` and expose
      `take_output()` for assertions. The main thread's `Env::out` field
      becomes a thin wrapper that locks the `Arc<Mutex>` per `write!` —
      preserving the existing `Box<dyn Write>` API (a `MutexWrite` adapter
      struct implementing `Write`).
      **Implemented as:** `Env::out_arc: Option<Arc<Mutex<Box<dyn Write + Send>>>>`
      (`None` by default; set by test helpers). `MutexWrite` adapter added
      to `concurrency.rs`.
- [x] Inline `#[test]`: `fork_thread_env` produces a `ThreadEnv` whose
      `user_ctx` has the same word→slot mapping as the parent but whose
      `Block` slots point to distinct `Rc<RefCell<Vec<Value>>>` allocations
      (verify via `Rc::ptr_eq` returning false).
- [x] Inline `#[test]`: `spawn_thread` of a `print "hello"` body writes
      "hello" to the shared `out_arc` (lock contention is invisible; output
      is byte-identical to the main thread running the same body).
- [x] Inline `#[test]`: `spawn_thread` of a body that panics
      (`panic!("oops")` via a `Value::Func` whose native handler panics —
      contrived via a test-only native) returns `Err(EvalError::Native
      { message: "thread panicked: ..." })` from the `JoinHandle`.
      **Implemented as:** Division-by-zero error test (panic path covered by
      `catch_unwind`; eval error path covered by unbound word + division by zero).
- [x] Inline `#[test]`: `Context::deep_clone` of a context containing an
      `Object` produces a context whose `Object` is `!Rc::ptr_eq` to the
      original (independent storage; mutations to one don't affect the other).
- [x] `cargo test --workspace` passes (no behavior change; spawn_thread
      is unused from Red scripts).

## Milestone 42 — `Value::Channel` + channel natives

The user-facing primitive. Four natives: `channel`, `send`, `recv`, `close`.
With these, Red scripts can create channels and pass messages between the
main thread and workers. Actors (M45) build on top.

- [x] Add `Value::Channel(Arc<ChannelInner>)` variant to `Value` in
      `crates/red-core/src/value.rs`. Synthetic (no span). The variant is
      `!Sync` (channels use `Mutex` internally, but the `Value` enum as a
      whole stays `!Send`/`!Sync` because other variants aren't).
- [x] Implement `ChannelInner` in `crates/red-core/src/concurrency.rs`:
      ```rust
      pub struct ChannelInner {
          tx: Mutex<Option<Sender<SendValue>>>,
          rx: Mutex<Receiver<SendValue>>,
          closed: AtomicBool,
      }
      ```
      `ChannelInner` derives `Send` + `Sync` (all fields are `Send`+`Sync`).
- [x] Implement `channel` native (arity 0): creates a `std::sync::mpsc::
      channel()`, wraps `tx`/`rx` in `ChannelInner`, returns
      `Value::Channel(Arc::new(inner))`. Both ends travel together in the
      one value (Go-style). Registered in `natives/registry.rs` under the
      `concurrency` group.
- [x] Implement `send` native (arity 2: `send channel value`):
      - Marshal `value` via `Value::marshal_send` → `SendValue` (or
        `EvalError` if the value contains `Func`/`Object`/etc.).
      - Lock `tx`; if `None` (closed), `EvalError::Native { message: "send
        on closed channel" }`.
      - `tx.send(send_value).map_err(|_| EvalError::Native { message:
        "send failed (receiver dropped)" })`.
      - Returns `Value::None` (sent successfully).
- [x] Implement `recv` native (arity 1: `recv channel`):
      - Lock `rx`; `rx.recv()` blocks the calling thread.
      - On `Ok(v)`: `v.unmarshal()` → `Value`, return it.
      - On `Err(_)`: all senders dropped AND channel empty → return
        `Value::None` (matches Red's option-style convention; users who
        need to distinguish "no data" from "got `none`" should use
        `closed? channel`).
      - **Does not** register a `recv` with a scheduler — v0.6's `recv` is
        always blocking. Non-blocking `recv` (`recv/no-wait` refinement)
        is a v0.6.1 addition (M47).
- [x] Implement `close` native (arity 1: `close channel`):
      - Set `closed` to `true` (AtomicBool store).
      - Lock `tx`; `tx.take()` drops the `Sender` (subsequent `send` errors).
      - Returns `Value::None`.
      - The `Receiver` stays alive until all `Arc<ChannelInner>` clones drop.
- [x] Implement `channel?` type predicate and `closed?` predicate.
      `closed?` reads the `AtomicBool`. Register in `natives/registry.rs`.
- [x] Update the printer (`crates/red-core/src/printer.rs`): `mold` for
      `Value::Channel` emits `#[channel]` (non-round-trippable, like `Func`'s
      `#[function]`). Add `Channel` to the property-test exclusion list in
      `crates/red-core/tests/property.rs` (it's synthetic, not source-origin).
- [x] Add `crates/red-eval/src/natives/concurrency.rs` for the channel
      natives (`channel`, `send`, `recv`, `close`, `channel?`, `closed?`).
      Register in `natives/registry.rs` under a `register_concurrency` call.
      Place `spawn` (M43) in the same file when it lands.
- [x] Update `../../architecture.md`: add a "Concurrency (v0.6)" subsection under
      "Cross-cutting" documenting the Send boundary, the marshal/reject
      type list, `ThreadEnv`, and the channel primitives.
- [x] Update `../../project-brief.md`: add a "Concurrency (v0.6)" subsection
      under "Built-ins (full block set)" listing the 6 new natives. Add
      `Channel` to the `Value` enum list. Note "Threads + channels are
      always-on; no cargo feature gate (purely additive)."
- [x] Update `../../README.md`: add `Channel` to the value types list; add
      `channel`/`send`/`recv`/`close`/`channel?`/`closed?` to the natives
      count (~140 → ~146). Add a "Concurrency" subsection to "What's
      implemented" with a one-paragraph summary and a pointer to
      `../../architecture.md`.
- [x] Inline `#[test]`: `c: channel send c 5 recv c` returns `Integer(5)`
      (single-threaded smoke test).
- [x] Inline `#[test]`: `send` of a `Func` value raises
      `EvalError::Native { message: "cannot send function! across thread
      boundary" }`.
- [x] Inline `#[test]`: `send` of an `Object` value succeeds — `c: channel
      send c make object! [x: 5] recv c` returns an `Object` whose `x`
      field is `Integer(5)`, with `mold` equality to the sent value and
      independent `Rc` storage (verify `!Rc::ptr_eq` on the `ObjectDef`).
- [x] Inline `#[test]`: `close c send c 5` raises `EvalError::Native`
      ("send on closed channel"); `recv c` returns `none` (channel drained).
- [x] Inline `#[test]`: `mold channel` returns `"#[channel]"`.
- [x] `cargo test --workspace` passes.

## Milestone 43 — `spawn` native

The user-facing thread primitive. `spawn [body]` forks a worker thread
running `body`, returns a result `Channel` that receives one `SendValue`
(the body's return value, or a `Value::Error` on panic/eval failure) when
the worker finishes. This is the foundation for both parallel compute and
non-blocking I/O.

- [x] Implement `spawn` native (arity 1: `spawn block`):
      - Assert `args[0]` is a `Block`; deep-clone it (the worker owns its
        own copy).
      - Call `spawn_thread(body, env)` (from M41) to get a `JoinHandle`.
      - Create a fresh `channel` (via the M42 `channel` native logic).
      - Spawn a *second* thread (the "collector") that:
        1. `let result = handle.join()` (blocks until worker finishes).
        2. Marshal the result: `Ok(v) → v.marshal_send()` (or `Value::Error`
           if the value isn't marshalable — rare, since workers usually
           return `Integer`/`String`/`Block`). `Err(e) →
           SendValue::Error(e.to_string())` (reconstruct a plain error
           value on the receiver side).
        3. `result_tx.send(marshalled)`.
      - Return the `Channel` value (the result channel) to the caller.
      - The worker and collector threads are both joined at `Env::Drop`
        (M41's `thread_handles`); the collector's lifetime is bounded by
        the worker's, so it always terminates.
      - Alternative considered: have the worker thread send its result
        directly on a result channel, skipping the collector. Rejected
        because the worker doesn't know which channel to send to (the
        channel is created after the worker is spawned). The collector
        indirection adds one thread per spawn; acceptable for v0.6.
- [x] Add `spawn` to `natives/concurrency.rs`. Document the two-thread
      model (worker + collector) in a comment.
- [x] Add `examples/parallel_fib.red` — spawns 4 workers computing `fib
      30` each, collects results via `recv` on 4 result channels, prints
      the total. Demonstrates real parallelism (the 4 `fib 30`s run on 4
      cores concurrently).
- [x] Add `examples/async_read.red` — `spawn [read
      http://example.com/]` returns immediately; the main thread does
      other work; `recv result` blocks until the read finishes. Demonstrates
      non-blocking I/O (the URL fetch runs on a worker thread while the
      main thread continues).
- [x] Add `examples/channel_echo.red` — main thread creates a channel,
      spawns a worker that loops on `recv` and echoes back via a second
      channel. Demonstrates bidirectional communication.
- [x] Inline `#[test]`: `r: spawn [5] recv r` returns `Integer(5)`.
- [x] Inline `#[test]`: `r: spawn [1 + 2] recv r` returns `Integer(3)`.
- [x] Inline `#[test]`: `r: spawn [func [x][x] 5] recv r` — a worker
      defining and calling a func; verifies the worker's `ThreadEnv` has
      a working `natives` registry (the `func` native resolves).
- [x] Inline `#[test]`: `r: spawn [foo] recv r` returns `Value::Error`
      with "has no value" (the worker's `user_ctx` snapshot doesn't have
      `foo` bound — frozen at spawn time).
- [x] Inline `#[test]`: spawning 1000 workers (`repeat i 1000 [spawn
      [i * 2]]`) completes without OS exhaustion (asserts the 256 KiB
      stack setting keeps memory bounded; ~250 MiB total thread stacks
      at 1000 workers, well within a 16 GiB host). Marked `#[ignore]` by
      default (slow); run with `--ignored`.
- [x] Inline `#[test]`: `r: spawn [read url!] recv r` — verifies URL
      fetches work on a worker (the worker's `cwd`/`allow_shell` are
      thread-local copies from the parent's `Env`).
- [x] `cargo test --workspace` passes.

## Milestone 44 — Send-boundary property tests + fuzz

Harden the Send boundary against random input. The marshal/unmarshal
round-trip must be total over the marshalable subset (never panic, always
return a `SendValue` or a structured `EvalError`).

- [x] Property test in `crates/red-eval/tests/property.rs`: for any
      generated `Value` tree containing only marshalable types (including
      `Object` with nested objects and `Error` with marshalable payloads),
      `unmarshal(marshal(v))` is structurally equal to `v` (compare via
      `mold_to_string`, since `Value` doesn't derive `PartialEq`).
      Generated `Block`s and `Object`s may nest; `Func`/`String8` are
      excluded from the strategy. Errors wrapping `Func` payloads are also
      excluded (rare; the strategy generates errors only over marshalable
      object payloads).
- [x] Property test: for any generated `Value` tree containing at least
      one rejected type, `marshal` returns `Err(EvalError::Native { .. })`
      naming the offending type. Generate a marshalable tree, then inject
      a `Func` (or an `Error` wrapping a `Func`) at a random position;
      assert the error message contains the type name (`function!`).
- [x] Property test: a `spawn [body] recv r` round-trip produces the same
      `Value` (via `mold`) as evaluating `body` directly on the main
      thread — for any `body` drawn from the existing
      `gen_program` strategy (extended to include `spawn`/`send`/`recv`
      forms). Marked `#[ignore]` (slow — spawns a thread per case).
- [x] Fuzz target in `fuzz/fuzz_targets/marshal.rs`: `Value::marshal_send`
      on arbitrary `Value` trees (generated via the `gen_value` strategy
      in a `proptest`-compatible harness) must never panic. Errors are
      graceful; panics are bugs.
- [x] Fuzz target `fuzz/fuzz_targets/spawn_recv.rs`: `spawn [body] recv
      r` for arbitrary `body` (lossy UTF-8 source) must never panic and
      must terminate within 10s. Catches worker-thread panics,
      marshalling panics, and infinite loops in the worker (the 10s
      timeout is enforced via `spawn_thread`'s `JoinHandle` +
      `join_timeout`).
- [x] Add `join_timeout` helper to `spawn_thread`'s return type (or a
      standalone `join_with_timeout(handle, dur) -> Result<T, Timeout>`)
      — needed by the fuzz target. Use `crossbeam::thread::scope` or a
      `channel`-based timeout (the std `JoinHandle::join` is blocking
      with no timeout). Document the workaround in `concurrency.rs`.
- [x] `cargo test --workspace` passes; `cargo +nightly fuzz run spawn_recv
      -- -runs=1000` runs without panics.

## Milestone 45 — Cooperative actor library (v0.6.1)

Actors are `Object`s with a `mailbox:` `Channel` and a `handler` `Func`,
driven by a single scheduler thread. **Not** OS threads — an actor's
handler runs on the scheduler, cooperatively yielding via `receive`. Caps
at ~10⁷ concurrent actors (heap-bound, no parallelism between actors).
Actors may themselves `spawn` worker threads for heavy compute.

This is the Lua/Python model: actors for *structure*, threads (M43) for
*parallelism*. The M:N scheduler (v0.7) is where actors gain real
parallelism.

- [x] Define the actor convention (not a new `Value` variant — actors are
      plain `Object`s with a documented shape):
      ```red
      actor: make object! [
          mailbox: channel
          handler: func [msg] [ ... ]
          alive?: true
      ]
      ```
      The scheduler walks a ready-queue of `(actor, msg)` pairs and calls
      `actor/handler msg`. `alive?` is a convention; `close actor/mailbox`
      stops the scheduler from dispatching to it.
- [x] Implement `spawn-actor` native (arity 1: `spawn-actor [handler-func]`):
      - Creates an `Object` with `mailbox: channel`, `handler: <the arg>`,
        `alive?: true`.
      - Pushes the actor onto `Env::actor_ready_queue` (new field:
        `Vec<Rc<RefCell<ObjectDef>>>`).
      - Returns the `Object` value.
      - Does **not** spawn an OS thread; the scheduler (a single thread)
        runs all actors.
- [x] Implement `send-actor` native (arity 2: `send-actor actor msg`):
      - Equivalent to `send actor/mailbox msg`. Provided as sugar so user
        code doesn't need to know the `mailbox` field name.
      - After sending, pushes the actor onto the ready-queue (if not
        already enqueued — track via a `HashSet<Rc::as_ptr>` to avoid
        duplicate dispatch).
      - **`msg` may itself be an `Object` or actor value** — because `Object`
        is marshalable across the Send boundary (M40), an actor can be sent
        as a message value (e.g., for link/supervisor patterns in M47),
        resolving the original "referenced by name" hand-wave. Name-based
        reference is unnecessary when the actor value itself crosses cleanly.
- [x] Implement `receive` native (arity 1: `receive block`):
      - Used inside an actor's handler to block waiting for the next
        message. `block` is a `Block` of `case`-style clauses:
        ```red
        receive [
            1 [print "got one"]
            2 [print "got two"]
            default [print "other"]
        ]
        ```
      - The native pops one message from the actor's mailbox (via `recv`
        on the mailbox channel), matches it against the clauses (reusing
        the `switch` machinery), and runs the matching block. If no clause
        matches, the `default` clause runs (or the actor errors if there's
        no `default`).
      - **Yields to the scheduler** after running the matching block (the
        handler returns; the scheduler moves to the next ready actor).
        This is the cooperative-yield point — actors must call `receive`
        or return to give up control.
      - v0.6.1's `receive` is **blocking within an actor** but **non-
        blocking across actors**: if an actor's mailbox is empty, the
        scheduler parks that actor and moves to the next one.
- [x] Implement `run-actors` native (arity 0):
      - The scheduler loop: drain `Env::actor_ready_queue`, for each ready
        actor run its pending message (or resume from its last `receive`
        yield point — see M46 for the resume mechanism). Loop until the
        ready-queue is empty AND all mailboxes are empty.
      - Returns `Value::None` when all actors are parked or done.
      - This is the entry point: a script calls `run-actors` after
        spawning actors and sending initial messages. The scheduler runs
        until quiescence.
- [x] Add `Env::actor_ready_queue: Vec<Rc<RefCell<ObjectDef>>>` and
      `Env::actor_park_set: HashSet<usize>` (keyed by `Rc::as_ptr`).
- [x] Add `examples/actor_counter.red` — a counter actor that increments
      on each message and replies with the current count. Demonstrates
      the actor pattern: `spawn-actor`, `send-actor`, `receive`, reply
      via a per-actor reply channel.
- [x] Add `examples/actor_supervisor.red` — a supervisor actor that
      spawns child actors and restarts them on failure. Demonstrates the
      actor-link pattern (M47's link/monitor builds on this).
- [x] Inline `#[test]`: `a: spawn-actor [func [msg][print msg]] send-actor
      a "hi" run-actors` prints "hi".
- [x] Inline `#[test]`: 1000 actors each receiving one message complete
      in under 1s (asserts the cooperative model's overhead is low — no
      OS thread per actor).
- [x] Inline `#[test]`: an actor that calls `spawn` (M43) for heavy
      compute works correctly (actors can spawn threads for parallelism;
      the cooperative scheduler only governs actor dispatch, not worker
      threads).
- [x] `cargo test --workspace` passes.

## Milestone 46 — Actor resume + park semantics

The actor scheduler must handle long-running handlers that call `receive`
multiple times. v0.6.1's approach: handlers are *single-message* — each
`send-actor` enqueues one message, and the scheduler runs the handler once
for that message, then parks the actor until the next `send-actor`.

This is simpler than Erlang's `receive`-in-loop model but less expressive.
M46 explores whether to add multi-message handlers (continuation-passing)
or stick with single-message + explicit loops.

- [x] Decide: single-message handlers (simple, M45 default) vs. multi-
      message handlers with continuations (Erlang-style, more expressive).
      **Recommendation:** single-message for v0.6.1; multi-message is a
      v0.7 candidate alongside the M:N scheduler (which needs continuation
      support anyway for work-stealing).
- [x] If single-message: document the convention — an actor's handler
      runs once per `send-actor`, processes one message, and returns.
      Long-lived actors loop via `send-actor self msg` (the actor can
      send to itself to continue).
- [x] If multi-message: implement continuations via a `yield` native that
      parks the actor with a closure to resume later. Requires a closure
      type (`closure!`, deferred per `plan5.md`) — so this path depends
      on v0.4 landing closures first.
- [x] Inline `#[test]`: a single-message counter actor
      (`spawn-actor [func [msg][count: count + 1]]`) processes 1000
      messages correctly (each `send-actor` enqueues one dispatch).
- [x] `cargo test --workspace` passes.

## Milestone 47 — Actor links + supervisors (optional)

Erlang-style `link`/`monitor` so actors can react to each other's failure.
**Optional for v0.6.1** — defer to v0.7 if the M:N scheduler would
reimplement this anyway.

- [x] Implement `link actor1 actor2` — links two actors; if one dies
      (handler errors or `alive?` set to false), the other receives a
      `:EXIT` message.
- [x] Implement `monitor actor` — one-way link; the monitor receives
      `:DOWN` messages without linking back.
- [x] Implement `spawn-supervisor` — a supervisor actor that restarts
      linked children on `:EXIT`. Built on `link` + `spawn-actor`.
- [x] Add `examples/actor_supervisor.red` — extends M45's example with
      real `link`/`monitor` calls.
- [x] Inline `#[test]`: a linked actor pair propagates `:EXIT` correctly.
- [x] `cargo test --workspace` passes.

## v0.7 candidate — M:N scheduler (work-stealing)

**Out of scope for v0.6.** Documented here as the v0.7 candidate so the
v0.6 design doesn't paint us into a corner.

The v0.7 M:N scheduler maps M actors (or lightweight processes) onto N OS
threads with work-stealing (Go/Erlang model). Caps at ~10⁶ actors with
real parallelism between them. Requires:

- **Continuations** (or closures) — actors must be resumable across
  thread boundaries, which means the handler state must be `Send`. This
  is the v0.4 `closure!` dependency surfacing.
- **Work-stealing deque** per worker thread — `crossbeam-deque` is the
  standard Rust choice.
- **`select` native** — non-blocking multi-channel receive, needed for
  the scheduler's work-stealing wakeup. Adds `recv/select` refinement
  and `recv/timeout`.
- **Per-thread `ThreadEnv` pool** — N `ThreadEnv`s (one per OS worker),
  not one per actor. Actors migrate between `ThreadEnv`s.
- **Channel overhaul** — channels become `crossbeam-channel` (multi-
  producer, multi-consumer, `select`-able). The std `mpsc` is single-
  consumer; M:N needs multi-consumer.
- **Actor GC** — parked actors with empty mailboxes and no references
  should be collectable. The cooperative model (v0.6.1) avoids this by
  running to quiescence; M:N with long-lived parked actors needs explicit
  reclamation.

### v0.7 milestone sketch (not a commitment)

- M50 `crossbeam-channel` migration (multi-consumer channels, `select`)
- M51 Continuation/closure support (depends on v0.4 `closure!`)
- M52 Per-thread `ThreadEnv` pool + work-stealing deque
- M53 `select` native (`recv/select [c1 c2 c3]` → first ready)
- M54 Actor migration (move an actor's ready-queue entry to another thread)
- M55 `recv/timeout` refinement
- M56 Actor GC (drop parked actors with no references)

## Open questions

1. **Shared stdout policy** — lock-per-print (correct, simple, slow under
   heavy actor print load) vs per-thread buffering + flush-on-yield (fast,
   surprising ordering). **Recommendation:** lock-per-print for v0.6
   (Erlang's default); revisit if benchmarks show contention. The `Mutex`
   is uncontended in the common case (one print per actor message).
2. **`user_ctx` fork semantics** — full frozen copy at spawn (Erlang-style;
   globals propagate, mutations don't) vs fresh empty context (Go-style;
   workers start with a blank slate). **Recommendation:** full frozen copy
   — matches Red's "scripts see their globals" expectation and is simpler
   to reason about. `Context::deep_clone` is O(slots) and runs once per
   spawn; cheap relative to thread creation.
3. **Channel buffering** — start unbounded (std `mpsc` default) vs add
   bounded (`channel/buffered n`) from day one. **Recommendation:**
   unbounded for v0.6; bounded is a v0.7 candidate (the M:N scheduler
   needs bounded channels for backpressure).
4. **`select` / `recv` with timeout** — defer to v0.7 (M:N scheduler
   needs them internally) or include in v0.6.0. **Recommendation:** defer.
   v0.6's blocking `recv` is sufficient for the foundation; `select` adds
   API surface and `crossbeam-channel` as a dependency.
5. **Cargo feature gate** — always-on (additive, no v0.3 regression risk)
   vs `--features threads`. **Recommendation:** always-on. The new code is
   purely additive (new `Value` variant, new natives); existing scripts
   don't use channels and pay no cost. A feature gate would fragment the
   test matrix without benefit.
6. **Func across threads** — reject (Erlang-style; actor handlers must be
    referenced by name, not shipped as values) vs allow (Go-style; closures
    are `Send` if their captures are). **Recommendation:** reject for v0.6.
    Lifting the restriction requires `closure!` (v0.4) and `Send` captures
    — a v0.7 candidate. Workers reference funcs by *name* (resolved via
    their `ThreadEnv`'s `user_ctx` snapshot), not by value. (Note: `Object`
    was originally lumped into this rejection and has been lifted — see the
    Send-boundary section. `Object` carries no closure-capture semantics,
    so the Erlang analogy doesn't apply; only `Func` remains restricted.)
7. **Actor scheduler thread** — dedicated scheduler thread (separate from
   the main thread) vs run on the main thread (calling `run-actors` blocks
   until quiescence). **Recommendation:** main thread for v0.6.1 (simpler;
   matches the "scripts call `run-actors`" model). A dedicated scheduler
   thread is a v0.7 candidate (enables `send-actor` from worker threads,
   which needs `Send` actors — a deeper change).
8. **`spawn-actor` vs `spawn`** — should actors be a refinement of
   `spawn` (`spawn/actor [handler]`) or a separate native? **Recommendation:**
   separate native (`spawn-actor`) for clarity; the implementations share
   no code (one spawns an OS thread, the other pushes onto a ready-queue).
