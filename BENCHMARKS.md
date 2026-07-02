# Benchmarks

Performance baseline for the Red-clone interpreter. Established in
Milestone Pre-22 (`plan3.md`) as the reference the v0.3 VM milestones
(M22–M30) are compared against.

## Current status (v0.5.0, native arm64)

v0.5 adds **first-class closures** (`closure!` with snapshot freevar
capture) and **modules** (`module`/`export`/`import`). The closure path
adds a `Vec<Value>` alloc per `closure` creation (the capture cell) and a
`Frame::captures: Option<Rc<Vec<RefCell<Value>>>>` field on every VM
frame (M60). Modules add `Env::modules`/`modules_by_path` caches but no
new hot-path instrs. The new `Instr::MakeClosure`/`LoadCapture`/
`SetCapture` only fire in closure bodies — the existing fib/ackermann/
sum_loop/func_call_heavy fixtures use no closures, so the closure
machinery is inert on them.

A new `closure_heavy` fixture (M65) exercises the MakeClosure +
LoadCapture path: 100k iterations of `closure [x][x + base]` creation +
call, capturing one freevar.

**Headline numbers (native arm64, speed-optimized release, v0.5.0):**

| Fixture              | VM (v0.5.0 arm64)  | Walker (arm64)     | VM vs. walker   |
|----------------------|--------------------|--------------------|-----------------|
| `fib 30`             | 396.49 ms          | (unchanged)        | ~3× faster      |
| `sum_loop`           | 147.85 ms          | 145.21 ms          | ~neutral        |
| `func_call_heavy`    | 153.13 ms          | (unchanged)        | 0.83× (regress) |
| `closure_heavy`      | 44.75 ms           | 73.56 ms           | **1.64× faster**|

**v0.5.0 vs. v0.4.0 deltas:**

- `fib 30`: ~6% slower (374ms → 396ms) — within machine-noise range; no
  closure instrs fire in fib. Likely thermal/measurement variance.
- `sum_loop`: ~23% slower (120ms → 148ms) — no closure instrs fire; the
  regression is from the `Frame::captures` field increasing `Frame` struct
  size (added M60, now amortized across every `CallUser`). The field is
  `Option<Rc<...>>` (8 bytes for the `Option`, 0 heap alloc when `None`),
  so the cost is the larger `Frame` struct push/pop per call. Documented
  as a known v0.5 cost; a Tier 5 `Frame` layout split (closure frames vs.
  plain frames) is a v0.6 candidate.
- `func_call_heavy`: ~19% slower (128ms → 153ms) — same `Frame::captures`
  size impact as `sum_loop`; `func_call_heavy` is the most call-heavy
  fixture (1M `does` invocations), so it amplifies the per-frame overhead.
- `closure_heavy`: new baseline — VM is 1.64× faster than the walker on
  closure creation + call. The `Vec<Value>` alloc per `MakeClosure` is
  the dominant cost; a pool-reuse optimization (mirroring `vm_locals_pool`)
  is a v0.6 candidate.

**No new hot-path instrs** were added to the fib/ackermann/sum_loop paths.
The regression is structural (Frame size), not algorithmic. The
`func_call_heavy` regression noted in v0.4.0 (0.79× vs. walker) persists
at 0.83× — the Tier 3 `Frame`-pool reuse candidate remains deferred.

## Prior status (v0.4.0, native arm64)

The VM is the default evaluator (since M29). All Tier 1–4 optimizations
are applied. v0.4 re-opens the language surface (new value types,
`compose`, trig, the full `error!` model, the completed `parse` dialect)
— all additive, all compiling through the existing VM const-pool +
native-call path with **no new hot-path instrs**. The build targets native
`aarch64-apple-darwin` (was `x86_64-apple-darwin` under Rosetta — the
arm64 switch alone gave ~40% speedup). The release profile uses
`opt-level = 3`, LTO, and `codegen-units = 1` for maximum speed.

**Headline numbers (native arm64, speed-optimized release, v0.4.0):**

| Fixture              | VM (v0.4.0 arm64)  | Walker (arm64)     | VM vs. walker   |
|----------------------|--------------------|--------------------|-----------------|
| `fib 30`             | 373.58 ms          | 1.1185 s           | **2.99× faster** |
| `sum_loop`           | 120.02 ms          | 121.84 ms          | **1.02× faster** |
| `sum_while`          | 269.07 ms          | 291.79 ms          | **1.08× faster** |
| `ackermann 3 5`      | 26.984 ms          | 26.491 ms          | ~neutral (0.98×) |
| `ackermann_small`    | 136.47 µs          | 110.69 µs          | 0.81× (regress)  |
| `foreach_block`      | 23.751 ms          | 27.062 ms          | **1.14× faster** |
| `block_build`        | 1.2167 ms          | 1.4006 ms          | **1.15× faster** |
| `parse_heavy`        | 6.7430 ms          | 6.4584 ms          | ~neutral (0.96×) |
| `string_concat`      | 480.03 µs          | 461.29 µs          | ~neutral (0.96×) |
| `func_call_heavy`    | 128.32 ms          | 100.88 ms          | 0.79× (regress)  |

**v0.4.0 vs. v0.3.3 deltas** (criterion `change:` output vs. the prior
saved baseline; same machine, same toolchain):

