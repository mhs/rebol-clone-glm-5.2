//! red-eval: tree-walking evaluator + native registry (natives land in M6+).
//!
//! Eval-related types (`Env`, `CallFrame`, `EvalError`, `NativeFn`) live in
//! `red-core` and are re-exported here via `context`; this crate contributes
//! the evaluation algorithm (`interp`) and, in later milestones, the native
//! implementations (`natives`, `series`, `binding`, `parse`).

pub mod binding;
pub mod context;
pub mod convert;
pub mod interp;
pub mod math;
pub mod natives;
pub mod object;
pub mod parse;
pub mod series;
pub mod strings;

pub use binding::{bind_pass, bind_pass_into};
pub use context::{Binding, CallFrame, Context, Env, EvalError, FuncDef, NativeFn, RefineArgs};
pub use interp::{
    eval, run_series, run_series_with_exit_output, run_series_with_output, run_source,
    run_source_with_exit, run_source_with_exit_output, run_source_with_output,
};
pub use natives::{install_constants, register_natives};
pub use series::register_series_natives;
pub use strings::register_string_natives;

// Re-exports from red-core used by the CLI (REPL): parsing the next line,
// molding the result, and matching on parse errors for multi-line input.
pub use red_core::{
    form, form_to_string, load_source, mold, mold_to_string, render_error, Error, ParseError,
    Series, Span, Symbol, Value,
};
