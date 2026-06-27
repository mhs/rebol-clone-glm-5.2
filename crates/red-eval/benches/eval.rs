//! Criterion benchmark harness for the Red-clone interpreter.
//!
//! v0.3 (M30) splits the bench into two parallel groups so the VM and the
//! legacy tree-walker can be A/B-compared directly:
//! 1. `fixtures/*` — VM mode (the production default since M29).
//! 2. `walk_fixtures/*` — `EvalMode::Walk` (the v0.2.0 tree-walker, kept
//!    callable behind `RunOptions { walk: true }`).
//! 3. `micro/*` / `micro_walk/*` — the same six isolated `eval`-only benches
//!    in each mode.
//!
//! Run: `cargo bench --bench eval`
//! Short sample (faster CI-like turnaround):
//!   `cargo bench --bench eval -- --profile-time=5`
//! Compare two runs: `critcmp`.
//!
//! The Pre-22 baseline numbers (v0.2.0 tree-walker) are recorded in
//! `BENCHMARKS.md`; M30 fills in the `fixtures/*` (VM) column for direct
//! comparison. `walk_fixtures/*` reproduces the v0.2.0 baseline within
//! machine noise — same code path, just re-run.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::rc::Rc;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};

use red_core::printer::mold_to_string;
use red_core::{Context, Env, Series, Span, Symbol, Value};
use red_eval::binding::bind_pass;
use red_eval::{
    eval, install_constants, register_natives, RunOptions,
};

// ---------------------------------------------------------------------------
// Fixture discovery
// ---------------------------------------------------------------------------

/// One `.red` fixture in `benches/programs/`, with its expected deterministic
/// result (the last line of stdout, parsed as a `Value` for the inline tests).
struct Fixture {
    name: String,
    src: String,
}

const FIXTURE_NAMES: &[&str] = &[
    "fib",
    "sum_loop",
    "sum_while",
    "ackermann",
    "ackermann_small",
    "foreach_block",
    "block_build",
    "parse_heavy",
    "string_concat",
    "func_call_heavy",
];

fn programs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("benches")
        .join("programs")
}

fn load_fixture(name: &str) -> Fixture {
    let path = programs_dir().join(format!("{name}.red"));
    let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    Fixture {
        name: name.to_string(),
        src,
    }
}

fn load_all_fixtures() -> Vec<Fixture> {
    FIXTURE_NAMES.iter().map(|n| load_fixture(n)).collect()
}

// ---------------------------------------------------------------------------
// Group 1: end-to-end fixture benches (lex + parse + bind + eval)
// ---------------------------------------------------------------------------

/// Run a fixture end-to-end in the given mode, black-boxing the result.
/// `fib 30` / `ackermann 3 5` overflow the default 8 MiB Rust stack in debug
/// builds under the tree-walker (and even the VM for very deep ackermann), so
/// those two fixtures run on a 256 MiB-stack thread. `Value` is `!Send`, so
/// the thread black-boxes the mold'd length and returns a sentry `usize`.
fn run_fixture(src: String, walk: bool) -> usize {
    let opts = RunOptions {
        walk,
        ..RunOptions::default()
    };
    if walk {
        // Tree-walker: `fib 30` and `ackermann 3 5` overflow the default
        // Rust stack in debug builds. Run on a 256 MiB-stack thread.
        if src.contains("fib 30") || src.contains("ackermann 3 5") {
            return std::thread::Builder::new()
                .stack_size(256 * 1024 * 1024)
                .spawn(move || {
                    let sink: Box<dyn Write> = Box::new(io::sink());
                    let v = red_eval::run_source_with_exit_opts(&src, sink, &opts)
                        .expect("fixture failed")
                        .0;
                    black_box(mold_to_string(&v).len())
                })
                .expect("spawn failed")
                .join()
                .expect("thread panicked");
        }
    }
    let sink: Box<dyn Write> = Box::new(io::sink());
    let v = red_eval::run_source_with_exit_opts(&src, sink, &opts)
        .expect("fixture failed")
        .0;
    black_box(mold_to_string(&v).len())
}

