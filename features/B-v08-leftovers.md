# Feature B: v0.8 Leftovers Batch (hash! cursors, refinement-arg types, typeset! algebra)

**Status:** Planned — not started
**Source:** Deferral comments overdue by two versions (crates at 0.10.0);
re-acknowledged open in `docs/plans/plan13-feature-parity.md:57-58` (typeset
algebra). hash! cursors and refinement-arg types were never re-triaged in any
plan doc.
**Effort:** S (typeset algebra) + M (refinement-arg types, low-risk) + M
(hash! cursored navigation)
**Depends on:** Nothing. Items 1 and 2 share `TypesetDef`/`parse_typeset_block`
but are independent of each other; item 3 is fully standalone.

## Goal

Clear the three overdue "deferred to v0.8" gaps and fix the stale bookkeeping
that references them.

## 1. typeset! algebra — S

Red's typeset op set, following the exact bitset precedent already in the
codebase (`(Bitset, Bitset)` arms in `series.rs:2464-2466, 2486-2488,
2509-2511`; `complement` Bitset arm in `math.rs:2097-2119`).

- [ ] Helpers over `HashSet<Symbol>` in `typeset.rs` (or `value.rs`):
  `union_defs`/`intersect_defs`/`difference_defs`/`complement_def` — value
  semantics (clone + build fresh `TypesetDef`; never mutate shared inputs,
  mirroring `math.rs:2110-2112`). The `RefCell` fields exist precisely for
  this (`value.rs:1834-1836`).
- [ ] Dispatch arms: `(Typeset, Typeset)` in `union`/`intersect`/`difference`
  in `series.rs` (before the `series_to_values` fallthrough); `Value::Typeset`
  arm in `complement` (`math.rs:2097`). Type errors otherwise.
- [ ] **Policy — group words under `complement`:** expand via
  `group_members` (`value.rs:1586-1619`) first, or error clearly. Prefer
  expand (matches `accepts` behavior); fall back to a clear error if any
  group word is unresolvable.
- [ ] **Policy — semantic refs:** a typeset carrying `semantic: Some(_)`
  under any algebra op errors, mirroring the no-mixing rule at
  `typeset.rs:182-194`.
- [ ] `/case` refinement on the set-op natives: ignore for typesets.
- [ ] Tests: algebra round-trips, complement vs group words, semantic-ref
  rejection, `equal?` deep on results, `same?` false (fresh `Rc`s — correct).
- [ ] Update stale comment `typeset.rs:16-18`.

## 2. Refinement-arg types in func specs — M (low-risk)

`func [x [integer!] /ref y [string!] [...]]` — today the `[string!]` block
after a refinement arg is silently skipped (`func.rs:219-226`, comments at
129–133 and 151–156).

- [ ] **Parse:** handle `Value::Block` in `Section::Refinement` in
  `extract_spec` via `parse_typeset_block` (`typeset.rs:148-222`); attach to
  the refinement's last arg word. Block with no preceding arg word → error.
- [ ] **Storage:** new `FuncDef.refinement_types: Vec<Vec<Option<Rc<TypesetDef>>>>`
  parallel to `refinements` in `red-core/src/value.rs`. Do **not** conflate
  into `param_types` (index-parallel to positional args only,
  `value.rs:172-179`). Thread through every FuncDef construction
  (`func.rs:39-49/78-87/360-369`, `vm/vm.rs:1638-1650/1696-1708`).
- [ ] **Check (walker-side is the complete surface):** add
  `check_refinement_types(fd, refs, env)` called right after
  `check_param_types` in `call_user_func` AND `call_closure_func`
  (`interp_walker.rs:2099, 2170`); for each *active* refinement, check each
  collected arg via `accepts_with_env`. Error format mirrors positional
  ("type error: …"), naming the refinement (precedent at
  `interp_walker.rs:2008-2017`).
- [ ] **VM:** no change — refined user-func calls already fall back to the
  walker (`vm/compiler.rs:708-748`); leave a pointer comment at
  `vm.rs:1276` for a future VM-side refinement dispatch milestone.
- [ ] Fix stale deferral comments: `func.rs:129-133/151-156/217-218`,
  `value.rs:172-179`, `interp_walker.rs:2040-2042`.
- [ ] Tests: accept/reject on refinement args, mixed positional+refinement,
  semantic-type refinement arg (M176), inactive refinement (args default
  `none`, no check), back-compat spec without types. Run both default and
  `--features force-walk`.
- [ ] **Gotcha:** audit existing fixtures for a literal block inside a
  refinement section used as data — its meaning would change.

## 3. hash! cursored navigation — M

`hash!` is a `series!`; Red positions a cursor within the alternating
key/value pair view over insertion order. Today all *positional* ops work
(`pick`/`poke` absolute, `first`/`last`, `length?` = pair_len) but every
*cursor* op errors with "deferred to v0.8" via `extract_series`
(`series.rs:41-84`, error at L74-77): `next/back/head/tail/at/skip/index?`,
`remove`, `take`, `change`, `forall`, `forskip`, `remove-each`, `sort`.

- [ ] Add `cursor: RefCell<usize>` to `HashDef` (`value.rs:841-968`),
  mirroring `VectorDef` (`value.rs:985-994`); reset on `clear`/`remove`;
  default 0.
- [ ] Replace the deferred error arm in `extract_series` with the
  vector-style snapshot: build the alternating key/value `Vec<Value>` from
  `key_order` + `entries`, seed `index` from the cursor, return a positioned
  **Block view** (same documented deviation as vector!,
  `plan8-missing-types.md:500-504`). Gives `next/back/head/tail/at/skip/
  index?` for free.
- [ ] Keep `pick`/`poke`/`first`/`last`/`length?`/`empty?` cursor-agnostic
  (vector precedent; preserves `hash.rs` tests 272–457). Full cursor-relative
  parity is out of scope.
- [ ] Add the genuinely missing Hash arms: `remove`/`take`/`change` via
  `remove_at`/`set_value_at` + cursor. **Also fix the stale comment at
  `series.rs:70-72`** — it claims these "handle Hash before reaching here";
  they do not.
- [ ] Decide: `forall`/`forskip`/`remove-each`/`sort` on hash — route
  through the snapshot view (preferred, consistent) or keep erroring with a
  *non-stale* message. Record the decision here when made.
- [ ] Tests: navigation round-trips, mold of views, interplay with absolute
  ops, VM/`force-walk` parity (automatic — natives are shared).
- [ ] Update header doc in `crates/red-eval/src/hash.rs` and
  `architecture.md`.

## 4. Bookkeeping

- [ ] Tick checkboxes in `docs/plans/plan8-missing-types.md` (M83/M89
  deferrals) and `plan13-feature-parity.md:57-58`.
- [ ] Sweep remaining "deferred to v0.8" comments (the three above) so none
  reference shipped work.

## Verification gates

Per item: `cargo test --workspace`, `cargo clippy --workspace --all-targets`,
`cargo test --workspace --features force-walk` (real parity surface for
item 2). Golden fixtures where source-expressible.

## Known deviations (documented, inherited)

- Snapshot views are not live (append through a view won't propagate — same
  as vector!). `same?` on a view is a fresh `Rc` → false.
- `key_order` is nominally test-only but is the series-view source; iteration
  here is insertion-ordered (Red's is unspecified).
