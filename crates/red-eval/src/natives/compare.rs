//! Comparison (infix `= <> < > <= >=`) and logic (`and`/`or` infix, `not`
//! prefix) natives.
//!
//! `and`/`or` dispatch on operand type (M17): both `logic!` → logic op;
//! both `integer!` → bitwise op; otherwise fall back to the truthiness-based
//! logic op (preserves the pre-M17 behavior for mixed/other truthy values
//! like `none and true`).

use super::{truthy, type_name};
use red_core::value::Value;
use red_core::{Env, EvalError, RefineArgs};

// ---------------------------------------------------------------------------
// Equality
// ---------------------------------------------------------------------------

pub(crate) fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Integer { n: x, .. }, Value::Integer { n: y, .. }) => x == y,
        (Value::Float { f: x, .. }, Value::Float { f: y, .. }) => x == y,
        (Value::Integer { n: x, .. }, Value::Float { f: y, .. }) => (*x as f64) == *y,
        (Value::Float { f: x, .. }, Value::Integer { n: y, .. }) => *x == (*y as f64),
        (Value::String { s: x, .. }, Value::String { s: y, .. }) => x == y,
        (Value::None, Value::None) => true,
        (Value::Logic(x), Value::Logic(y)) => x == y,
        (Value::Error(a), Value::Error(b)) => a.message == b.message,
        (Value::Object(a), Value::Object(b)) => {
            // Shallow value equality: same words, same slot values.
            let a = a.borrow();
            let b = b.borrow();
            let aw = a.ctx.words();
            let bw = b.ctx.words();
            aw.len() == bw.len()
                && aw.iter().zip(bw.iter()).all(|(x, y)| x == y)
                && aw
                    .iter()
                    .filter(|s| s.as_str() != "self")
                    .all(|s| values_equal(&a.ctx.get(s).unwrap(), &b.ctx.get(s).unwrap()))
        }
        _ => false,
    }
}

pub(crate) fn equal(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    Ok(Value::Logic(values_equal(&args[0], &args[1])))
}

pub(crate) fn not_equal(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    Ok(Value::Logic(!values_equal(&args[0], &args[1])))
}

// ---------------------------------------------------------------------------
// Ordering
// ---------------------------------------------------------------------------

fn compare(op: &str, ord: std::cmp::Ordering) -> bool {
    matches!(
        (op, ord),
        ("<", std::cmp::Ordering::Less)
            | (">", std::cmp::Ordering::Greater)
            | ("<=", std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
            | (
                ">=",
                std::cmp::Ordering::Greater | std::cmp::Ordering::Equal
            )
    )
}

pub(crate) fn less_than(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    Ok(Value::Logic(compare("<", num_cmp(&args[0], &args[1])?)))
}

pub(crate) fn greater_than(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    Ok(Value::Logic(compare(">", num_cmp(&args[0], &args[1])?)))
}

pub(crate) fn less_equal(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    Ok(Value::Logic(compare("<=", num_cmp(&args[0], &args[1])?)))
}

pub(crate) fn greater_equal(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    Ok(Value::Logic(compare(">=", num_cmp(&args[0], &args[1])?)))
}

/// A numeric value extracted from `Value::Integer` or `Value::Float`.
enum Num {
    Int(i64),
    Float(f64),
}

fn as_number(v: &Value) -> Option<Num> {
    match v {
        Value::Integer { n, .. } => Some(Num::Int(*n)),
        Value::Float { f, .. } => Some(Num::Float(*f)),
        _ => None,
    }
}

/// Compare two numeric values, returning their `Ordering`. Errors carry the
/// offending operand's span.
fn num_cmp(a: &Value, b: &Value) -> Result<std::cmp::Ordering, EvalError> {
    let x = match as_number(a) {
        Some(n) => n,
        None => {
            return Err(EvalError::TypeError {
                expected: "integer! or float!",
                found: type_name(a),
                span: a.span_or_default(),
            })
        }
    };
    let y = match as_number(b) {
        Some(n) => n,
        None => {
            return Err(EvalError::TypeError {
                expected: "integer! or float!",
                found: type_name(b),
                span: b.span_or_default(),
            })
        }
    };
    Ok(match (x, y) {
        (Num::Int(x), Num::Int(y)) => x.cmp(&y),
        (Num::Int(x), Num::Float(y)) => (x as f64)
            .partial_cmp(&y)
            .unwrap_or(std::cmp::Ordering::Equal),
        (Num::Float(x), Num::Int(y)) => x
            .partial_cmp(&(y as f64))
            .unwrap_or(std::cmp::Ordering::Equal),
        (Num::Float(x), Num::Float(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
    })
}

// ---------------------------------------------------------------------------
// Logic: and, or (infix), not (prefix)
// ---------------------------------------------------------------------------

pub(crate) fn and_op(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    match (&args[0], &args[1]) {
        (Value::Logic(a), Value::Logic(b)) => Ok(Value::Logic(*a && *b)),
        (Value::Integer { n: a, .. }, Value::Integer { n: b, .. }) => Ok(Value::integer(*a & *b)),
        _ => Ok(Value::Logic(truthy(&args[0]) && truthy(&args[1]))),
    }
}

pub(crate) fn or_op(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    match (&args[0], &args[1]) {
        (Value::Logic(a), Value::Logic(b)) => Ok(Value::Logic(*a || *b)),
        (Value::Integer { n: a, .. }, Value::Integer { n: b, .. }) => Ok(Value::integer(*a | *b)),
        _ => Ok(Value::Logic(truthy(&args[0]) || truthy(&args[1]))),
    }
}

pub(crate) fn not_op(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    Ok(Value::Logic(!truthy(&args[0])))
}
