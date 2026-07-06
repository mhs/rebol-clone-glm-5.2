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
        // M150: decimal! equality — exact (rust_decimal implements Eq). Cross-
        // type with Integer promotes the int to Decimal (exact). Cross-type
        // with Float converts Decimal to f64 (Float wins; precision already
        // lost on the Float side). Matches the Integer/Float precedent at
        // lines 35-36.
        (Value::Decimal { d: x, .. }, Value::Decimal { d: y, .. }) => x == y,
        (Value::Integer { n: x, .. }, Value::Decimal { d: y, .. }) => {
            rust_decimal::Decimal::from(*x) == *y
        }
        (Value::Decimal { d: x, .. }, Value::Integer { n: y, .. }) => {
            *x == rust_decimal::Decimal::from(*y)
        }
        (Value::Float { f: x, .. }, Value::Decimal { d: y, .. }) => {
            let yf: f64 = (*y).try_into().unwrap_or(f64::NAN);
            *x == yf
        }
        (Value::Decimal { d: x, .. }, Value::Float { f: y, .. }) => {
            let xf: f64 = (*x).try_into().unwrap_or(f64::NAN);
            xf == *y
        }
        // M80: percent! strict equality — distinct from Float (cross-type `=` is
        // false). `50% = 0.5` ⇒ false (different types). Ordering (`<`/`>`)
        // promotes via `as_number` below.
        (Value::Percent { value: x, .. }, Value::Percent { value: y, .. }) => x == y,
        // M80: money! strict equality — compares both cents and currency.
        // Cross-currency `$10.00:USD = $10.00:EUR` is false. `money = int`
        // is false (distinct types).
        (Value::Money { amount: a, .. }, Value::Money { amount: b, .. }) => a == b,
        // M80: issue! equality by string compare.
        (Value::Issue { s: x, .. }, Value::Issue { s: y, .. }) => x == y,
        // M80: email! equality by string compare.
        (Value::Email { addr: x, .. }, Value::Email { addr: y, .. }) => x == y,
        // M81: tag! equality by string compare on the body text.
        (Value::Tag { text: x, .. }, Value::Tag { text: y, .. }) => x == y,
        (Value::Integer { n: x, .. }, Value::Float { f: y, .. }) => (*x as f64) == *y,
        (Value::Float { f: x, .. }, Value::Integer { n: y, .. }) => *x == (*y as f64),
        (Value::String { s: x, .. }, Value::String { s: y, .. }) => x == y,
        (Value::Char { c: x, .. }, Value::Char { c: y, .. }) => x == y,
        (Value::Pair { x: ax, y: ay, .. }, Value::Pair { x: bx, y: by, .. }) => {
            values_equal(ax, bx) && values_equal(ay, by)
        }
        (Value::Tuple { bytes: x, .. }, Value::Tuple { bytes: y, .. }) => x == y,
        (Value::String8 { bytes: x, .. }, Value::String8 { bytes: y, .. }) => x == y,
        (Value::None, Value::None) => true,
        // M86: `unset!` is distinct from `none!`. `unset = none` falls
        // through to the catch-all `_ => false` below.
        (Value::Unset, Value::Unset) => true,
        (Value::Logic(x), Value::Logic(y)) => x == y,
        (Value::Error(a), Value::Error(b)) => {
            // M42: structural equality — compare all fields. `args`/`near`
            // carry `Value`s (no `PartialEq` impl), so compare them via
            // `values_equal` recursively.
            a.message == b.message
                && a.code == b.code
                && a.kind == b.kind
                && a.cause == b.cause
                && a.by == b.by
                && a.args.len() == b.args.len()
                && a.args
                    .iter()
                    .zip(b.args.iter())
                    .all(|(x, y)| values_equal(x, y))
                && match (&a.near, &b.near) {
                    (None, None) => true,
                    (Some(x), Some(y)) => values_equal(x, y),
                    _ => false,
                }
        }
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
        (Value::Map(a), Value::Map(b)) => {
            // Deep entry equality: same keys (order-independent for equality,
            // though insertion order is preserved for iteration), same values.
            let a = a.borrow();
            let b = b.borrow();
            a.len() == b.len()
                && a.entries
                    .borrow()
                    .iter()
                    .all(|(k, v)| b.get(k).is_some_and(|bv| values_equal(v, &bv)))
        }
        (Value::Hash(a), Value::Hash(b)) => {
            // M83: deep entry equality, order-independent (the headline
            // discriminator vs map! — two hashes with the same entries in
            // different insertion order are equal).
            let a = a.borrow();
            let b = b.borrow();
            a.len() == b.len()
                && a.entries
                    .borrow()
                    .iter()
                    .all(|(k, v)| b.get(k).is_some_and(|bv| values_equal(v, &bv)))
        }
        // M84: vector! equality — same kind, same length, elementwise
        // `values_equal`. Cross-kind inequality (integer! ≠ float! even if
        // values match numerically — mirrors Red's strict `=`).
        (Value::Vector(a), Value::Vector(b)) => {
            let a = a.borrow();
            let b = b.borrow();
            a.kind() == b.kind()
                && a.len() == b.len()
                && a.elements()
                    .iter()
                    .zip(b.elements().iter())
                    .all(|(x, y)| values_equal(x, y))
        }
        // M85: image! equality — same dimensions, byte-identical pixel
        // buffers (RGBA8 row-major). Pixel values are raw bytes; no further
        // `values_equal` recursion is needed.
        (Value::Image(a), Value::Image(b)) => {
            let a = a.borrow();
            let b = b.borrow();
            a.width == b.width
                && a.height == b.height
                && a.pixels.borrow().as_slice() == b.pixels.borrow().as_slice()
        }
        // M45: date! equality. Normalize `None` zone → `Some(0)` (UTC) for
        // comparison, so a zone-naive date equals the same UTC date. Two
        // dates are equal iff their `dt` matches AND normalized zones match.
        (Value::Date { dt: da, .. }, Value::Date { dt: db, .. }) => {
            da.dt == db.dt && da.zone.unwrap_or(0) == db.zone.unwrap_or(0)
        }
        // M140: duration! equality — by nanos, not by original unit
        // (`30s = 30000ms` ⇒ true). Cross-type `duration = integer` is false
        // (distinct types).
        (Value::Duration { d: da, .. }, Value::Duration { d: db, .. }) => da == db,
        // M46: bitset! equality — same length and same bit pattern.
        (Value::Bitset(a), Value::Bitset(b)) => {
            let a = a.borrow();
            let b = b.borrow();
            a.len == b.len && a.bits.borrow().as_slice() == b.bits.borrow().as_slice()
        }
        // M89: typeset! equality — same set of type words (order-independent;
        // group words compare as words, not via expansion).
        (Value::Typeset(a), Value::Typeset(b)) => {
            let at = a.types.borrow();
            let bt = b.types.borrow();
            *at == *bt
        }
        // Word-family equality by name. Deviation from Red: real Red `=`
        // on `word!` compares bound values, not names; only `lit-word!`
        // compares by identity. The POC compares by name for all three
        // (strictly better than the prior `_ => false` catch-all, which
        // made any word-family pair unequal). Documented in
        // `project-brief.md`.
        (Value::LitWord { sym: x, .. }, Value::LitWord { sym: y, .. }) => x == y,
        (Value::Word { sym: x, .. }, Value::Word { sym: y, .. }) => x == y,
        (Value::GetWord { sym: x, .. }, Value::GetWord { sym: y, .. }) => x == y,
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
    if let Some(ord) = money_cmp(args)? {
        return Ok(Value::Logic(compare("<", ord)));
    }
    if let Some(ord) = duration_cmp(args)? {
        return Ok(Value::Logic(compare("<", ord)));
    }
    Ok(Value::Logic(compare("<", num_cmp(&args[0], &args[1])?)))
}

