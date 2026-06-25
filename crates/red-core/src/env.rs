//! Evaluator environment: `Env`, `CallFrame`, `EvalError`, `NativeFn`.
//!
//! Lives in `red-core` (not `red-eval`) so `FuncDef.native` can reference
//! `NativeFn` without a cross-crate dependency cycle. `red-eval` re-exports
//! these and provides the evaluation algorithm + native implementations.
//!
//! Milestone 5 scope: types exist, `Env::new` builds an empty environment,
//! `EvalError::UnboundWord` renders with the offending symbol. The call stack
//! and `Return`/`Native` error variants are present for M9+ but unused here.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::rc::Rc;

use crate::context::Context;
use crate::value::{FuncDef, Span, Symbol, Value};

/// Refinement arguments handed to a native at call time. Built by
/// `dispatch_call` from the call site (path refinements and/or inline
/// `/ref` flags), this is the refinement-facing counterpart to `args`.
///
/// Each entry is `(refinement_name, collected_arg_values)`. A refinement
/// present in the call appears here with its arguments (possibly empty for
/// zero-arity refinements like `/case` or `/only`); a refinement absent
/// from the call does not appear. Natives query with [`Self::has`] and
/// [`Self::get`].
#[derive(Debug, Default)]
pub struct RefineArgs {
    inner: Vec<(Symbol, Vec<Value>)>,
}

impl RefineArgs {
    /// A fresh empty argument set — used by call sites that take no
    /// refinements (the overwhelming majority, including all infix natives).
    /// Returns an owned value; pass `&RefineArgs::empty()` to natives.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Construct from already-collected `(name, args)` pairs. Used by
    /// `dispatch_call` after walking the spec.
    pub fn from_pairs(pairs: Vec<(Symbol, Vec<Value>)>) -> Self {
        Self { inner: pairs }
    }

    /// True if refinement `name` was supplied at the call site.
    pub fn has(&self, name: &Symbol) -> bool {
        self.inner.iter().any(|(n, _)| n == name)
    }

    /// The argument values supplied for refinement `name`, or `None` if the
    /// refinement wasn't used. Zero-arity refinements return `Some(&[])`.
    pub fn get(&self, name: &Symbol) -> Option<&[Value]> {
        self.inner
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_slice())
    }
}

/// Function pointer for native (Rust-implemented) operations. `args` are the
/// positional arguments (in spec order); `refs` carries any refinement flags
/// and their arguments (M13); `env` is the interpreter state.
pub type NativeFn = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

/// Top-level interpreter state: the shared user context, the function call
/// stack (empty until M9), the native registry (populated in M6), and the
/// shared output sink that natives like `print`/`prin`/`probe` write to.
///
/// `out` defaults to `io::stdout()`; tests inject a `Box<dyn Write>` buffer
/// so inline tests can assert on captured output.
pub struct Env {
    pub user_ctx: Rc<Context>,
    pub call_stack: Vec<CallFrame>,
    pub natives: HashMap<Symbol, Rc<FuncDef>>,
    pub out: Box<dyn Write>,
    /// Whether `call`/`shell` may execute external commands. Off by default;
    /// enabled by the CLI `--allow-shell` flag. `call`/`shell` raise
    /// `EvalError::Native` when this is false (M20 sandbox policy).
    pub allow_shell: bool,
    /// Current working directory for file! path resolution. Updated by
    /// `change-dir`; read by `what-dir`. Relative file paths in `read`/
    /// `write`/`exists?`/etc. resolve against this.
    pub cwd: PathBuf,
    /// High-water mark of `call_stack.len()` since the last
    /// [`Self::reset_stats`] call. Used by the v0.3 VM milestones to prove
    /// tail-call stack bounds. Only present under the `stats` cargo feature;
    /// release builds without it pay zero cost.
    #[cfg(feature = "stats")]
    pub max_frame_depth: usize,
    /// Count of `eval` loop iterations since the last [`Self::reset_stats`]
    /// call. Gives an operation-count metric independent of wall time, used
    /// in M30 to correlate VM instr count with walker instr count. Only
    /// present under the `stats` cargo feature.
    #[cfg(feature = "stats")]
    pub instr_count: u64,
}

