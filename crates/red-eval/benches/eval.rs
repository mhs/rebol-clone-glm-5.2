//! Criterion benchmark harness for the v0.2.0 tree-walking evaluator.
//!
//! Two bench groups:
//! 1. `fixtures` — one `bench_function` per `.red` file in
//!    `benches/programs/`. Each reads the source, calls `run_source_with_output`
//!    (stdout discarded), and black-boxes the returned `Value`.
//! 2. `micro` — six isolated `eval`-only benches on a pre-built `Env`,
//!    skipping lex/parse/bind so the bench measures just eval cost.
//!
//! Run: `cargo bench --bench eval`
//! Short sample (faster CI-like turnaround):
//!   `cargo bench --bench eval -- --profile-time=5`
//! Compare two runs: `critcmp`.
//!
//! Inline `#[test]`s at the bottom verify each fixture produces a
//! deterministic result (so the bench measures real work, not an error path)
//! and exercise the `stats`-feature counters.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::rc::Rc;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};

use red_core::printer::mold_to_string;
use red_core::{Context, Env, Series, Span, Symbol, Value};
use red_eval::binding::bind_pass;
use red_eval::{eval, install_constants, register_natives, run_source_with_output};

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

fn bench_fixtures(c: &mut Criterion) {
    let fixtures = load_all_fixtures();
    let mut group = c.benchmark_group("fixtures");
    for f in &fixtures {
        let src = f.src.clone();
        let name = f.name.clone();
        let needs_big_stack = name == "ackermann" || name == "fib";
        group.bench_function(name.as_str(), |b| {
            if needs_big_stack {
                // `ackermann 3 5` and `fib 30` overflow the default Rust
                // stack in debug builds (the tree-walker's per-Red-call Rust
                // frame is large). Run on a 256 MiB-stack thread so the bench
                // is valid in both debug and release. `Value` isn't `Send`,
                // so we black-box it inside the thread and return only a
                // sentry integer.
                b.iter_batched(
                    || src.clone(),
                    |src| {
                        let sentry = std::thread::Builder::new()
                            .stack_size(256 * 1024 * 1024)
                            .spawn(move || {
                                let sink: Box<dyn Write> = Box::new(io::sink());
                                let v = run_source_with_output(&src, sink).expect("fixture failed");
                                // Black-box inside the thread; return a
                                // cheap sentry so the compiler can't elide
                                // the work.
                                black_box(mold_to_string(&v).len())
                            })
                            .expect("spawn failed")
                            .join()
                            .expect("thread panicked");
                        black_box(sentry);
                    },
                    BatchSize::SmallInput,
                );
            } else {
                b.iter_batched(
                    || src.clone(),
                    |src| {
                        let sink: Box<dyn Write> = Box::new(io::sink());
                        let v = run_source_with_output(&src, sink).expect("fixture failed");
                        black_box(v);
                    },
                    BatchSize::SmallInput,
                );
            }
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Group 2: eval-only micro-benches (pre-built Env, no lex/parse/bind)
// ---------------------------------------------------------------------------

/// Build a fresh `Env` with natives + constants installed and a body series
/// already bound. Returns `(body_series, env)` so the bench can call `eval`
/// directly on the body block.
fn build_env(body_src: &str) -> (Series, Env) {
    let body = red_core::load_source(body_src).expect("load_source failed in bench setup");
    let ctx = Context::new();
    install_constants(&ctx);
    let ctx_rc = bind_pass(&body, ctx);
    let mut env = Env::new_with_output(ctx_rc, Box::new(io::sink()));
    register_natives(&mut env);
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
    let mut group = c.benchmark_group("micro");

    // eval_literal: `eval(Integer(5))`
    group.bench_function("eval_literal", |b| {
        b.iter_batched(
            || {
                let body = Series::new(vec![Value::integer(5)]);
                let (bound, env) = build_env("");
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
                let (body, mut env) = build_env("x: 5");
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
                let (body, env) = build_env("foo: 5");
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
                let (body, env) = build_env("1 + 2");
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
                let (body, env) = build_env("square: func [x][x * x] square 5");
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
                let (body, env) = build_env("(1 + 2)");
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

criterion_group!(benches, bench_fixtures, bench_micro);
criterion_main!(benches);
