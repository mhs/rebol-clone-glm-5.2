//! red-eval: tree-walking evaluator + native registry (natives land in M6+).
//!
//! Eval-related types (`Env`, `CallFrame`, `EvalError`, `NativeFn`) live in
//! `red-core` and are re-exported here via `context`; this crate contributes
//! the evaluation algorithm (`interp`) and, in later milestones, the native
//! implementations (`natives`, `series`, `binding`, `parse`).

pub mod context;
pub mod interp;

pub use context::{Binding, CallFrame, Context, Env, EvalError, FuncDef, NativeFn};
pub use interp::{bind_pass, eval, run_series, run_source};