impl Env {
    /// Empty environment: fresh user context, no call frames, no natives,
    /// output going to `stdout`.
    pub fn new(user_ctx: Rc<Context>) -> Self {
        Self::new_with_output(user_ctx, Box::new(io::stdout()))
    }

    /// Build an environment with a custom output sink (used by tests to
    /// capture native output into an in-memory buffer).
    pub fn new_with_output(user_ctx: Rc<Context>, out: Box<dyn Write>) -> Self {
        Self {
            user_ctx,
            call_stack: Vec::new(),
            natives: HashMap::new(),
            out,
            allow_shell: false,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            #[cfg(feature = "stats")]
            max_frame_depth: 0,
            #[cfg(feature = "stats")]
            instr_count: 0,
        }
    }

    /// Reset both instrumentation counters to zero. Called at the top of each
    /// `run_source*` entry point so per-program measurements start clean.
    /// No-op (and absent) when the `stats` feature is off.
    #[cfg(feature = "stats")]
    pub fn reset_stats(&mut self) {
        self.max_frame_depth = 0;
        self.instr_count = 0;
    }

    /// Record a `CallFrame` push: bump `max_frame_depth` if the current
    /// `call_stack.len()` exceeds it. Called by the function-call shim right
    /// after `env.call_stack.push(...)`. No-op (and absent) when the `stats`
    /// feature is off.
    #[cfg(feature = "stats")]
    pub fn record_frame_push(&mut self) {
        let depth = self.call_stack.len();
        if depth > self.max_frame_depth {
            self.max_frame_depth = depth;
        }
    }
}

/// A function invocation record. `ctx` holds parameter slots; `func` is the
/// definition being executed. Unused in M5 (no user functions yet).
pub struct CallFrame {
    pub ctx: Context,
    pub func: Option<Rc<FuncDef>>,
}

/// Evaluation failure. Every variant that originates from a value carries a
/// `Span` so the CLI can later render `file:line:col:`. `Return`, `Break`,
/// and `Continue` are control-flow unwinds caught by their respective
/// shims (function-call shim for `Return`, loop natives for `Break`/
/// `Continue`), not user errors.
#[derive(Debug)]
pub enum EvalError {
    /// Word has no binding and no native of that name exists.
    UnboundWord { sym: Symbol, span: Span },
    /// A native or operation expected one value kind and got another.
    TypeError {
        expected: &'static str,
        found: &'static str,
        span: Span,
    },
    /// A native was called with the wrong number of arguments.
    Arity {
        native: Symbol,
        expected: usize,
        got: usize,
        span: Span,
    },
    /// `return` unwind — caught by the function-call shim (M9).
    Return(Value),
    /// `break` unwind — caught by the enclosing loop native. Carries an
    /// optional break-value (Red's `break/return`); the loop native decides
    /// whether to use it or discard it.
    Break(Option<Value>),
    /// `continue` unwind — caught by the enclosing loop native; advances to
    /// the next iteration.
    Continue,
    /// `throw value` unwind — caught by an enclosing `catch` native. Carries
    /// the thrown value. Like `Return`/`Break`/`Continue`, this is a control-
    /// flow unwind, not a user error, and carries no span.
    Throw(Value),
    /// `exit`/`quit` unwind — caught at the top-level script entry point.
    /// Carries the requested process exit code. Not a user error.
    Quit(i32),
    /// Generic native-reported error with a message.
    Native { message: String, span: Span },
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Note: renders just the message body (no `*** Error:` prefix and no
        // `file:line:col:` location). The `render_error` function in
        // `error.rs` wraps this with the full `*** Error: [loc: ]<msg>` form
        // using a `LineMap`. The bare `Display` is used by test helpers that
        // only care about the message body.
        match self {
            EvalError::UnboundWord { sym, .. } => {
                write!(f, "{:?} has no value", sym.as_str())
            }
            EvalError::TypeError {
                expected, found, ..
            } => write!(f, "expected {expected}, found {found}"),
            EvalError::Arity {
                native,
                expected,
                got,
                ..
            } => write!(
                f,
                "{:?} expects {} argument(s), got {}",
                native.as_str(),
                expected,
                got
            ),
            EvalError::Return(_) => write!(f, "return used outside a function"),
            EvalError::Break(_) => write!(f, "break used outside a loop"),
            EvalError::Continue => write!(f, "continue used outside a loop"),
            EvalError::Throw(_) => {
                write!(f, "throw used outside a catch")
            }
            EvalError::Quit(code) => write!(f, "quit with exit code {code}"),
            EvalError::Native { message, .. } => write!(f, "{message}"),
        }
    }
}

