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
use std::rc::Rc;

use crate::context::Context;
use crate::value::{FuncDef, Span, Symbol, Value};

/// Function pointer for native (Rust-implemented) operations.
pub type NativeFn = fn(&[Value], &mut Env) -> Result<Value, EvalError>;

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
            EvalError::Return(_) | EvalError::Break(_) | EvalError::Continue => None,
        }
    }
}

impl std::error::Error for EvalError {}
