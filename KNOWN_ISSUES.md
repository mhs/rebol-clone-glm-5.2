# Known Issues

Pre-existing issues that surface during testing but are not caused by any
single milestone's changes. Each entry identifies the failing test, the
minimal reproducer, the root cause (if known), and the recommended
workaround.

## `vm_walk_stdout_parity_for_programs` — RESOLVED

**Test:** `crates/red-eval/tests/property.rs:vm_walk_stdout_parity_for_programs`

**Status:** Fixed. The proptest, the pinned seed in
`property.proptest-regressions` (which pins the shrunk case
`if 0 + if [0] a: 0`), and the explicit regression cases in
`tests/parity.rs::if_either_arg_fetch_parity` all pass.

**What the divergence was:** on `if 0 - if [0] a: 0` the VM reported
`expected block!, found set-word!` (col 15) while the walker reported
`expected block!, found integer!` (col 18). The original diagnosis blamed
`collect_args`/`eval_expression` — that was wrong. Those two are aligned;
the actual cause was that `if`/`either` **bypass** the VM's generic
argument collection via the special-case in `compile_word`
(`vm/compiler.rs`), and `compile_if`/`compile_either`'s fallback branches
pushed the *raw next token* as a `Const` instead of compiling it as a full
expression — so `a: 0` was never evaluated (no `SetGlobal` side effect)
and the type check fired on the unevaluated set-word one token early.

**The fix** (in `vm/compiler.rs`): the `if`/`either` fallbacks now compile
their non-literal-Block branch args via `compile_expr` and dispatch
generically (`Call(if, 2)` / `Call(either, 3)`), matching the walker's
`collect_call_args`. The fast paths additionally refuse to inline a
literal branch block when the *next* token is an infix native — the
walker's argument fetch lets that native steal the block as its left
operand (`if true [3] + 2` → `add([3], 2)` → type error), which inlining
would have turned into arithmetic on `if`'s result. The duplicated
argument-collection tables (`uneval_first` names, `module`/`loop` arity
overrides, native-word predicates) were consolidated into
`natives/tables.rs` so the VM and walker can't drift apart again.

## `values_equal` has no `(Block, Block)` arm — block! records never dedup in `query/distinct`

**Test:** discovered by `query::tests::query_distinct_without_projection`
(coverage push, Feature C).

**Status:** Open. `natives/compare.rs::values_equal` compares
Integer/Float/Decimal/Object/Map/Hash/Vector/… and word-family pairs, but
falls through to `_ => false` for two `block!` values — so two structurally
identical blocks compare unequal.

**Impact:** `query [from <block-records> distinct]` returns duplicates
(object! records dedup correctly via the Object arm; only key/value-pair
block! records are affected). Any other consumer of `values_equal` on
blocks (e.g. `find` on nested blocks, if it routes through here) is
similarly affected.

**Proper fix:** add a `(Value::Block, Value::Block)` arm that element-wise
recurses (mirroring the existing Vector arm in `compare.rs` — same
`zip`/`all` shape, over `data.iter().skip(index)`). The question to settle
when fixing: should `Rc::ptr_eq` on the shared series be a fast path, and
should the cursor (`index`) participate? For `distinct` semantics it must
NOT (a positioned sub-block view equals the same elements from position 0).

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
