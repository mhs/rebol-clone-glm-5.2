# Plan 4: Code Hygiene & Structural Cleanup (v0.2.x)

Execution checklist extending the v0.2.0/v0.3 baseline in `plan2.md` / `plan3.md`.
Unlike the prior milestone-driven plans, **plan4 is a code-review remediation release**:
no new language features, no perf work. It pays down the debt surfaced by a full
review of `crates/red-core`, `red-eval`, and `red-cli`. Per `project-brief.md`,
GUI/draw/VID/reactive dialects remain permanently out of scope.

Each item below carries a severity tag and file:line references. Work top-to-bottom;
within a milestone, items are independent and can be parallelized.

## Design summary

Three themes, in priority order:

1. **Correctness hardening** — VM error paths lose span info; `unsafe`/
   `unreachable!`/`.unwrap()` sites that can panic in release on a compiler or
   registration-order bug. These are user-visible (unlocated errors) or
   silent-corruption risks.
2. **Structural cleanup** — dead stub files (`vm/frame.rs`, `vm/ir.rs`,
   `context.rs`), a misleadingly-named 1629-line "legacy" module that is actually
   the live walker fallback, a 2488-line `natives.rs` mixing unrelated concerns,
   stale module docs pointing at M22/M25 in a post-M29 tree.
3. **Test & example coverage** — `crates/red-core/src/context.rs` and `value.rs`
   (which host the `unsafe` slot accessors the VM depends on) have zero unit
   tests; 35 of 36 `examples/*.red` files are never run by CI.

Non-goal: behavior changes, perf tuning, new natives. The golden parity harness
(`tests/parity.rs`) and `cargo test --workspace` must remain green after every
milestone; the `force-walk` parity run (`cargo test --workspace --features
force-walk`) is the regression gate for any change touching `interp_legacy` or
the VM.

---

## Milestone 31 — VM error spans & panic-prone code paths

The highest-impact items. Users running `red script.red` currently get unlocated
errors (`Span::new(0, 0)`) when a type/arity/unbound error fires inside a
VM-evaluated block. This breaks the documented parity contract
(`tests/parity.rs:14`) and is the single most user-visible issue.

