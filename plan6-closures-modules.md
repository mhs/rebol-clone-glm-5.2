# Plan 8: Closures & Modules (v0.5)

Execution checklist extending the v0.4.0 baseline in `plan5.md`. v0.5 lands the
two features `README.md:294–296` calls out as the "headline v0.5 candidates":
**first-class closures** (`closure!`) and **modules** (`module`/`import`/
`export`). GUI/draw/vid remain permanently out of scope; reactivity (plan6) and
concurrency (plan7) are *not* prerequisites and are not built here — v0.5 is a
**language-organization release**.

Per `project-brief.md`, every new construct is additive through the existing VM
`Const`-pool + `Call`/`CallUser`/`MakeFunc` path; the v0.3.3 VM stays the
default evaluator. The parity harness (`tests/parity.rs`) and
`cargo test --workspace --features force-walk` remain the regression gates.

Deferred to v0.6+ (acknowledged, not built here): reactivity, concurrency,
`tag!`/`ref!`/`image!`/`vector!`/`hash!`/`regex!`, `routine!` FFI,
`compose`-on-closure, named timezones, full port model, distributed modules,
shared-cell closures, `unimport`. v0.5 deliberately ships the two
smallest-but-highest-leverage items so real programs can organize code.

## Design summary

Three themes, in priority order:

