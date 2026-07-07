//! Eval natives: `do`, `reduce`.
//!
//! `do` evaluates a block (or a string parsed as Red source). `reduce`
//! evaluates each expression in a block, collecting the results into a new
//! block. (`load` is registered by `io::register_io_natives` with the
//! file-aware `load_extended` impl, which also handles the string! case.)

use red_core::parser::load_source;
use red_core::value::Value;
use red_core::{Env, EvalError, RefineArgs};

use super::{arity_err, expect_block, type_name};
use crate::interp::{dispatch_block, dispatch_block_reduce};

/// `do block-or-string` — evaluates a block (or a string parsed as Red source),
/// returning the last value. When given a string, lexes+parses it via
/// `load_source`, binds the resulting body against the live `env.user_ctx`
/// (so `do "x: 5"` writes to the user context like a top-level script), then
/// evaluates it.
pub(crate) fn do_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "do", 1, 0));
    }
    match &args[0] {
        Value::Block { .. } | Value::Paren { .. } => dispatch_block(&args[0], env),
        Value::String { s, span } => {
            let body = load_source(s).map_err(|e| EvalError::Native {
                message: e.to_string(),
                span: *span,
            })?;
            crate::binding::bind_pass_into(&body, &env.user_ctx);
            let block = Value::block(body);
            dispatch_block(&block, env)
        }
        other => Err(EvalError::TypeError {
            expected: "block! or string!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `reduce block` — evaluates each expression in the block, returning a new
/// block of the results.
pub(crate) fn reduce(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "reduce")?;
    dispatch_block_reduce(&body, env)
}