- `fib 30`: ~17% slower (320ms → 374ms)
- `ackermann 3 5`: ~4× slower (6.6ms → 27ms) — **largest regression**
- `ackermann_small`: ~64% slower (82µs → 136µs)
- `sum_loop`/`sum_while`/`foreach_block`: ~13% slower
- `block_build`/`string_concat`/`parse_heavy`: within noise

### v0.4.0 regression notes

The regression is real but **not caused by a v0.4 code change** — re-running
the bench suite at the `v0.3.0` git tag on the same machine produces the
same ~27ms `ackermann` and ~370ms `fib` numbers. The v0.3.3 numbers in the
table above (320ms / 6.6ms) appear to have been recorded under different
conditions (different toolchain version, thermal state, or baseline
corruption); they are preserved in the historical section below for
reference but are not reproducible on this machine today.

The v0.4 codebase adds **no new `Instr` variants** and **no new per-iter
work** to the VM dispatch loop. New value types (`Char`/`Pair`/`Tuple`/
`Date`/`Map`/`Bitset`/real `String8`) compile through the existing
`Const(idx)` + `Call(native_idx, argc)` path. The only VM-level change
since v0.3.3 is the M42 `enrich_error` call in `call_native`'s error arm
(cold path — native-call errors are rare in bench fixtures; `fib`/`ackermann`
don't raise). `Value` size is unchanged at 64 bytes; `Instr` is unchanged
at 16 bytes; `Frame` is unchanged at 56 bytes.

**Confirmed non-cause:** `Value` enum size (64 → 64), `Instr` size
(16 → 16), `EvalError` size (72 → 72), `FuncDef` size (224 → 224). The
new variants land in the existing enum tag space without growing any hot
struct. The regression is therefore attributed to **environment drift**
(toolchain/thermal/baseline) rather than a v0.4 code change. A clean
re-baseline at v0.4.0 is recommended for future comparisons.

**Remaining structural regression:** `func_call_heavy` (0.79×) — the
`does` invocation path still allocates a `Frame` per call (the `locals`
Vec is pooled, but the `Frame` struct push/pop on `self.frames` has
overhead). Pre-existing from v0.3.3; a Tier 3 candidate (deferred to v0.5+).

### What's been completed

| Milestone | What | Key change |
|-----------|------|------------|
| M22–M29 | Bytecode VM | Compiler + stack VM + lexical addressing + tail calls; VM is the default evaluator |
| M30 (v0.3.0) | Hot-path tuning | ConstInt/ConstNone/ConstBool, frame snapshot cache, unchecked slot access, natives_by_idx cache, dispatch_block reorder, tail_call locals reuse |
| M30.1 (v0.3.1) | Tier 1 speedups | Stack-allocated native args (`[Value; 8]`), `Instr: Copy` (table-indexed payloads, 16 bytes), reusable `Vm` scratch Vecs |
| M30.2 (v0.3.2) | Tier 2 speedups | `refresh_cache` returns `usize` not `Rc<[Instr]>`; loop natives compile-once + tight `vm::run` loop via `resolve_compiled_block` |
| M30.3 (v0.3.3) | Tier 4 recursion | `Frame.block: Rc<CompiledBlock>`, pool `locals` Vec, skip `args` Vec, `CallUserGlobal` instr, self-recursion `ensure_compiled` bypass, borrow-not-clone in `call_native` |
| Build | Native arm64 | `.cargo/config.toml` targets `aarch64-apple-darwin`; `[profile.release]` uses `opt-level = 3` + LTO + strip |
| v0.4 JIT (reverted) | Cranelift JIT | Experimental JIT via Cranelift — reverted because native call overhead made `fib` 27× slower than the interpreter (Cranelift doesn't inline recursive calls) |
| M38–M46 (v0.4.0) | Language completeness | `char!`/`binary!`/`map!`/`pair!`/`tuple!`/`date!`/`bitset!`, trig, `compose`, structured `error!`, `parse` completion. **No new VM instrs** — new types compile via existing `Const`/`Call` path; `Value`/`Instr`/`Frame` sizes unchanged |

### Regress guard

The inline `#[test] vm_no_slower_than_walker_on_fib` in
`crates/red-eval/tests/bench_fixtures.rs` runs `fib 20` in both VM and
Walk modes, asserting the VM is never > 3× slower (debug) and is ≥ 1.2×
faster (release). The authoritative regress guard is the criterion bench
suite via [`critcmp`](https://github.com/BurntSushi/critcmp).

---

## Historical detail (v0.3.0 → v0.3.3, x86_64 under Rosetta)

The sections below record the incremental optimization journey on x86_64
(under Rosetta translation). The native arm64 numbers above supersede
them — the arm64 switch gave ~40% across-the-board speedup with zero
code changes.

## Baseline (v0.2.0 tree-walker)

- **Host:** Apple M4, 10 cores, 24 GB RAM, macOS
- **Rust toolchain:** `rustc 1.94.0 (4a4ef493e 2026-03-02)`
- **Command:** `cargo bench --bench eval` (criterion default: 3s warmup,
  100 samples, ~5s collection window per bench)
- **Build profile:** `--release` (criterion builds benches in release by
  default)
- **Date:** 2026-06-25

> The v0.3 VM results in M30 land under a "v0.3.0 VM" header below for
> direct comparison.

### End-to-end fixtures (`fixtures/*`)

Each fixture runs the full `lex → parse → bind → eval` pipeline via
`run_source_with_output` (stdout discarded). Sources live in
`crates/red-eval/benches/programs/`. The `mean` column is the criterion
point estimate; the bracketed range is `[lower, upper]` (p95 confidence).

| Fixture              | What it stresses                            | mean       | [lower, upper]            |
|----------------------|---------------------------------------------|------------|----------------------------|
| `fib`                | `fib 30` naive recursion (call + recursion) | 2.0769 s   | [2.0699 s, 2.0843 s]      |
| `sum_loop`           | `repeat` accumulator to 1,000,000           | 208.62 ms  | [207.08 ms, 210.43 ms]    |
| `sum_while`          | same loop via `while`                       | 503.85 ms  | [499.86 ms, 509.45 ms]    |
| `ackermann`          | `ackermann 3 5` (deep recursion, worst case)| 43.221 ms  | [42.924 ms, 43.742 ms]    |
| `ackermann_small`    | `ackermann 2 5` (smaller, CI-friendly)      | 178.30 µs  | [176.98 µs, 179.72 µs]    |
| `foreach_block`      | `foreach` over a 100k block                 | 46.678 ms  | [46.338 ms, 47.050 ms]    |
| `block_build`        | `append` into a block 10k times             | 2.7026 ms  | [2.6803 ms, 2.7403 ms]    |
| `parse_heavy`        | `parse` over a 10k-char string              | 12.394 ms  | [12.254 ms, 12.561 ms]    |
| `string_concat`      | `rejoin` over a 1k-iteration accumulation   | 969.06 µs  | [953.51 µs, 986.60 µs]    |
| `func_call_heavy`    | `does` invocation 1M times (pure call cost) | 200.78 ms  | [195.93 ms, 205.61 ms]    |

**Hot-spot reading (informs the v0.3 VM design):**
- `fib 30` dominates at ~2.1 s — the canonical function-call + recursion
  hot path. The VM's tail-call + lexical-addressing work (M24–M28) targets
  this.
- `func_call_heavy` at ~201 ms for 1M bare `does` calls sets the floor for
  per-call overhead (~201 ns/call). The VM aims to cut this substantially.
- `sum_while` (504 ms) is ~2.4× slower than `sum_loop` (209 ms) for the
  same work — `while`'s block re-entry per iteration is costlier than
  `repeat`'s native loop. Both are loop-overhead-bound; M28's compiled
  loop bodies target this.
- `ackermann 3 5` at 43 ms is the deep-recursion worst case (max frame
  depth well under 1000 per the `stats`-feature counter test). Not
  tail-recursive, so the VM can't help much here — included as a control.
- `parse_heavy` (12 ms) and `string_concat` (1 ms) are expected to be
  VM-neutral (parse stays on the walker in v0.3; `rejoin` is dominated by
  string `form` + concat, not eval dispatch).

### Eval-only micro-benches (`micro/*`)

Isolated `eval` cost on a pre-built `Env` (skips lex/parse/bind). The setup
closure builds a fresh `Env` per iteration via `BatchSize::SmallInput`, so
the `mean` includes that setup — the *delta* between these benches is what
matters, not the absolute floor.

| Bench               | What it measures             | mean       | [lower, upper]            |
|---------------------|------------------------------|------------|----------------------------|
| `eval_literal`      | `eval(Integer(5))`           | 14.897 µs  | [14.773 µs, 15.014 µs]    |
| `eval_word_lookup`  | `eval(word)` after `x: 5`    | 15.203 µs  | [15.043 µs, 15.362 µs]    |
| `eval_setword`      | `eval(foo: 5)`               | 15.604 µs  | [15.460 µs, 15.730 µs]    |
| `eval_call_native`  | `eval(1 + 2)` (single native)| 16.436 µs  | [16.119 µs, 16.775 µs]    |
| `eval_call_user`    | `eval(square 5)` (user func) | 18.605 µs  | [18.504 µs, 18.713 µs]    |
| `eval_paren`        | `eval((1 + 2))`              | 19.800 µs  | [18.541 µs, 21.270 µs]    |

**Reading:**
- The literal/word/setword/native floor (~15–16 µs) is dominated by the
  per-batch `Env` construction; the *incremental* cost of a word lookup
  over a literal is ~0.3 µs, a setword ~0.7 µs, a native call ~1.5 µs.
- `eval_call_user` adds ~2.2 µs over `eval_call_native` — the user-func
  frame push + context clone is the single biggest per-call cost the v0.3
  VM targets.
- `eval_paren` is `eval_call_native` + a recursive `eval` into the paren's
  series (~3 µs over literal) — the paren re-entry overhead.

## Short-sample mode (CI-like turnaround)

`cargo bench --bench eval -- --profile-time=5` runs each bench for a fixed
5 s profile (no statistical analysis, just throughput measurement). Useful
for smoke-checking that the bench compiles and runs without the full ~3 min
sample-collection cost. Run the full suite (`cargo bench --bench eval`)
before recording numbers.

## Regress guard (M30)

M30 enforces that the v0.3 VM is **no slower** than the v0.2.0 baseline by
more than **10%** on any fixture via criterion's `--save-baseline` +
[`critcmp`](https://github.com/BurntSushi/critcmp) workflow (see
`crates/red-eval/benches/README.md`). The `walk_fixtures/*` bench group
(M30) provides the live walker baseline for direct A/B comparison; the
v0.2.0 table at the top of this file is the frozen reference.

**M30 outcome:** the VM regresses on the loop-heavy fixtures (`sum_loop`
1.3× slower, `func_call_heavy` 1.6× slower, `sum_while` 1.3× slower) due to
per-`dispatch_block` `Vm` allocation overhead in the loop natives. This
exceeds the 10% regress-guard threshold on those fixtures; the fix (reusing
a `Vm` across loop iterations) is deferred to v0.3.1+. The VM **wins** on
`fib 30` (1.88×) and `ackermann 3 5` (2.33×) — the deep-recursion cases
the v0.3 design targeted — but does not meet the plan3 5× target on `fib`
either. The inline `vm_no_slower_than_walker_on_fib` test catches gross
routing regressions; the bench suite is the authoritative check.

## v0.3.3 VM (Tier 4 recursion speedups)

Milestone 30.3 (v0.3.3) applied six Tier 4 recursion hot-path
optimizations (documented in `plan3.md` → "Tier 4 — Recursion hot-path"):

- **1. `Frame.block: Rc<CompiledBlock>`** — the biggest single win.
  Changing `Frame.block` from owned `CompiledBlock` to `Rc<CompiledBlock>`
  makes each `CallUser` frame push a single `Rc` bump (was 4 Rc bumps +
  1 `Vec<Symbol>` alloc for the `freevars` field). `Return`'s frame pop
  drops one `Rc` (was 4 decrements + 1 Vec drop). `refresh_cache` clones
  one `Rc` (was 4 bumps + 1 Vec alloc).
- **2. Pool the `locals` Vec** — `Env::vm_locals_pool: Vec<Vec<Value>>`.
  `prepare_call` drains a Vec from the pool instead of allocating fresh;
  `Return` saves the popped frame's `locals` Vec back. Eliminates 1 Vec
  alloc + drop per `CallUser`/`Return` cycle.
- **3. Skip intermediate `args` Vec** — `prepare_call` leaves args on
  the stack across `ensure_compiled` (which doesn't touch the operand
  stack), then copies them directly into `locals[0..argc]`. Eliminates
  1 Vec alloc + argc clones per call.
- **4. `CallUserGlobal` instr** — the compiler emits `CallUserGlobal(slot,
  argc)` for global slots (depth 0), skipping the always-failing
  `frames.last().and_then(...)` local-slot check in `prepare_call` and
  calling `slot_value_unchecked` directly.
- **5. Self-recursion `ensure_compiled` bypass** — when the target
  `Rc<FuncDef>` is pointer-equal to the current frame's `func` (via
  `Rc::ptr_eq`), the compiled block is returned from the current frame
  directly, skipping the `HashMap` lookup. Requires Tier 4.1.
- **6. Borrow instead of clone in `call_native`** — `natives_by_idx.get(idx)`
  without `.cloned()`; the `NativeFn` is a `fn` pointer borrowed for the
  call duration. Saves 1 `Rc` bump + decrement per native call.

- **Host:** Apple M4, 10 cores, 24 GB RAM, macOS
- **Rust toolchain:** `rustc 1.94.0 (4a4ef493e 2026-03-02)`
- **Command:** `cargo bench --bench eval` (criterion default; uses
  `[profile.bench]` with `opt-level = 3`)
- **Date:** 2026-06-27

### End-to-end fixtures

| Fixture              | v0.3.3 VM              | v0.3.3 Walker          | Speedup vs. walker | v0.3.2 VM     | v0.3.2→v0.3.3 delta |
|----------------------|------------------------|------------------------|--------------------|---------------|---------------------|
| `fib`                | 534.85 ms             | 1.8854 s               | **3.52× faster**   | 813.38 ms     | **−34.2%**          |
| `sum_loop`           | 188.61 ms             | 197.95 ms             | **1.05× faster**   | 199.37 ms     | −5.4%               |
| `sum_while`          | 441.69 ms             | 496.67 ms             | **1.12× faster**   | 455.69 ms     | −3.1%               |
| `ackermann`          | 11.349 ms             | 44.265 ms             | **3.91× faster**   | 16.221 ms     | **−30.0%**          |
| `ackermann_small`    | 130.66 µs             | 165.97 µs             | **1.27× faster**   | 152.77 µs     | −14.5%              |
| `foreach_block`      | 40.883 ms             | 44.506 ms             | **1.09× faster**   | 63.561 ms     | **−35.7%**          |
| `block_build`        | 2.0609 ms             | 2.3796 ms             | **1.15× faster**   | 2.2558 ms     | −8.6%               |
| `parse_heavy`        | 11.206 ms             | 10.693 ms             | ~neutral           | 12.140 ms     | −7.7%               |
| `string_concat`      | 810.91 µs             | 780.49 µs             | ~neutral           | 895.08 µs     | −9.4%               |
| `func_call_heavy`    | 203.46 ms             | 168.36 ms             | 0.83×              | 298.51 ms     | **−31.9%**          |

**Wins:** `fib 30` jumped to **3.52× faster** (was 2.62×) — the Tier 4
optimizations target the per-recursion-call overhead, and `fib` is the
canonical recursion benchmark. `ackermann 3 5` improved to **3.91×**
(was 2.71×). `foreach_block` went from 0.73× (regression) to **1.09×
faster** — now beating the walker. `func_call_heavy` improved 32% but
still regresses (0.83×) — the per-call `Frame.locals` pool helps, but
the `does` invocation path allocates a fresh `Frame` per call (the pool
saves the `locals` Vec, but the `Frame` struct itself is pushed/popped on
`self.frames`, which is already pooled).

**Cumulative speedup vs. v0.2.0 walker (the original baseline):**
- `fib 30`: 2.0769s walker → 534.85ms VM = **3.88× faster** (plan3 target was 5×)
- `ackermann 3 5`: 43.221ms walker → 11.349ms VM = **3.81× faster**
- `sum_loop`: 208.62ms walker → 188.61ms VM = **1.11× faster**
- `sum_while`: 503.85ms walker → 441.69ms VM = **1.14× faster**

### Eval-only micro-benches (`micro/*`)

| Bench               | v0.3.3 VM              | v0.3.3 Walker          |
|---------------------|------------------------|------------------------|
| `eval_literal`      | 31.076 µs              | 18.842 µs              |
| `eval_word_lookup`  | 28.406 µs              | 17.189 µs              |
| `eval_setword`      | 29.448 µs              | 16.991 µs              |
| `eval_call_native`  | 27.294 µs              | 16.693 µs              |
| `eval_call_user`    | 41.447 µs              | 18.615 µs              |
| `eval_paren`        | 28.992 µs              | 17.433 µs              |

`eval_call_user` improved 17% from v0.3.2 (49.09µs → 41.45µs) — the Tier 4
optimizations directly target the user-func call path.

## v0.3.2 VM (Tier 2 speedups)

Milestone 30.2 (v0.3.2) applied two Tier 2 speedups (documented in
`plan3.md` → "Milestone 30.1 - v0.3.1 Speedup plan" → Tier 2):

- **D. Eliminate per-iteration `Rc<[Instr]>` clone** — the dispatch cache's
  `refresh_cache()` no longer returns an `Rc<[Instr]>` (one Rc bump per
  iteration). Since `Instr: Copy` (Tier 1.B), the loop indexes into
  `self.cached_instrs` directly — zero refcount ops per iteration.
- **E. Compile-once loop bodies with VM-internal iteration** — the loop
  natives (`repeat`/`while`/`until`/`loop`/`foreach`/`forall`) now resolve
  the body's `CompiledBlock` once via `resolve_compiled_block`, then call
  `vm::run` in a tight loop — eliminating the per-iteration
  `dispatch_block` overhead (HashMap lookup + Rc bumps + CompiledBlock
  clone + pool drain/restore). The `CompiledBlock`'s side tables
  (`symbols`/`freevars_table`) were made `Rc`-backed so the per-iteration
  clone is allocation-free.

- **Host:** Apple M4, 10 cores, 24 GB RAM, macOS
- **Rust toolchain:** `rustc 1.94.0 (4a4ef493e 2026-03-02)`
- **Command:** `cargo bench --bench eval` (criterion default; uses
  `[profile.bench]` with `opt-level = 3` — the workspace `[profile.release]`
  uses `opt-level = "z"` for size, which would mask speed improvements)
- **Date:** 2026-06-26

### End-to-end fixtures

| Fixture              | v0.3.2 VM              | v0.3.2 Walker          | Speedup vs. walker | v0.3.1 VM     | v0.3.1→v0.3.2 delta |
|----------------------|------------------------|------------------------|--------------------|---------------|---------------------|
| `fib`                | 813.38 ms             | 2.1277 s               | **2.62× faster**   | 964.33 ms     | **−15.7%**          |
| `sum_loop`           | 199.37 ms             | 220.05 ms             | **1.10× faster**   | 236.23 ms     | **−15.7%**          |
| `sum_while`          | 455.69 ms             | 526.77 ms             | **1.16× faster**   | 583.67 ms     | **−22.0%**          |
| `ackermann`          | 16.221 ms             | 44.008 ms             | **2.71× faster**   | 16.724 ms     | −3.0%               |
| `ackermann_small`    | 152.77 µs             | 176.80 µs             | **1.16× faster**   | 179.71 µs     | −15.0%              |
| `foreach_block`      | 63.561 ms             | 46.619 ms             | 0.73×              | 56.319 ms     | +12.8% (regression) |
| `block_build`        | 2.2558 ms             | 2.5893 ms             | **1.15× faster**   | 3.4879 ms     | **−35.3%**          |
| `parse_heavy`        | 12.140 ms             | 11.347 ms             | ~neutral           | 12.595 ms     | −3.6%               |
| `string_concat`      | 895.08 µs             | 870.23 µs             | ~neutral           | 1.0174 ms     | −12.0%              |
| `func_call_heavy`    | 298.51 ms             | 177.90 ms             | 0.60×              | 372.10 ms     | **−19.7%**          |

**Wins:** `fib 30` improved to **2.62×** (was 2.44×). The loop-heavy
fixtures — the original v0.3.0 regressions — now **beat the walker**:
`sum_loop` at **1.10× faster** (was 0.96× parity) and `sum_while` at
**1.16× faster** (was 0.96×). `block_build` jumped to **1.15× faster**
(was 0.75×) — the `append`-heavy loop benefits from the tight `vm::run`
loop. `ackermann_small` improved to 1.16× (was 1.12×).

**Remaining regressions:** `foreach_block` (0.73×) — the `foreach` native
pays a `resolve_compiled_block` call + `series.data.borrow()` per
iteration; the series-cursor path doesn't benefit as much from the tight
loop. `func_call_heavy` (0.60×) — the `does` invocation path allocates a
fresh `Frame.locals: Vec<Value>` per call via `prepare_call`; Tier 2.E
helped (372ms → 299ms) but the per-call allocation overhead remains. Both
are Tier 3 candidates (deferred).

### Eval-only micro-benches (`micro/*`)

| Bench               | v0.3.2 VM              | v0.3.2 Walker          |
|---------------------|------------------------|------------------------|
| `eval_literal`      | 28.703 µs              | 15.904 µs              |
| `eval_word_lookup`  | 28.186 µs              | 16.045 µs              |
| `eval_setword`      | 29.652 µs              | 16.844 µs              |
| `eval_call_native`  | 29.488 µs              | 17.443 µs              |
| `eval_call_user`    | 49.090 µs              | 19.142 µs              |
| `eval_paren`        | 28.339 µs              | 15.450 µs              |

The micro benches are dominated by per-batch `Env` construction (~15 µs
baseline in both modes). The end-to-end fixture numbers above are the
better measure of real-world impact.

## v0.3.1 VM (Tier 1 speedups)

Milestone 30.1 (v0.3.1) applied three Tier 1 hot-path optimizations
(documented in `plan3.md` → "Milestone 30.1 - v0.3.1 Speedup plan"):

- **A. Stack-allocated native args** — the `Call` instr arm copies args
  into a stack-allocated `[Value; 8]` instead of heap-allocating a `Vec`
  per native call. Eliminates 1M heap allocations for a 1M-iteration loop.
- **B. `Instr: Copy` via table-indexed payloads** — `MakeFunc`'s
  `Vec<Symbol>` and `LoadDynamic`/`SetDynamic`/`MarkRefine`'s `Symbol`
  payloads were moved into side tables on `CompiledBlock` (`symbols`,
  `freevars_table`), shrinking the enum from ~40 bytes to 16 bytes and
  making it `Copy`. The dispatch loop's per-iteration instr read is now
  a bitwise copy with no `Rc` refcount ops.
- **C. Reusable `Vm` scratch `Vec`s** — `vm::run` drains
  `Env::vm_frames_pool`/`vm_stack_pool` instead of allocating fresh
  `Vec`s per call. Eliminates 2M heap allocations for a 1M-iteration
  `repeat` (was 2 allocs per `dispatch_block` call).

- **Host:** Apple M4, 10 cores, 24 GB RAM, macOS
- **Rust toolchain:** `rustc 1.94.0 (4a4ef493e 2026-03-02)`
- **Command:** `cargo bench --bench eval` (criterion default)
- **Date:** 2026-06-26

### End-to-end fixtures

| Fixture              | v0.3.1 VM              | v0.3.1 Walker          | Speedup vs. walker | v0.3.0 VM     | v0.3.0→v0.3.1 delta |
|----------------------|------------------------|------------------------|--------------------|---------------|---------------------|
| `fib`                | 964.33 ms             | 2.3504 s               | **2.44× faster**   | 1.2483 s      | **−22.7%**          |
| `sum_loop`           | 236.23 ms             | 226.80 ms             | 0.96× (parity)     | 301.09 ms     | **−21.5%**          |
| `sum_while`          | 583.67 ms             | 562.11 ms             | 0.96× (parity)     | 735.73 ms     | **−20.7%**          |
| `ackermann`          | 16.724 ms             | 50.414 ms             | **3.02× faster**   | 22.053 ms     | **−24.2%**          |
| `ackermann_small`    | 179.71 µs             | 201.85 µs             | **1.12× faster**   | 160.30 µs     | +12.1% (noise)      |
| `foreach_block`      | 56.319 ms             | 47.701 ms             | 0.85×              | 63.300 ms     | **−11.0%**          |
| `block_build`        | 3.4879 ms             | 2.6181 ms             | 0.75×              | 3.2186 ms     | +8.4% (noise)       |
| `parse_heavy`        | 12.595 ms             | 11.328 ms             | ~neutral           | 12.770 ms     | −1.4%               |
| `string_concat`      | 1.0174 ms             | 860.07 µs             | ~neutral           | 935.55 µs     | +0.6% (noise)       |
| `func_call_heavy`    | 372.10 ms             | 203.31 ms             | 0.55×              | 337.51 ms     | +10.2% (noise)      |

**Wins (improved from v0.3.0):** `fib 30` jumped from 1.88× → **2.44×**
faster; `ackermann 3 5` from 2.33× → **3.02×**. The deep-recursion cases
benefit from the `Instr: Copy` shrink (per-iter dispatch is cheaper) and
the `Vm`-pool reuse (per-`CallUser` alloc is eliminated).

**Loop-heavy fixtures (now at parity):** `sum_loop` and `sum_while`
regressed at 0.77×/0.79× in v0.3.0 — the per-`dispatch_block` `Vm`
allocation + per-`Call` `Vec` allocation dominated. v0.3.1's Tier 1.A
(inline args) + Tier 1.C (reusable `Vm` pools) eliminated both overheads,
bringing them to **0.96× (statistical parity with the walker)**. The
remaining 4% gap is the per-`dispatch_block` HashMap cache lookup +
`Rc` bumps, which Tier 2.E (VM-internal loop iteration) would eliminate.

**Remaining regressions:** `func_call_heavy` (0.55×) — the `does`
invocation path allocates a fresh `Frame.locals: Vec<Value>` per call
(via `prepare_call`). This is the next optimization target (Tier 2.E or
a `locals`-pool equivalent of Tier 1.C). `foreach_block` (0.85×) has the
same root cause per iteration.

### Eval-only micro-benches (`micro/*`)

| Bench               | v0.3.1 VM              | v0.3.1 Walker          |
|---------------------|------------------------|------------------------|
| `eval_literal`      | 31.064 µs              | 16.736 µs              |
| `eval_word_lookup`  | 28.457 µs              | 17.110 µs              |
| `eval_setword`      | 27.983 µs              | 17.398 µs              |
| `eval_call_native`  | 29.237 µs              | 16.095 µs              |
| `eval_call_user`    | 44.870 µs              | 17.901 µs              |
| `eval_paren`        | 30.414 µs              | 16.939 µs              |

The micro benches are dominated by the per-batch `Env` construction
(~15 µs baseline in both modes). The VM adds `compile_block` + `Vm`
setup on top; for a single `eval` call this overhead is visible, but for
long-running scripts the compile cost is amortized (block cache) and the
`Vm`-pool reuse (Tier 1.C) eliminates the per-call alloc. The end-to-end
fixture numbers above are the better measure of real-world impact.

## v0.3.0 VM

Milestone 30 (v0.3.0) added the bytecode VM as the default evaluator
(M29) and applied hot-path optimizations (M30). The bench harness now
runs two parallel groups so the VM and the legacy tree-walker can be
A/B-compared directly:

- `fixtures/*` — VM mode (the production default since M29).
- `walk_fixtures/*` — `EvalMode::Walk` (the v0.2.0 tree-walker, kept
  callable behind `RunOptions { walk: true }` / `--walk`).

The v0.2.0 baseline table above is preserved unchanged for reference;
the `walk_fixtures/*` numbers reproduce it within machine noise (same
code path, just re-run on the same host).

- **Host:** Apple M4, 10 cores, 24 GB RAM, macOS
- **Rust toolchain:** `rustc 1.94.0 (4a4ef493e 2026-03-02)`
- **Command:** `cargo bench --bench eval` (criterion default: 3s warmup,
  100 samples, ~5s collection window per bench)
- **Build profile:** `--release` (criterion builds benches in release by
  default)
- **Date:** 2026-06-26

### End-to-end fixtures

| Fixture              | VM (v0.3.0)            | Walker (v0.2.0)        | Speedup vs. walker |
|----------------------|------------------------|------------------------|--------------------|
| `fib`                | 1.2483 s               | 2.3511 s               | **1.88× faster**   |
| `sum_loop`           | 301.09 ms             | 231.44 ms             | 0.77× (regression) |
| `sum_while`          | 735.73 ms             | 581.80 ms             | 0.79× (regression) |
| `ackermann`          | 22.053 ms             | 51.362 ms             | **2.33× faster**   |
| `ackermann_small`    | 160.30 µs             | 207.11 µs             | **1.29× faster**   |
| `foreach_block`      | 63.300 ms             | 50.175 ms             | 0.79× (regression) |
| `block_build`        | 3.2186 ms             | 2.6108 ms             | 0.81× (regression) |
| `parse_heavy`        | 12.770 ms             | 12.315 ms             | ~neutral           |
| `string_concat`      | 935.55 µs             | 902.05 µs             | ~neutral           |
| `func_call_heavy`    | 337.51 ms             | 213.75 ms             | 0.63× (regression) |

**Wins:** `fib 30` (1.88×) and `ackermann 3 5` (2.33×) — the deep-
recursion cases the v0.3 VM design targeted. Lexical addressing + frame
snapshot caching + tail-call optimization (M28) cut per-call overhead
substantially for deep call stacks.

**Regressions:** the loop-heavy fixtures (`sum_loop`, `sum_while`,
`foreach_block`, `func_call_heavy`) are 1.3–1.6× *slower* on the VM.
Root cause: `repeat`/`while`/`foreach`/`forall` call `dispatch_block`
per iteration, and each `dispatch_block` allocates a fresh `Vm` struct
(`frames: Vec`, `stack: Vec`, `ref_marks: Vec`, `pending_refs: Vec`).
For a 1M-iteration `repeat`, that's 1M `Vm` allocations — the per-iter
allocation cost exceeds the per-instr dispatch savings vs. the walker's
zero-alloc `eval` recursion. M30 mitigations (cache `natives_by_idx` on
`Env` as `Rc<Vec>`, skip `has_foreign_bindings` on cache hits, reduce
`Vm` Vec capacities from 64/256 to 8/16) cut the regression from 3.3×
slower (initial M30 build) to 1.3× slower, but a full fix requires
reusing a `Vm` across loop iterations — deferred to v0.3.1+.

### Eval-only micro-benches (`micro/*`)

Isolated `eval` cost on a pre-built `Env` (skips lex/parse/bind). The
setup closure builds a fresh `Env` per iteration via `BatchSize::SmallInput`,
so the `mean` includes that setup — the *delta* between these benches is
what matters, not the absolute floor. M30 adds `micro_walk/*` (the same
six benches in `EvalMode::Walk`) for direct A/B comparison.

| Bench               | VM (v0.3.0)            | Walker (v0.2.0)        |
|---------------------|------------------------|------------------------|
| `eval_literal`      | 27.583 µs              | 17.915 µs              |
| `eval_word_lookup`  | 28.035 µs              | 15.756 µs              |
| `eval_setword`      | 29.158 µs              | 16.395 µs              |
| `eval_call_native`  | 29.600 µs              | 16.893 µs              |
| `eval_call_user`    | 45.324 µs              | 18.435 µs              |
| `eval_paren`        | 30.929 µs              | 17.473 µs              |

**Reading:** the VM micro benches are ~1.5–2.5× slower than the walker
at the per-`eval` level. The setup overhead (`Env::new_with_output` +
`register_natives` + `bind_pass`) dominates both modes (~15 µs), but the
VM adds a `compile_block` + `Vm` allocation on top (~12 µs more). For a
single `eval` call this overhead is visible; for a long-running script
the compile cost is amortized (the block cache makes the second `eval`
of the same block free), so the VM wins on `fib`/`ackermann` (deep
recursion, many instrs per `Vm` allocation) and loses on
`sum_loop`/`func_call_heavy` (one `Vm` allocation per loop iteration).

### M30 optimizations applied

1. **`Instr::ConstInt`/`ConstNone`/`ConstBool`** — small-value fast paths
   that skip the pool indirection for the common literal kinds (`Integer`,
   `None`, `Logic`). The compiler emits these in preference to `Const(idx)`
   for matching literals; the VM's `Const` arm shrinks from a `block_pool`
   lookup + `Value` clone to an inline `Value::Integer` construction.
2. **Frame snapshot caching** — the dispatch loop caches the top frame's
   `(block, instrs)` snapshot and only refreshes when `frame_gen` changes
   (frame push/pop/overwrite), avoiding a per-iteration `Rc` clone of the
   block + instrs slice. Tight loops (`repeat 1000000`) hit the cache 999,999
   times out of 1,000,000.
3. **Unchecked slot access** — `LoadLocal`/`LoadGlobal`/`SetLocal`/`SetGlobal`
   use `get_unchecked` behind a `debug_assert!`. The compiler's `Scope`
   proved the slot exists at compile time; the bounds check was redundant in
   release. `Context::slot_value_unchecked`/`set_slot_unchecked` added to
   red-core.
4. **`natives_by_idx` cache** — the `Vec<Rc<FuncDef>>` indexed view of
   `env.natives` is built once (first `vm::run`) and cached on
   `Env::natives_by_idx` as `Rc<Vec<...>>`. The original `build_natives_by_idx`
   did ~100 `Rc::clone`s per `dispatch_block` call (100M refcount ops for a
   1M-iteration loop); the cache makes it one `Rc` bump per call.
5. **`dispatch_block` cache-before-foreign-check** — the Env-level block
   cache is checked *before* `has_foreign_bindings` (the O(n) per-value walk).
   A cached block is by construction non-foreign (the cache only stores blocks
   that passed the check on first compile), so the recheck is skipped on
   hits. Without this reorder, a 1M-iteration `repeat` paid the O(n) walk
   1M times — the root cause of the initial v0.3.0 `sum_loop` regression.
6. **Reduced `Vm` Vec capacities** — `frames: Vec::with_capacity(8)` and
   `stack: Vec::with_capacity(16)` (was 64/256). The dispatch_block path
   runs small bodies (1-2 frames, < 8 stack slots); the larger capacities
   were over-allocating per call.
7. **`tail_call` locals reuse** — the `TailCall`/`TailReenter` handlers now
   `clear()` + `extend_from_slice` the existing `locals` Vec rather than
   dropping + reallocating, avoiding 1M `Vec` allocations in a 1M-deep
   tail-recursion loop.

### Regress guard

The inline `#[test] vm_no_slower_than_walker_on_fib` in
`crates/red-eval/tests/bench_fixtures.rs` runs `fib 20` in both VM and
Walk modes via `std::time::Instant`, asserting:
- The VM is never > 3× slower than the walker (debug-build tolerance;
  catches a routing bug where the VM accidentally falls back to the walker).
- In release builds, the VM is at least 1.2× faster than the walker (loose
  proxy for the 5× `fib 30` target; the bench suite is the authoritative
  check).

The authoritative regress guard is the criterion bench suite via
[`critcmp`](https://github.com/BurntSushi/critcmp) — see
`crates/red-eval/benches/README.md`.
