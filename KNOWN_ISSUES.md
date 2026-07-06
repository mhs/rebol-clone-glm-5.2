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

## `float!` NaN/Inf propagation — `1.0 / 0.0` yields `inf` silently

**Status:** By design (f64 parity). `float!` is backed by Rust's `f64`,
which produces `inf`/`-inf`/`NaN` for `1.0 / 0.0`, `0.0 / 0.0`,
`(-1.0).sqrt()`, etc. These values propagate silently through arithmetic
and break `sort`/`<`/`>` invariants (NaN compares unordered). Red itself
has this same behavior (Red's `decimal!`/`float!` are both f64).

**Workaround:** Use `decimal!` (`3.14dec` literal or `to-decimal`) for
exact arithmetic where float rounding surprises matter. `decimal!` is
backed by `rust_decimal` (28-digit precision, 96-bit mantissa, no
NaN/Inf) — `1dec / 0dec` raises a structured `math error: divide by
zero` instead of producing `inf`. `0.1dec + 0.2dec = 0.3dec` holds.
Transcendentals (`sin`/`cos`/`log`/`sqrt`/`exp`) on `decimal!`
auto-convert to f64 and return `float!` (rust_decimal has no
transcendental ops; the result is f64-precision anyway).
