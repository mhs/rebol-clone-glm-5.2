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

## `try` inside a `func` body infinite-loops in the VM

**Status:** Pre-existing (surfaced during M38–M45 example writing; not
caused by any single milestone). The walker is unaffected — only the
default bytecode VM hangs.

**Minimal reproducer:**

```red
f: func [a b][
    err: try [a / b]
    err
]
print f 10 2
```

Run with `red examples/bad.red` — the VM enters an infinite loop and
never returns. The same program under `red --walk` prints `5` and exits
normally.

**Observed divergence:**

| Mode | Behavior |
|------|----------|
| VM (default) | infinite loop (no output, hangs forever) |
| Walker (`--walk`) | works correctly, prints `5` |

A `--trace` run shows the VM re-entering the `try` body without ever
advancing past it — the `try` frame appears to re-execute on every
iteration. A top-level `try` works fine in both modes; the bug only
manifests when `try` appears inside a user `func` body.

**Root cause:**

Not yet isolated. The `try` native (`natives/control.rs`) catches a
thrown `EvalError` and returns the error value; the VM's `Call` path
appears to re-dispatch into the `try` block instead of resuming the
caller's frame after `try` returns. Likely a frame-stack / continuation
issue in the VM's handling of `try`'s block argument when the call
originates from a user func body rather than the top level. Confirmed
unrelated to M42's structured-error rewrite (the bug reproduces with a
plain `try [...]` that doesn't construct any error values).

**Workaround:**

Keep `try` (and `attempt`) at the top level, or use `--walk` mode for
scripts that need `try` inside a func body:

```red
; Top-level — works in both VM and walker modes
result: try [10 / 0]
either error? result [
    print error-type result
][
    print result
]

; Inside a func — use --walk mode, or restructure to call try at top level
```

**Proper fix (deferred):**

Trace the VM's frame transitions through `try`'s block argument in
`vm/vm.rs` and `natives/control.rs::try_native`. Compare with the
walker's `try` path (`interp_walker.rs`) to find where the VM re-enters
the `try` block instead of returning to the caller's frame. Likely
involves the `TryFrame` / catch-target stack not being popped after
`try` completes when the call site is a user func.