pub(crate) fn greater_than(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if let Some(ord) = money_cmp(args)? {
        return Ok(Value::Logic(compare(">", ord)));
    }
    if let Some(ord) = duration_cmp(args)? {
        return Ok(Value::Logic(compare(">", ord)));
    }
    Ok(Value::Logic(compare(">", num_cmp(&args[0], &args[1])?)))
}

pub(crate) fn less_equal(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if let Some(ord) = money_cmp(args)? {
        return Ok(Value::Logic(compare("<=", ord)));
    }
    if let Some(ord) = duration_cmp(args)? {
        return Ok(Value::Logic(compare("<=", ord)));
    }
    Ok(Value::Logic(compare("<=", num_cmp(&args[0], &args[1])?)))
}

pub(crate) fn greater_equal(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if let Some(ord) = money_cmp(args)? {
        return Ok(Value::Logic(compare(">=", ord)));
    }
    if let Some(ord) = duration_cmp(args)? {
        return Ok(Value::Logic(compare(">=", ord)));
    }
    Ok(Value::Logic(compare(">=", num_cmp(&args[0], &args[1])?)))
}

/// M140: duration! ordering dispatcher. Returns `Some(Ordering)` when both
/// operands are duration! (compares by nanos). Returns `None` when neither is
/// duration! (falls through to `num_cmp`). Errors when exactly one is
/// duration! (asymmetric — duration vs non-duration is a type error).
fn duration_cmp(args: &[Value]) -> Result<Option<std::cmp::Ordering>, EvalError> {
    let a = &args[0];
    let b = &args[1];
    let a_d = matches!(a, Value::Duration { .. });
    let b_d = matches!(b, Value::Duration { .. });
    if !a_d && !b_d {
        return Ok(None);
    }
    if a_d && b_d {
        let Value::Duration { d: da, .. } = a else {
            unreachable!()
        };
        let Value::Duration { d: db, .. } = b else {
            unreachable!()
        };
        return Ok(Some(da.cmp(db)));
    }
    // Asymmetric: duration vs non-duration.
    Err(EvalError::TypeError {
        expected: "duration!",
        found: if a_d { type_name(b) } else { type_name(a) },
        span: if a_d {
            b.span_or_default()
        } else {
            a.span_or_default()
        },
    })
}

