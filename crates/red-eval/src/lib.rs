//! red-eval: evaluator + native registry.
//!
//! Since v0.3 (M29), the default evaluator is the bytecode VM
//! (`EvalMode::Vm`); the tree-walker lives in [`interp_walker`] and is
//! reachable via the CLI `--walk` flag or the `force-walk` cargo feature.
//! The public [`interp`] module is a thin dispatch shim re-exporting both
//! the walker surface and a mode-aware [`eval`][interp::eval].
//!
//! Eval-related types (`Env`, `CallFrame`, `EvalError`, `NativeFn`) live in
//! `red-core` and are re-exported here; this crate contributes the
//! evaluation algorithm (`interp`/`interp_walker`) and the native
//! implementations (`natives`, `series`, `binding`, `parse`, …).

pub mod binding;
pub mod convert;
pub mod interp;
pub mod interp_runner;
pub mod interp_walker;
pub mod io;
pub mod math;
pub mod natives;
pub mod object;
pub mod parse;
pub mod path;
pub mod series;
pub mod strings;
pub mod vm;

pub use binding::{bind_pass, bind_pass_into};
pub use interp::{
    disasm_source, eval, run_series, run_series_with_exit_output, run_series_with_output,
    run_source, run_source_with_exit, run_source_with_exit_opts, run_source_with_exit_output,
    run_source_with_output, RunOptions,
};
pub use natives::{install_constants, register_natives};
pub use series::register_series_natives;
pub use strings::register_string_natives;

// Re-exports from red-core used by the CLI (REPL) and the eval algorithm:
// parsing the next line, molding the result, matching on parse errors for
// multi-line input, and the eval-related types (Env/CallFrame/Context/…).
pub use red_core::{
    form, form_to_string, load_source, mold, mold_to_string, render_error, Binding, CallFrame,
    Context, Env, Error, EvalError, EvalMode, FuncDef, NativeFn, ParseError, RefineArgs, Series,
    Span, Symbol, Value,
};
