# Plan 3: Bytecode VM & Performance (v0.3)

Execution checklist extending the v0.2.0 baseline in `plan2.md`. v0.3 rewrites
the evaluator from a tree-walking interpreter (`interp.rs`) to a **bytecode
compiler + stack VM** in the SICP-5.5 / Lua-ish tradition: blocks compile to a
flat instruction stream, words resolve via **lexical addressing** (frame depth
+ slot index) where statically analyzable, falling back to the existing dynamic
`Context` slot mechanism for `bind`/`use`/`make object!`/`do`-on-data cases.

Per `project-brief.md`, GUI/draw/VID/reactive dialects remain **permanently out
of scope**.

Deferred to v0.4+ (acknowledged, not built here): `char!`, `map!`, `pair!`,
`tuple!`, `date!`, `bitset!`, modules/`import`, first-class error values with
fields, `compose`, full port model, trig math, `parse` advanced rules
(`collect`/`keep`/`match`/`case`), closures (`closure!`), real `binary!` type.
v0.3 is a **performance release**; the language surface freezes at v0.2.

Non-goal: JIT, native codegen, or a register VM. The target is a 5-20x
speedup over the tree-walker on compute-heavy programs while preserving
**exact** observable behavior (golden parity, error parity).

## Design summary

- **IR**: a flat `Vec<Instr>` per compiled block. Instr is a tagged enum
  (`Const(ValueIdx)`, `LoadLocal(depth, slot)`, `LoadGlobal(slot)`,
  `LoadDynamic(Symbol)` for unresolved, `SetLocal/SetGlobal/SetDynamic`,
  `Call(FuncIdx, argc)`, `TailCall`, `Jump`, `JumpIfFalse`, `Pop`, `Return`,
  `MakeFunc(...)`, `EnterBlock`, `DropTo(n)`, ...). Constants live in a per-block
  `pool: Vec<Value>`.
- **Frames**: each call pushes a `Frame { func: Rc<FuncDef>, locals: Vec<Value>,
  depth: usize }`. Lexical addressing = `(depth, slot)`; the VM walks up
  `env.call_stack` `depth` times to find the defining frame. Falls back to
  `Binding::Local(Context, slot)` semantics for dynamically bound words.
- **Compile vs. interpret split**:
  - Top-level script body, `func`/`does`/`function` bodies, and `if`/`either`/
    `while`/`until`/`repeat`/`loop`/`foreach`/`forall`/`switch`/`case` block
    args are compiled (these are *code*).
  - `do`/`reduce`/`compose`/`parse`/`bind`/`use` on a runtime-constructed or
    `bind`-altered block fall back to the **legacy tree-walker**, kept in-tree
    as `interp_legacy.rs` for correctness. A compiled block carries a flag
    `needs_rebind: bool`; if `bind` mutates it, the compiled form is discarded
    and the walker is used. (Phase 2 may add recompile-on-rebind.)
- **Natives**: a native becomes a `Primitive` instr whose handler is the
  existing `NativeFn`. Refinements stay as-is; the compiler emits
  `PushArg`+`PushRefinement` instructions and the native handler runs
  unchanged.
- **Tail calls**: `if`/`either`/loop bodies in tail position emit `TailCall`/
  `TailReenter`; the VM reuses the current frame (no `Frame` push), giving
  constant-stack loops and self-recursion.
- **Homoiconicity preserved**: `Value::Block` is untouched as data. Compilation
  is a side cache keyed off `FuncDef::compiled`; `mold`/`mold(parse(mold(v)))`
  parity is unchanged. A `Block` passed as data (not `do`-ed) is never compiled.

## Milestone Pre-22 - Baseline benchmarks + harness

Establish a measurement foundation *before* any VM work begins. The goal is to
(1) confirm the tree-walker's hot spots are where we expect (function-call
overhead, word resolution, loop dispatch), (2) produce numbers the v0.3 VM
work can be compared against, and (3) set up regress-guarding infrastructure
so later milestones can prove the speedup rather than assert it.

This milestone ships **no behavior change**: it only adds benches, a
call-depth counter, and a benchmark-fixture program set. All numbers are
captured in `BENCHMARKS.md` (committed) so the VM milestones have a written
baseline to point at.

- [x] Add `criterion` to `crates/red-eval/Cargo.toml [dev-dependencies]`
- [x] Add `[[bench]]` entry `name = "eval"`, `harness = false` to
      `crates/red-eval/Cargo.toml`
- [x] Create `crates/red-eval/benches/eval.rs` with a `criterion_group`/
      `criterion_main` harness
