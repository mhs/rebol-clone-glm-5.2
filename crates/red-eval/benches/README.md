# red-eval benchmarks

Criterion-based micro-benchmarks for the interpreter. Established in
Milestone Pre-22 as the **baseline** the v0.3 VM milestones (M22–M30)
compare against. M30 added the `walk_fixtures/*` + `micro_walk/*` groups
so the VM and the legacy tree-walker can be A/B-compared directly.

## Layout

```
benches/
├── eval.rs          # criterion harness (main entry point)
├── programs/        # .red fixture sources (on-disk, inspectable)
└── README.md         # this file
```

Four bench groups live in `eval.rs` (M30 split the original two into A/B
pairs):

1. **`fixtures/*`** — VM mode (the production default since M29). One bench
   per `.red` file in `programs/`. Each runs the full
   `lex → parse → bind → eval` pipeline via `run_source_with_exit_opts`
   (stdout discarded), black-boxing the returned `Value`.

2. **`walk_fixtures/*`** — `EvalMode::Walk` (the v0.2.0 tree-walker, kept
   callable behind `RunOptions { walk: true }`). Same fixtures, same
   pipeline — the delta vs. `fixtures/*` is the VM speedup (or regression).

3. **`micro/*`** — VM mode, six isolated `eval`-only benches on a pre-built
   `Env`, skipping lex/parse/bind so the bench measures just eval cost:
   - `eval_literal` — `eval(Integer(5))`
   - `eval_word_lookup` — `eval(word)` after `x: 5`
   - `eval_setword` — `eval(foo: 5)`
   - `eval_call_native` — `eval(1 + 2)`
   - `eval_call_user` — `eval(square 5)` where `square: func [x][x * x]`
   - `eval_paren` — `eval((1 + 2))`

4. **`micro_walk/*`** — walker mode, same six benches for direct A/B.

## Fixture list

| Fixture           | What it stresses                                     | Result   |
|-------------------|-------------------------------------------------------|----------|
| `fib`             | `fib 30` naive recursion (function-call + recursion)  | `832040` |
| `sum_loop`        | `repeat` accumulator to 1,000,000 (loop overhead)    | `500000500000` |
| `sum_while`       | same loop via `while` (alt loop native)              | `500000500000` |
| `ackermann`       | `ackermann 3 5` (deep recursion, worst case for stack) | `253` |
| `ackermann_small` | `ackermann 2 5` (smaller, CI-friendly variant)       | `13` |
| `foreach_block`   | `foreach` over a 100k block (series iteration)         | `5000050000` |
| `block_build`     | `append` into a block 10k times (series mutation)     | `10000` |
| `parse_heavy`    | `parse` over a 10k-char string (parse dialect)        | `10000` |
| `string_concat`   | `rejoin` over a 1k-iteration accumulation            | `"1000-"` |
| `func_call_heavy` | `does` invocation 1M times (pure call overhead)       | `1` |

Each fixture's deterministic result is asserted by an inline `#[test]` in
`crates/red-eval/tests/bench_fixtures.rs` so the bench is provably measuring
real work, not an error path.

## Running

```sh
# Full suite (default ~10s sample per bench; takes several minutes).
cargo bench --bench eval

# Short sample (5s profile time) for faster CI-like turnaround.
cargo bench --bench eval -- --profile-time=5

# A single fixture group.
cargo bench --bench eval -- fixtures
cargo bench --bench eval -- micro

# A single fixture (criterion takes one filter substring).
cargo bench --bench eval -- fixtures/fib

# M30 A/B comparison: run both groups, then compare with critcmp.
cargo bench --bench eval -- fixtures
cargo bench --bench eval -- walk_fixtures
# (or run the full suite and diff the two groups by eye in the output)
```

### Stack-size note (debug builds)

`fib 30` and `ackermann 3 5` overflow the default 8 MiB Rust stack under
debug builds in the tree-walker (the walker's per-Red-call Rust frame is
large). The bench harness runs those two walker fixtures on a dedicated
256 MiB-stack thread so the bench is valid in both debug and release. The
inline `#[test]`s for the `ackermann` stats counters do the same.

## Comparing runs (`critcmp`)

[critcmp](https://github.com/BurntSushi/critcmp) compares two criterion
benchmark runs (e.g. before/after a change). The M30 regress guard
enforces that the v0.3 VM is **no slower** than the v0.2.0 baseline by
more than 10% on any fixture (see `BENCHMARKS.md` for the M30 outcome:
the VM wins on `fib`/`ackermann` but regresses on loop-heavy fixtures).

```sh
# Install once.
cargo install critcmp

# Save the baseline under a name, then compare after a change.
cargo bench --bench eval -- --save-baseline v0.2.0
# ... make the change ...
cargo bench --bench eval -- --save-baseline v0.3.0
critcmp v0.2.0 v0.3.0

# Or just diff the `fixtures/*` vs. `walk_fixtures/*` groups from a single
# run — they're the same fixtures in VM vs. Walk mode.
```

## The `stats` feature

`red-eval/stats` (which re-exports `red-core/stats`) adds two zero-cost-
when-off counters to `Env`, used by the v0.3 VM milestones to prove
tail-call stack bounds and correlate VM-vs-walker instr counts:

- `Env::max_frame_depth` — high-water mark of `call_stack.len()` since the
  last `reset_stats` call. Proves tail-call stack bounds (M28).
- `Env::instr_count` — count of `eval` loop iterations since the last
  `reset_stats`. Gives an operation-count metric independent of wall time
  (M30).

Both are absent from the struct layout when the feature is off (compile-time
check in `crates/red-core/src/env.rs` tests), so release builds pay zero
cost. Run the stats-counter tests with:

```sh
cargo test --workspace --features red-eval/stats --test bench_fixtures
```
