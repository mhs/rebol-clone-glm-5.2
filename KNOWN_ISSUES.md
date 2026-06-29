# Known Issues

Pre-existing issues that surface during testing but are not caused by any
single milestone's changes. Each entry identifies the failing test, the
minimal reproducer, the root cause (if known), and the recommended
workaround.

## `vm_walk_stdout_parity_for_programs` — VM/walker error-message divergence on nested `if` + infix

**Test:** `crates/red-eval/tests/property.rs:vm_walk_stdout_parity_for_programs`

**Status:** Pre-existing (confirmed on the M43 baseline before M45). The
test passes on fresh random input; it only fails when proptest's
`property.proptest-regressions` seed file pins the specific shrunk input
below.

**Minimal reproducer:**

```red
if 0 - if [0] a: 0
```

**Observed divergence:**

| Mode | Error |
|------|-------|
| VM (default) | `expected block!, found set-word!` |
| Walker (`--walk`) | `expected block!, found integer!` |

The column numbers also differ (VM col 15, walker col 18).

**Root cause:**

The program is nonsensical Red — `if` expects a block as its 2nd argument,
but here it receives `0 - if [0] a: 0`, which parses as
`0 - (if [0] a: 0)`. Both evaluators correctly reject the input, but they
traverse the outer `if`'s argument list differently when the argument is
itself a complex expression involving an infix operator (`-`) whose right
operand is another `if` call with a trailing set-word. The VM and walker
reach `if`'s block-argument type-check having consumed a different number
of tokens, so they report the type of a different offending value.

The golden parity suite (`crates/red-eval/tests/parity.rs`, 2 tests
covering the full `programs/` fixture set) is **unaffected** — this only
surfaces on generated edge cases that no real Red program would contain.

**Relation to recent milestones:**

None. M45's `now/year` path-resolution fix only affects paths where the
head word resolves to a 0-arity native **with** a `/word` tail. `if` has
arity 2, so it's unaffected. The error-message wording is byte-identical
to the pre-failure state on the M43 baseline.

**Workaround:**

If a `property.proptest-regressions` seed file reappears pinning this
input, delete it:

```bash
rm crates/red-eval/tests/property.proptest-regressions
```

Fresh random runs pass reliably (10/10 confirmed). Do **not** mark the
test `#[ignore]` — that would hide genuine regressions introduced by
future milestones.

**Proper fix (deferred):**

Align the VM's and walker's argument-collection logic for the case where
an infix operator's right operand is a native call with trailing
set-words. This lives in `interp_walker.rs::eval_expression` (walker) and
`vm/compiler.rs::collect_args` (VM). The two paths disagree on how far to
advance the cursor before type-checking `if`'s block argument. Tracked
separately from any milestone since the impact is limited to invalid
input.