1. **Closures with real free-variable capture** — close the v0.3 gap
   (plan3:443–449): the VM currently captures freevars by frame-chain walking
   (`LoadLocal(d, slot)`), which is **wrong** the moment a `func` value
   escapes its defining frame (returned, stored in an object, passed as an arg
   to a later-called native). v0.5 adds a `Value::Closure(Rc<ClosureDef>)`
   variant that captures freevar *values* into an owned `Vec<RefCell<Value>>`
   cell (snapshot at creation time — see open-q #1), and a new
   `Instr::MakeClosure` that builds the cell at `closure`-creation time by
   reading the defining frame's locals. `closure`/`does`-with-capture work like
   `func` but produce a `Closure` value; `func`/`function` keep their
   shallow-copy semantics (back-compat with v0.2–v0.4 golden fixtures).
2. **Module system** — a `module!` value owning its own `Context` (the
   module's namespace), a set of exported words, and an optional parent (the
   script `user_ctx` or another module, for lexical chaining). `module [body]`
   evaluates `body` in the module's context; `export 'word` (or `export
   [words]`) marks words for public visibility; `import %path.red` or `import
   'module-name` loads/registers a module and imports its exports into the
   *current* context. Modules are singletons (cached by canonical path on
   `Env`); re-`import` returns the cached value.
3. **Visibility & path resolution** — `module/word` path resolves into the
   module's context (mirrors `object/field`); unbound words at eval time
   resolve through a single `user_ctx.get(sym)` dynamic fallback before
   erroring (the one behavior change — see open-q #2). A `private`/`public`
   distinction is enforced at `import` time and on `module/word` paths from
   outside; inside the module body, all words are visible.

Non-goals: a separate `closure!` lexer token (closures are constructed by the
`closure` native, not by a literal), a new `Instr` for module import (modules
are constructed and imported via natives — the `MakeFunc`/`Call` machinery is
sufficient), reactivity, or behavior changes to existing v0.2/v0.3/v0.4
features beyond the documented `resolve_word` fallback. Existing
`func`/`does`/`function` semantics are unchanged — closure capture is opt-in
via `closure`.

## Ground-truth references (from research)

- `FuncDef.freevars: Vec<Symbol>` exists (`crates/red-core/src/value.rs:155`)
  but holds *names* only; capture is via frame-chain walking
  (`crates/red-eval/src/vm/vm.rs:392` `LoadLocal`).
- `FuncDef.compiled: Option<Rc<CompiledBlock>>` (`value.rs:161`) is a hint;
  authoritative cache is `Env::func_cache` (`env.rs:173`).
- `FuncDef.ctx: Context` (`value.rs:163`) is per-func, shallow-cloned per call
  by the walker (`binding.rs:19–21`).
- `Binding` has no closure variant (`value.rs:97–103`); `Lexical(d, slot)`
  walks ancestor frames only.
- `Context` has no parent link, no visibility field (`context.rs:24–27`).
  `ObjectDef` adds a retained `parent` but inheritance is copy-based
  (`value.rs:415–419`).
- `Env` has no module registry, no per-script-context map, no `observing`
  (`env.rs:142–235`). Each `run_series_inner_opts` builds a fresh `Context`/
  `Env` (`interp_runner.rs:132–180`).
- `mold`/`form` of `Value::Func` is the literal placeholder `"#[function]"`
  (`printer.rs:86`, `:227`) — no closure scaffolding exists in `Value` or
  `FuncDef`.
- `bind` accepts only a `word!` operand (`natives/words.rs:673–734`); `in`
  accepts only an `object!` (`object.rs:301–340`); `use` clones `user_ctx`
  into a child and swaps `env.user_ctx` temporarily (`words.rs:603–661`).
- No `module`/`import`/`export` stubs exist anywhere; only doc mentions
  (`README.md:296`, `architecture.md:1026`, `binding.rs:19–27`).
- `system` is an `object!` in `user_ctx["system"]` (`registry.rs:386–404`)
  — no module-path field exists.
- `make object!` swaps `env.user_ctx` to the object's ctx during spec eval
  (`object.rs:104–117`) — the pattern `module` will follow.

---

## Milestone 60 — `closure!` value + capture cells ✅ LANDED

The foundational milestone. Adds the `Value::Closure` variant, the
`ClosureDef` struct, the `Instr::MakeClosure` instruction, the `closure`
native, and the VM/walker call path. **No module work yet** — closures stand
alone. The freevar *capture* fix lands here (the v0.3 escaping-closure bug).

### Files

- [x] **Edit: `crates/red-core/src/value.rs`** — add `Value::Closure(Rc<ClosureDef>)`
  variant after `Value::Func` (value.rs:254). Define `ClosureDef`:
  ```rust
  pub struct ClosureDef {
      pub func: FuncDef,                 // the underlying FuncDef (spec/body/ctx/etc.)
      pub captures: Vec<RefCell<Value>>,  // freevar values, indexed by `freevars` order
  }
  ```
  Add `Value::closure(func, captures)` constructor. Derive `Debug` for
  `ClosureDef` (delegates to `FuncDef`).
- [x] **Edit: `crates/red-core/src/printer.rs`** — `mold`/`form` of
  `Value::Closure(_)` emits `#[closure]` placeholder (matches the
  `#[function]` style; no spec/body molding — POC parity with `Func`).
  Inline test: `mold(closure) == "#[closure]"`.
- [x] **Edit: `crates/red-core/src/value.rs`** — extend `Binding` with
  `Closure(usize)` variant (the index into `ClosureDef::captures`). Update
  `is_lexical`/`as_lexical` helpers to return false for `Closure`. Update
  every exhaustive `match binding` site (the M22 audit cataloged them:
  `interp_walker.rs` `resolve_word`/`write_setword`, `natives/words.rs`
  `get`/`set_one`, `object.rs` `try_resolve_object`, `vm/lex.rs`
  `attach_lexical`) to add a `Binding::Closure(idx)` arm. Walker arm: read
  from the closure's capture cell via a new `Env::closure_captures:
  Vec<Vec<Value>>` stack (pushed on closure call, popped on return). VM arm:
  `Instr::LoadCapture(idx)` — see below.
- [x] **Edit: `crates/red-core/src/vm_ir.rs`** — add
  `Instr::MakeClosure(spec_idx, body_idx, fv_idx)` (mirrors `MakeFunc`) and
  `Instr::LoadCapture(u32)` / `Instr::SetCapture(u32)` (read/write the current
  frame's capture cell). Add `Frame::captures:
  Option<Rc<Vec<RefCell<Value>>>>` field (None for plain funcs, Some for
  closures) — sized at frame push from the `FuncDef` if it's a closure.
- [x] **Edit: `crates/red-eval/src/binding.rs`** — add
  `bind_closure_body(&mut fd, &env.user_ctx, captures: &[Value])` analogous
  to `bind_function_body` but: (a) for each `sym` in `fd.freevars`, the body
  word's `Binding` becomes `Closure(idx)` (new `Binding` variant) instead of
  `Lexical(d, slot)`; (b) the closure's own `FuncDef.ctx` is seeded with the
  captured values so the walker can read them. **Update the doc at
  binding.rs:19–21** to remove the "closures explicitly out of scope" line.
- [x] **Edit: `crates/red-eval/src/vm/lex.rs`** — in `attach_lexical`, when a
  word resolves to a *closure* scope (a new `Scope::is_closure` flag set by
  the `closure` native's analyzer arm), emit `Binding::Closure(idx)` instead
  of `Binding::Lexical(d, slot)`. The freevar *names* still propagate up via
  `AnalysisResult.freevars` as today.
- [x] **Edit: `crates/red-eval/src/vm/compiler.rs`** — in
  `compile_word`/`compile_setword`, when `Binding::Closure(idx)` is found,
  emit `LoadCapture(idx)`/`SetCapture(idx)` instead of `LoadLocal`/
  `SetLocal`. In the `func`/`does`/`function` detection path, add a parallel
  `closure` form that emits `MakeClosure` (the analyzer already computes
  freevars; reuse `analyze_func_form`).
- [x] **Edit: `crates/red-eval/src/vm/vm.rs`** — implement `Instr::MakeClosure`
  (analogous to `MakeFunc` but reads the freevar *values* from the current
  frame's `captures`/locals at construction time and stores them into the new
  `ClosureDef.captures`), `Instr::LoadCapture`/`SetCapture` (read/write
  `self.frames.last().captures.as_ref().unwrap()[idx]`). Update `call_user`/
  `prepare_call`: when the resolved `Value` is `Value::Closure(cd)`, push a
  `Frame` with `captures = Some(Rc::clone(&cd.captures))` and
  `func = Some(Rc::clone(&cd.func))`. `ensure_compiled` for a closure
  compiles `cd.func.body` (the body's freevar words now have
  `Binding::Closure(idx)` so the compiler emits `LoadCapture`).
- [x] **Edit: `crates/red-eval/src/interp_walker.rs`** — `eval_prefix` arm
  for `Value::Closure(_)`: invoke the closure's `FuncDef` body with a
  `CallFrame` whose `ctx` is seeded from the captures (clone each capture
  into a fresh slot under the freevar's name), so `resolve_word`'s
  `Binding::Closure(idx)` arm can read `env.closure_captures.last()[idx]` OR
  (simpler) the `Binding::Local(fd.ctx, slot)` arm reads the seeded ctx slot.
  **Decision: the latter** — seed `fd.ctx` with capture values at call time
  (matches the existing shallow-copy pattern), so the walker needs no new
  `Env` field; only the VM uses `Frame::captures` directly.
- [x] **Edit: `crates/red-eval/src/natives/func.rs`** — add `closure_native`
  (arity 2: spec block, body block) and `does_closure_native` (arity 1: body
  block, zero-arg closure). Both build a `FuncDef`, compute freevars via
  `analyze_block`, read the freevar *values* from the current scope (walker:
  `env.user_ctx`/`call_stack.last().ctx`; VM: the current frame's locals —
  the native handler runs with `&mut Env` so it can read `env.call_stack`),
  build a `ClosureDef`, return `Value::Closure(Rc::new(cd))`. Register in
  `natives/registry.rs` alongside `func`/`does`.
- [x] **Edit: `crates/red-eval/src/natives/registry.rs`** — register
  `closure`, `does` (extended to detect closure? No — add `closure` as a
  distinct word; `does` keeps its `func`-style semantics for back-compat).
  Add `closure?` predicate.

### Natives

- [x] `closure [spec] [body]` — like `func` but captures freevar values into
      a `ClosureDef`. Returns `Value::Closure`.
- [x] `closure [] [body]` — zero-arg closure (explicit empty spec; covers the
      `does`-equivalent case). No separate `does-closure` word.
- [x] `closure?` predicate — true on `Value::Closure`.
- [x] `function?` extended to return true on `Value::Closure` too (a closure
      is a function). `closure?`/`function?` are in subset relation.

### Capture semantics (matches upstream Red, with documented deviation)

- **Capture is by value at creation time (snapshot).** `ClosureDef.captures:
  Vec<RefCell<Value>>` — closing over `x` copies `x`'s value into the cell at
  `MakeClosure` time. An outer write *after* closure creation does NOT
  propagate inward; an inner write does NOT propagate outward. The
  `RefCell<Value>` per capture permits interior mutability across multiple
  *invocations of the same closure* (a closure that does `count: count + 1`
  sees its own prior write on the next call), but two separate `closure`
  forms closing over the same outer `x` get *independent* cells.
- **Deviation from Red:** real Red `closure!` shares the cell across multiple
  closures closing over the same variable, and across outer/inner (inner
  writes propagate outward). Shared cells require heap-promoting the outer
  variable (the `SetWord` would write to a shared `Rc<RefCell<Value>>`
  instead of a context slot) — a deeper change touching `bind_pass` and
  `Context::set`. v0.5 ships snapshot semantics (simpler, correct-for-most-
  cases, fixes the v0.3 escaping bug); shared-cell is documented as a v0.6
  candidate. See open-q #1.
- **Freevar order:** `FuncDef.freevars: Vec<Symbol>` (already populated by the
  analyzer); `captures` is indexed in the same order. `Binding::Closure(idx)`
  uses that index.
- **Recursion:** a closure's body referencing its own name resolves via the
  outer slot (the `SetWord` binding), not via a capture — matches `func`
  (`lex.rs:759` `recursive_self_reference_is_global_not_freevar`).

### Golden fixtures

- [x] `closure_basic` — `f: closure [x][x + y] y: 10 f 5` → `15`; then
      `y: 99 f 5` → still `15` (capture is by value at creation time).
- [x] `closure_escape` — `make-adder: func [n][closure [x][x + n]] add5:
      make-adder 5 add5 10` → `15` (the closure escapes its defining frame;
      the v0.3 frame-chain-walking bug would have returned wrong values or
      panicked here).
- [x] `closure_internal_mutation` — `c: closure [][count: 0]` then a wrapper
      that calls `c` twice with `count: count + 1` in the body — verify the
      closure's own `count` slot persists across invocations (the
      `RefCell<Value>` cell).
- [x] `closure_local_shadow` — `c: closure [][acc: 0 acc: 10]` (the closure
      has its own `acc` local; outer `acc` is not the capture). Confirm
      closure-local `acc` is independent.
- [x] `closure_in_object` — `o: object [base: 100 adder: closure [x][x + base]]
      o/adder 5` → `105` (closure captures object field at `closure`-eval
      time — `base` is set before `adder` in the spec, so it's in scope).
      Include the *failing* form `o: object [adder: closure [x][x + base]
      base: 100]` as an error fixture (base unbound at closure creation time).
- [x] `closure_recursive` — `fact: closure [n][either n <= 1 [1] [n * fact
      n - 1]] fact 5` → `120` (recursion via the outer `fact` slot, not via
      capture).

### Tests

- [x] Inline `#[test]`: `closure` returns a `Value::Closure`.
- [x] Inline `#[test]`: `closure? closure [] []` → true; `closure? func [] []`
      → false.
- [x] Inline `#[test]`: `function? closure [] []` → true.
- [x] Inline `#[test]`: escaping closure returns the captured value, not a
      stale frame read (the v0.3 bug regression test).
- [x] Inline `#[test]`: capture is by-value (outer write after creation
      doesn't propagate).
- [x] Inline `#[test]`: closure internal mutation persists across
      invocations (the `RefCell` cell).
- [x] Inline `#[test]`: closure in object spec captures the field's value at
      `closure`-eval time.
- [x] Inline `#[test]`: recursive closure works via the outer slot.
- [x] `cargo test --workspace` green; `--features force-walk` green.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [x] Update `crates/red-core/tests/property.rs` to include `Closure` in
      round-trip (**skip** — `#[closure]` is a placeholder, not reparseable;
      add a separate test asserting `mold(closure)` is the stable string
      `#[closure]`).

---

## Milestone 61 — `module!` value + `module` native ✅ LANDED

Adds the `Value::Module` variant, `ModuleDef` struct, the `module`/`export`
natives, and the module-context machinery. No `import` yet (M62).

### Files

- [x] **Edit: `crates/red-core/src/value.rs`** — add
  `Value::Module(Rc<RefCell<ModuleDef>>)` variant (synthetic, no span — like
  `Object`). Define:
  ```rust
  pub struct ModuleDef {
      pub ctx: Rc<Context>,                  // the module's namespace
      pub exports: RefCell<HashSet<Symbol>>, // words marked `export`
      pub name: Option<Symbol>,              // for named modules (`module 'foo [...]`)
      pub source: Option<Rc<str>>,           // canonical path for caching (M62)
      pub parent: Option<Rc<Context>>,       // script user_ctx or another module, for lexical chaining
  }
  ```
  Add `Value::module(ctx, exports, name)` constructor. Derive `Debug`.
  *(Deviation: the constructor takes a `ModuleDef` rather than the
  individual fields — matches `Value::object`'s `ObjectDef`-taking shape.
  `HashSet` import added to value.rs.)*
- [x] **Edit: `crates/red-core/src/printer.rs`** — `mold`/`form` of
  `Value::Module(_)` emits `make module! [name: <name> exports: [words] ...]`
  (a reparseable form — `make module!` is a real constructor, and molding a
  module round-trips through `make module!` + the exported-word list. Inline
  test: `mold(m) == "make module! [...]"`.) Follow the `make map!` template
  (`printer.rs` `Map` arm).
- [x] **New: `crates/red-eval/src/module.rs`** *(top-level, matching
  `object.rs`/`map.rs`/`bitset.rs` rather than `natives/` per the plan text —
  the closest structural analog is `object.rs`, which `module` mirrors for
  the `env.user_ctx` swap; plan6's `natives/module.rs` reference was a
  plan-vs-tree discrepancy; chose top-level for consistency with the object
  machinery it reuses).* —
  - [x] `module_native` — arity 1–2. Form 1: `module [body]` (anonymous).
        Form 2: `module 'name [body]` (named). Form 3: `module/cached 'name
        [body]` (named + cache by name). Builds a fresh `Context`, **swaps
        `env.user_ctx` to it** (mirrors `make_object` at object.rs:104), runs
        `bind_pass_into` + `eval` of the body, **restores `env.user_ctx`**,
        collects exported words from a per-module `exports` RefCell, returns
        `Value::Module`. **`export` inside the body** is a native that writes
        to `env.current_module().exports` — see below.
        *(Form 3 `/cached` refinement deferred — only Forms 1–2 land in M61;
        caching for Form 2 named modules IS implemented via `env.modules`.)
        Variable-arity dispatch: `module` is registered as arity 1 with a
        variable-arity peek in both collectors (walker `collect_call_args` +
        VM compiler `collect_args`) that gathers 2 args when the next value
        is a Word-family (named form) and 1 when it's a Block (anonymous).
        `module` added to `uneval_first` so the name arg is pushed as-is.
        Avoids the variadic collector's over-collection problem.)*
  - [x] `export_native` — arity 1. `export 'word` adds `word` to the current
        module's `exports` set (the current module is tracked on
        `Env::module_stack: Vec<Rc<RefCell<ModuleDef>>>` — see below).
        `export [w1 w2 ...]` adds each. Outside a module body, `export` errors
        (`EvalError::Native "export used outside module"`).
  - [x] `module?` predicate.
  - [x] `make_module` (`make module! [...]`) — the mold inverse; interprets
        `name:`/`exports:` keyword pairs in the spec to pre-populate the
        module's name/exports, then evaluates the remaining spec items as the
        body. Added beyond plan6's explicit list because the mold form
        `make module! [...]` must be evaluable for `do load mold m` to
        reconstruct.
- [x] **Edit: `crates/red-core/src/env.rs`** — add
  `pub module_stack: Vec<Rc<RefCell<ModuleDef>>>` (default empty).
  `Env::current_module() -> Option<&Rc<RefCell<ModuleDef>>>` returns
  `module_stack.last()`. Add
  `pub modules: HashMap<Symbol, Rc<RefCell<ModuleDef>>>` for named-module
  caching (M62 `import` consults this; M61 populates it for `module 'name`
  forms).
- [x] **Edit: `crates/red-eval/src/natives/registry.rs`** —
  `pub mod module;`, call `module::register_module_natives(&mut env)`
  alongside the other `register_*` calls. *(Wired via `lib.rs` `pub mod
  module;` + `crate::module::register_module_natives(env)` in registry.rs
  alongside `crate::object::register_object_natives`.)*

### Additional edits beyond plan6's file list (required for exhaustiveness)

- [x] **Edit: `crates/red-eval/src/natives/mod.rs`** — `type_name` gains a
      `Value::Module(_) => "module!"` arm (otherwise non-exhaustive).
- [x] **Edit: `crates/red-eval/src/object.rs`** — `words-of`/`values-of`/
      `reflect` extended with `Value::Module` arms (exports only, `ctx`
      insertion order — iterate `ctx.words()` and filter by `exports`, never
      iterate the unordered `HashSet`).
- [x] **Edit: `crates/red-eval/src/convert.rs`** — `make` native gains a
      `"module!" | "module"` arm dispatching to `module::make_module`.
- [x] **Edit: `crates/red-eval/src/interp_walker.rs`** — `Value::Module`
      arms in `eval_prefix` (data-return), `eval_path_call` (new
      `select_module_path` helper with method-call semantics), `eval_get_path`
      (new `get_module_path` helper), `write_path_slot` (export-restricted
      set-path); plus `select_module_field`/`inside_module_body` helpers
      enforcing export visibility (private → `UnboundWord` from outside;
      unrestricted inside the module body via `Rc::ptr_eq` against
      `env.module_stack` top). `module` variable-arity peek +
      `uneval_first` addition in `collect_call_args`.
- [x] **Edit: `crates/red-eval/src/vm/compiler.rs`** — `Value::Module` arm
      in the const-fold match; `module` variable-arity peek +
      `uneval_first` addition in `collect_args` (mirrors the walker).
- [x] **Edit: `crates/red-core/src/lib.rs`** — re-export `ModuleDef` (and
      `ClosureDef`, which M60 had left un-re-exported).

### Natives

- [x] `module [body]` — anonymous module; returns `Value::Module`. Body
      evaluated in the module's context.
- [x] `module 'name [body]` — named module; cached in `env.modules[name]`.
      Re-evaluating `module 'name [different body]` returns the *cached*
      module (the body is ignored — matches Red's "module is a singleton by
      name"). Document.
- [x] `export 'word` / `export [words]` — marks words for public visibility.
      Only valid inside a `module` body.
- [x] `module?` predicate.
- [x] `words-of`/`values-of` extended to accept `module!` — returns only
      *exported* words/values (the public surface). `reflect module 'words` /
      `'values` same. **Inside the module body**, all words are visible;
      `words-of` from outside returns exports only.

### Visibility rules

- **Inside the module body:** all words in the module's `Context` are visible
  (private + public). `export` is a side-effect declaration that adds to the
  `exports` set; it doesn't restrict inner access.
- **Outside the module:** `module/word` path resolves only into `exports`.
  `m/private-word` from outside → `UnboundWord` error (or a `Native` error
  "private word" — decision: `UnboundWord` for consistency with `in_native`'s
  absent-word behavior at object.rs:334–339). *(Implemented: `UnboundWord`,
  matching plan6's decision note.)*
- **Bare-word `import`-aliased access:** deferred to M62.

### Golden fixtures

- [x] `module_basic` — `m: module [a: 1 b: 2 export 'a] print [m/a words-of m]`
      → `1 [a]`. *(Fixture adjusted: `print [m/a words-of m]` would print the
      literal block since `print` molds rather than reduces; the fixture
      prints the two values separately to match plan6's intent —
      `print m/a print words-of m` → `1\n[a]`.)*
- [x] `module_named` — `m: module 'utils [helper: func [x][x * 2] export
      'helper] print m/helper 5` → `10`. (Named modules are reachable via the
      returned `Value::Module` only until M62.)
- [x] `module_private` — `m: module [priv: 42 pub: priv export 'pub] print
      m/pub` → `42`; `print m/priv` → `*** Error: ... UnboundWord: priv`.
      *(Split: the `m/pub` success case is covered by
      `module_words_of_exports_only`; the `m/priv` error is the
      `programs_errors/module_private` fixture.)*
- [x] `module_export_block` — `module [a: 1 b: 2 c: 3 export [a c]] words-of
      m` → `[a c]`.
- [x] `module_singleton` — `m1: module 'once [x: 1] m2: module 'once [x: 999]
      print m2/x` → `1` (cached; second body ignored). *(Fixture adds
      `export 'x` to the body so `m2/x` is reachable from outside per the
      visibility rule — plan6's original fixture accessed an unexported `x`
      from outside, which contradicts its own visibility rule; the fixture
      exports `x` to make the caching demonstration consistent.)*
- [x] `module_words_of_exports_only` — `m: module [priv: 1 pub: 2 export
      'pub] words-of m` → `[pub]` (not `[priv pub]`).
- [x] `module_mold_roundtrip` — `m: module [a: 1 export 'a] load mold m` →
      reparseable. *(Fixture is `module_mold.red` using `probe m` since `mold`
      isn't a script-level native — `probe` molds via the printer internally.
      Inline `module_mold_load_roundtrips` test covers the `load mold m`
      parse-round-trip directly.)*

### Tests

- [x] Inline `#[test]`: `module` returns `Value::Module`.
- [x] Inline `#[test]`: `module? module []` → true; `module? object []` →
      false.
- [x] Inline `#[test]`: `export` outside module errors.
- [x] Inline `#[test]`: `words-of` returns exports only.
- [x] Inline `#[test]`: private word path from outside errors.
- [x] Inline `#[test]`: named module is cached by name.
- [x] `cargo test --workspace` green; `--features force-walk` green.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` clean *(added
      beyond plan6's M61 test list, matching M60's bar)*.
- [x] `cargo fmt --all --check` clean *(added beyond plan6's M61 test list,
      matching M65's bar)*.

---

## Milestone 62 — `import` + bare-word resolution ✅ LANDED

Adds `import` (from file or named module), the imported-alias mechanism, and
the path-resolution rules for modules. Closes the module usability loop.

### The `resolve_word` behavior change (open-q #2)

The POC's `resolve_word` `Unbound` arm currently errors immediately. After
M62, it does a single `user_ctx.get(sym)` lookup first. This is the *one*
behavior change in v0.5, and it's required to make `import` work without AST
re-walking:

- `bind_pass` runs at script-load time (before any eval). `import 'm` runs
  mid-eval. So a bare `foo` in the script body that resolves to `m`'s
  exported `foo` is `Unbound` at `bind_pass` time (nothing in `user_ctx`
  yet). At eval time, `import` has written `foo`'s value into `user_ctx`.
  The unbound word needs to look it up.
- In VM mode: `LoadDynamic(foo)` already does `env.user_ctx.get(foo)` (vm.rs
  `LoadDynamic` arm). **No VM change needed** — the compiler already emits
  `LoadDynamic` for `Unbound` words, and the runtime already consults
  `user_ctx`.
- In walker mode: `resolve_word` `Unbound` arm currently errors. Change to
  `env.user_ctx.get(sym).ok_or_else(|| EvalError::UnboundWord { ... })`.
- **Parity:** the change brings the walker in line with the VM's existing
  `LoadDynamic` behavior. Document the caveat in `project-brief.md` under
  "Binding" (the `Unbound → error` rule at project-brief.md:260–261 gains a
  caveat: "unless a later `import`/`set` populated the user context after
  `bind_pass`").
- **Regression guard:** all existing `unbound_word` error fixtures must still
  error (nothing writes those words to `user_ctx`). Add a parity test
  asserting this.

### Files

- [x] **New (continued): `crates/red-eval/src/module.rs`** — add
  `import_native`:
  - Form 1: `import 'name` — look up `env.modules[name]`; for each exported
    word, call `importer_ctx.set(word, module.ctx.get(word).unwrap())` (write
    the value into the importer's context under the same name). Returns
    `Value::None`.
  - Form 2: `import %path.red` — resolve `%path.red` against
    `system/options/path` (cwd) and `system/options/module-path` (new — see
    M63); `read` the file, `load` it, `eval` it as a module body (the file is
    expected to contain a `module 'name [...]` form or be a bare script
    treated as an anonymous module); cache by canonical path in
    `env.modules_by_path: HashMap<PathBuf, Rc<RefCell<ModuleDef>>>`; then run
    the Form-1 aliasing.
  - Form 3: `import <module-value>` — `import m` where `m` is a
    `Value::Module`. Same aliasing as Form 1.
  *(File lives in `crates/red-eval/src/module.rs` — the M61 top-level module
  file, not `natives/module.rs` as the plan header suggested. `import_file`
  resolves against `env.cwd` via `io::resolve_path` (now `pub(crate)`);
  `system/options/module-path` deferred to M63 per the implementation
  decision. `eval_body_for_module` evaluates the file body in a throwaway
  child context and adopts the result if it's a `Value::Module`; otherwise
  wraps the body as an anonymous module via `build_module`.)*
- [x] **Edit: `crates/red-eval/src/interp_walker.rs`** `resolve_word`
  `Unbound` arm — change from `Err(EvalError::UnboundWord)` to
  `env.user_ctx.get(sym).ok_or_else(|| EvalError::UnboundWord { ... })`. Add
  an inline test asserting the fallback works and that truly-unbound words
  still error.
  *(Also updated `write_setword`'s `Unbound` arm to write to `env.user_ctx`
  for parity with the VM's `SetDynamic`. The walker now checks `user_ctx`
  FIRST, then `natives` — matching the VM's `LoadDynamic` order exactly.)*
- [x] **Edit: `crates/red-eval/src/object.rs`** (or `path.rs`) — extend the
  `eval_get_path`/`set_path_value` resolver: when the head is
  `Value::Module(m)`, look up the tail word in `m.ctx`; **if the word is not
  in `m.exports`, return `UnboundWord`** (private-path-from-outside error).
  Inside the module body (when `env.module_stack.last()` is `m`), skip the
  export check.
  *(Landed in M61: `select_module_field`/`select_module_path`/
  `get_module_path`/`write_path_slot` module arm in `interp_walker.rs`.)*
- [ ] **Edit: `crates/red-eval/src/interp_runner.rs`** —
  `run_series_inner_opts`: after `install_constants`, if `opts.module_paths`
  is set, populate `system/options/module-path` (a new field on the `opts`
  object). The CLI passes `--module-path` args (see M63).
  *(Deferred to M63 per the implementation decision — `import %file` resolves
  against `env.cwd` only via `io::resolve_path` for now.)*
- [ ] **Edit: `crates/red-eval/src/natives/registry.rs`** — extend
  `install_system` to seed `system/options/module-path` (a `block!` of
  `file!` values, default `[%./]`).
  *(Deferred to M63.)*

### Natives

- [x] `import 'name` — alias a named module's exports into the current
      context.
- [x] `import %file.red` — load + cache + alias a file-based module.
- [x] `import <module-value>` — alias a module value's exports.
- [ ] `unimport 'name` / `unimport 'module-name` — remove aliases (write
      `Value::None` into the aliased slots, or remove from `user_ctx` —
      `Context::set` overwrites with `None`; document that `unimport` leaves
      the slot present-but-none). **Optional; defer to v0.6 if
      controversial.** *(Deferred to v0.6 per the implementation decision.)*

### Path resolution for modules

- [x] `module/word` from outside → export check; error if private.
- [x] `module/word` from inside the module body → no export check (the head
      is the module value on the stack, not a word — but inside the body,
      references to `self/word` or the module's own name `m/word` work; bare
      `word` resolves via the module's `ctx` which is `env.user_ctx` during
      body eval).
- [x] `set-path` `module/word: value` from outside → export check (only
      exported words are settable from outside); from inside → no check.
  *(All three landed in M61: `select_module_field`/`select_module_path`/
  `get_module_path`/`write_path_slot` module arm in `interp_walker.rs`.)*

### Golden fixtures

- [x] `import_named` — `module 'm [a: 1 export 'a] import 'm print a` → `1`.
- [x] `import_file` — `write %/tmp/mod.red {module [x: 42 export 'x]} import
      %/tmp/mod.red print x` → `42` (use `tempfile` dev-dep).
      *(Implemented as an inline `#[test]` (`import_file_caches_by_canonical_path`)
      using `tempfile`, not a pure `.red` fixture — the fixture needs a
      filesystem scratch file.)*
- [x] `import_value` — `m: module [a: 1 export 'a] import m print a` → `1`.
- [x] `import_private_unbound` — `m: module [priv: 1 pub: 2 export 'pub]
      import 'm print pub` → `2`; `print priv` → `*** Error: UnboundWord:
      priv` (the private word was not aliased into `user_ctx`).
      *(Fixture uses the named form `module 'm [...]` so `import 'm` can find
      it in `env.modules`.)*
- [x] `import_path` — `m: module [a: 1 export 'a] import 'm print m/a` → `1`;
      `print m/priv` (unexported) → error.
      *(Fixture uses `m: module 'm [...]` so both `import 'm` and `m/a` path
      access work — `module 'name` caches by name but doesn't bind the name
      word; the `m:` assignment does.)*
- [x] `import_cached` — `import %mod.red` twice; the file is read once
      (verify via a side-effect counter in the module body).
      *(Inline `#[test]` (`import_file_caches_by_canonical_path`) — imports
      the same temp file twice and verifies the second call returns the
      cached module.)*
- [x] `import_shadow` — `a: 0 module 'm [a: 1 export 'a] import 'm print a`
      → `1` (import overwrites the existing `a` in `user_ctx`). Document.

### Tests

- [x] Inline `#[test]`: `import 'name` aliases exports into `user_ctx`.
- [x] Inline `#[test]`: bare unbound word resolves after `import` (the
      `resolve_word` change).
- [x] Inline `#[test]`: private word stays unbound after `import`.
- [x] Inline `#[test]`: file import caches by canonical path.
- [x] Inline `#[test]`: `resolve_word` `Unbound` arm now checks `user_ctx`
      (parity test — VM and walker agree).
      *(Covered by `import_makes_bare_word_resolvable` (VM mode) and
      `resolve_word_truly_unbound_still_errors` (regression guard).)*
- [x] Parity test: existing `unbound_word` error fixtures still error
      (nothing wrote those words to `user_ctx`).
      *(`resolve_word_truly_unbound_still_errors` inline test + the
      `unbound_word` / `closure_unbound_capture` golden error fixtures.)*
- [x] `cargo test --workspace` green; `--features force-walk` green.

---

## Milestone 63 — CLI flags + `system/options` extension

Surfaces module paths and module-related options to the CLI and scripts.

- [ ] **Edit: `crates/red-cli/src/main.rs`** — add `--module-path <dir>` flag
      (repeatable; appends to `system/options/module-path`). Add `--no-stdlib`
      flag (skips auto-import of the stdlib module — see M64). Update
      `--help`.
- [ ] **Edit: `crates/red-eval/src/interp_runner.rs`** — `RunOptions` gains
      `module_paths: Vec<PathBuf>` and `no_stdlib: bool`.
      `run_series_inner_opts` populates `system/options/module-path` from
      `opts.module_paths` and (unless `opts.no_stdlib`) auto-imports the
      stdlib (M64) before evaluating the script body.
- [ ] **Edit: `crates/red-eval/src/natives/registry.rs`** — `install_system`
      adds `module-path: [%./]` to `system/options` (a `block!` of `file!`).
- [ ] Inline `#[test]`: `--module-path /tmp` sets `system/options/module-path`
      to `[%/tmp/]`.
- [ ] Inline `#[test]`: `--no-stdlib` skips stdlib auto-import (the stdlib's
      words are unbound).
- [ ] CLI integration test: `red --module-path examples/modules import 'm
      print m/x` runs.
- [ ] `cargo test --workspace` green.

---

## Milestone 64 — Stdlib module + `examples/modules/`

Lands a small stdlib as a module, demonstrating the system end-to-end. Not a
language change — pure content + auto-import wiring.

- [ ] **New: `crates/red-eval/stdlib/stdlib.red`** — a module file with
      ~20–30 utility functions (string utils, list utils, etc.) exported.
      Compiled into the binary via `include_str!` (no file-system dependency
      at runtime).
- [ ] **Edit: `crates/red-eval/src/interp_runner.rs`** — unless
      `opts.no_stdlib`, load `stdlib.red` (via `include_str!`), eval it as a
      module, `import` its exports into `user_ctx`. Cache the compiled
      module on `Env` (so repeated `run_source` calls in the REPL don't
      recompile).
- [ ] **New: `examples/modules/`** — 4–5 example modules: `mathutils.red`,
      `stringutils.red`, `tree.red` (a tree module using closures for
      traversal), `counter.red` (closures + shared state across invocations),
      `main.red` (imports the others).
- [ ] **Edit: `crates/red-cli/tests/examples.rs`** — the existing examples
      harness (M35) runs `examples/modules/main.red` and asserts exit 0.
- [ ] Golden fixtures: `stdlib_basic` (use a stdlib function),
      `module_compose` (modules + closures + import in one program).
- [ ] Inline `#[test]`: stdlib auto-import makes `str/upper` (or whatever)
      available bare.
- [ ] Inline `#[test]`: `--no-stdlib` makes stdlib words unbound.
- [ ] `cargo test --workspace` green.

---

## Milestone 65 — Polish & v0.5.0 release

- [ ] Audit `EvalError` rendering for new error sources (closure capture
      OOB, module private access, `export` outside module, `import`
      file-not-found, circular import).
- [ ] Add spans to `Value::Closure`/`Value::Module` (synthetic —
      `Span::default()`; the originating `closure`/`module` native call's
      span is already on the `EvalError`).
- [ ] Golden fixture per new error case.
- [ ] Property test: `mold(parse(mold(v)))` for `Module` (reparseable via
      `make module!`); skip `Closure` (placeholder mold).
- [ ] Extend `red-core/tests/golden/` to cover `#[closure]` and
      `make module!` literals.
- [ ] Expand `red-eval/tests/programs/` to 20+ new fixtures (closures ×
      modules × paths).
- [ ] Run `cargo bench --bench eval`; record in `BENCHMARKS.md` under
      "v0.5.0". The closure-capture path adds a `Vec<Value>` alloc per
      `closure` creation — expected to be neutral on existing benches (no
      closures in fib/ackermann/sum_loop). If any bench regresses >5%,
      investigate the `Frame::captures: Option<Rc<...>>` field's impact on
      frame size.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`; fix.
- [ ] Run `cargo fmt --all --check`; fix.
- [ ] Update `project-brief.md`:
  - [ ] Remove "closures" and "modules/`import`/`export`" from the
        v0.5-candidates note (project-brief.md:270–271).
  - [ ] Add "Closures & Modules (v0.5)" subsection under "Binding &
        contexts": `closure!` snapshot-capture semantics, `module!`/
        `export`/`import`, the `resolve_word` `Unbound` → `user_ctx`
        fallback change, module path resolution rules.
  - [ ] Add `Closure`/`Module`/`ModuleDef`/`ClosureDef` to the value-model
        section.
  - [ ] Note stdlib auto-import and `--module-path`/`--no-stdlib` CLI
        flags.
- [ ] Update `architecture.md`:
  - [ ] New value variants in the value-model section.
  - [ ] `ClosureDef`/`ModuleDef` struct definitions.
  - [ ] Closure capture cell mechanism (snapshot at `MakeClosure`,
        `Frame::captures`, `LoadCapture`/`SetCapture`).
  - [ ] Module context lifecycle, `Env::modules`/`modules_by_path`
        caches, `import` aliasing.
  - [ ] Path resolution rules for `module!`.
  - [ ] The `resolve_word` `Unbound` → `user_ctx` fallback (behavior
        change).
- [ ] Update `README.md`:
  - [ ] Bump version to v0.5.0.
  - [ ] Remove "No closures" and "No modules" from "Known gaps"
        (README.md:294–296).
  - [ ] Add "Closures & Modules" bullet under "What's implemented".
  - [ ] Add `closure`/`module`/`export`/`import`/`module?`/`closure?` to
        the natives list.
  - [ ] Add `--module-path`/`--no-stdlib` to the CLI section.
  - [ ] Update "Known gaps" with the new deferrals (reactivity v0.6,
        concurrency v0.7, shared-cell closures v0.6, `unimport` v0.6).
- [ ] Final `cargo test --workspace` green.
- [ ] Final `cargo test --workspace --features force-walk` green.
- [ ] Final `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] Tag release `v0.5.0`.

---

## Open questions

1. **Closure capture: snapshot vs. shared cell (M60).** Plan ships snapshot
   (each `closure` copies freevar values at creation; outer writes don't
   propagate). Real Red `closure!` shares the cell across multiple closures
   and across outer/inner (inner writes propagate outward). Shared cells
   require heap-promoting the outer variable (the `SetWord` would write to a
   shared `Rc<RefCell<Value>>` instead of a context slot). This is a deeper
   change touching `bind_pass` and `Context::set`. Recommendation: ship
   snapshot in v0.5 (simpler, correct-for-most-cases, fixes the v0.3 escaping
   bug); document shared-cell as a v0.6 candidate. Confirm before
   implementing.
2. **`resolve_word` `Unbound` → `user_ctx` fallback (M62).** This is a
   behavior change: today `Unbound` errors immediately; after M62, it does
   one `user_ctx.get` lookup first. This makes `import` work without AST
   re-walking. Risk: a script that relies on `Unbound` erroring *before* a
   later `set` could now resolve. Audit shows the only way a word becomes
   `Unbound` at `bind_pass` time but present in `user_ctx` at eval time is
   via `import` or `set` — both intentional. Recommendation: ship the
   fallback; add a parity test asserting all existing `unbound_word` error
   fixtures still error (they should — nothing writes those words). Confirm
   before implementing.
3. **`module` body eval: `env.user_ctx` swap (M61).** Mirrors `make_object`
   (object.rs:104). The body's top-level `SetWord`s write into the module's
   `ctx`, not the script's `user_ctx`. This is correct (matches Red). But:
   the body's *words* (not SetWords) resolve via the module's `ctx` — which
   is `env.user_ctx` during the swap. So `module [a: 1 b: a]` works (b = 1).
   Confirm the binding pass runs *after* the swap (today `bind_pass` runs at
   `run_series_inner_opts:139` before eval; `module` runs mid-eval, so
   `module` does its own `bind_pass_into` on its body — mirrors
   `make_object` at object.rs:108). Yes, this is the existing `make_object`
   pattern. No new design.
4. **`import` and the VM compiler (M62).** After `import 'm`, bare `foo` in
   the *already-compiled* script body has `LoadDynamic(foo)`. The VM's
   `LoadDynamic` arm reads `env.user_ctx.get(foo)` — which `import` has
   populated. **Works without recompilation.** But: if `import` runs
   *before* the body is compiled (e.g. `import 'm` at the top of the script,
   before `foo` is compiled), the compiler still emits `LoadDynamic(foo)`
   (because `foo` isn't in `user_ctx` at `bind_pass` time — `bind_pass` runs
   before any eval, so `import` hasn't run yet). So `LoadDynamic` is correct
   in both orderings. Confirm with a test where `import` is *after* the use
   of `foo` (forward reference) — that should error (Red doesn't allow
   forward references either).
5. **`module` caching and `Env` lifetime (M61).** `env.modules` caches named
   modules per-`run_source` call. In the REPL, `Env` persists across lines,
   so a `module 'm` defined on line 1 is importable on line 2. Confirm REPL
   behavior. For `import %file.red`, the cache is keyed by canonical path;
   a file imported on line 1 is not re-read on line 2. Document.
6. **Stdlib scope (M64).** What goes in the stdlib? Recommendation: keep it
   tiny (~20 functions) — string utils (`starts-with?`/`ends-with?`/
   `contains?`), list utils (`sum`/`product`/`range`/`sort` — `sort` was
   skipped in M30 per plan3:932), math utils (`gcd`/`lcm`/`fib`). Defer a
   full stdlib to v0.6. Confirm the scope before implementing.
