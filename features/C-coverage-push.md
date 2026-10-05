# Feature C: Coverage Push (weak files → 80%+)

**Status:** DONE (commits 4f93d6e, 65422a3, bd79f9e) — 2026-10-05

**Result vs. targets:**

| File | Before | After | Target |
|---|---|---|---|
| query.rs | 69% | **88.9%** | ~85% ✓ |
| json.rs | 72% | **86.6%** | ~85% ✓ |
| parse.rs | 72% | **85.2%** | — ✓ |
| object.rs | 68% | **80.8%** | — ✓ |
| vm/vm.rs | 68% | **70.9%** | high 70s ✗ (partial) |
| interp_walker.rs | 62% | **65.1%** | high 70s ✗ (partial) |
| **workspace total** | 81.1% | **82.8%** | ≥83% ~ (82.8) |

Remaining vm.rs/interp_walker.rs gap: deep VM dispatch arms
(`run_loop_reduce` nested returns, `call_native` heap fallback,
`build_closure_def` ancestor-frame captures) and the walker's
`resolve_word`/`write_setword` `Binding::Lexical` arms — reachable
mainly via VM-invoked frames that fall back to the walker; follow-up
if needed.

**Bugs found by this push** (see KNOWN_ISSUES.md):
1. `values_equal` has no `(Block, Block)` arm — `query/distinct` never
   dedups block records (entry added).
2. Refinement args collected after positionals — Red's
   `parse/part input limit rules` order mis-collects (entry added).
3. **Fixed** (65422a3): `collect into 'w` always failed — the binding
   pass allocated a slot for the `into` keyword itself, so Phase 2
   bound it and `is_word(.., "into")` no longer recognized the form.
4. Fixed the 3 no-assert placeholder walker tests.

**Baseline:** 81.1% lines workspace-wide; `just coverage` (RUST_MIN_STACK=
33554432). Weak files in `crates/red-eval/src/`: `vm/vm.rs` 68%,
`interp_walker.rs` 62%, `query.rs` 69%, `object.rs` 68%, `json.rs` 72%,
`parse.rs` 72%.
**Effort:** S per file (cheapest-first ordering below); M total.
**Depends on:** Nothing. **Lands first** — de-risks Tasks A and B by widening
the safety net before the `Value` enum and call paths are touched.

## Conventions (from the research)

- **Golden fixtures are the cheapest tests**: a pair in
  `tests/programs/` (+`.expected`) or `tests/programs_errors/` auto-joins
  the parity suite (`tests/parity.rs`) — VM and Walk coverage for free.
  Reserve inline `#[cfg(test)] mod tests` for Rust-only arms
  (VmInvariant, hand-built blocks) and direct-value assertions.
- **Inline test boilerplate**: `load_source` → `Context::new()` +
  `install_constants` → `bind_pass` → `Env::new_with_output` →
  `register_natives` → eval. Walker tests MUST set
  `env.mode = EvalMode::Walk` (`run_walk` helper); VM tests
  `EvalMode::Vm` (`compile_for_vm`).
- **Avoid the no-assert anti-pattern** — several existing walker tests do
  `let _ = v;` and assert nothing. Every new test asserts concrete output.
- **Instr has no PartialEq** — compare via `mold_to_string`/`matches!`/
  Debug strings (`compilation_is_idempotent` precedent).
- **Deep walker recursion** (no TCO) — keep new Walk-mode tests shallow, or
  use the 256 MiB `run_on_big_stack` pattern (`tests/bench_fixtures.rs:187`).
- Runs: default suite, `--features force-walk` (parity gate), and coverage
  with `RUST_MIN_STACK=33554432`.

## 1. Table tests: `query.rs` + `json.rs` — S each, biggest line wins

**query.rs** (69%, error arms + fallbacks almost entirely uncovered):

- [ ] Error-arm table (~12 pairs, one `#[test]`): `query [from nosuchword]`,
  `[from 5]`, `[from people where 5]`, `[from people order [5]]`,
  `[order age]`, `[limit "x"]`, `[limit -1]`, `[offset -1]`,
  `[select [name 5]]`, `[select 5]`, `[from people bogus]`, `query [5]` —
  each asserts the specific message substring.
- [ ] WHERE-block error propagation: `query [from people where [nosuchword]]`
  → UnboundWord from `filter_rows`'s dispatch.
- [ ] Record type error: `data: [1 2 3] query [from data where [x > 0]]` →
  "record must be object! or block!".
- [ ] String-field ordering (`compare_values` form fallback): order by
  `[city]`.
- [ ] `distinct` without projection, on duplicate full records.
- [ ] Add `tests/programs/query_basic.red` + `.expected` (parity rides free).

**json.rs** (72%):

- [ ] Exotic-encode table: `$10.50`, `$5.25:EUR` (non-USD),
  negative money, date-only / date+time / +zone (`Z` vs `+HH:MM`),
  `50%`, `#"A"`, `<b>` tag, issue, `%file.txt`, email, url,
  `3h30m` (duration→seconds), decimal, `#{0102}` (String8→base64),
  word-keyed map (`map_key_to_json_string` Sym/Int/Char/Bool/None arms),
  paren→array, path/refinement→string.
- [ ] `encode_float` NaN/Inf error; `encode_string` control-char
  `\u00XX` arm.
- [ ] Surrogate pairs: `{"\uD83D\uDE00"}` → 😀; all 4 error cases
  (lone high, lone low, invalid pair order, non-hex digit).
