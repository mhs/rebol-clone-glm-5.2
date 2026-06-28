//! I/O natives: `print`, `prin`, `probe`.
//!
//! These mold every argument uniformly (including strings, which appear
//! quoted). This diverges from real Red's `form`-based printing but keeps
//! the POC printer surface small; the divergence is documented for the
//! M12 audit pass.

use std::io::Write;

use red_core::printer::mold_to_string;
use red_core::value::Value;
use red_core::{Env, EvalError, RefineArgs};

/// `print`: mold each arg, join with a single space, append a newline.
/// Returns `Value::None`.
pub(crate) fn print(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let joined = join_molded(args);
    let _ = writeln!(env.out, "{joined}");
    Ok(Value::None)
}

/// `prin`: like `print` but without the trailing newline.
pub(crate) fn prin(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let joined = join_molded(args);
    let _ = write!(env.out, "{joined}");
    Ok(Value::None)
}

/// `probe`: print `== <mold>` for each arg (joined with space), newline,
/// and return the first arg (or `none` if no args).
pub(crate) fn probe(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let joined = join_molded(args);
    let _ = writeln!(env.out, "== {joined}");
    Ok(args.first().cloned().unwrap_or(Value::None))
}

pub(crate) fn join_molded(args: &[Value]) -> String {
    let mut out = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&mold_to_string(a));
    }
    out
}
