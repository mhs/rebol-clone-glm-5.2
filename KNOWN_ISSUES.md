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

## Refinement args are collected after positional args — Red's leading-refinement call order mis-collects

**Test:** discovered by `parse::tests::parse_part` (coverage push, Feature C §3).

**Status:** Open (design limitation of both collectors). The walker's
`collect_call_args` and the VM's `collect_args` gather **positional args
first, then each refinement's args at the tail** (in refinement-spec
order). In Red, a leading path refinement's args are consumed immediately
after the path — so Red's `parse/part "abcde" 3 [rules]` order fails here:
`3` is consumed as the (second) positional (the rules arg) →
`expected block!, found integer!`.

**Impact:** any call where a refinement's arguments appear *before* later
positional args (`parse/part input limit rules`, `copy/part series n
/other …`). The repo's own fixtures use the working order
(`parse/part "abcde" ["a" "b" "c"] 3` — refinement args after all
positionals — see `tests/programs/refinements_basic.red`).

**Proper fix:** collect each active refinement's args at the point the
refinement token appears (spaced form) or immediately after the leading
path (path form), rather than deferring all refinement-arg collection to
the tail. Both `interp_walker.rs::collect_call_args` and
`vm/compiler.rs::collect_args` (plus the infix-operand path) need the
same change, and the parity suite should pin the new order.

## `string!` is not a series — series natives (`first`/`find`/`skip`/…) reject strings

**Test:** discovered while testing refinement-arg types (Feature B2);
`length?` fixed separately in this round.

**Status:** Open (design gap). `series? "abc"` ⇒ `false` — `string!` is
deliberately not routed through `extract_series` (`series.rs`), so every
series-position native (`first`/`second`/`skip`/`at`/`find`/`select`/…)
raises `expected series!, found string!`. Only `length?` (fixed via a
dedicated arm, char count) and the natives in `strings.rs` work on
strings. Real Red treats `string!` as a series (cursor over chars).

**Impact:** no string cursor navigation; the string API lives entirely in
`strings.rs` (copy/part, find, etc. where implemented).

**Proper fix:** a `Value::String` arm in `extract_series` returning a
cursor-over-chars view (likely a `Vec<Value>` of `char!`s or a byte-index
cursor — needs a design pass for multi-byte UTF-8), plus audit of
`mk_series` (a positioned view should render as the substring, not a
block). Substantial — belongs in a dedicated round.

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
