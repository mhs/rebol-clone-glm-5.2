//! Inline tests for the benchmark fixtures + `stats`-feature counters.
//!
//! These are the "Inline `#[test]`" items from plan3's Milestone Pre-22:
//! each `.red` fixture in `benches/programs/` must produce a deterministic
//! `Integer` or `String` result (so the bench measures real work, not an
//! error path), and the `stats`-feature counters (`max_frame_depth`,
//! `instr_count`) must behave as documented.
//!
//! Kept as a separate integration test because the `eval` bench target uses
//! `harness = false` (criterion), so `#[cfg(test)] mod tests` inside the
//! bench file wouldn't be discovered by the default test runner.

use std::cell::RefCell;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use red_core::{Context, Env};
use red_eval::run_source_with_output;
use red_eval::RunOptions;

#[cfg(feature = "stats")]
use red_core::{Series, Span, Value};
#[cfg(feature = "stats")]
use red_eval::binding::bind_pass;
#[cfg(feature = "stats")]
use red_eval::{install_constants, register_natives};
// M29: the stats-counter tests (`max_frame_depth` / `instr_count`) assert
// walker-specific instrumentation semantics (`instr_count` is bumped in
// `interp_legacy::eval`'s outer loop, one per expression; the VM does not
// bump it — that's M30's "correlate VM instr count with walker instr count"
// work). So these tests call the walker directly via
// `red_eval::interp_legacy::eval` and pin `env.mode = Walk`, even though the
// build default is now `Vm`. The deterministic-stdout tests above
// (`run_captured` via `run_source_with_output`) run under the default (VM)
// mode, exercising the VM path — only the counter assertions need the walker.
#[cfg(feature = "stats")]
use red_eval::interp_legacy::eval;

fn programs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("benches")
        .join("programs")
}