- [ ] Decoder error table: `[1 2}`, `{a:1}`, `{"a" 1}`, `"\q"`, `tru`, `""`,
  invalid/truncated UTF-8.
- [ ] i64-overflow → float promotion (`99999999999999999999` molds as float).
- [ ] 300-deep nesting (built in Rust) → "JSON nesting depth exceeded".
- [ ] `load-json` Arity/TypeError arms; binary (`String8`) input arm.
- [ ] Add `tests/programs/json_codec.red` round-trip fixture.

## 2. `vm/vm.rs` (68%) — correctness-critical — M

- [ ] Param type-check parity: `f: func [x [integer!]][x + 1] f "a"` →
  "type error: arg 1 expected integer!, got string!" — locks the
  byte-for-byte message parity promised at `vm.rs:1277`.
- [ ] Closure end-to-end: `y: 10 c: closure [x][x + y] c 5` → 15; capture
  mutation: counter closure called twice → 2 (`MakeClosure`,
  `prepare_call` closure arm, `LoadCapture`/`SetCapture` happy paths).
- [ ] Hand-built `CompiledBlock` invariants (raw `Instr::` arrays,
  compiler-test precedent): `[Halt]` → "VM reached Halt";
  `[EndRefine]` → "EndRefine without MarkRefine"; no trailing `Return` →
  "ran off instr stream"; `Const(999)` with empty pool → "pool index out of
  bounds"; `CallUser(slot, argc)` with argc > stack → Arity; slot holding an
  integer → TypeError "expected function!". ~80 otherwise-dead lines.
- [ ] `invoke_via_walker` assertion test (higher-order func-valued param —
  currently only indirectly covered via `higher_order.red`).
- [ ] `needs_rebind` top-level boundary contract: run a `[Halt]` stub
  directly → VmInvariant (documents that `dispatch_block` is the router).

## 3. `parse.rs` (72%) — untested-rule sweep — S

- [ ] `behind`: `parse "abc" ["a" "b" behind #"b" "c"]` → true.
- [ ] `reject` / `accept`: `parse "abc" ["a" reject]` → false; accept+collect.
- [ ] `opt` / `while` (only `any`/`some` are tested today):
  `parse "ab" [opt "z" "a" "b"]`, `parse "aab" [while "a" "b"]`.
- [ ] `into 'w rule` (completely untested, `parse.rs:1102-1177`): success +
  failure cases.
- [ ] `/part` refinement (M136, L465–512): string + block forms, over-long
  and truncating cases.
- [ ] `collect into 'w` append mode; `keep 'word` word-operand arm.
- [ ] Paren input arm; char `to`/`thru` (`to #"y"`).
- [ ] Error arms: `parse 5 [...]`, `parse "x" 5`.

## 4. `object.rs` (68%) — S

- [ ] Error-arm table: `make object! 5`, `words-of 5`, `in 5 'x`,
  `in o 'nosuch`, `spec-of 5`, `body-of 5`, `has 5 'x`, `extend o 5`,
  `extend 5 []`, `reflect o 5`, `reflect o 'bogus`.
- [ ] `not-same?` (never tested) + `same?` identity matrix: `not-same? o o` →
  false; distinct objects → true; `same?` Func arm (`get 'print` twice),
  Map/Hash/Vector/Bitset/Typeset arms.
- [ ] Protected-object mutation: `o: make object! [a: 1] protect o o/a: 2`
  → "set-path: object is protected" (covers `check_protected`'s object arm
  AND the walker's `write_path_slot` protect hook — two files at once).
- [ ] `bound?`/`bind?` word arms + TypeError; `context-of` → none
  (documents known limitation).
- [ ] Closure `spec-of`/`body-of` arms; `protect-system`.

## 5. `interp_walker.rs` (62%) — M

- [ ] **Fix the 3 no-assert placeholder tests** first (near-zero cost, real
  coverage): `walk_closure_with_refinements` → assert `f/ref 99` → 99 and
  bare `f` → "no"; same for module and poke placeholders.
- [ ] Typed-param + refinement-arg errors in Walk mode: `f "a"` → "type
  error: arg 1"; `g/r` (missing refinement args) → "refinement /r expects
  1 argument(s)" (`collect_call_args:2008-2017`).
- [ ] `loop`/`module` arity overrides in Walk mode (count vs block forms;
  `module 'm [..]` vs `module [..]`).
- [ ] Path error-arm table (one test, dozens of arms): `100x200/z`,
  3-byte tuple `/alpha`, date/duration/email/vector bogus fields,
  `o/nosuch`, mixed `o/items/2`, `now/year`-style call-then-select,
  literal-headed data paths (`[1 2 3]/1`).
- [ ] `Binding::Lexical` bridge: VM-invoked func whose loop body lifts a
  lexical binding to the walker (`current_vm_locals` bridge,
  `write_setword` Func arm 2382–2405); depth>0 → "not supported for
  VM-invoked funcs" error arm.
- [ ] `resolve_word`/`write_setword` error arms: depth-exceeds-stack,
  no-frame, closure bounds (M65).

## 6. Fixture additions (parity rides free)

- [ ] `tests/programs/query_basic.red` (see §1)
- [ ] `tests/programs/json_codec.red` (see §1)
- [ ] Any source-expressible case from §§2–5 that reads naturally as a
  program.

## Verification gates

After each file: `cargo test -p red-eval` + `--features force-walk`.
Target checkpoints: re-run `just coverage` after §1–2 and at the end —
expect `query.rs` and `json.rs` to ~85%+, `vm/vm.rs` and
`interp_walker.rs` into the high 70s, workspace total ≥ 83%.
