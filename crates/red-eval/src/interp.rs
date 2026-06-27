//! Evaluator dispatch shim (M29).
//!
//! This module is the public entry point for evaluation. It re-exports the
//! full surface of [`interp_legacy`] (the v0.2 tree-walker: `run_source*`,
//! `run_series*`, `RunOptions`, `dispatch_block`, `eval_expression`, path
//! helpers) and provides a top-level [`eval`] that dispatches on
//! [`Env::mode`][red_core::Env::mode]:
//!
//! - `EvalMode::Walk` → [`interp_legacy::eval`] (the tree-walker).
//! - `EvalMode::Vm`   → compile-on-demand + `vm::run` (via [`dispatch_block`]).
//!
//! Since M29 the default mode is `Vm` (flipped in `Env::new_with_output`).
//! The CLI `--walk` flag (via `RunOptions.walk`) and the `force-walk` cargo
//! feature both override to `Walk` for debugging and the golden parity
//! baseline (`cargo test --workspace --features force-walk`).
//!
//! Callers that recurse into a block argument (`do`/`if`/`either`/`loop`/
//! `while`/`repeat`/`until`/`foreach`/`forall`/`switch`/`case`/`try`/
//! `attempt`/`catch`/`use`/`reduce`) already call [`dispatch_block`] /
//! [`dispatch_block_reduce`] directly — they route correctly under both
//! modes. The legacy walker's internal recursion (e.g. `eval_expression`
//! evaluating a `Paren` inline) always stays on the walker, which is correct:
//! a `Paren` inside a walker-evaluated block is walker territory.
//!
//! For the raw tree-walker entry (bypassing mode dispatch — used by the
//! `bench_fixtures` stats tests and the `interp_legacy` unit tests), call
//! [`interp_legacy::eval`] directly.

// Public API (re-exported by `lib.rs` to external callers — the CLI, tests):
pub use crate::interp_legacy::{
    run_source, run_source_with_exit, run_source_with_exit_opts, run_source_with_exit_output,
    run_source_with_output, run_series, run_series_with_exit_output, run_series_with_output,
    RunOptions,
};
// Crate-internal helpers (used by `natives.rs`, `vm/vm.rs`, etc.; not part
// of the external `red_eval::` surface — they're `pub(crate)` in
// `interp_legacy`):
pub(crate) use crate::interp_legacy::{
    dispatch_block, dispatch_block_reduce, eval_expression, eval_get_path,
    resolve_compiled_block, set_path_value,
};

use red_core::{Env, EvalError, EvalMode, Value};

/// Top-level evaluator: dispatches on [`Env::mode`].
///
/// - `Walk` → [`crate::interp_legacy::eval`] (the tree-walker).
/// - `Vm`   → compile-on-demand + `vm::run` (delegated to [`dispatch_block`]).
///
/// This is the function re-exported as `red_eval::eval` and called by the
/// REPL and by inline unit tests. `dispatch_block` handles the
/// `needs_rebind` / foreign-binding fallback (routing to the walker when the
/// VM can't lexically address the block's words), so callers don't need to
/// duplicate that logic.
///
/// Non-block/paren values are returned as-is (cloned), mirroring the walker.
pub fn eval(block: &Value, env: &mut Env) -> Result<Value, EvalError> {
    // Fast path: in `Walk` mode (or under `force-walk`), skip straight to the
    // walker. `dispatch_block` would reach the same conclusion but this
    // avoids the `has_foreign_bindings` / cache-lookup overhead on the
    // common walker path (e.g. the `bench_fixtures` stats tests, which pin
    // `env.mode = Walk` and assert walker-specific instr counts).
    if env.mode == EvalMode::Walk {
        return crate::interp_legacy::eval(block, env);
    }
    dispatch_block(block, env)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M29: `interp::eval` dispatches on `env.mode`. Constructing an `Env`
    /// with the default (no `force-walk` feature) yields `Vm`, so `eval`
    /// routes to `dispatch_block` → `vm::run`. This test confirms the
    /// dispatch wiring end-to-end: `1 + 2` evaluates to `3` under the VM.
    #[cfg(not(feature = "force-walk"))]
    #[test]
    fn eval_dispatches_to_vm_by_default() {
        use red_core::parser::load_source;
        use red_core::Context;
        let body = load_source("1 + 2").expect("parse failed");
        let ctx = Context::new();
        let ctx_rc = crate::binding::bind_pass(&body, ctx);
        let mut env = Env::new_with_output(ctx_rc, Box::new(std::io::sink()));
        crate::natives::register_natives(&mut env);
        // Default mode is Vm (no force-walk feature).
        assert_eq!(env.mode, EvalMode::Vm);
        let block = Value::Block {
            series: body,
            span: red_core::Span::new(0, 0),
        };
        let result = eval(&block, &mut env).expect("eval failed");
        match result {
            Value::Integer { n, .. } => assert_eq!(n, 3),
            other => panic!("expected Integer(3), got {other:?}"),
        }
    }

    /// Under `force-walk`, `eval` routes to the walker directly.
    #[cfg(feature = "force-walk")]
    #[test]
    fn eval_dispatches_to_walker_under_force_walk() {
        use red_core::parser::load_source;
        use red_core::Context;
        let body = load_source("1 + 2").expect("parse failed");
        let ctx = Context::new();
        let ctx_rc = crate::binding::bind_pass(&body, ctx);
        let mut env = Env::new_with_output(ctx_rc, Box::new(std::io::sink()));
        crate::natives::register_natives(&mut env);
        assert_eq!(env.mode, EvalMode::Walk);
        let block = Value::Block {
            series: body,
            span: red_core::Span::new(0, 0),
        };
        let result = eval(&block, &mut env).expect("eval failed");
        match result {
            Value::Integer { n, .. } => assert_eq!(n, 3),
            other => panic!("expected Integer(3), got {other:?}"),
        }
    }

    /// M29 parity: `mold(parse(mold(v)))` is unaffected by compilation —
    /// compilation is a side cache off `FuncDef::compiled`; `Value::Block`
    /// passed as data is never compiled. This round-trips a few `Value`s
    /// through `load_source` + `mold_to_string`, asserting the mold is
    /// stable. (Data-model side is untouched by the VM.)
    #[test]
    fn mold_parse_mold_roundtrip_unaffected_by_vm() {
        use red_core::parser::load_source;
        use red_core::printer::mold_to_string;
        // `load_source("5")` returns a body series containing `5`. Mold each
        // value in the body, concatenate, re-parse, re-mold — must match.
        // This confirms compilation never touches the data-model side.
        let cases = [
            "5",
            "1.5",
            "\"hello\"",
            "[1 2 3]",
            "(1 2 3)",
            "'foo",
            "foo: 5",
            ":foo",
            "%some/file.txt",
            "http://example.com/path",
            "[a [b c] d]",
            "[1 2.5 \"x\" 'y z:]",
        ];
        for src in cases {
            let body1 = load_source(src).expect("parse failed");
            let data1 = body1.data.borrow();
            // Mold each value in the body (space-separated, matching the
            // source format). This is what `mold_to_string(&Block{..})` does
            // but without the outer `[]` wrapper.
            let mold1: String = data1
                .iter()
                .map(|v| mold_to_string(v))
                .collect::<Vec<_>>()
                .join(" ");
            drop(data1);
            // Re-parse the mold and re-mold.
            let body2 = load_source(&mold1).expect("re-parse failed");
            let mold2: String = body2
                .data
                .borrow()
                .iter()
                .map(|v| mold_to_string(v))
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(mold1, mold2, "round-trip mismatch for {src:?}");
        }
    }
}