/// M80: money! ordering dispatcher. Returns `Some(Ordering)` when both
/// operands are money! (compares by cents; cross-currency is a TypeError).
/// Returns `None` when neither operand is money! (so the caller falls
/// through to `num_cmp`). Errors when exactly one operand is money! (asymmetric
/// — money vs non-money is a type error for ordering).
fn money_cmp(args: &[Value]) -> Result<Option<std::cmp::Ordering>, EvalError> {
    let a = &args[0];
    let b = &args[1];
    let a_m = matches!(a, Value::Money { .. });
    let b_m = matches!(b, Value::Money { .. });
    if !a_m && !b_m {
        return Ok(None);
    }
    if a_m && b_m {
        let (ca, cca) = money_parts(a);
        let (cb, ccb) = money_parts(b);
        if cca != ccb {
            return Err(EvalError::Native {
                message: format!("money error: currency mismatch ({cca} vs {ccb})"),
                span: a.span_or_default(),
            });
        }
        return Ok(Some(ca.cmp(&cb)));
    }
    // Asymmetric: money vs non-money.
    Err(EvalError::TypeError {
        expected: "money!",
        found: if a_m { type_name(b) } else { type_name(a) },
        span: if a_m {
            b.span_or_default()
        } else {
            a.span_or_default()
        },
    })
}

/// Extract `(cents, currency)` from a `Value::Money`.
fn money_parts(v: &Value) -> (i64, &str) {
    match v {
        Value::Money { amount, .. } => (amount.cents, amount.currency.as_ref()),
        _ => unreachable!(),
    }
}

/// A numeric value extracted from `Value::Integer` or `Value::Float`.
enum Num {
    Int(i64),
    Float(f64),
    Dec(rust_decimal::Decimal),
}

fn as_number(v: &Value) -> Option<Num> {
    match v {
        Value::Integer { n, .. } => Some(Num::Int(*n)),
        Value::Float { f, .. } => Some(Num::Float(*f)),
        // M150: decimal! promotes to its exact Decimal value for ordering
        // (`<`/`>`/`<=`/`>=`). Equality stays strict (above). Cross-type
        // with Float converts Decimal to f64 (Float wins on mix).
        Value::Decimal { d, .. } => Some(Num::Dec(*d)),
        // M80: percent! promotes to its fractional float value for ordering
        // (`<`/`>`/`<=`/`>=`). Equality stays strict (above).
        Value::Percent { value, .. } => Some(Num::Float(*value)),
        // M38: char! ordered by codepoint for `<`/`>`/`<=`/`>=`.
        Value::Char { c, .. } => Some(Num::Int(*c as i64)),
        _ => None,
    }
}

/// Compare two numeric values, returning their `Ordering`. Errors carry the
/// offending operand's span.
pub(crate) fn num_cmp(a: &Value, b: &Value) -> Result<std::cmp::Ordering, EvalError> {
    let x = match as_number(a) {
        Some(n) => n,
        None => {
            return Err(EvalError::TypeError {
                expected: "integer!, float!, or decimal!",
                found: type_name(a),
                span: a.span_or_default(),
            })
        }
    };
    let y = match as_number(b) {
        Some(n) => n,
        None => {
            return Err(EvalError::TypeError {
                expected: "integer!, float!, or decimal!",
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
        // M150: Decimal comparisons. Dec/Dec and Dec/Int use exact Decimal
        // ordering (precision-preserving). Dec/Float converts Decimal to f64
        // (Float wins — precision already lost on the Float side).
        (Num::Dec(x), Num::Dec(y)) => x.cmp(&y),
        (Num::Int(x), Num::Dec(y)) => rust_decimal::Decimal::from(x).cmp(&y),
        (Num::Dec(x), Num::Int(y)) => x.cmp(&rust_decimal::Decimal::from(y)),
        (Num::Dec(x), Num::Float(y)) => {
            let xf: f64 = x.try_into().unwrap_or(f64::NAN);
            xf.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
        }
        (Num::Float(x), Num::Dec(y)) => {
            let yf: f64 = y.try_into().unwrap_or(f64::NAN);
            x.partial_cmp(&yf).unwrap_or(std::cmp::Ordering::Equal)
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use red_core::value::{Symbol, Value};
    use red_core::Span;

    fn lw(s: &str) -> Value {
        Value::LitWord {
            sym: Symbol::new(s),
            span: Span::default(),
        }
    }
    fn word(s: &str) -> Value {
        Value::Word {
            sym: Symbol::new(s),
            binding: red_core::value::Binding::Unbound,
            span: Span::default(),
        }
    }

    #[test]
    fn litword_equality_by_name() {
        assert!(values_equal(&lw("foo"), &lw("foo")));
        assert!(!values_equal(&lw("foo"), &lw("bar")));
    }

    #[test]
    fn word_equality_by_name() {
        assert!(values_equal(&word("foo"), &word("foo")));
        assert!(!values_equal(&word("foo"), &word("bar")));
    }

    #[test]
    fn litword_word_not_equal() {
        // Different variants are unequal even with the same name.
        assert!(!values_equal(&lw("foo"), &word("foo")));
    }
}
