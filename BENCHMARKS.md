# Benchmarks

Performance baseline for the Red-clone interpreter. Established in
Milestone Pre-22 (`plan3.md`) as the reference the v0.3 VM milestones
(M22–M30) will be compared against.

## Baseline (v0.2.0 tree-walker)

- **Host:** Apple M4, 10 cores, 24 GB RAM, macOS
- **Rust toolchain:** `rustc 1.94.0 (4a4ef493e 2026-03-02)`
- **Command:** `cargo bench --bench eval` (criterion default: 3s warmup,
  100 samples, ~5s collection window per bench)
- **Build profile:** `--release` (criterion builds benches in release by
  default)
- **Date:** 2026-06-25

> The v0.3 VM results in M30 land under a "v0.3.0 VM" header below for
> direct comparison.

### End-to-end fixtures (`fixtures/*`)

Each fixture runs the full `lex → parse → bind → eval` pipeline via
`run_source_with_output` (stdout discarded). Sources live in
`crates/red-eval/benches/programs/`. The `mean` column is the criterion
point estimate; the bracketed range is `[lower, upper]` (p95 confidence).

| Fixture              | What it stresses                            | mean       | [lower, upper]            |
|----------------------|---------------------------------------------|------------|----------------------------|
| `fib`                | `fib 30` naive recursion (call + recursion) | 2.0769 s   | [2.0699 s, 2.0843 s]      |
| `sum_loop`           | `repeat` accumulator to 1,000,000           | 208.62 ms  | [207.08 ms, 210.43 ms]    |
| `sum_while`          | same loop via `while`                       | 503.85 ms  | [499.86 ms, 509.45 ms]    |
| `ackermann`          | `ackermann 3 5` (deep recursion, worst case)| 43.221 ms  | [42.924 ms, 43.742 ms]    |
| `ackermann_small`    | `ackermann 2 5` (smaller, CI-friendly)      | 178.30 µs  | [176.98 µs, 179.72 µs]    |
| `foreach_block`      | `foreach` over a 100k block                 | 46.678 ms  | [46.338 ms, 47.050 ms]    |
| `block_build`        | `append` into a block 10k times             | 2.7026 ms  | [2.6803 ms, 2.7403 ms]    |
| `parse_heavy`        | `parse` over a 10k-char string              | 12.394 ms  | [12.254 ms, 12.561 ms]    |
| `string_concat`      | `rejoin` over a 1k-iteration accumulation   | 969.06 µs  | [953.51 µs, 986.60 µs]    |
| `func_call_heavy`    | `does` invocation 1M times (pure call cost) | 200.78 ms  | [195.93 ms, 205.61 ms]    |

**Hot-spot reading (informs the v0.3 VM design):**
- `fib 30` dominates at ~2.1 s — the canonical function-call + recursion
  hot path. The VM's tail-call + lexical-addressing work (M24–M28) targets
  this.
- `func_call_heavy` at ~201 ms for 1M bare `does` calls sets the floor for
  per-call overhead (~201 ns/call). The VM aims to cut this substantially.
- `sum_while` (504 ms) is ~2.4× slower than `sum_loop` (209 ms) for the
  same work — `while`'s block re-entry per iteration is costlier than
  `repeat`'s native loop. Both are loop-overhead-bound; M28's compiled
  loop bodies target this.
- `ackermann 3 5` at 43 ms is the deep-recursion worst case (max frame
  depth well under 1000 per the `stats`-feature counter test). Not
  tail-recursive, so the VM can't help much here — included as a control.
- `parse_heavy` (12 ms) and `string_concat` (1 ms) are expected to be
  VM-neutral (parse stays on the walker in v0.3; `rejoin` is dominated by
  string `form` + concat, not eval dispatch).

### Eval-only micro-benches (`micro/*`)

Isolated `eval` cost on a pre-built `Env` (skips lex/parse/bind). The setup
closure builds a fresh `Env` per iteration via `BatchSize::SmallInput`, so
the `mean` includes that setup — the *delta* between these benches is what
matters, not the absolute floor.

| Bench               | What it measures             | mean       | [lower, upper]            |
|---------------------|------------------------------|------------|----------------------------|
| `eval_literal`      | `eval(Integer(5))`           | 14.897 µs  | [14.773 µs, 15.014 µs]    |
| `eval_word_lookup`  | `eval(word)` after `x: 5`    | 15.203 µs  | [15.043 µs, 15.362 µs]    |
| `eval_setword`      | `eval(foo: 5)`               | 15.604 µs  | [15.460 µs, 15.730 µs]    |
| `eval_call_native`  | `eval(1 + 2)` (single native)| 16.436 µs  | [16.119 µs, 16.775 µs]    |
| `eval_call_user`    | `eval(square 5)` (user func) | 18.605 µs  | [18.504 µs, 18.713 µs]    |
| `eval_paren`        | `eval((1 + 2))`              | 19.800 µs  | [18.541 µs, 21.270 µs]    |

**Reading:**
- The literal/word/setword/native floor (~15–16 µs) is dominated by the
  per-batch `Env` construction; the *incremental* cost of a word lookup
  over a literal is ~0.3 µs, a setword ~0.7 µs, a native call ~1.5 µs.
- `eval_call_user` adds ~2.2 µs over `eval_call_native` — the user-func
  frame push + context clone is the single biggest per-call cost the v0.3
  VM targets.
- `eval_paren` is `eval_call_native` + a recursive `eval` into the paren's
  series (~3 µs over literal) — the paren re-entry overhead.

## Short-sample mode (CI-like turnaround)

`cargo bench --bench eval -- --profile-time=5` runs each bench for a fixed
5 s profile (no statistical analysis, just throughput measurement). Useful
for smoke-checking that the bench compiles and runs without the full ~3 min
sample-collection cost. Run the full suite (`cargo bench --bench eval`)
before recording numbers.

## Regress guard (M30)

Milestone M30 will enforce that the v0.3 VM is **no slower** than this
baseline by more than **10%** on any fixture. criterion's
`--save-baseline` + [`critcmp`](https://github.com/BurntSushi/critcmp)
workflow is the intended comparison tool (see
`crates/red-eval/benches/README.md`).

## v0.3.0 VM

(M30 will fill this in with the post-VM numbers for direct comparison.)