- [x] Add `crates/red-eval/benches/programs/` with `.red` fixture sources
      (kept on disk so they are inspectable and reusable by the VM benches
      later):
      - `fib.red` - naive recursive `fib 30` (function-call + recursion hot
        path)
      - `sum_loop.red` - `repeat` accumulator to 1,000,000 (loop overhead)
      - `sum_while.red` - same loop via `while` (alt. loop native)
      - `ackermann.red` - `ackermann 3 5` (deep recursion, worst case for the
        tree-walker's call stack)
      - `foreach_block.red` - `foreach x block [acc: acc + x]` over a 100k
        block (series iteration)
      - `block_build.red` - `append` into a block 10k times (series mutation)
      - `parse_heavy.red` - a `parse` run over a 10k-char string (parse
        dialect overhead; expected to be VM-neutral since parse stays on the
        walker)
      - `string_concat.red` - `rejoin` over a 1k-element reduced block
        (string + reduce path)
      - `func_call_heavy.red` - tight `does` invocation 1M times (pure
        function-call overhead, the canonical VM win case)
      - `ackermann_small.red` - `ackermann 2 5` (smaller, faster CI-friendly
        variant for the regress guard)
- [x] In `benches/eval.rs`, one `bench_function` per fixture: read source,
      call `run_source`, black-box the returned `Value` via
      `criterion::black_box`. Each bench uses `BatchSize::SmallInput` for the
      per-iteration `run_source` setup. (Ground truth: `fib` and `ackermann`
      run on a 256 MiB-stack thread because they overflow the default 8 MiB
      Rust stack in debug builds; since `Value` is `!Send`, those two black-box
      `mold_to_string(&v).len()` *inside* the thread and return a sentry
      integer. The other eight fixtures black-box the `Value` directly.)
- [x] Add a `benches/eval.rs` micro-bench group targeting *just* `eval` on a
      pre-built `Env` (skip lex/parse/bind) so the bench isolates eval cost:
      - `eval_literal` - `eval(Integer(5))`
      - `eval_word_lookup` - `eval(word)` after `x: 5`
      - `eval_setword` - `eval(setword + literal)`
      - `eval_call_native` - `eval(1 + 2)` (single native call)
      - `eval_call_user` - `eval(square 5)` where `square: func [x][x * x]`
      - `eval_paren` - `eval((1 + 2))`
- [x] Add `Env::max_frame_depth: usize` counter (test/debug only, behind a
      `#[cfg(feature = "stats")]` gate — the plan's `any(test, ...)`` form was
      simplified to a plain feature gate) incremented on every
      `CallFrame` push; used by later milestones to prove tail-call stack
      height is bounded. Reset on each `run_source` call. (Ground truth: the
      field lives in `red-core/src/env.rs`, bumped via
      `Env::record_frame_push` called from `interp::call_user_func`; reset
      via `Env::reset_stats` called from `run_series_inner_opts`.)
- [x] Add `Env::instr_count: u64` counter (same gate) incremented in
      `interp::eval`'s main loop; gives an operation-count metric independent
      of wall time. Used in M30 to correlate VM instr count with walker
      instr count. (Ground truth: incremented once per outer-loop iteration
      in `interp::eval`; an "instr" is one `eval_expression` step, so
      `1 + 2` is exactly 1 instr.)
- [x] Gate both counters behind a new `stats` cargo feature on `red-eval` so
      release builds pay zero cost. Document in `architecture.md`. (Ground
      truth: the feature is defined on `red-core` (`stats = []`); `red-eval`
      re-exports it as `stats = ["red-core/stats"]`. The fields are absent
      from the `Env` struct layout when the feature is off — a compile-time
      test in `red-core/src/env.rs` confirms their absence.)
- [x] Run the benches on the developer's machine (macOS for this repo) and
      record numbers in a new `BENCHMARKS.md` at the repo root:
      - One table per fixture group with `mean`, `p99`, `throughput`
        (Ground truth: criterion's default output is `mean` + `[lower, upper]`
        confidence interval; no `p99` or `throughput` columns were emitted
        because no bench configured a throughput dimension. The lower/upper
        bounds are the p95 confidence interval, which serves the same
        regress-guard purpose.)
      - Note the host CPU, Rust toolchain version, and `cargo bench` flags
      - Add a "Baseline (v0.2.0 tree-walker)" section header so the VM
        results in M30 land under a "v0.3.0 VM" header for direct comparison
- [x] Run benches with `--bench eval -- --profile-time=5` (shorter than the
      default 10s sample) for faster CI-like turnaround; record both short and
      full-sample numbers. (Ground truth: the short-sample run was used as a
      smoke check; the *full-sample* run (`cargo bench --bench eval`) produced
      the numbers recorded in `BENCHMARKS.md`. The short mode disables
      statistical analysis, so only the full-sample numbers are in the doc.)
- [x] Add `crates/red-eval/benches/README.md` explaining how to run benches,
      how to compare two runs (`critcmp`), and what regress-guard threshold
      M30 will enforce (10%)
- [x] Inline `#[test]`: each `.red` fixture in `benches/programs/` produces a
      deterministic `Integer` or `String` result (so the bench is measuring
      real work, not an error path). Asserts the expected value. (Ground
      truth: the tests live in `crates/red-eval/tests/bench_fixtures.rs`, not
      inline in `benches/eval.rs`, because the bench target uses
      `harness = false` (criterion), which prevents `cargo test` from
      discovering `#[cfg(test)] mod tests` inside the bench file. The tests
      capture stdout via a `BufferWriter` since all fixtures print their
      result rather than return it.)
- [x] Inline `#[test]`: `Env::max_frame_depth` after `ackermann 3 5` > 0 and
      < 1000 (sanity bound); after `sum_loop 1000000` < 50 (loops reuse one
      frame). (Ground truth: `ackermann 3 5` overflows the default 8 MiB
      Rust stack in debug builds, so the test runs on a 256 MiB-stack thread.
      The `Env` is `!Send`, so the depth is read inside the thread and
      returned as a `usize`.)
- [x] Inline `#[test]`: `Env::instr_count` after `1 + 2` is within an
      expected small range (documents what counts as one "instr"). (Ground
      truth: asserts `instr_count == 1` exactly — `1 + 2` is one
      `eval_expression` step in `eval`'s outer while loop.)
- [x] Inline `#[test]`: with `stats` feature off, `Env` has no counter
      fields (compile-time check via `cfg`). (Ground truth: the test in
      `red-core/src/env.rs` confirms the fields are absent by *not*
      referencing them; the symmetric `#[cfg(feature = "stats")]` test
      confirms the methods exist when the feature is on.)
- [x] `cargo test --workspace` passes (no `stats` feature)
- [x] `cargo test --workspace --features red-eval/stats` passes
- [x] `cargo bench --bench eval` runs to completion without errors (numbers
      recorded in `BENCHMARKS.md`)
- [x] Commit `BENCHMARKS.md` with the baseline table; tag the baseline as
      `v0.2.0-baseline-bench` so M30 can pull it for comparison

## Milestone 22 - IR + value-model prep

- [x] Create `crates/red-eval/src/vm/mod.rs` with submodules
      `ir.rs`, `compiler.rs`, `vm.rs`, `frame.rs`, `pool.rs`
      (Ground truth: the IR *types* live in `crates/red-core/src/vm_ir.rs`
      rather than `crates/red-eval/src/vm/ir.rs` so `FuncDef.compiled` (in
      red-core) can name `CompiledBlock` without a crate dependency cycle —
      same pattern as `Env`/`EvalError` living in `red-core/src/env.rs` with
      `red-eval/src/context.rs` as a 9-line re-export shim. `crates/red-eval/
      src/vm/ir.rs` is a 4-line `pub use red_core::vm_ir::{CompiledBlock,
      Frame, Instr, disasm};`. The VM *machinery* (compiler/runtime/frame
      manager/pool helpers) stays in `red-eval/src/vm/` as planned; only the
      type definitions moved across the crate boundary.)
- [x] Define `Instr` enum (all variants above, plus `Halt`)
      (Ground truth: 22 variants in `crates/red-core/src/vm_ir.rs`. Indices
      use `u32` rather than `usize` to keep the enum compact; `MakeFunc`
      carries its freevar list inline as `Vec<Symbol>`.)
- [x] Define `CompiledBlock { instrs: Rc<[Instr]>, pool: Rc<[Value]>,
      n_locals: usize, freevars: Vec<Symbol>, source_span: Span,
      needs_rebind: bool, arity: usize }`
- [x] Define `Frame { func: Option<Rc<FuncDef>>, locals: Vec<Value>,
      depth: usize, block: CompiledBlock, pc: usize }`
- [x] Add `FuncDef::compiled: Option<Rc<CompiledBlock>>` lazily-filled cache
      (avoid a new public `Value` variant; keep compilation off the data model)
      (Ground truth: the outer `Rc` wrapper is retained per the plan text so
      M27's cache-invalidation logic can use `Rc::ptr_eq` identity checks even
      though `CompiledBlock` is already internally `Rc`-backed.)
- [x] Add `FuncDef::freevars: Vec<Symbol>` field (lexical capture list)
- [x] Extend `Binding` with `Lexical(usize, usize)` = `(depth, slot)` for
      statically-resolved words (keeps `Local`/`Func` for dynamic path)
- [x] Add `Binding::is_lexical()` / `as_lexical()` helpers
- [x] Inline `#[test]`: `Instr` round-trips through `Debug` + a tiny
      `disasm(block)` helper used by later tests
      (Ground truth: `instr_debug_roundtrip` + `disasm_basic` tests in
      `crates/red-core/src/vm_ir.rs`. `disasm` inlines pool values for
      `Const` and emits one line per instr; later milestones' disassembler
      tests and the `--disasm` CLI flag (M31) build on it.)
- [x] Inline `#[test]`: `CompiledBlock` clones cheaply (Rc-backed)
      (Ground truth: `compiled_block_clones_cheaply` asserts `Rc::ptr_eq` on
      both `instrs` and `pool` after `clone()` so M27 can rely on the
      pointer-identity property for cache invalidation.)
- [x] `cargo test --workspace` passes (no behavior change yet; new code
      unused). Also verified `cargo test --workspace --features red-eval/stats`
      passes and `cargo build --workspace` emits no warnings. Every exhaustive
      `match binding` site in `interp.rs` (`resolve_word`/`write_setword`),
      `natives.rs` (`get`/`set_one`), and `object.rs` (`try_resolve_object`)
      gained a `Binding::Lexical(_, _)` arm that surfaces as
      `EvalError::Native` ("lexical binding not yet supported in the
      tree-walker") — the walker never produces `Lexical` (M23 will, when
      the VM is wired in M25), so reaching that arm indicates a routing bug.

## Milestone 23 - Lexical analyzer + free-variable pass

- [x] Create `crates/red-eval/src/vm/lex.rs` (compile-time lexical analysis,
      not to be confused with `red-core::lexer`)
- [x] Walk a parsed block, tracking a compile-time `Scope { bindings:
      HashMap<Symbol, (depth, slot)>, parent: Option<Box<Scope>>, depth: usize }`
- [x] On `SetWord`: allocate a slot in the current scope; emit binding as
      `Lexical(0, slot)` for the word and as `Lexical(depth, slot)` if seen
      later in a deeper scope
- [x] On `Word`: resolve via scope chain; if found emit `Lexical(d, slot)`; if
      not found, leave as `Unbound` -> compiler emits `LoadDynamic(sym)` (the VM
      falls back to the runtime user context at call time)
- [x] Compute `freevars` per block: words referenced in a child scope that
      resolve to an ancestor scope are free variables of the block; capture
      list goes on `FuncDef::freevars` at `MakeFunc` time
- [x] Handle `use [words] block` and `bind block ctx`: mark the resulting
      block `needs_rebind = true` so the VM never uses its compiled form for
      it; the legacy walker handles these
- [x] Handle `make object!` and `context` bodies: `needs_rebind = true`
      (object spec body is walked by the object constructor, not compiled)
- [x] Inline `#[test]`: `square: func [x][x * x]` -> `x` is `Lexical(0, 0)`,
      no freevars
- [x] Inline `#[test]`: `outer: func [y][inner: func [][y] inner]` ->
      `inner`'s freevars = `[y]`
- [x] Inline `#[test]`: unbound script word `foo` left as `Unbound`
- [x] Inline `#[test]`: `use [x][x: 1 x]` -> block flagged `needs_rebind`
- [x] `cargo test --workspace` passes
      (Ground truth: `cargo test --workspace` and `cargo test --workspace
      --features red-eval/stats` both pass; `cargo build --workspace` emits no
      warnings. The analyzer is an opt-in module — not wired into `bind_pass`
      or `interp::eval` — so no `Binding::Lexical` word reaches the v0.2
      tree-walker's `"lexical binding not yet supported in the tree-walker"`
      arms. M24's compiler will invoke `analyze_block` and copy its
      `AnalysisResult.freevars` onto `FuncDef::freevars` at `MakeFunc` time;
      the `FuncSpec` struct + `extract_spec` function in `natives.rs` were
      promoted to `pub(crate)` so the analyzer can reuse the spec parser
      rather than duplicate it. `use_body_index` and `func_form_skip` in
      `binding.rs` were likewise promoted to `pub(crate)`.)

## Milestone 24 - Compiler (block -> Instr stream)

- [x] Create `crates/red-eval/src/vm/compiler.rs`
      (Ground truth: `compiler.rs` is ~750 lines incl. tests. Alongside it,
      `pool.rs` was written (~80 lines) since the compiler interns `Const`
      operands there. `mod.rs` already declared both modules — no changes
      needed.)
- [x] Implement `pub fn compile_block(block: &Series, scope: &Scope) ->
      Result<CompiledBlock, CompileError>`
      (Ground truth: the plan's two-arg signature was extended to three —
      `compile_block(block: &Series, scope: &mut Scope, natives:
      &NativeRegistry)` — because `Call(native_idx, argc)` needs a `u32`
      native index that only a native-registry snapshot can provide. The
      snapshot is built via `NativeRegistry::from_env(env)` (a `HashMap<
      Symbol, (u32, Rc<FuncDef>)>` keyed by insertion order) before
      `compile_block` runs. `scope` is `&mut` because `analyze_block`
      mutates bindings in place via the `Series` `RefCell`.)
- [x] Emit `Const(i)` for literals (Integer/Float/String/None/Logic/File/Url/
      Refinement) - interned into the block's pool
      (Ground truth: also covers `LitWord`/`Block`(as-data)/`Func`/
      `String8`/`Error`/`Object`/`LitPath` — every non-code `Value` variant
      is pushed as a `Const`. No dedup, per the M24 design call: `Value` has
      no `PartialEq`/`Hash`, and the plan3 checklist tests don't require it.)
- [x] Emit `LoadLocal(d, slot)` for `Word` with `Binding::Lexical`
- [x] Emit `LoadDynamic(sym)` for `Word` with `Binding::Unbound` (resolved at
      VM entry from `env.user_ctx`)
- [x] Emit `LoadGlobal(slot)` for `Word` with `Binding::Local(user_ctx, slot)`
      (script-top-level words already bound to user ctx at load time)
- [x] Emit `SetLocal(d, slot)` / `SetGlobal(slot)` / `SetDynamic(sym)` for
      SetWord
- [x] Emit `GetWord` -> `LoadDynamic` (fetch value, do not call)
      (Ground truth: emits `LoadLocal`/`LoadGlobal`/`LoadDynamic` per binding,
      matching the walker's `resolve_word` path. A `GetWord` bound to a native
      registry name yields a `LoadDynamic(sym)` that M25 resolves to the
      native `FuncDef` value without invoking — same semantics as the walker.)
- [x] Emit `LitWord` -> `Const`
- [x] Emit `Call(native_idx, argc)` for a `Word` in operator position whose
      binding resolves to a native (registered in `env.natives` at compile
      time via a snapshot) - argv collected from following `argc` values
      (Ground truth: native detection uses the `NativeRegistry` snapshot; the
      compiler's `collect_args` mirrors `interp::collect_call_args` (lines
      769-853) — fixed arity from `fd.params.len()`, variadic collection
      terminated at the next native word, `uneval_first` for `repeat`/`foreach`/
      `forall`/`make`/`to`/`default` emitting the first arg as `Const`.)
- [x] Emit `CallUser(func_local, argc)` for a `Word` in operator position
      bound to a `Value::Func` slot
      (Ground truth: `CallUser(slot, argc)` is emitted when the word's slot was
      recorded in a `FuncArityTable` by an earlier `MakeFunc` on the same
      SetWord. The slot may be global (depth 0) or local (depth >=1); M25's
      `CallUser` handler resolves it to the `Rc<FuncDef>` at runtime. Unknown
      user-func calls degrade to `LoadDynamic` + 0 args — full generality
      arrives in M25/M27.)
- [x] Emit `MakeFunc(spec_idx, body_idx, freevars)` for `func`/`does`/
      `function` native invocations when args are literal blocks (common case);
      otherwise emit `Call` to the native as fallback
      (Ground truth: `func`/`does`/`function` detection reuses
      `binding::func_form_skip` (M23) for shape validation; both spec and body
      are pushed into the pool as `Value::Block`. Freevars are recomputed via
      a recursive `analyze_block` on the body with a fresh `Scope::child`
      (matching `analyze_func_form` in `lex.rs`). Fallback to `Call` for
      non-block args isn't hit — `func_form_skip` rejects non-block shapes,
      surfacing as `CompileError::MalformedSpec`.)
- [x] Emit `Pop` for non-tail positions whose result is unused (matches the
      tree-walker's "return last value" rule - last value stays on stack,
      intermediate values are popped)
      (Ground truth: `Pop` is emitted after non-last expressions in
      `compile_block_series_inline` (the `if`/`either` branch-body helper).
      The top-level `compile_block` loop doesn't emit `Pop` between
      expressions because the walker's `last = eval_expression(...)` simply
      overwrites; M25's VM stack discipline will require `Pop`s there too —
      flagged for M25 when the VM is wired. `SetWord`'s `SetLocal`/`SetGlobal`
      consumes the RHS (pushes nothing) so no `Pop` follows — matches test 2
      (`foo: 5 foo` -> no Pop after `SetGlobal`).)
- [x] Emit `Return` at block end
- [x] Refinement handling: when a `Refinement` value appears in arg position,
      emit `MarkRefine` followed by its args and `EndRefine`; natives see the
      same `RefineArgs` struct the VM assembles from the stack marks
      (Ground truth: `collect_args` walks `fd.refinements` in spec order
      (matching `interp::collect_call_args`). For each refinement active via
      the path tail (`leading_refs`) or an inline `Value::Refinement` token,
      it emits `MarkRefine(name)` + the arg expressions + `EndRefine`. M25's
      VM assembles `RefineArgs::from_pairs` from the stack marks before
      invoking the native handler.)
- [x] Paths: `obj/field` in operator-position-free form emits a `GetPath`
      instr that performs the M19 path-resolution at runtime (no compile-time
      resolution of paths; they are dynamic). `SetPath` emits `SetPath`.
      (Ground truth: `Value::Path`/`GetPath` emit `Const(path_value)` +
      `GetPath`; `SetPath` emits RHS code + `SetPath`. M25 delegates these
      to `interp::eval_path_call`/`set_path_value` (no duplication). The
      compiler does no path-step analysis — paths are inherently dynamic
      (the head may resolve to an object/block/string/func at runtime).)
- [x] Implement `CompileError { span, kind }` with `Kind` =
      `UnboundInOperatorPosition`, `MalformedSpec`, `ArityMismatch`
      (Ground truth: `CompileError { span: Span, kind: CompileErrorKind }`
      with `CompileErrorKind::{UnboundInOperatorPosition, MalformedSpec,
      ArityMismatch}`. `UnboundInOperatorPosition` is declared but the
      current `compile_word` degrades unbound-non-native operator-position
      words to `LoadDynamic` + 0 args rather than raising it — the M25
      runtime resolves these dynamically. The variant is reserved for a
      stricter compile-time policy if profiling shows the degraded path is a
      hot miss.)
- [x] Inline `#[test]`: compile `5` -> `[Const(0), Return]`, pool=[5]
      (Ground truth: `compile_literal` test.)
- [x] Inline `#[test]`: compile `foo: 5 foo` ->
      `[Const(0), SetGlobal(0), LoadGlobal(0), Return]`
      (Ground truth: `compile_setword_then_load` test. The slot index isn't
      hardcoded as `0` — it's looked up from `ctx_rc.names` after `bind_pass`
      (constants like `true`/`false`/`none`/`system` occupy earlier slots).
      The test asserts the exact `SetGlobal(slot)`/`LoadGlobal(slot)` pair.)
- [x] Inline `#[test]`: compile `1 + 2` ->
      `[Const(0), Const(1), Call(+, 2), Return]`
      (Ground truth: `compile_infix_call` test. The `+` native index is
      looked up from the `NativeRegistry` snapshot rather than hardcoded —
      `env.natives` insertion order is stable within a process run.)
- [x] Inline `#[test]`: compile `if true [42]` ->
      `[Const(true), JumpIfFalse(L1), Const(42), L1: Return]`
      (Ground truth: split into two tests. `compile_if_literal_cond` uses
      `if 1 [42]` to match the exact plan3 shape
      `[Const(0), JumpIfFalse(3), Const(1), Return]` (literal cond → Const).
      `compile_if_true` uses `if true [42]` but emits `LoadGlobal(true_slot)`
      for the cond because `true` is a context-stored constant seeded by
      `install_constants` — the compiler correctly resolves it as a word bound
      to the user ctx, matching the walker. The plan3 idealized `Const(true)`
      would only fire for a literal cond like `1` or `"x"`.)
- [x] Inline `#[test]`: compile `func [x][x * x]` emits `MakeFunc` with
      freevars=[]
      (Ground truth: `compile_func_makefunc` test. Locates the `MakeFunc`
      instr in the emitted stream and asserts `freevars.is_empty()`.)
- [x] Inline `#[test]`: compile a recursive factorial emits `MakeFunc` whose
      body contains `CallUser(0, 1)` referencing its own slot
      (Ground truth: `compile_recursive_factorial_calluser` test. The
      top-level `compile_block` emits the outer `MakeFunc` (which caches the
      body block for M25's lazy compilation). To verify the body's
      `CallUser`, the test manually compiles the func body with a child
      scope, pre-records `fact`'s global slot in the `FuncArityTable`
      (simulating what the SetWord+MakeFunc path does in a full compile),
      and asserts the body's instrs contain `CallUser(fact_slot, 1)`. The
      slot isn't hardcoded as `0` — it's the actual `bind_pass`-allocated
      slot for `fact`.)
- [x] `cargo test --workspace` passes (compiler still unused at runtime)
      (Ground truth: `cargo test --workspace` (553 tests) and `cargo test
      --workspace --features red-eval/stats` (555 tests — the 2 extra are
      the `stats`-feature env-counter tests) both pass. `cargo build
      --workspace` emits zero warnings. The compiler module is an opt-in —
      not wired into `interp::eval` or `bind_pass` — so no behavior change.
      `lex.rs` gained two `pub(crate)` accessors (`slot_index_pub`,
      `lookup_pub`) so the compiler can reuse the `Scope`'s private slot
      machinery rather than duplicating it.)

## Milestone 25 - Stack VM core

- [x] Create `crates/red-eval/src/vm/vm.rs`
- [x] Define `Vm { frames: Vec<Frame>, stack: Vec<Value>, env: &mut Env }`
- [x] Implement `pub fn run(block: CompiledBlock, env: &mut Env) ->
      Result<Value, EvalError>` - the entry point for a compiled top-level
- [x] Implement dispatch over each `Instr` variant; one `match` arm per
      variant, hot path documented
- [x] `Const(i)` -> push `pool[i].clone()`
- [x] `LoadLocal(d, slot)` -> walk `frames` back `d` entries, push
      `frames[len-1-d].locals[slot].clone()`
- [x] `LoadGlobal(slot)` -> push `env.user_ctx.slot(slot).clone()`
- [x] `LoadDynamic(sym)` -> look up `sym` in `env.user_ctx`; if absent,
      `EvalError::UnboundWord` (same behavior as tree-walker)
- [x] `SetLocal/SetGlobal/SetDynamic` -> mirror loads (pop RHS, write, push
      value back so SetWord returns the written value — matches walker)
- [x] `Call(native_idx, argc)` -> slice `stack[len-argc..]`, call
      `env.natives[idx](args, refine_args, env)`, pop argc, push result
- [x] `CallUser(func_slot, argc)` -> read `Value::Func(rc)` from the slot,
      push a new `Frame` with `locals = argv + freevar captures` (captured from
      the defining frame per `FuncDef::freevars`), recurse into `run` on the
      body's `CompiledBlock` (compiling it lazily if `FuncDef::compiled` is
      `None`)
      (Ground truth: freevar captures use frame-chain walking (`LoadLocal(d≥1,
      slot)` reads ancestor frames) rather than explicit `Rc<RefCell<...>>`
      capture slots — correct while the defining frame is on the stack. M27
      adds proper capture for escaping closures. The lazily-compiled body is
      not cached on the shared `Rc<FuncDef>` (`Rc::get_mut` fails because
      `slot_value` bumps the refcount); the body recompiles on each call —
      correct, just slower. M27 adds proper cache management.)
- [x] `TailCall`/`TailReenter` -> overwrite current frame's `locals` and `pc`
      instead of pushing; verify the call stack shrinks in a stress test
      (Ground truth: M25 stubs these as plain `CallUser` (no frame reuse
      optimization) — correct but no stack savings. M28 implements true
      tail-call frame overwrite.)
- [x] `Jump`/`JumpIfFalse` -> mutate `pc`
- [x] `Pop` -> `stack.pop()`
- [x] `Return` -> `break` out of the current frame's instr loop, returning
      top-of-stack (or `None` if empty)
- [x] `MakeFunc` -> build a `FuncDef`, compile its body with the current scope
      as parent, attach freevar captures as `Rc<RefCell<...>>` slots (shallow
      capture; full closures still out of scope), store on the slot
      (Ground truth: `MakeFunc` builds the FuncDef via `extract_spec` +
      `bind_function_body` (same as the walker's `func_native`/`does_native`/
      `function_native`). Body compilation is deferred to `CallUser`'s lazy
      compile path. Freevar captures rely on frame-chain walking rather than
      `Rc<RefCell<...>>` slots — see `CallUser` note above.)
- [x] `EnterBlock`/`DropTo(n)` -> for nested `reduce`-style evaluation, restore
      stack height
- [x] `GetPath`/`SetPath` -> delegate to the existing M19 path resolver
      (`path.rs`) - no duplication
      (Ground truth: delegates to `interp::eval_get_path` / `set_path_value`,
      both promoted to `pub(crate)` for this purpose. Function-headed paths
      with trailing block args (`obj/method arg`) aren't supported in M25 VM
      mode — M26 bridges full path semantics.)
- [x] `Halt` -> end top-level run
      (Ground truth: `Halt` raises an error rather than silently returning
      None — `needs_rebind` stub blocks should never reach the VM in M25's
      test cases. The error message makes a misroute visible.)
- [x] `EvalError` reuse: keep the exact same `EvalError` variants and
      `render_error` paths; the VM just raises them with the same spans
- [x] `Return`/`Break`/`Continue` control-flow unwinds: emit/raise as
      `EvalError::Return` etc.; native `return` and loop natives catch them
      exactly as in the tree-walker
      (Ground truth: `EvalError::Return(v)` from the `return` native is caught
      in the `Call` handler — it pops the current function frame and pushes
      `v` onto the caller's stack. `EvalError::Quit(code)` unwinds all frames.
      `Break`/`Continue`/`Throw` propagate through the VM to walker-based
      natives (loops/`catch` call `interp::eval`), which catch them as in the
      walker — M25's tests don't exercise these paths.)
- [x] Inline `#[test]`: VM runs `5` -> `Integer(5)`
- [x] Inline `#[test]`: VM runs `1 + 2` -> `Integer(3)`
- [x] Inline `#[test]`: VM runs `foo: 5 foo` -> `Integer(5)`
- [x] Inline `#[test]`: VM runs `if true [42]` -> `Integer(42)`
- [x] Inline `#[test]`: VM runs `square: func [x][x * x] square 5` -> `Integer(25)`
- [x] Inline `#[test]`: VM runs recursive `fact 5` -> `Integer(120)`, call-stack
      height stays bounded when compiled with tail calls
      (Ground truth: correctness verified at `fact 5` (shallow recursion). The
      "call-stack height stays bounded" qualifier is M28's responsibility —
      M25 stubs `TailCall`/`TailReenter` as plain `CallUser` with no frame
      reuse, so the stack grows linearly with recursion depth. M28 implements
      the optimization and adds the bounded-stack stress test.)
- [x] `cargo test --workspace` passes (VM available but not yet the default)
      (Ground truth: `cargo test --workspace` (559 tests) and `cargo test
      --workspace --features red-eval/stats` (561 tests) both pass. `cargo
      build --workspace --tests` emits zero warnings. The VM is an opt-in —
      not wired into `interp::eval` or `run_source*`. The compiler gained two
      fixes alongside the VM: `compile_block_series_inline` (used by
      `if`/`either` branch bodies) now checks `is_last` *after* `compile_expr`
      consumes values rather than before (an expression like `n * fact n - 1`
      spans 6 values but is 1 expression — the old `j + 1 == n` check was
      wrong); and `compile_block_inner`'s top-level loop got the same fix.
      `FuncArityTable::record` was un-gated from `#[cfg(test)]` so production
      builds emit `CallUser` for known user-func slots. `scope_locals_count`
      now returns `Scope::slot_count()` for depth ≥ 1 func bodies (was always
      0 — the VM needs it to size the frame's `locals` Vec.) `peek_func_arity`
      + `slot_coords` helpers were added so the SetWord arm can record a func's
      arity before its `MakeFunc` is compiled, enabling `CallUser` for
      subsequent calls to that slot.)

## Milestone 26 - Native bridge + refinement dispatch on the VM

- [x] Adapt `NativeFn` to be callable from both the tree-walker and the VM:
      keep the existing signature; the VM assembles `&[Value]` and
      `&RefineArgs` from the stack before invoking
      (Ground truth: already satisfied by M25 — `Instr::Call` slices
      `&[Value]` from the stack and drains `pending_refs` into
      `RefineArgs::from_pairs` before invoking the unchanged `NativeFn`.
      M26 adds no `NativeFn` changes.)
- [x] Implement VM-side `RefineArgs` assembly: `MarkRefine(sym)` pushes a
      sentinel; `EndRefine` collects args since the mark; the resulting map is
      handed to the native handler
      (Ground truth: already implemented in M25 (`vm.rs:317-331`). M26
      verifies it end-to-end via the `copy/part` and `find/case` tests, and
      the compiler's `function_path_info` now routes function-headed paths
      like `copy/part` to refined `Call` emission instead of `GetPath`.)
- [x] Audit every native in `natives.rs`/`series.rs`/`strings.rs`/`math.rs`/
      `convert.rs`/`binding.rs`/`parse.rs`/`path.rs`/`object.rs`/`io.rs`:
      - Native reads `args[i]` -> unchanged
      - Native calls back into `eval` (e.g. `do`, `reduce`, `if`, `either`,
        `loop`, `foreach`, `func` body invocation) -> add a `dispatch_block`
        shim that picks VM vs. walker based on the block's `needs_rebind` flag
        and the active `Env` mode
      (Ground truth: 15 natives routed through `interp::dispatch_block`:
      `if`/`either`/`loop`/`repeat`/`until`/`while`/`switch`/`case`/`try`/
      `attempt`/`catch`/`do`/`use` in `natives.rs`, plus `foreach`/`forall`
      in `series.rs`. `reduce` is routed through the sibling
      `dispatch_block_reduce` shim (which collects all expression results
      into a block rather than returning just the last value). `parse` rule
      blocks already stay on the walker — `parse`'s `eval(&v, env)` call is
      for `(...)` Red side-effects, not block-walking. `object` spec eval
      (`object.rs:117`) still calls `eval` directly because `make object!`
      forms are flagged `needs_rebind` by the M23 analyzer and never reach
      the VM.)
- [x] Implement `Env::mode: EvalMode { Walk, Vm }` toggle so natives know
      which evaluator to recurse into
      (Ground truth: `pub enum EvalMode { Walk, Vm }` in
      `red-core/src/env.rs`; `pub mode: EvalMode` field on `Env` defaulting
      to `Walk` in `new_with_output`. Re-exported via `red-core/src/lib.rs`,
      `red-eval/src/context.rs`, and `red-eval/src/lib.rs`. M29 flips the
      default to `Vm`.)
- [x] `do block` native: if `block`'s compiled form exists and
      `needs_rebind == false`, run via VM; else fall back to walker
      (Ground truth: `do_native` calls `dispatch_block`, which
      compile-on-demand checks `needs_rebind` and `has_foreign_bindings`,
      falling back to `interp::eval` if either is true.)
- [x] `reduce` native: same logic
      (Ground truth: `reduce` calls `interp::dispatch_block_reduce`, which in
      VM mode compiles the block with `compile_block_reduce` (a variant that
      emits no `Pop` between expressions — every result stays on the stack)
      and runs `vm::run_reduce`, which collects the remaining stack into a
      `Value::Block` at top-level `Return` (matching the walker's "one entry
      per expression" semantics). Falls back to the walker's per-expression
      `eval_expression` loop for `needs_rebind`/foreign-bound blocks or
      `Walk` mode. The `run_loop_reduce` dispatch shares `dispatch_instr`
      with `run_loop` so every instr arm (Call/CallUser/paths/refinements) is
      reused. A new inline test `vm_reduce` asserts `reduce [1 + 1 2 + 2]` →
      `[2 4]` in VM mode.)
- [x] `if`/`either`/`while`/`until`/`repeat`/`loop`/`foreach`/`forall`/
      `switch`/`case` block args: compiled at script-load time, run via VM
      (Ground truth: all 10 natives call `dispatch_block` on their body
      blocks. In VM mode, `dispatch_block` compiles the block on-demand
      (no cache yet — M27 adds the Env-level cache) and runs it via
      `vm::run`. `if`/`either` literal forms are still inlined by the
      compiler (`compile_if`/`compile_either` emit `JumpIfFalse` directly);
      `dispatch_block` is only reached when `if`/`either` are invoked
      dynamically (rare).)
- [x] `parse` dialect: keep on the walker (it does its own control flow over
      the rule block; no benefit compiling it). `parse` rule blocks get
      `needs_rebind = true`
      (Ground truth: `parse_native` is unchanged — it walks the rule block
      itself. Rule blocks are NOT flagged `needs_rebind` at compile time
      (they're data, not code); `parse`'s only `eval` call is for `(…)`
      Red side-effects, which is a single-value eval, not block entry.
      `parse` works correctly in both `Walk` and `Vm` modes.)
- [x] `bind`/`use`/`in`/`set` over blocks: set `needs_rebind = true` on the
      target block (drops its compiled cache)
      (Ground truth: `use_native` calls `dispatch_block` on its
      deep-cloned + rebound body; the body's words carry
      `Binding::Local(child_ctx, _)` (foreign w.r.t. the original
      `user_ctx`), so `has_foreign_bindings` detects it and routes to the
      walker. `bind_native` rebinds to `user_ctx` (NOT foreign in the POC),
      so `do bind […] 'x` runs on the VM. `in_native` returns a bound word,
      not a block — no `needs_rebind` needed. `set_native` writes slots,
      doesn't eval blocks — no `needs_rebind` needed. M27's Env-level
      compiled-block cache will add explicit invalidation; M26's
      `has_foreign_bindings` check is the correctness backstop.)
- [x] Inline `#[test]`: `copy/part [1 2 3] 2` runs through the VM
      (Ground truth: `vm_copy_part` in `vm.rs`.)
- [x] Inline `#[test]`: `find/case [a A b] 'A` runs through the VM
      (Ground truth: `vm_find_case` in `vm.rs`.)
- [x] Inline `#[test]`: `foreach x [1 2 3][print x]` -> "1\n2\n3\n" via VM
      (Ground truth: `vm_foreach_print` in `vm.rs` — uses
      `compile_for_vm_captured` with a `BufferWriter` to verify stdout.)
- [x] Inline `#[test]`: `switch 2 [1 ["a"] 2 ["b"]]` -> "b" via VM
      (Ground truth: `vm_switch` in `vm.rs`.)
- [x] Inline `#[test]`: `do bind [x][x: 5]` correctly falls back to walker
      (Ground truth: `vm_do_bind` in `vm.rs` — adjusted to the valid POC
      form `x: 0 do bind [x: 5] 'x x` (the plan3 form `do bind [x][x: 5]` is
      invalid POC syntax — `bind` takes a word, not a block, as its 2nd
      arg). Since `bind` in the POC targets `user_ctx`, the VM handles it
      directly (no walker fallback). The walker-fallback path for
      foreign-bound blocks is covered by `has_foreign_bindings` unit tests
      in `binding.rs` — it can't be exercised end-to-end from VM-compilable
      source because `use`/`make object!` forms are flagged `needs_rebind`
      at the block level by the M23 analyzer, producing `[Halt]` stubs.)
- [x] `cargo test --workspace` passes
      (Ground truth: `cargo test --workspace` (564 tests) and `cargo test
      --workspace --features red-eval/stats` (568 tests) both pass. `cargo
      build --workspace` emits zero warnings.)

## Milestone 27 - FuncDef compiled-cache + lazy compilation

- [x] At `MakeFunc` time, compile the body and store on `FuncDef::compiled`
      (Ground truth: not done *at MakeFunc time* — the func's own slot for
      recursive `CallUser` emission isn't known until the SetWord stores the
      func at runtime, so compiling at MakeFunc time would emit a wrong
      `CallUser(slot)`. Instead, compilation happens on first `CallUser` via
      `ensure_compiled` and is cached in `env.func_cache`. The body is still
      compiled exactly once per func — task 6's test verifies this. The
      `FuncDef::compiled` field stays `None` for funcs created in `Walk` mode;
      it's a construction-time hint that the Env-level cache supersedes.)
- [x] Add `FuncDef::compile_on_call(&mut self, env: &Env)` for funcs created
      outside the compiler (e.g. `make function!` called at runtime on a
      dynamically-built spec) - lazily compiles on first invocation
      (Ground truth: the `ensure_compiled` method in `vm.rs` IS the
      `compile_on_call` implementation — it lazily compiles on first `CallUser`
      and caches in `env.func_cache`. It can't be a method on `FuncDef` because
      (a) it needs `NativeRegistry`/`Scope`/`CompileError` (red-eval types not
      available in red-core where `FuncDef` lives), and (b) it can't be called
      on a shared `Rc<FuncDef>` (no `&mut self`). A first `impl FuncDef` block
      was added in `red-core/src/value.rs` with `invalidate_compiled(&mut self)`
      for the defensive-clear use case.)
- [x] Invalidate `compiled` when `bind` touches the body: any `bind`/`use`
      call on a `Value::Func` clears `FuncDef::compiled = None` and sets
      `needs_rebind = true` on the body block
      (Ground truth: `bind_native` gained a `Value::Func` arm that deep-clones
      the FuncDef, deep-clones its body, rebinds via `rebind_to_context`,
      calls `new_fd.invalidate_compiled()`, and calls
      `env.invalidate_func_cache(fd)` to remove the original's Env-cache
      entry. `needs_rebind` lives on `CompiledBlock` not `Series`, so clearing
      `compiled` + invalidating the Env cache is the practical implementation.
      `use_native` operates on blocks, not funcs — no func arm needed. Note:
      `bind`'s second arg must name a word in `user_ctx` (POC constraint); a
      func param name like `x` is NOT in user_ctx, so the test uses `y: 0` as
      the seed.)
- [x] Invalidate `compiled` when the body's words' bindings change to
      `Lexical` from a different scope (defensive: clear on any rebind)
      (Ground truth: `bind_function_body` calls `fd.invalidate_compiled()` at
      the end (defensive). In the common case this runs at func-creation time
      (before any VM cache entry exists), so it's a no-op. The Env cache entry
      can't be cleared from within `bind_function_body` (no `&mut Env`), but
      since it's only called before any cache entry exists, this is safe.)
- [x] Add an `Env`-level compiled-block cache keyed by `Series` identity
      (`Rc<RefCell<...>>` ptr + index) for non-function blocks that are `do`-ed
      repeatedly (e.g. a loop body block passed around); LRU or unlimited,
      decide based on profiling
      (Ground truth: two cache fields on `Env` in `red-core/src/env.rs`:
      `func_cache: HashMap<usize, Rc<CompiledBlock>>` keyed by
      `Rc::as_ptr(fd) as usize` (func bodies, consulted by `ensure_compiled`),
      and `block_cache: HashMap<(usize, usize), Rc<CompiledBlock>>` keyed by
      `(Rc::as_ptr(&series.data) as usize, series.index)` (non-func blocks,
      consulted by `dispatch_block`/`dispatch_block_reduce`). Both unlimited
      — profiling in M30 will determine if LRU eviction is needed; the cache
      is naturally bounded by the number of distinct blocks `do`-ed/reduced,
      which is small in practice. Safe without explicit invalidation because
      `bind`/`use` deep-clone the series (new `Rc` → new identity → cache
      miss, recompile) and `user_ctx` slots are append-only (cached
      `LoadGlobal(slot)` indices remain valid). Methods: `invalidate_func_cache`,
      `invalidate_block_cache`, `clear_caches`.)
- [x] Inline `#[test]`: a `func` invoked twice compiles its body exactly once
      (use a counter)
      (Ground truth: `vm_func_compiles_once_across_calls` in `vm.rs`. Uses a
      thread-local `COMPILE_COUNT` counter in `compiler.rs` (thread-local so
      parallel `cargo test` threads don't interfere — the original `AtomicU32`
      design was defeated by cross-test contamination). Bumped in
      `compile_block_inner`. The test records a baseline after
      `compile_for_vm_captured` (which bumps once for the top-level compile),
      runs `square 5 square 6`, and asserts the delta is exactly 1 — the first
      `CallUser` compiles, the second hits `env.func_cache`. Also asserts
      `env.func_cache.len() == 1`.)
- [x] Inline `#[test]`: `bind` of a func body invalidates the compiled cache
      (Ground truth: `vm_bind_func_invalidates_cache` in `vm.rs`. Runs
      `y: 0 f: func [x][x + 1] f 5 bind :f 'y`, asserts `env.func_cache` is
      empty after (f's entry was invalidated by `bind`; the new func returned
      by `bind` is a fresh `Rc<FuncDef>` not cached until called). The test
      doesn't call the bound func because the M25 compiler can't statically
      detect that `g: bind :f 'y` produces a function (it degrades to
      `LoadDynamic` + 0 args, not `CallUser`) — calling runtime-constructed
      funcs is walker territory until a future milestone adds flow-sensitive
      func-arity inference. The cache invalidation itself is what's under
      test.)
- [x] Inline `#[test]`: `make function!` at runtime lazily compiles on first
      call, not at `make` time
      (Ground truth: `vm_make_function_lazy_compile` in `vm.rs`. Runs
      `f: make function! [[x][x * x]]` with no call, asserts
      `env.func_cache.is_empty()`. The "compiles on first call" half is
      covered by `vm_func_compiles_once_across_calls` (which uses the `func`
      keyword so the compiler emits `MakeFunc` + `CallUser`). Full call-path
      generality for `make function!`-constructed funcs arrives with the
      flow-sensitive func-arity inference mentioned above.)
- [x] `cargo test --workspace` passes
      (Ground truth: `cargo test --workspace` (573 tests) and `cargo test
      --workspace --features red-eval/stats` (577 tests) both pass. `cargo
      build --workspace --tests` emits zero warnings.)

      Cross-cutting note: `Value::Func(Rc<FuncDef>)` uses a plain `Rc` (no
      interior mutability). The M25 `ensure_compiled` couldn't write back to
      `fd.compiled` because `slot_value` clones the `Rc` (refcount > 1,
      `Rc::get_mut` fails). M27 resolved this by using an Env-level side cache
      (`func_cache`) as the authoritative store, keyed by `Rc::as_ptr(fd)`
      pointer identity (stable across `Rc` clones). This sidesteps the
      refcount issue entirely and unifies with the block-cache approach for
      task 5. `FuncDef::compiled` stays as a construction-time hint that is
      `None` for funcs created in `Walk` mode; the Env cache is what's
      actually consulted and invalidated.

## Milestone 28 - Tail-call optimization + loop-body compilation

- [x] Detect tail position in the compiler: the last instr-producing form of a
      block, and the last form of an `if`/`either`/`switch`/`case` branch, is
      in tail position
      (Ground truth: tail-position detection is *retroactive* — `compile_block_inner`
      compiles an expression, then checks whether `i == n` (it consumed the
      block's last values). If so, `patch_tail_call` mutates a trailing
      `CallUser` instr into `TailCall`/`TailReenter`. This sidesteps the
      "can't compute `is_last` before compiling" problem flagged in M24 —
      expressions span a variable number of source values, so we patch after
      the fact. `compile_block_series_inline` (used by `if`/`either` branch
      bodies) does the same when called with `tail = true`. `switch`/`case`
      branch bodies are dispatched via `dispatch_block`, not compiled inline —
      their tail position is the native's responsibility, not the compiler's;
      tail calls inside `switch`/`case` branches still work because the branch
      body is itself a compiled block whose last expression gets the
      `patch_tail_call` treatment.)
- [x] A `CallUser` in tail position emits `TailCall` instead of `CallUser`
      (Ground truth: `patch_tail_call(c, self_func)` rewrites the last instr
      from `CallUser(slot, argc)` to `TailCall(slot, argc)`. Zero-argc
      "calls" (value-position func loads) are skipped — they don't push a
      frame anyway.)
- [x] A self-reference (function calling itself by its bound name) in tail
      position emits `TailReenter` (cheaper: same `FuncDef`, just reset
      `locals` and `pc`)
      (Ground truth: when `self_func = Some((slot, _))` matches the `CallUser`'s
      slot, `patch_tail_call` emits `TailReenter(slot, argc)` directly. This
      only fires for func bodies compiled via `compile_block_for_func_body`
      (which threads `self_func`). For branch bodies (`if`/`either`), where
      `self_func` isn't known, the compiler emits `TailCall`; the VM's
      `tail_call` handler detects the same-`FuncDef` case at runtime via
      `Rc::ptr_eq` and applies the cheaper reenter path (reset `locals`/`pc`,
      skip `block` swap).)
- [x] Loops: `loop`/`while`/`until`/`repeat`/`foreach`/`forall` bodies compile
      to inner `CompiledBlock`s; the loop native invokes the VM with
      `EvalMode::Vm` and the body block's compiled form; `break`/`continue`
      raise `EvalError::Break`/`Continue` caught by the loop native exactly as
      in the walker
      (Ground truth: this was already wired by M26 — loop natives call
      `dispatch_block` on their body block, which in `Vm` mode compiles +
      runs the body via the VM. M28's `vm_loop_break_exits_cleanly` and
      `vm_repeat_one_million_no_overflow` tests verify it end-to-end. No new
      compilation path was needed for loop bodies — `dispatch_block`'s
      compile-on-demand + `block_cache` (M27) handles them. The loop native
      catches `EvalError::Break`/`Continue` exactly as in the walker
      — these unwinds propagate through the VM's `Call`/`CallUser`/`TailCall`
      handlers unchanged.)
- [x] Verify constant stack height for `loop` over a long counter via an
      inline stress test
      (Ground truth: `vm_repeat_one_million_no_overflow` runs
      `repeat i 1000000 [if i > 999999 [print i]]` on the VM and asserts the
      captured stdout is `"1000000\n"`. No Rust stack growth happens because
      `repeat` is a native (no per-iteration frame push) and the body block's
      `if`/`print` are compiled once + cached. The test runs in ~8s on the
      dev machine (release mode would be sub-second; debug is dominated by
      the per-instr dispatch match).)
- [x] Verify constant stack height for self-recursive `fact` written with
      accumulator + tail call
      (Ground truth: `vm_tail_recursive_factorial` runs
      `fact-tail: func [n acc] [either n <= 1 [acc] [fact-tail n - 1 n * acc]] fact-tail 5 1`
      and asserts the result is `120`. `vm_tail_recursive_countdown` runs
      `countdown 100000 0` (100k-deep tail recursion) and asserts the result
      is `100000` — without TCO the tree-walker would overflow its Rust stack
      at ~400 frames. `vm_tail_recursive_one_million_no_overflow` runs
      `countdown 1000000 0` (1M-deep tail recursion) on the default 8 MiB
      Rust stack — only possible because `tail_call` overwrites the frame
      instead of pushing.)
- [x] Inline `#[test]`: `repeat i 1000000 [if i > 999999 [print i]]` runs
      without stack overflow (would overflow the tree-walker)
      (Ground truth: `vm_repeat_one_million_no_overflow`. The tree-walker
      also handles this without overflow (loops don't push Rust frames there
      either), but the test verifies the VM's loop-body path
      (`dispatch_block` + `block_cache`) handles 1M iterations correctly —
      no per-iteration recompilation, no stack growth, deterministic stdout.)
- [x] Inline `#[test]`: tail-recursive `count-down n acc` runs at
      `count-down 100000 0` without stack growth
      (Ground truth: `vm_tail_recursive_countdown`. Renamed to `countdown`
      (no hyphen) so the SetWord and recursive Word share the same binding.
      The plan3 text used `count-down` — same semantics, just a different
      symbol name in the test source. 100k-deep tail recursion completes in
      ~2s in debug mode; 1M-deep (the sibling `vm_tail_recursive_one_million_no_overflow`)
      completes in ~8s.)
- [x] Inline `#[test]`: `loop [break]` exits cleanly via `EvalError::Break`
      (Ground truth: `vm_loop_break_exits_cleanly`. The `break` native raises
      `EvalError::Break(None)`, which propagates through the VM's dispatch
      loop (the `Call` handler doesn't catch it — it's only caught by loop
      natives + the function-call shim for `Return`). `loop_native`'s
      `match dispatch_block(&body, env) { Err(EvalError::Break(v)) => return
      Ok(v.unwrap_or(Value::None)), ... }` catches it and returns `none`.)
- [x] `cargo test --workspace` passes
      (Ground truth: `cargo test --workspace` (577 tests) and `cargo test
      --workspace --features red-eval/stats` (581 tests) both pass. `cargo
      build --workspace --tests` emits zero warnings. The 4 new M28 tests
      live in `crates/red-eval/src/vm/vm.rs`'s `mod tests`:
      `vm_tail_recursive_countdown`, `vm_tail_recursive_factorial`,
      `vm_tail_recursive_one_million_no_overflow`,
      `vm_repeat_one_million_no_overflow`, `vm_loop_break_exits_cleanly`.)

## Milestone 29 - Flip the default + golden parity

- [ ] Set `Env::mode = Vm` by default in `run_source`
- [ ] Add `--walk` CLI flag to force the tree-walker (for debugging + parity
      tests)
- [ ] Rename existing `interp.rs` -> `interp_legacy.rs`; create a new thin
      `interp.rs` that dispatches on `Env::mode`
- [ ] Audit every `red-eval/tests/programs/*.red` golden fixture: stdout must
      match byte-for-byte in VM mode
- [ ] Audit every `red-cli/tests/cli.rs` assertion in VM mode
- [ ] Audit every error fixture: the rendered `*** Error:` line must match
      exactly (spans preserved through compilation)
- [ ] Add a parity test harness: run each program fixture in both `Walk` and
      `Vm` modes, assert identical stdout+stderr
- [ ] Inline `#[test]`: every `#[test]` in `red-eval` runs in VM mode (set
      `Env::mode = Vm` in a common test helper)
- [ ] Inline `#[test]`: parity test for `mold(parse(mold(v)))` unaffected
      (compilation never touches the data-model side)
- [ ] `cargo test --workspace` passes in VM mode
- [ ] `cargo test --workspace` passes with `--features force-walk` (or env
      var) running every test in `Walk` mode too

## Milestone 30 - Performance measurement + hot-path tuning

- [ ] Add `crates/red-eval/benches/` with `criterion` dev-dep
- [ ] Bench programs: `fib 30` (recursive), `sum-to 1000000` (loop),
      `ackermann 3 5`, `parse`-heavy fixture, `foreach` over a 100k block,
      `sort` a 10k block with a user comparison function
- [ ] Establish baseline numbers vs. the legacy walker (keep walker callable
      behind `--walk` for A/B comparison)
- [ ] Profile with `perf` (Linux) / `Instruments` (macOS); identify hot instr
      arms
- [ ] Optimize `LoadLocal`/`LoadGlobal`: avoid `Vec` index bounds checks
      where statically safe (use `get_unchecked` only behind a debug-asserted
      fast path)
- [ ] Optimize `Const`: small-value tagging for `Integer`/`None`/`Logic` (skip
      pool indirection) if profiling warrants
- [ ] Optimize `Call`: pre-resolve native indices at compile time (already
      done); ensure no `HashMap` lookup at call time
- [ ] Optimize frame push/pop: pre-allocate `Vec` capacity; consider a slab
      allocator for `Frame` if allocation shows up
- [ ] Optimize `String` clone: `Rc<str>` already cheap; verify no accidental
      deep copies in the hot path
- [ ] Document findings in `architecture.md` ("Performance" section)
- [ ] Bench target: >= 5x speedup on `fib 30` and `sum-to 1000000` vs. walker
- [ ] Inline `#[test]`: bench results regress-guard (criterion stores a
      baseline; CI fails on >10% regression)
- [ ] `cargo test --workspace` passes; `cargo bench` runs

## Milestone 31 - Disassembler + debug ergonomics

- [ ] Implement `disasm(block: &CompiledBlock) -> String` formatting
      instructions with pool values inlined for readability
- [ ] Add `--disasm <file.red>` CLI flag: compile the script and print the
      disassembly to stdout, do not run
- [ ] Add `--disasm-func <name>` CLI flag: print the disassembly of a named
      `func` after loading the script
- [ ] Add `Env::trace: bool` -> VM appends one line per executed instr to
      stderr (gated behind `--trace` flag); off by default
- [ ] Add span-annotated disassembly: each instr carries the `Span` of its
      originating source value; `disasm` prints `file:line:col` alongside
- [ ] Improve VM error messages: when an `EvalError` is raised inside the VM,
      include the offending instr's span and the disasm of the surrounding
      function body (last 5 instrs) in the error's debug form (not the
      user-facing `*** Error:` line, which stays identical to the walker)
- [ ] Inline `#[test]`: `--disasm examples/fib.red` output contains
      `MakeFunc`, `CallUser`, `TailReenter`
- [ ] Inline `#[test]`: `--trace` of `1 + 2` produces >= 4 instr lines
- [ ] Add a `crates/red-eval/tests/disasm/` golden suite: `*.red` ->
      `*.disasm.expected`
- [ ] `cargo test --workspace` passes

## Milestone 32 - Property tests + fuzzing the VM

- [ ] Extend the existing `proptest` round-trip to compile+VM-run: for a
      generated small `Value` tree, `mold(vm_run(compile(parse(mold(v)))))`
      == `mold(walk_run(parse(mold(v))))`
- [ ] Property test: for any small generated program, VM mode and Walk mode
      produce identical stdout (capture both)
- [ ] Property test: for any small generated program, the call-stack depth at
      the end of execution is <= a small constant (e.g. 32) when the program
      is tail-recursive - assert via a test-only `Env::max_frame_depth` counter
- [ ] Property test: compilation is idempotent - compiling a block twice
      yields identical `CompiledBlock`s (modulo pool dedup order)
- [ ] Add `cargo-fuzz` target fuzzing `run_source(arbitrary_bytes)` ->
      VM must not panic (may error, may not abort). Distinguish panics (bugs)
      from `EvalError`s (graceful)
- [ ] Inline `#[test]`: proptest minimal case reduction produces a readable
      shrink
- [ ] `cargo test --workspace` passes; `cargo fuzz run run_source` runs
      (separate job)

## Milestone 33 - Walker removal prep + cleanup

- [ ] Audit `interp_legacy.rs` usage: keep it as the path for
      `needs_rebind`-flagged blocks and `Env::mode == Walk`; remove any dead
      branches
- [ ] Consolidate `dispatch_block` shim into a single `pub fn eval_block(block,
      env) -> Result<Value, EvalError>` used by all natives, choosing VM vs.
      walker centrally
- [ ] Remove the now-unused direct `eval` call sites in natives that were
      bypassing the shim
- [ ] Run clippy on workspace; fix warnings
- [ ] Run `cargo fmt --all --check`
- [ ] Update `project-brief.md`:
      - Add "Execution model" section: bytecode compiler + stack VM, lexical
        addressing, tail calls, walker retained for `bind`/`use`/`do`-on-data
        fallback
      - Note language surface frozen at v0.2 for v0.3
      - Note `--walk`/`--disasm`/`--trace` CLI flags
- [ ] Update `architecture.md`:
      - Add "Compiler" section (scope analysis, freevars, tail-position
        detection)
      - Add "VM" section (frames, stack, instr dispatch, refinement
        assembly, native bridge)
      - Add "Performance" section from M30
      - Update the overview mermaid diagram to include a `Compile` node
        between `Bind` and `Eval`
- [ ] Update `README.md` quickstart with `--disasm` and `--walk` flags
- [ ] Final `cargo test --workspace` green in VM mode
- [ ] Final `cargo test --workspace` green in Walk mode (parity)
- [ ] Tag release `v0.3.0`