fn bench_fixtures(c: &mut Criterion) {
    let fixtures = load_all_fixtures();
    let mut group = c.benchmark_group("fixtures");
    for f in &fixtures {
        let src = f.src.clone();
        let name = f.name.clone();
        group.bench_function(name.as_str(), |b| {
            b.iter_batched(
                || src.clone(),
                |src| {
                    let sentry = run_fixture(src, /*walk*/ false);
                    black_box(sentry);
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// M30 A/B comparison group: same fixtures, but in `EvalMode::Walk` (the
/// v0.2.0 tree-walker). Runs alongside `fixtures/*` so `critcmp` (or a
/// eyeball comparison of the two tables in `BENCHMARKS.md`) shows the
/// VM-vs-walker delta directly.
fn bench_walk_fixtures(c: &mut Criterion) {
    let fixtures = load_all_fixtures();
    let mut group = c.benchmark_group("walk_fixtures");
    for f in &fixtures {
        let src = f.src.clone();
        let name = f.name.clone();
        group.bench_function(name.as_str(), |b| {
            b.iter_batched(
                || src.clone(),
                |src| {
                    let sentry = run_fixture(src, /*walk*/ true);
                    black_box(sentry);
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Group 2: eval-only micro-benches (pre-built Env, no lex/parse/bind)
// ---------------------------------------------------------------------------

/// Build a fresh `Env` with natives + constants installed and a body series
/// already bound. Returns `(body_series, env)` so the bench can call `eval`
/// directly on the body block. `walk` pins `env.mode` so the micro benches
/// can measure either evaluator.
fn build_env(body_src: &str, walk: bool) -> (Series, Env) {
    let body = red_core::load_source(body_src).expect("load_source failed in bench setup");
    let ctx = Context::new();
    install_constants(&ctx);
    let ctx_rc = bind_pass(&body, ctx);
    let mut env = Env::new_with_output(ctx_rc, Box::new(io::sink()));
    register_natives(&mut env);
    env.mode = if walk {
        red_core::EvalMode::Walk
    } else {
        red_core::EvalMode::Vm
    };
    (body, env)
}

/// Wrap a body series as a `Value::Block` for `eval`.
fn as_block(body: Series) -> Value {
    Value::Block {
        series: body,
        span: Span::new(0, 0),
    }
}

fn bench_micro(c: &mut Criterion) {
    bench_micro_mode(c, "micro", /*walk*/ false);
}

/// M30 A/B comparison: same six micro-benches in `EvalMode::Walk` so the
/// VM-vs-walker delta is visible at the micro level (not just end-to-end).
fn bench_micro_walk(c: &mut Criterion) {
    bench_micro_mode(c, "micro_walk", /*walk*/ true);
}

fn bench_micro_mode(c: &mut Criterion, group_name: &str, walk: bool) {
    let mut group = c.benchmark_group(group_name);

    // eval_literal: `eval(Integer(5))`
    group.bench_function("eval_literal", |b| {
        b.iter_batched(
            || {
                let body = Series::new(vec![Value::integer(5)]);
                let (bound, env) = build_env("", walk);
                let _ = bound;
                (as_block(body), env)
            },
            |(block, mut env)| {
                let v = eval(&block, &mut env).expect("eval failed");
                black_box(v);
            },
            BatchSize::SmallInput,
        )
    });

    // eval_word_lookup: `x: 5 x` — but we only eval the `x` lookup.
    // Build the env with `x: 5` set, then eval a single-word body.
    group.bench_function("eval_word_lookup", |b| {
        b.iter_batched(
            || {
                // Set up `x: 5` in the user context.
                let (body, mut env) = build_env("x: 5", walk);
                let _ = eval(&as_block(body), &mut env).expect("setup eval failed");
                // Now build a body that's just `x` (already bound to user ctx).
                let x_body = Series::new(vec![Value::Word {
                    sym: Symbol::new("x"),
                    binding: red_core::Binding::Local(
                        Rc::clone(&env.user_ctx),
                        env.user_ctx.slot_index(Symbol::new("x")),
                    ),
                    span: Span::new(0, 0),
                }]);
                (as_block(x_body), env)
            },
            |(block, mut env)| {
                let v = eval(&block, &mut env).expect("eval failed");
                black_box(v);
            },
            BatchSize::SmallInput,
        )
    });

    // eval_setword: `foo: 5` (single set-word + literal)
    group.bench_function("eval_setword", |b| {
        b.iter_batched(
            || {
                let (body, env) = build_env("foo: 5", walk);
                (as_block(body), env)
            },
            |(block, mut env)| {
                let v = eval(&block, &mut env).expect("eval failed");
                black_box(v);
            },
            BatchSize::SmallInput,
        )
    });

    // eval_call_native: `1 + 2` (single native call)
    group.bench_function("eval_call_native", |b| {
        b.iter_batched(
            || {
                let (body, env) = build_env("1 + 2", walk);
                (as_block(body), env)
            },
            |(block, mut env)| {
                let v = eval(&block, &mut env).expect("eval failed");
                black_box(v);
            },
            BatchSize::SmallInput,
        )
    });

    // eval_call_user: `square: func [x][x * x] square 5`
    group.bench_function("eval_call_user", |b| {
        b.iter_batched(
            || {
                let (body, env) = build_env("square: func [x][x * x] square 5", walk);
                (as_block(body), env)
            },
            |(block, mut env)| {
                let v = eval(&block, &mut env).expect("eval failed");
                black_box(v);
            },
            BatchSize::SmallInput,
        )
    });

    // eval_paren: `(1 + 2)`
    group.bench_function("eval_paren", |b| {
        b.iter_batched(
            || {
                let (body, env) = build_env("(1 + 2)", walk);
                (as_block(body), env)
            },
            |(block, mut env)| {
                let v = eval(&block, &mut env).expect("eval failed");
                black_box(v);
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_fixtures,
    bench_walk_fixtures,
    bench_micro,
    bench_micro_walk,
);
criterion_main!(benches);