- [x] **Thread real spans into VM-raised errors** *(high)*
      Replace the 13 `Span::new(0, 0)` sites in
      `crates/red-eval/src/vm/vm.rs:288, 339, 502, 517, 570, 601, 640, 763, 803,
      807, 815, 916, 931` with a real span source. For `prepare_call` TypeError
      use `func_val.span_or_default()`; for `LoadDynamic`/`SetDynamic`
      UnboundWord use `self.cached_block.source_span` as the fallback (the
      per-symbol span isn't in the side table). Add a `Vm::current_span()`
      helper that returns `cached_block.source_span` for arms with no better
      source.
- [x] **Add `EvalError::Compile { kind, span }` variant** *(medium)*
      `crates/red-eval/src/vm/vm.rs:1150-1153` stringifies a `CompileError`
      into `EvalError::Native { message, .. }`, losing the structured
      `CompileErrorKind`. Add a proper variant so callers can match on it.
- [x] **Replace `unreachable!()` in compiler with recoverable errors** *(medium)*
      `crates/red-eval/src/vm/compiler.rs:772` (`Binding::Unbound`) and `:1336`
      (non-Block body). `compile_block` is `pub` and called by tests; a misroute
      should surface as a `CompileError`, not a release panic. Add
      `CompileErrorKind::UnboundWord` and `MalformedSpec`.
- [x] **Make `block_pool` OOB a real error, not silent `none`** *(medium)*
      `crates/red-eval/src/vm/vm.rs:1213-1219` returns `Value::None` on a bad
      pool index — a compiler bug becomes silent wrong values. Add
      `debug_assert!` and return `Err(EvalError::Native { .. })` (or the new
      `Compile` variant) in release.
- [x] **Convert `math.rs` infix lookups to fallible** *(medium)*
      `crates/red-eval/src/math.rs:157, 159, 174, 176, 191, 193, 208, 210` use
      `.expect()`/`.unwrap()` for `+`/`-`/`*`/`/` native lookup. A
      registration-order bug panics with an opaque message. Extract
      `fn infix_lookup(env, sym) -> Result<NativeFn, EvalError>` and use from
      all four. Standardizes the mixed `expect`/`unwrap` style in the same file.
- [x] **Document & statically assert the `unsafe` slot-access invariants** *(medium)*
      `crates/red-core/src/context.rs:86-93` (`slot_value_unchecked`) and
      `:107-116` (`set_slot_unchecked`) are sound only if the compiler's `Scope`
      analysis is correct. Add a
      `const _: () = assert!(size_of::<Value>() == size_of::<MaybeUninit<Value>>());`
      near `vm.rs:831-839` for the `from_raw_parts` cast, and document the "no
      invalid bit patterns" assumption. Optionally add a
      `#[cfg(debug_assertions)]` frame-chain walker that verifies every
      `LoadLocal`/`SetLocal` target exists.
- [x] **Guard the "ran off instr stream" fallthrough** *(low)*
      `crates/red-eval/src/vm/vm.rs:260-264` silently returns top-of-stack or
      `none` if `pc >= instrs.len()`. Add `debug_assert!(pc < instrs.len())` and
      return an `EvalError` in release.
- [x] `cargo test --workspace` green; `cargo test --workspace --features
      force-walk` green; `cargo clippy --workspace --all-targets` shows no new
      warnings.

## Milestone 32 — Dead-code & re-export cleanup

Low-risk removals that shrink the public surface and remove stale docs. Each
item is independently shippable.

- [x] **Delete `crates/red-eval/src/vm/frame.rs`** *(low)*
      One-line stub (`//! VM call-frame management (M25). Stub — real code
      lands in M25.`). `Frame` actually lives in
      `crates/red-core/src/vm_ir.rs:146`. Drop `pub mod frame;` from
      `crates/red-eval/src/vm/mod.rs:13`.
- [x] **Delete `crates/red-eval/src/vm/ir.rs`** *(low)*
      Five-line pure re-export shim. No workspace consumer imports
      `red_eval::vm::{CompiledBlock, Frame, Instr, disasm}` — internal code
      uses `red_core::vm_ir::*` directly (`vm.rs:36`, `compiler.rs:41`). Drop
      `pub mod ir;` and `pub use ir::{...};` from `crates/red-eval/src/vm/mod.rs:12,19`.
- [x] **Delete `crates/red-eval/src/context.rs`** *(low)*
      Eleven-line pure re-export shim. `lib.rs:30-32` re-exports from
      `context::{...}` while `lib.rs:44-47` re-exports other names directly
      from `red_core`. Inline the `context::{...}` re-export into a single
      `pub use red_core::{...};` block in `crates/red-eval/src/lib.rs`, drop
      `pub mod context;` from `lib.rs:15`, delete the file.
- [x] **Update stale `vm/mod.rs` module doc** *(low)*
      `crates/red-eval/src/vm/mod.rs:1-10` says "M22 (this milestone) ships
      only the type foundation… real code lands in M24/M25" and "Nothing under
      `vm/` is wired into `interp.rs` yet — the tree-walker remains the sole
      evaluator until M29." Both stale post-M29. Rewrite to reflect that the
      VM is the default evaluator and `vm/` is wired in via `interp.rs:66`.
- [x] **Derive CLI version from `CARGO_PKG_VERSION`** *(low)*
      `crates/red-cli/src/main.rs:14` hard-codes
      `const VERSION: &str = "red 0.2.0";` and `cli.rs:46` hard-codes the test
      expectation. Replace with
      `const VERSION: &str = concat!("red ", env!("CARGO_PKG_VERSION"));` and
      update the test to match. Prevents Cargo.toml/main.rs/test drift across
      the v0.2.x / v0.4 bumps.
- [x] **Rename `interp_legacy.rs` → `interp_walker.rs`** *(medium — touches imports)*
      The module name is misleading: it's the live walker fallback, not
      removable dead code. `interp.rs:30-41` re-exports its public surface;
      `natives.rs:30` and `vm.rs:40` import from it. Rename the file, update
      `lib.rs:17`, `interp.rs`, `natives.rs:30`, `vm.rs:40`, and any other
      `use crate::interp_legacy` sites. Behavior unchanged.
- [x] `cargo test --workspace` green; `cargo clippy --workspace --all-targets`
      shows no new warnings.

## Milestone 33 — Lint remediation & API surface

Address the `cargo clippy` warnings (currently ~13 lib + 3 test) and
consolidate the re-export surface.

- [x] **Fix the 4 `too_many_arguments` warnings in `vm/compiler.rs`** *(low)*
      `crates/red-eval/src/vm/compiler.rs:731, 1040, 1091, 1157`. Introduce a
      small `CompileCtx { scope: &Scope, frames: &mut Vec<Frame>, env: &Env, … }`
      struct (or `CompileEmit` builder) to group the repeated args, or apply
      `#[allow(clippy::too_many_arguments)]` with a one-line justification per
      site if a struct would harm readability.
- [x] **Apply mechanical clippy fixes** *(low)*
      - `crates/red-eval/src/interp.rs:157, 167` — replace
        `.map(|v| mold_to_string(v))` with `.map(mold_to_string)`.
      - `crates/red-eval/src/vm/compiler.rs:56` — replace `Cell::new(0)` with
        `const { Cell::new(0) }` for the `thread_local!`.
      - `crates/red-eval/src/vm/lex.rs:502` — collapse the nested `if`.
      - `crates/red-eval/src/vm/vm.rs:886` — factor the
        `(Rc<FuncDef>, Rc<CompiledBlock>, Vec<Value>)` return into a `type`
        alias.
      - The remaining `module_inception` (`pub mod vm` inside `mod vm`),
        `manual_assign_op`, `length_comparison_to_one`, `unnecessary_cast`,
        `explicit_counter_loop` nits — fix where obvious, `#[allow]` with
        justification where the lint is wrong.
- [x] **Consolidate `red-eval/src/lib.rs` re-exports** *(low)*
      `lib.rs:30-32` (via `context`) and `lib.rs:44-47` (via `red_core`)
      re-export names from `red_core` via two inconsistent paths. After M32
      deletes `context.rs`, fold everything into one
      `pub use red_core::{...};` block.
- [x] **Rename the `_pub`-suffix `pub(crate)` fns** *(low)*
      `crates/red-eval/src/vm/lex.rs:89` (`slot_index_pub`), `:106`
      (`lookup_pub`), `crates/red-eval/src/vm/compiler.rs:1456`
      (`block_source_span_pub`). The suffix is redundant with `pub(crate)`.
      Rename the private counterparts to `_inner`/`_priv` or merge the
      visibility variants.
- [x] **Fix `examples/fib.rb` → `examples/fib.red`** *(low)*
      Misleading extension; the file is Red code, not Ruby.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` passes.

## Milestone 34 — Test coverage for core types

The `unsafe` slot accessors the VM depends on have no direct tests. Add unit
tests at the source-file level (consistent with the existing
`#[cfg(test)] mod tests` pattern in `vm/compiler.rs:1602` and `vm/vm.rs:1231`).

- [ ] **Unit tests for `crates/red-core/src/context.rs`** *(medium)*
      Currently zero inline tests. Cover: `slot_index` idempotency (same word →
      same slot on re-add); `set`/`get` round-trip; `slot_value_unchecked` /
      `set_slot_unchecked` panic on OOB in `debug_assertions`; `words()`
      ordering invariant; `index_of` miss returns `None`; empty-context
      behavior.
- [ ] **Unit tests for `crates/red-core/src/value.rs`** *(medium)*
      Currently zero inline tests. Cover: `Span::is_default` (true for `0,0`,
      false otherwise); `Binding::is_lexical`/`as_lexical` round-trip;
      `FuncDef::invalidate_compiled` clears `compiled` and `needs_rebind`;
      `Value::word`/`set_word`/`integer`/`block` constructors set the expected
      variant and zero-span; `Value::span()` per variant.
- [ ] **Add CLI flag-parsing tests** *(low)*
      `crates/red-cli/src/main.rs:44-58` flag loop is untested for edge cases.
      Add integration tests in `crates/red-cli/tests/cli.rs`: unknown flag
      rejected (`red --typo file.red` → exit 2); flag after positional
      (`red file.red --walk` works); `--help`/`--version` mixed with other
      args. Also fix the wrong comment at `main.rs:44` ("anywhere before the
      script path" — code accepts flags anywhere).
- [ ] `cargo test --workspace` green; new tests pass under both default and
      `--features force-walk`.

## Milestone 35 — Examples harness

35 of 36 `examples/*.red` files are never run by CI; they could be silently
broken. Close the gap.

- [ ] **Add a test that runs every `examples/*.red`** *(medium)*
      New `crates/red-cli/tests/examples.rs` (or extend `cli.rs`) that walks
      `examples/*.red`, runs `red <file>`, and asserts exit 0 (or compares
      against a `.expected` if present). Skip `fib.rb`-style misnamed files;
      gate shell-using examples behind `--allow-shell`. This makes `examples/`
      a regression surface rather than dead weight.
- [ ] **Deduplicate `examples/hello.red` vs `tests/programs/hello.red`** *(low)*
      `crates/red-cli/tests/cli.rs:20` references `examples/hello.red`; the
      golden harness uses `tests/programs/`. Either symlink or delete the
      duplicate.
- [ ] **Add trailing newline to `examples/tree-walk.red`** *(low)*
      Last line lacks a trailing newline. POSIX text-file convention.
- [ ] `cargo test --workspace` green with the new examples test.

## Milestone 36 — Modularity (optional, larger)

Larger refactors that improve navigability but carry real diff size. Defer
until M31–M35 land; treat as opt-in.

- [ ] **Split `crates/red-eval/src/natives.rs` (2488 lines) into `natives/`** *(medium)*
      Currently mixes I/O, arithmetic, control flow, function creation, word
      ops, and the registry. Proposed split: `natives/io.rs`
      (print/prin/probe), `natives/control.rs`
      (if/either/loops/break/continue/switch/case/all/any/try/attempt/catch/throw/cause_error),
      `natives/func.rs` (function/func/does + `extract_spec`),
      `natives/words.rs` (get/set/use/bind/value?/type-of/same?/comment),
      `natives/registry.rs` (register_natives/install_constants/install_system),
      `natives/mod.rs` (re-exports). Arithmetic already lives in `math.rs` —
      keep it there.
- [ ] **Extract `run_source*`/`run_series*` from `interp_legacy.rs`** *(low)*
      `crates/red-eval/src/interp_legacy.rs:1300-1450` (the runner entry
      points) into `interp_runner.rs`. Shrinks the 1629-line walker by ~150
      lines and separates "entry points" from "eval algorithm".
- [ ] **Extract `#[cfg(test)] mod tests` from `vm/compiler.rs` and `vm/vm.rs`** *(low)*
      `compiler.rs:1602-1946` (~344 lines) and `vm.rs:1231-1613` (~380 lines)
      into `crates/red-eval/tests/compiler_tests.rs` / `tests/vm_tests.rs`.
      Shrinks the source files and lets the integration tests use the public
      API only (catches visibility bugs).
- [ ] `cargo test --workspace` green; no behavior change.

## Milestone 37 — Cosmetic consistency (deferred)

Lowest priority; bundle into a single cleanup commit at the end.

- [ ] **Standardize "no span" idiom** *(low)*
      Replace `Span::new(0, 0)` with `Span::default()` at the
      error-construction sites that remain after M31 (e.g.
      `crates/red-core/src/value.rs:368` constructors,
      `crates/red-eval/src/interp_legacy.rs:1106, 1380` synthetic Block
      wrappers, `crates/red-eval/src/interp.rs:89, 112` inline `Value::Block`
      constructions). Use `Value::block(series)` shorthand instead of inline
      `Value::Block { series, span: Span::new(0,0) }`.
- [ ] **Use `Value::block(series)` shorthand consistently** *(low)*
      `crates/red-eval/src/interp_legacy.rs:1104-1107`, `:1378-1381`,
      `crates/red-eval/src/interp.rs:89-92`, `:112-115` construct `Value::Block`
      inline. Use the existing `Value::block` constructor (`value.rs:422-428`).
- [ ] **Add a compile-time assertion for `INLINE_ARGS_CAP`** *(low)*
      `crates/red-eval/src/vm/vm.rs:50` — `const INLINE_ARGS_CAP: usize = 8;`.
      Add a `const _: () = { ... };` or a test iterating `env.natives`
      asserting no native exceeds the cap, so a future higher-arity native
      trips a build/test failure rather than silently falling back to heap
      allocation.
- [ ] **Document the `Rc::as_ptr` ABA mitigation** *(low)*
      `crates/red-core/src/env.rs:121, 129` and
      `crates/red-eval/src/interp_legacy.rs:98-107`. The M29 `source_span`
      secondary check is documented in-line but not in `architecture.md`. Add
      a paragraph so the trade-off (probabilistic collision on equal spans for
      synthetic test blocks) is discoverable.

---

## Open questions

1. **Renaming `interp_legacy` → `interp_walker` (M32):** touches ~5 import
   sites across `interp.rs`, `natives.rs`, `vm.rs`, plus `lib.rs`.
   Behavior-neutral but a visible diff. Proceed, or keep the name and just fix
   the misleading `lib.rs:1-7` doc?
2. **Splitting `natives.rs` (M36):** real navigability win but a large
   mechanical diff (2488 lines moved into 5 files + re-exports). Ship as part
   of plan4, or defer to v0.4 alongside the next feature work?
3. **Examples harness (M35):** assert exit-0 only, or require `.expected`
   golden output for each example (matches the `tests/programs/` convention)?
   Exit-0 is cheap; golden output doubles the maintenance surface.
4. **`unsafe` invariant verification (M31):** the optional
   `#[cfg(debug_assertions)]` frame-chain walker that verifies
   `LoadLocal`/`SetLocal` targets is a real debug-mode slow path (~5–10% perf
   hit in debug builds). Worth it for the safety net, or trust the existing
   `debug_assert!` in `Context::slot_value_unchecked`?