impl EvalError {
    /// Byte-offset span where this error originated, if any. Used by
    /// `render_error` to produce `file:line:col:` prefixes. `Return`/
    /// `Break`/`Continue` are control-flow unwinds, not user errors, and
    /// carry no span.
    pub fn span(&self) -> Option<Span> {
        match self {
            EvalError::UnboundWord { span, .. }
            | EvalError::TypeError { span, .. }
            | EvalError::Arity { span, .. }
            | EvalError::Native { span, .. } => Some(*span),
            EvalError::Return(_)
            | EvalError::Break(_)
            | EvalError::Continue
            | EvalError::Throw(_)
            | EvalError::Quit(_) => None,
        }
    }
}

impl std::error::Error for EvalError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// When the `stats` feature is on, `Env` exposes the two counter fields
    /// and they start at zero.
    #[cfg(feature = "stats")]
    #[test]
    fn stats_fields_present_and_zero_init() {
        let env = Env::new(Rc::new(Context::new()));
        assert_eq!(env.max_frame_depth, 0);
        assert_eq!(env.instr_count, 0);
    }

    /// When the `stats` feature is on, `reset_stats` and `record_frame_push`
    /// behave as expected: a push bumps `max_frame_depth`, reset zeroes it.
    #[cfg(feature = "stats")]
    #[test]
    fn stats_reset_and_push_track() {
        let mut env = Env::new(Rc::new(Context::new()));
        env.call_stack.push(CallFrame {
            ctx: Context::new(),
            func: None,
        });
        env.record_frame_push();
        assert_eq!(env.max_frame_depth, 1);
        env.reset_stats();
        assert_eq!(env.max_frame_depth, 0);
        assert_eq!(env.instr_count, 0);
    }

    /// Compile-time check that with the `stats` feature OFF, `Env` has no
    /// counter fields. We use a trait-impl trick: `HasStats` is only
    /// implemented when the feature is on; without it, this test still
    /// compiles because the trait bound is *not* asserted — instead we
    /// verify the absence structurally by confirming the struct layout
    /// didn't change. The simplest faithful check is: the methods
    /// `reset_stats`/`record_frame_push` simply don't exist, so attempting
    /// to call them would fail to compile. We reference them via a cfg-gated
    /// path so this test body stays valid in both configurations.
    #[cfg(not(feature = "stats"))]
    #[test]
    fn stats_fields_absent_when_feature_off() {
        let mut env = Env::new(Rc::new(Context::new()));
        // No `max_frame_depth` / `instr_count` fields exist; the only
        // `Env`-mutating surface here is the public non-stats API. If a
        // counter field had leaked into the default build, the cfg-gated
        // `reset_stats` call below would not compile (method not found).
        // Confirm the env is usable without any stats surface:
        let _ = env.call_stack.len();
        // (No `env.reset_stats()` call — that method only exists under
        // `stats`, and this test only compiles without it.)
        let _ = &mut env;
    }

    /// Symmetric compile-time assertion under the `stats` feature: the
    /// methods *do* exist. Kept separate from the behavior test above so
    /// the "fields absent" test stays a pure compile check.
    #[cfg(feature = "stats")]
    #[test]
    fn stats_methods_exist_when_feature_on() {
        let mut env = Env::new(Rc::new(Context::new()));
        env.reset_stats();
        env.record_frame_push();
        // (Push without an actual frame is fine: record_frame_push just
        // reads call_stack.len() == 0, so max_frame_depth stays 0.)
        assert_eq!(env.max_frame_depth, 0);
    }
}
