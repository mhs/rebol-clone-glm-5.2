# Feature A: Tagged Semantic Values (close out plan18 M175 + M178)

**Status:** Planned — not started
**Source:** `docs/plans/plan18-semantic-types.md` lines 480–511 (M175) and 669–694 (M178)
**Effort:** M (largest of the three feature files; the `Value::SemanticTagged`
variant is a workspace-wide change)
**Depends on:** Nothing. Benefits from Task C (coverage push) landing first as
a safety net.

## Goal

Retire the semantic-types plan by implementing its remaining unchecked items:
tagged semantic values (a runtime value that carries its semantic type tag),
plus the M178 stragglers (count forms, Paren operands in `range`/`where`,
`days-in-month`, docs, fixtures).

Semantics per plan:

- Constructors (`rgb 255 0 0`) stay **untagged**; `make rgb! 255.0.0` returns a
  **tagged** value (`Value::SemanticTagged`).
- Tagged values behave exactly like their inner value everywhere (mold,
  arithmetic, equality, pick/poke, paths) — the tag is metadata, not a new
  type. `type_name` unwraps; `semantic-type?` is the discriminator.
- Equality: tagged==tagged requires tag match AND inner equality;
  tagged vs plain compares inners (plan L472–476).

## Scope

### 1. `days-in-month year month` native (M178 L693) — S, zero deps

- [ ] Implement arity-2 date helper (leap-year + per-month table) in the
  dates natives module; register it.
- [ ] Unit tests: leap years (`2024 2` → 29), non-leap (`2023 2` → 28),
  30/31-day months, December boundary.

### 2. `Value::SemanticTagged` variant (M175 L480–484) — one mechanical commit

- [ ] Add `Value::SemanticTagged { tag: Symbol, value: Box<Value>, span: Span }`
  in `crates/red-core/src/value.rs` (+ `Value::semantic_tagged()` ctor near
  L3065; hash arm near L2730).
- [ ] Add `unwrap_semantic(v: &Value) -> &Value` helper (plan L488's
  suggested approach) — returns inner for `SemanticTagged`, `v` otherwise.
- [ ] `type_name_for` (value.rs:1528): unwrap and recurse (tagged tuple →
  `tuple!`); no `TYPE_WORDS` entry (L483–484).

### 3. Match-site audit (M175 L485–495) — fix in dependency order

Let the compiler enumerate the sites (plan's stated mitigation, L702–705);
expected ~30–50. Order:

- [ ] `crates/red-core/src/printer.rs` — mold/form arms (default: render
  inner), L181 & L285.
- [ ] Walker `eval_prefix` (`interp_walker.rs`) — self-evaluates (clone),
  same treatment as `Value::SemanticType`.
- [ ] VM const pool (`vm/compiler.rs` literal path, sites L659/701/750/757/
  763/1441; execution `vm/vm.rs` L393/793) — intern + `Instr::Const`.
- [ ] `crates/red-core/src/concurrency.rs` — `SendValue` variant
  (mirror `SemanticType(Arc<SendSemanticType>)` at L533/922) + marshal/
  unmarshal (L678/1027 pattern) + `marshal_never_panics`/round-trip
  proptest coverage for it.
- [ ] `natives/compare.rs::values_equal` (L19) — tagged/tagged + tagged/plain
  rules above.
- [ ] Mechanical unwrap audit: arithmetic, series pick/poke, path resolution
  (`pair/x`, `tuple/r`), `copy` (L511: preserves tag).

### 4. `make <semantic>! <value>` returns tagged (M175 L496–500)

- [ ] `convert.rs:541–549` (`make_native` fallback): on validation success
  wrap as `SemanticTagged` (replaces today's untagged `spec.clone()`);
  on failure raise the M177 rich error (already exists via
  `semantic::validate_value`).

### 5. Re-semantic `semantic-type?` (M175 L501–504) — BREAKING

- [ ] Change `semantic.rs:1659–1669` from boolean to: `SemanticType(_)` →
  type name as `lit-word!`; `SemanticTagged` → tag as `lit-word!`;
  else → `none`.
- [ ] Update existing tests at `semantic.rs:1769–1775`.

**Open decision (needs user sign-off):** plan specifies the breaking change.
Alternative if rejected: keep the predicate, add a separate `tag-of` native.

### 6. `mold/tagged` refinement (M175 L505–508)

- [ ] Add 0-arg `tagged` refinement to `mold_native` registration
  (`convert.rs:1549–1560`, next to `/only`); when set and value is
  `SemanticTagged`, emit `make <tag>! <inner-mold>`. Default mold stays
  unwrapped (printer untouched).
- [ ] Doc note: constructors untagged vs `make` tagged (L509–510).

### 7. M178 compiler extensions

- [ ] Count forms (`N constraint`, `lo hi constraint`) in
  `compile_streamed`/`compile_positional` (`semantic.rs`) — L688–689.
- [ ] Paren operands in `range`/`where`, evaluated with captured components
  in scope via a synthetic func-local parse scratch context wired through
  `env` (L690–692) — uses `days-in-month` as the motivating example.
  Largest remaining compiler change; do last.

### 8. Polish (L669–677)

- [ ] 10+ golden fixtures `crates/red-eval/tests/programs/semantic-*.red`
  + `.expected` covering each shape, errors, tagged values (fixtures
  auto-join the parity suite).
- [ ] `docs/semantic-types.md` user guide mirroring the plan's examples.
- [ ] `project-brief.md` value-model section update.
- [ ] Tick all boxes in `docs/plans/plan18-semantic-types.md`.

## Verification gates

Per commit: `cargo test --workspace`, `cargo clippy --workspace --all-targets`.
Before closing: `cargo test --workspace --features force-walk` (parity gate),
`just coverage` (RUST_MIN_STACK=33554432). Golden fixtures assert byte-for-byte
stdout in both modes via `tests/parity.rs`.

## Risks

- The exhaustive `match` audit is the main risk — mitigated by the compiler
  enumerating missed arms, plus the coverage push (Task C) having landed.
- `semantic-type?` break: any fixture relying on boolean return must be
  updated in the same commit.