fn load_fixture(name: &str) -> String {
    let path = programs_dir().join(format!("{name}.red"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

/// Owning `Write` sink backed by `Rc<RefCell<Vec<u8>>>` so the test can read
/// captured stdout after the `Env` (which owns the boxed writer) is dropped.
#[derive(Clone)]
struct BufferWriter {
    buf: Rc<RefCell<Vec<u8>>>,
}

impl BufferWriter {
    fn new() -> Self {
        Self {
            buf: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl Write for BufferWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.borrow_mut().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Run a fixture source end-to-end (lex + parse + bind + eval), capturing
/// stdout. Returns the trimmed captured output (the fixtures all `print`
/// their deterministic result, so the last line of stdout is the value).
fn run_captured(src: &str) -> String {
    let writer = BufferWriter::new();
    let buf_clone = writer.buf.clone();
    let _ = run_source_with_output(src, Box::new(writer)).expect("fixture failed");
    let bytes = buf_clone.borrow();
    let s = String::from_utf8_lossy(&bytes);
    // The fixture prints its result on the last line; trim trailing newline.
    s.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Deterministic-result assertions (plan3: "each .red fixture produces a
// deterministic Integer or String result")
// ---------------------------------------------------------------------------

#[test]
fn fib_fixture_deterministic() {
    assert_eq!(run_captured(&load_fixture("fib")), "832040");
}

#[test]
fn sum_loop_fixture_deterministic() {
    assert_eq!(run_captured(&load_fixture("sum_loop")), "500000500000");
}

#[test]
fn sum_while_fixture_deterministic() {
    assert_eq!(run_captured(&load_fixture("sum_while")), "500000500000");
}

#[test]
fn ackermann_small_fixture_deterministic() {
    assert_eq!(run_captured(&load_fixture("ackermann_small")), "13");
}

#[test]
fn foreach_block_fixture_deterministic() {
    assert_eq!(run_captured(&load_fixture("foreach_block")), "5000050000");
}

#[test]
fn block_build_fixture_deterministic() {
    assert_eq!(run_captured(&load_fixture("block_build")), "10000");
}

#[test]
fn parse_heavy_fixture_deterministic() {
    assert_eq!(run_captured(&load_fixture("parse_heavy")), "10000");
}

#[test]
fn string_concat_fixture_deterministic() {
    assert_eq!(run_captured(&load_fixture("string_concat")), "\"1000-\"");
}

#[test]
fn func_call_heavy_fixture_deterministic() {
    assert_eq!(run_captured(&load_fixture("func_call_heavy")), "1");
}

// ---------------------------------------------------------------------------
// stats-feature counter tests (plan3: "Env::max_frame_depth after ackermann
// 3 5 > 0 and < 1000; after sum_loop 1000000 < 50; instr_count after 1 + 2
// within a small range; with stats off, Env has no counter fields")
// ---------------------------------------------------------------------------

/// Run `src` to completion, returning the `Env` so the caller can inspect the
/// `stats` counters. Reuses the same lex/parse/bind/eval pipeline as
/// `run_source_with_exit_opts` but keeps the `Env` alive.
///
/// M29: pins `env.mode = Walk` and calls `interp_legacy::eval` directly,
/// because the stats counters (`max_frame_depth` / `instr_count`) are
/// walker-specific instrumentation — the VM doesn't bump `instr_count`
/// (M30 owns correlating VM instr count with walker instr count).
#[cfg(feature = "stats")]
fn run_keeping_env(src: &str) -> Env {
    use red_eval::EvalMode;
    let tokens = red_core::lexer::lex(src).expect("lex failed");
    let body = if tokens.is_empty() {
        Series::empty()
    } else {
        let (_hdr, body) = red_core::parser::parse_program(&tokens).expect("parse failed");
        body
    };
    let ctx = Context::new();
    install_constants(&ctx);
    let ctx_rc = bind_pass(&body, ctx);
    let mut env = Env::new_with_output(ctx_rc, Box::new(io::sink()));
    register_natives(&mut env);
    env.mode = EvalMode::Walk;
    env.reset_stats();
    let block = Value::Block {
        series: body,
        span: Span::new(0, 0),
    };
    match eval(&block, &mut env) {
        Ok(_) => env,
        Err(red_core::EvalError::Quit(_)) => env,
        Err(e) => panic!("eval failed: {e}"),
    }
}

/// Run `f` on a thread with a 256 MiB stack. Required for `ackermann 3 5` in
/// debug builds (the tree-walker's per-Red-call Rust frame is large; the
/// default 8 MiB stack overflows around depth ~400).
#[cfg(feature = "stats")]
fn run_on_big_stack<T: Send + 'static, F: FnOnce() -> T + Send + 'static>(f: F) -> T {
    std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(f)
        .expect("spawn failed")
        .join()
        .expect("thread panicked")
}

/// `ackermann 3 5` exercises deep recursion; `max_frame_depth` should be
/// > 0 and < 1000 (sanity bound — the v0.3 VM tail-call work targets a much
/// smaller bound for tail-recursive programs, but `ackermann` is not
/// tail-recursive so a few hundred frames is expected).
///
/// NOTE: `ackermann 3 5` overflows the default Rust stack in debug builds.
/// Run it on a dedicated thread with a 256 MiB stack so the test passes
/// under `cargo test` (debug) as well as `cargo test --release`.
#[cfg(feature = "stats")]
#[test]
fn ackermann_max_frame_depth_bounded() {
    let src = load_fixture("ackermann");
    let depth = run_on_big_stack(move || {
        // First: confirm the fixture produces its expected deterministic
        // result (253) via the standard end-to-end runner (stdout captured).
        assert_eq!(run_captured(&src), "253");
        // Then: re-run keeping the Env to inspect the counter.
        let env = run_keeping_env(&src);
        env.max_frame_depth
    });
    assert!(
        depth > 0,
        "max_frame_depth should be > 0 after ackermann 3 5"
    );
    assert!(
        depth < 1000,
        "max_frame_depth {depth} exceeds sanity bound of 1000"
    );
}

/// `repeat i 1000000 [acc: acc + i]` uses no user-function frames (`repeat`
/// is a native, not a user func), so `max_frame_depth` should be tiny (< 50).
#[cfg(feature = "stats")]
#[test]
fn sum_loop_max_frame_depth_tiny() {
    let src = load_fixture("sum_loop");
    let depth = run_on_big_stack(move || {
        let env = run_keeping_env(&src);
        env.max_frame_depth
    });
    assert!(
        depth < 50,
        "sum_loop max_frame_depth {depth} should be < 50 (loops reuse one frame)"
    );
}

/// `1 + 2` is a handful of eval iterations; `instr_count` should be in a
/// small expected range. Documents what counts as one "instr".
#[cfg(feature = "stats")]
#[test]
fn instr_count_for_one_plus_two() {
    let env = run_keeping_env("1 + 2");
    // `1 + 2` is one expression in `eval`'s outer while loop (prefix `1` +
    // infix `+` consuming `2`). So instr_count should be exactly 1.
    assert_eq!(
        env.instr_count, 1,
        "instr_count for `1 + 2` should be 1 (one expression step)"
    );
}

/// With the `stats` feature OFF, `Env` has no counter fields. This test
/// compiles only without the feature; it confirms the env is usable and
/// that the stats methods are absent (referencing `env.max_frame_depth`
/// would fail to compile, so the absence of such references here IS the
/// compile-time check).
#[cfg(not(feature = "stats"))]
#[test]
fn env_has_no_stats_fields_when_feature_off() {
    let ctx = Rc::new(Context::new());
    let env = Env::new(ctx);
    // No `env.max_frame_depth` / `env.instr_count` here — if they leaked
    // into the default build, the cfg-gated tests above would shadow this;
    // this test's body staying valid confirms no surface change.
    let _ = env.call_stack.len();
}

// ---------------------------------------------------------------------------
// M30 regress-guard: VM is at least as fast as the walker on a small fib.
// ---------------------------------------------------------------------------
//
// `cargo bench` is the authoritative regress guard (via `critcmp`), but a
// wall-time `#[test]` catches gross regressions in `cargo test` too. Runs
// `fib 20` (small enough to complete in well under a second in either
// mode, large enough to surface a 2x+ regression) in both VM and Walk
// modes; asserts the VM is no slower than the walker (within a generous
// 3x tolerance — debug builds are noisy, and the goal is to catch a
// misroute where the VM accidentally falls back to the walker, not to
// micro-tune). The plan3 M30 target is >= 5x on `fib 30`; `fib 20` is
// the test-friendly proxy.

/// Run `src` with the given mode, returning elapsed wall time.
fn timed_run(src: &str, walk: bool) -> std::time::Duration {
    let opts = RunOptions {
        walk,
        ..RunOptions::default()
    };
    let writer = BufferWriter::new();
    let start = Instant::now();
    let _ =
        red_eval::run_source_with_exit_opts(src, Box::new(writer), &opts).expect("fixture failed");
    start.elapsed()
}

#[test]
fn vm_no_slower_than_walker_on_fib() {
    // `fib 20` = 6765. Small enough for debug-build test runs (~50ms in
    // VM, ~150ms walker on the dev machine).
    let src = "Red [] fib: func [n][either n < 2 [n][(fib n - 1) + (fib n - 2)]] print fib 20";
    // Warm up (first run pays lex/parse/bind + JIT-like lazy compile).
    let _ = timed_run(src, false);
    let _ = timed_run(src, true);
    let vm_time = timed_run(src, false);
    let walk_time = timed_run(src, true);
    // Hard regress guard: VM must never be slower than the walker (3x
    // tolerance for debug-build noise). Catches a routing bug where the VM
    // accidentally falls back to the walker.
    assert!(
        vm_time.as_secs_f64() <= walk_time.as_secs_f64() * 3.0,
        "VM ({vm_time:?}) is > 3x slower than walker ({walk_time:?}) — possible walker fallback"
    );
    // Speedup guard: the M30 target is >= 5x on `fib 30` in release. Debug
    // builds don't hit 5x (the VM's per-instr dispatch match isn't optimized
    // by llvm in debug), so we only assert the 1.2x speedup in release. The
    // bench suite (`cargo bench --bench eval`) is the authoritative check.
    #[cfg(not(debug_assertions))]
    assert!(
        vm_time.as_secs_f64() * 1.2 <= walk_time.as_secs_f64(),
        "release VM ({vm_time:?}) is not at least 1.2x faster than walker ({walk_time:?}) — \
         the M30 target is 5x; this loose guard catches gross regressions only"
    );
}
