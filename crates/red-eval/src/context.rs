//! Re-exports of the eval-related types that live in `red-core`.
//!
//! Per the architecture decision for Milestone 5, `Env`/`CallFrame`/
//! `EvalError`/`NativeFn` are defined in `red-core` (so `FuncDef.native` can
//! reference `NativeFn` without a cross-crate cycle). `red-eval` simply
//! re-exports them and contributes the evaluation algorithm (`interp`) and,
//! in later milestones, the native implementations.

pub use red_core::{
    Binding, CallFrame, Context, Env, EvalError, EvalMode, FuncDef, NativeFn, RefineArgs,
};
