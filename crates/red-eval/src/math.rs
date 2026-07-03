//! Math + bitwise natives (Milestone 17).
//!
//! Covers:
//!   - `//` modulo (infix) — registered separately in `natives.rs`.
//!   - `abs`, `negate` (prefix arity 1).
//!   - `add`/`subtract`/`multiply`/`divide` (prefix arity 2) — word aliases
//!     for the `+ - * /` infix operators.
//!   - `min`/`max` (prefix arity 2) over any orderable numeric value.
//!   - `round` (arity 1) with `/to` (scale) and `/even` (banker's rounding)
//!     refinements.
//!   - `random` (arity 1) with `/seed`/`/only`/`/secure` refinements.
//!   - `power` (infix `**`) and prefix alias.
//!   - `even?`/`odd?` (predicates).
//!   - Bitwise on integers: `and`/`or`/`xor` (infix, dispatch shared with the
//!     logic ops in `natives.rs`), `complement`, `shift-left`, `shift-right`.
//!
//! RNG note: no `rand` dependency is pulled in (workspace keeps deps
//! minimal). A small per-thread LCG seeded from a fixed constant provides
//! deterministic output; `random/seed` reseeds it. `random/secure` falls
//! back to the same LCG in the POC (deferring a real entropy source to v0.3).

use std::cell::Cell;

use red_core::value::{ErrorValue, FuncDef, Symbol, Value};
use red_core::{CompileErrorKind, Env, EvalError, NativeFn, RefineArgs};

use crate::natives::{arity_err, type_name};

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Numeric payload extracted from `Integer`/`Float`.
enum Num {
    Int(i64),
    Float(f64),
}

fn as_number(v: &Value) -> Option<Num> {
    match v {
        Value::Integer { n, .. } => Some(Num::Int(*n)),
        Value::Float { f, .. } => Some(Num::Float(*f)),
        // M80: percent! promotes to its fractional float value for arithmetic
        // (`percent + float → float`, `percent * scalar → float`). The
        // percent-preserving case (`percent + percent → percent`) is handled
        // up-front by `percent_binop` before `num_binop` runs.
        Value::Percent { value, .. } => Some(Num::Float(*value)),
        _ => None,
    }
}

/// A numeric view over `integer!`/`float!`/`char!` (char takes its codepoint).
/// Used by `for`'s direction-aware step/comparison so the loop native supports
/// all three numeric kinds Red does.
pub(crate) enum CoercedNum {
    Int(i64),
    Float(f64),
    Char(i64),
}

impl CoercedNum {
    pub(crate) fn from(v: &Value) -> Option<Self> {
        match v {
            Value::Integer { n, .. } => Some(Self::Int(*n)),
            Value::Float { f, .. } => Some(Self::Float(*f)),
            Value::Char { c, .. } => Some(Self::Char(*c as i64)),
            _ => None,
        }
    }

    /// Promote to `f64` for mixed int/float comparisons/additions.
    fn as_f64(&self) -> f64 {
        match self {
            Self::Int(n) => *n as f64,
            Self::Float(f) => *f,
            Self::Char(c) => *c as f64,
        }
    }

    fn is_float(&self) -> bool {
        matches!(self, Self::Float(_))
    }
}

/// `for`-step helper: add `bump` to `cur`, preserving the operand kind
/// (int+int→int, char+int→char, float-involved→float). Mirrors the
/// `char + int → char` and numeric promotion rules of `math::add` without
/// pulling in string/pair/tuple/date dispatch.
pub(crate) fn numeric_add(cur: &Value, bump: &Value) -> Result<Value, EvalError> {
    let a = CoercedNum::from(cur).ok_or_else(|| num_type_err(cur))?;
    let b = CoercedNum::from(bump).ok_or_else(|| num_type_err(bump))?;
    let result = match (a, b) {
        (CoercedNum::Char(c), CoercedNum::Int(k)) => {
            // char + int → char (codepoint arithmetic, like char_binop)
            return Ok(Value::char(char::from_u32((c + k) as u32).ok_or_else(
                || EvalError::Native {
                    message: format!("for: char step out of range: {}", c + k),
                    span: cur.span_or_default(),
                },
            )?));
        }
        (CoercedNum::Int(x), CoercedNum::Int(y)) => {
            x.checked_add(y).ok_or_else(|| EvalError::Native {
                message: "for: integer overflow".into(),
                span: cur.span_or_default(),
            })? as f64
        }
        (a, b) => a.as_f64() + b.as_f64(),
    };
    if CoercedNum::from(cur).map(|c| c.is_float()).unwrap_or(false)
        || CoercedNum::from(bump)
            .map(|c| c.is_float())
            .unwrap_or(false)
    {
        Ok(Value::float(result))
    } else {
        Ok(Value::integer(result as i64))
    }
}

/// `for`-comparison helper: numeric ordering of `cur` vs `end`. Char compares
/// by codepoint; mixed int/float promotes to float. Returns `None` if either
/// side isn't numeric (caller should have validated earlier).
pub(crate) fn numeric_cmp(cur: &Value, end: &Value) -> Option<std::cmp::Ordering> {
    let a = CoercedNum::from(cur)?;
    let b = CoercedNum::from(end)?;
    let (af, bf) = (a.as_f64(), b.as_f64());
    af.partial_cmp(&bf)
}

fn num_type_err(v: &Value) -> EvalError {
    EvalError::TypeError {
        expected: "integer! or float!",
        found: type_name(v),
        span: v.span_or_default(),
    }
}

/// M42: build a structured `math` error (`type: 'math`, `code: 400`) for a
/// division/modulo by zero. The `op` string fills the message body.
fn math_by_zero(args: &[Value], op: &str) -> EvalError {
    let span = args[0].span_or_default();
    let near = if span.is_default() {
        None
    } else {
        Some(Value::Block {
            series: red_core::value::Series::new(Vec::new()),
            span,
        })
    };
    EvalError::Raised(std::rc::Rc::new(ErrorValue::new_structed(
        format!("math error: {op} by zero"),
        Some(400),
        Some(Symbol::new("math")),
        Vec::new(),
        near,
        None,
        None,
    )))
}

/// M38: char codepoint extracted from a `char!` value, for char arithmetic
/// (`char + int → char`, `char - char → int`, `char + char → int`).
fn as_codepoint(v: &Value) -> Option<i64> {
    match v {
        Value::Char { c, .. } => Some(*c as i64),
        _ => None,
    }
}

/// M38: char arithmetic dispatcher. Returns `Some(result)` if either operand
/// is a `char!` (and the combination is valid); `None` if neither is a char
/// (caller falls through to numeric `num_binop`). Errors on char+float.
fn char_binop(
    args: &[Value],
    op: &str,
    f_char_int: fn(i64, i64) -> i64,
    f_char_char: fn(i64, i64) -> i64,
) -> Result<Option<Value>, EvalError> {
    let a_cp = as_codepoint(&args[0]);
    let b_cp = as_codepoint(&args[1]);
    match (a_cp, b_cp) {
        (Some(a), Some(b)) => {
            // char + char → int (or char - char → int). Error if result is not
            // a valid codepoint when caller expects char output — but Red's
            // rule is char+char → integer, so always return int.
            Ok(Some(Value::integer(f_char_char(a, b))))
        }
        (Some(a), None) => {
            // char + int → char. Float operand is a type error.
            if let Value::Float { .. } = &args[1] {
                return Err(EvalError::TypeError {
                    expected: "char! or integer!",
                    found: type_name(&args[1]),
                    span: args[1].span_or_default(),
                });
            }
            let b = as_number(&args[1]).ok_or_else(|| num_type_err(&args[1]))?;
            let n = match b {
                Num::Int(k) => k,
                Num::Float(_) => unreachable!("guarded above"),
            };
            let r = f_char_int(a, n);
            let cp = r as u32;
            let c = char::from_u32(cp).ok_or_else(|| EvalError::Native {
                message: format!("{op}: result {r} is not a valid char codepoint"),
                span: args[0].span_or_default(),
            })?;
            Ok(Some(Value::char(c)))
        }
        (None, Some(b)) => {
            // int + char → char (only valid for `+`; `-` is not symmetric on
            // char — `int - char` is a type error in Red). Caller (`subtract`)
            // rejects this path before dispatching.
            if let Value::Float { .. } = &args[0] {
                return Err(EvalError::TypeError {
                    expected: "char! or integer!",
                    found: type_name(&args[0]),
                    span: args[0].span_or_default(),
                });
            }
            let a = as_number(&args[0]).ok_or_else(|| num_type_err(&args[0]))?;
            let n = match a {
                Num::Int(k) => k,
                Num::Float(_) => unreachable!("guarded above"),
            };
            let r = f_char_int(n, b);
            let cp = r as u32;
            let c = char::from_u32(cp).ok_or_else(|| EvalError::Native {
                message: format!("{op}: result {r} is not a valid char codepoint"),
                span: args[0].span_or_default(),
            })?;
            Ok(Some(Value::char(c)))
        }
        (None, None) => Ok(None),
    }
}

/// `EvalError::Native` with the span sourced from `from`.
fn native_err(from: &Value, msg: impl Into<String>) -> EvalError {
    EvalError::Native {
        message: msg.into(),
        span: from.span_or_default(),
    }
}

// ---------------------------------------------------------------------------
// M44: pair! / tuple! arithmetic
// ---------------------------------------------------------------------------

/// Extract the two components of a `pair!` as a tuple of references.
fn pair_components(v: &Value) -> (&Value, &Value) {
    match v {
        Value::Pair { x, y, .. } => (x, y),
        _ => unreachable!("caller checks Value::Pair"),
    }
}

/// M44 pair arithmetic dispatcher. Returns `Some(result)` if either operand
/// is a `pair!`; `None` if neither is (caller falls through to `num_binop`).
/// Component arithmetic delegates to `num_binop` so int/int→int, mixed→float.
///
/// Supported combos (callers gate the asymmetric ones):
/// - `pair OP pair` → pair (componentwise)
/// - `pair OP scalar` → pair (scalar broadcast to both components)
/// - `scalar OP pair` → only valid for commutative `+`/`*`; `subtract`/`divide`
///   reject `scalar - pair` / `scalar / pair` before calling.
fn pair_binop(
    args: &[Value],
    op: &str,
    f_int: fn(i64, i64) -> Option<i64>,
    f_float: fn(f64, f64) -> f64,
) -> Result<Option<Value>, EvalError> {
    let a = &args[0];
    let b = &args[1];
    let a_pair = matches!(a, Value::Pair { .. });
    let b_pair = matches!(b, Value::Pair { .. });
    if !a_pair && !b_pair {
        return Ok(None);
    }
    // Reject non-numeric non-pair operands (e.g. pair + string).
    let check_num = |v: &Value| -> Result<(), EvalError> {
        if !matches!(v, Value::Pair { .. }) && as_number(v).is_none() {
            return Err(num_type_err(v));
        }
        Ok(())
    };
    check_num(a)?;
    check_num(b)?;

    let apply = |x: &Value, y: &Value| -> Result<Value, EvalError> {
        num_binop(&[x.clone(), y.clone()], op, f_int, f_float)
    };

    match (a_pair, b_pair) {
        (true, true) => {
            let (ax, ay) = pair_components(a);
            let (bx, by) = pair_components(b);
            let nx = apply(ax, bx)?;
            let ny = apply(ay, by)?;
            Ok(Some(Value::pair(nx, ny)))
        }
        (true, false) => {
            let (ax, ay) = pair_components(a);
            let nx = apply(ax, b)?;
            let ny = apply(ay, b)?;
            Ok(Some(Value::pair(nx, ny)))
        }
        (false, true) => {
            // scalar OP pair — only reached for commutative +/*. subtract/divide
            // guard this before calling.
            let (bx, by) = pair_components(b);
            let nx = apply(a, bx)?;
            let ny = apply(a, by)?;
            Ok(Some(Value::pair(nx, ny)))
        }
        (false, false) => Ok(None),
    }
}

/// M44 tuple arithmetic dispatcher. Returns `Some(result)` if either operand
/// is a `tuple!`; `None` if neither is. Supported:
/// - `tuple + tuple` → tuple (componentwise, clamped 0–255; lengths must match)
/// - `tuple - tuple` → tuple (clamped)
/// - `tuple * float` → tuple (scaled, clamped)
///
/// Other combos (tuple + int, scalar + tuple, tuple / *) raise TypeError.
fn tuple_binop(
    args: &[Value],
    op: &str,
    _f_int: fn(i64, i64) -> Option<i64>,
    f_float: fn(f64, f64) -> f64,
) -> Result<Option<Value>, EvalError> {
    let a = &args[0];
    let b = &args[1];
    let a_tup = matches!(a, Value::Tuple { .. });
    let b_tup = matches!(b, Value::Tuple { .. });
    if !a_tup && !b_tup {
        return Ok(None);
    }

    let a_bytes = |v: &Value| match v {
        Value::Tuple { bytes, .. } => bytes.clone(),
        _ => unreachable!(),
    };
    let clamp_byte = |r: i64| r.clamp(0, 255) as u8;

    if a_tup && b_tup {
        let a_b = a_bytes(a);
        let b_b = a_bytes(b);
        if a_b.len() != b_b.len() {
            return Err(native_err(
                a,
                format!(
                    "{op}: tuple length mismatch ({} vs {})",
                    a_b.len(),
                    b_b.len()
                ),
            ));
        }
        let out: Vec<u8> = a_b
            .iter()
            .zip(b_b.iter())
            .map(|(&x, &y)| {
                let r = match op {
                    "add" => x as i64 + y as i64,
                    "subtract" => x as i64 - y as i64,
                    _ => unreachable!("tuple_binop: unsupported tuple-tuple op"),
                };
                clamp_byte(r)
            })
            .collect();
        return Ok(Some(Value::tuple(out)));
    }

    if a_tup && !b_tup {
        let a_b = a_bytes(a);
        match b {
            Value::Float { f, .. } => {
                let scale = *f;
                let out: Vec<u8> = a_b
                    .iter()
                    .map(|&x| clamp_byte((f_float(x as f64, scale)).round() as i64))
                    .collect();
                return Ok(Some(Value::tuple(out)));
            }
            Value::Integer { n, .. } => {
                // tuple * int (broadcast, clamped) — only valid for *
                if op != "multiply" {
                    return Err(native_err(
                        b,
                        format!("{op}: tuple OP integer not supported (only tuple * integer)"),
                    ));
                }
                let k = *n;
                let out: Vec<u8> = a_b.iter().map(|&x| clamp_byte(x as i64 * k)).collect();
                return Ok(Some(Value::tuple(out)));
            }
            _ => return Err(num_type_err(b)),
        }
    }

    // scalar OP tuple — not supported (asymmetric).
    Err(native_err(
        a,
        format!("{op}: scalar {op} tuple not supported"),
    ))
}

/// `EvalError::Native` shaped `math` error (for `tuple` arithmetic failures).
#[allow(dead_code)]
fn math_err(from: &Value, msg: impl Into<String>) -> EvalError {
    native_err(from, msg)
}

// ---------------------------------------------------------------------------
// M45: date! arithmetic
// ---------------------------------------------------------------------------

/// `date + integer` → date + N days (zone preserved).
/// `date + date` (time-shaped) → date+time (the second date contributes its
/// time component). `date + date` (full date) → TypeError.
/// Returns `Some(result)` if either operand is a `date!`; `None` otherwise.
fn date_add(args: &[Value]) -> Result<Option<Value>, EvalError> {
    let a = &args[0];
    let b = &args[1];
    let a_date = matches!(a, Value::Date { .. });
    let b_date = matches!(b, Value::Date { .. });
    if !a_date && !b_date {
        return Ok(None);
    }
    match (a, b) {
        // `date + integer` → date + N days.
        (Value::Date { dt, .. }, Value::Integer { n, .. }) => {
            Ok(Some(Value::date(dt.add_days(*n))))
        }
        // `integer + date` → date + N days (commutative).
        (Value::Integer { n, .. }, Value::Date { dt, .. }) => {
            Ok(Some(Value::date(dt.add_days(*n))))
        }
        // `date + date` → only valid if the second is time-shaped (epoch date).
        // The result is the first date with the second's time component.
        (Value::Date { dt: da, .. }, Value::Date { dt: db, .. }) => {
            // If the second date is epoch (1970-01-01), treat it as a time.
            let epoch = red_core::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
            if db.dt.date() == epoch {
                Ok(Some(Value::date(da.with_time(db.dt.time()))))
            } else {
                Err(EvalError::TypeError {
                    expected: "integer! or time! (date + date not supported)",
                    found: "date!",
                    span: b.span_or_default(),
                })
            }
        }
        _ => Ok(None),
    }
}

/// `date - date` → integer (day difference, zone-adjusted on the absolute
/// instant). Returns `Some(result)` if both operands are `date!`; `None`
/// otherwise.
fn date_subtract(args: &[Value]) -> Result<Option<Value>, EvalError> {
    let a = &args[0];
    let b = &args[1];
    match (a, b) {
        (Value::Date { dt: da, .. }, Value::Date { dt: db, .. }) => {
            // Compute the day difference on the absolute instant (zone-adjusted).
            let a_utc = da.to_offset_utc();
            let b_utc = db.to_offset_utc();
            let a_date = a_utc.date_naive();
            let b_date = b_utc.date_naive();
            let diff = (a_date - b_date).num_days();
            Ok(Some(Value::integer(diff)))
        }
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Arithmetic infix: + - * /  (and the shared `num_binop` helper)
// ---------------------------------------------------------------------------

/// Apply a numeric binary operator to `args[0]` (left) and `args[1]` (right).
/// Int+Int → Int; any Float involved → Float. `op` names the operation for
/// error messages. Errors carry the offending operand's span.
fn num_binop(
    args: &[Value],
    op: &str,
    f_int: fn(i64, i64) -> Option<i64>,
    f_float: fn(f64, f64) -> f64,
) -> Result<Value, EvalError> {
    let a = as_number(&args[0]).ok_or_else(|| num_type_err(&args[0]))?;
    let b = as_number(&args[1]).ok_or_else(|| num_type_err(&args[1]))?;
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => match f_int(x, y) {
            Some(r) => Ok(Value::integer(r)),
            // `f_int` returns None to signal a domain error (e.g. div-by-zero).
            // M42: raise a structured `math` error with a numeric code so
            // `try [1 / 0]` produces an error with `type: 'math`.
            None => {
                let span = args[0].span_or_default();
                let near = if span.is_default() {
                    None
                } else {
                    Some(Value::Block {
                        series: red_core::value::Series::new(Vec::new()),
                        span,
                    })
                };
                Err(EvalError::Raised(std::rc::Rc::new(
                    ErrorValue::new_structed(
                        format!("math error: {op} by zero"),
                        Some(400),
                        Some(Symbol::new("math")),
                        Vec::new(),
                        near,
                        None,
                        None,
                    ),
                )))
            }
        },
        (Num::Int(x), Num::Float(y)) => Ok(Value::float(f_float(x as f64, y))),
        (Num::Float(x), Num::Int(y)) => Ok(Value::float(f_float(x, y as f64))),
        (Num::Float(x), Num::Float(y)) => Ok(Value::float(f_float(x, y))),
    }
}

/// M80 percent arithmetic dispatcher. Returns `Some(Value)` for the
/// percent-preserving cases (`percent + percent → percent`,
/// `percent - percent → percent`); returns `None` otherwise so the caller
/// falls through to `num_binop`, which promotes percent → float via
/// `as_number` (`percent + float → float`, `percent * scalar → float`).
///
/// Per `plan8-missing-types.md` M80: `50% + 25%` ⇒ `75%` (stays percent);
/// `50% * 2` ⇒ `1.0` (float). Percent-scalar and percent-percent multiply/
/// divide all promote to float.
fn percent_binop(
    args: &[Value],
    op: &str,
    f_float: fn(f64, f64) -> f64,
) -> Result<Option<Value>, EvalError> {
    let a = &args[0];
    let b = &args[1];
    let a_pct = matches!(a, Value::Percent { .. });
    let b_pct = matches!(b, Value::Percent { .. });
    if a_pct && b_pct {
        // Both percent — preserve the percent wrapper only for add/subtract.
        // (Multiply/divide promote to float even when both operands are
        // percent, matching `percent * scalar → float`.)
        if op == "add" || op == "subtract" || op == "subtraction" {
            let x = match a {
                Value::Percent { value, .. } => *value,
                _ => unreachable!(),
            };
            let y = match b {
                Value::Percent { value, .. } => *value,
                _ => unreachable!(),
            };
            return Ok(Some(Value::percent(f_float(x, y))));
        }
    }
    Ok(None)
}

/// M80 money arithmetic dispatcher. Returns `Some(Value)` when either operand
/// is `money!`; `None` otherwise. Rules (per plan8 M80):
/// - `money + money` (same currency) → money (cents added).
/// - `money - money` (same currency) → money.
/// - `money + integer` → money (treat int as cents).
/// - `money - integer` → money.
/// - `money * integer` → money.
/// - `integer * money` → money.
/// - `money / money` (same currency) → float (ratio of cents).
/// - `money / integer` → money (cents / int, error on non-divisible? no —
///   Red floors; here we just integer-divide cents).
/// - Cross-currency `money OP money` → Native error ("currency mismatch").
/// - Asymmetric `money OP float`, `float OP money`, `money * float` etc →
///   TypeError (money arithmetic is exact; mixing with float loses that).
fn money_binop(args: &[Value], op: &str) -> Result<Option<Value>, EvalError> {
    let a = &args[0];
    let b = &args[1];
    let a_m = matches!(a, Value::Money { .. });
    let b_m = matches!(b, Value::Money { .. });
    if !a_m && !b_m {
        return Ok(None);
    }
    let money_parts = |v: &Value| -> (i64, std::rc::Rc<str>) {
        match v {
            Value::Money { amount, .. } => (amount.cents, amount.currency.clone()),
            _ => unreachable!(),
        }
    };

    // money OP money — same-currency required for add/subtract; multiply is
    // a type error (money * money is meaningless); divide → float ratio.
    if a_m && b_m {
        let (ca, cca) = money_parts(a);
        let (cb, ccb) = money_parts(b);
        match op {
            "add" | "subtract" | "subtraction" => {
                if cca != ccb {
                    return Err(EvalError::Native {
                        message: format!("money error: currency mismatch ({cca} vs {ccb})"),
                        span: a.span_or_default(),
                    });
                }
                let r = if op == "add" {
                    ca.checked_add(cb)
                } else {
                    ca.checked_sub(cb)
                };
                let cents = r.ok_or_else(|| EvalError::Native {
                    message: "money error: integer overflow".into(),
                    span: a.span_or_default(),
                })?;
                return Ok(Some(Value::money(cents, cca)));
            }
            "divide" => {
                if cca != ccb {
                    return Err(EvalError::Native {
                        message: format!("money error: currency mismatch ({cca} vs {ccb})"),
                        span: a.span_or_default(),
                    });
                }
                if cb == 0 {
                    return Err(EvalError::Native {
                        message: "money error: divide by zero".into(),
                        span: a.span_or_default(),
                    });
                }
                return Ok(Some(Value::float(ca as f64 / cb as f64)));
            }
            "multiply" => {
                return Err(EvalError::TypeError {
                    expected: "integer! (money * money is not supported)",
                    found: "money!",
                    span: a.span_or_default(),
                });
            }
            _ => {}
        }
    }

    // money OP integer (or integer OP money for commutative +/*).
    let as_int = |v: &Value| -> Result<i64, EvalError> {
        match v {
            Value::Integer { n, .. } => Ok(*n),
            _ => Err(EvalError::TypeError {
                expected: "integer! (money arithmetic with non-int is a type error)",
                found: type_name(v),
                span: v.span_or_default(),
            }),
        }
    };
    match op {
        "add" | "subtract" | "subtraction" => {
            let (cents, cur) = if a_m {
                let (c, cu) = money_parts(a);
                let n = as_int(b)?;
                let r = if op == "add" {
                    c.checked_add(n)
                } else {
                    c.checked_sub(n)
                };
                (
                    r.ok_or_else(|| EvalError::Native {
                        message: "money error: integer overflow".into(),
                        span: a.span_or_default(),
                    })?,
                    cu,
                )
            } else {
                // int + money (commutative for add; subtract guards below).
                let (c, cu) = money_parts(b);
                let n = as_int(a)?;
                let r = if op == "add" {
                    n.checked_add(c)
                } else {
                    // int - money is asymmetric — handled as a type error.
                    return Err(EvalError::TypeError {
                        expected: "money! (scalar - money is asymmetric)",
                        found: type_name(a),
                        span: a.span_or_default(),
                    });
                };
                (
                    r.ok_or_else(|| EvalError::Native {
                        message: "money error: integer overflow".into(),
                        span: a.span_or_default(),
                    })?,
                    cu,
                )
            };
            Ok(Some(Value::money(cents, cur)))
        }
        "multiply" => {
            let (cents, cur) = if a_m {
                let (c, cu) = money_parts(a);
                let n = as_int(b)?;
                let r = c.checked_mul(n);
                (
                    r.ok_or_else(|| EvalError::Native {
                        message: "money error: integer overflow".into(),
                        span: a.span_or_default(),
                    })?,
                    cu,
                )
            } else {
                let (c, cu) = money_parts(b);
                let n = as_int(a)?;
                let r = n.checked_mul(c);
                (
                    r.ok_or_else(|| EvalError::Native {
                        message: "money error: integer overflow".into(),
                        span: a.span_or_default(),
                    })?,
                    cu,
                )
            };
            Ok(Some(Value::money(cents, cur)))
        }
        "divide" => {
            // money / integer → money (integer-divide cents). integer / money
            // is asymmetric → type error.
            if a_m {
                let (c, cu) = money_parts(a);
                let n = as_int(b)?;
                if n == 0 {
                    return Err(EvalError::Native {
                        message: "money error: divide by zero".into(),
                        span: a.span_or_default(),
                    });
                }
                Ok(Some(Value::money(c / n, cu)))
            } else {
                Err(EvalError::TypeError {
                    expected: "money! (scalar / money is asymmetric)",
                    found: type_name(a),
                    span: a.span_or_default(),
                })
            }
        }
        _ => Ok(None),
    }
}

/// `+` infix — numeric addition, with string concatenation when both operands
/// are strings (M15), and char arithmetic (M38: `char + int → char`,
/// `char + char → int`). M44: pair/tuple arithmetic. Falls through to numeric
/// addition otherwise.
pub(crate) fn add(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if let (Value::String { s: a, .. }, Value::String { s: b, .. }) = (&args[0], &args[1]) {
        let mut cat = String::with_capacity(a.len() + b.len());
        cat.push_str(a);
        cat.push_str(b);
        return Ok(Value::string(std::rc::Rc::from(cat.as_str())));
    }
    if let Some(r) = char_binop(args, "add", |a, b| a + b, |a, b| a + b)? {
        return Ok(r);
    }
    if let Some(r) = pair_binop(args, "add", |a, b| Some(a + b), |a, b| a + b)? {
        return Ok(r);
    }
    if let Some(r) = tuple_binop(args, "add", |a, b| Some(a + b), |a, b| a + b)? {
        return Ok(r);
    }
    // M45: date arithmetic.
    if let Some(r) = date_add(args)? {
        return Ok(r);
    }
    // M80: money arithmetic (same-currency add/sub; money ± int; money * int).
    if let Some(r) = money_binop(args, "add")? {
        return Ok(r);
    }
    // M80: percent + percent → percent (everything else promotes to float
    // via num_binop).
    if let Some(r) = percent_binop(args, "add", |a, b| a + b)? {
        return Ok(r);
    }
    num_binop(args, "add", |a, b| Some(a + b), |a, b| a + b)
}

/// `-` infix — numeric subtraction, with char arithmetic (M38:
/// `char - char → int`, `char - int → char`). M44: pair/tuple arithmetic.
/// `int - pair`/`int - tuple`/`scalar - tuple` are type errors (asymmetric).
pub(crate) fn subtract(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    // `int - char`/`int - pair`/`int - tuple` is not allowed (asymmetric).
    if as_codepoint(&args[0]).is_none()
        && !matches!(&args[0], Value::Pair { .. } | Value::Tuple { .. })
        && (as_codepoint(&args[1]).is_some()
            || matches!(&args[1], Value::Pair { .. } | Value::Tuple { .. }))
    {
        return Err(EvalError::TypeError {
            expected: "char!, pair!, tuple!, or float!",
            found: type_name(&args[0]),
            span: args[0].span_or_default(),
        });
    }
    if let Some(r) = char_binop(args, "subtract", |a, b| a - b, |a, b| a - b)? {
        return Ok(r);
    }
    if let Some(r) = pair_binop(args, "subtraction", |a, b| Some(a - b), |a, b| a - b)? {
        return Ok(r);
    }
    if let Some(r) = tuple_binop(args, "subtract", |a, b| Some(a - b), |a, b| a - b)? {
        return Ok(r);
    }
    // M45: date subtraction (`date - date → integer`).
    if let Some(r) = date_subtract(args)? {
        return Ok(r);
    }
    // M80: money subtraction (same-currency; or money - int).
    if let Some(r) = money_binop(args, "subtract")? {
        return Ok(r);
    }
    // M80: percent - percent → percent (everything else promotes to float).
    if let Some(r) = percent_binop(args, "subtraction", |a, b| a - b)? {
        return Ok(r);
    }
    num_binop(args, "subtraction", |a, b| Some(a - b), |a, b| a - b)
}

/// `*` infix — numeric multiplication. M44: pair/tuple arithmetic.
pub(crate) fn multiply(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if let Some(r) = pair_binop(args, "multiply", |a, b| Some(a * b), |a, b| a * b)? {
        return Ok(r);
    }
    if let Some(r) = tuple_binop(args, "multiply", |a, b| Some(a * b), |a, b| a * b)? {
        return Ok(r);
    }
    // M80: money * integer → money (integer * money → money).
    if let Some(r) = money_binop(args, "multiply")? {
        return Ok(r);
    }
    num_binop(args, "multiply", |a, b| Some(a * b), |a, b| a * b)
}

/// `/` infix — numeric division. Integer division by zero → error. M44:
/// pair/scalar division (pair/pair not supported). Tuple division is a type
/// error (tuples are bytes, not divisible).
pub(crate) fn divide(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    // `int / pair`/`scalar / tuple` — asymmetric, type error.
    if !matches!(&args[0], Value::Pair { .. } | Value::Tuple { .. })
        && matches!(&args[1], Value::Pair { .. } | Value::Tuple { .. })
    {
        return Err(EvalError::TypeError {
            expected: "number!",
            found: type_name(&args[1]),
            span: args[1].span_or_default(),
        });
    }
    if let Some(r) = pair_binop(
        args,
        "division",
        |a, b| if b == 0 { None } else { Some(a / b) },
        |a, b| a / b,
    )? {
        return Ok(r);
    }
    if matches!(&args[0], Value::Tuple { .. }) || matches!(&args[1], Value::Tuple { .. }) {
        return Err(native_err(
            &args[0],
            "division: tuple division not supported",
        ));
    }
    // M80: money / money → float (ratio); money / integer → money.
    if let Some(r) = money_binop(args, "divide")? {
        return Ok(r);
    }
    num_binop(
        args,
        "division",
        |a, b| {
            if b == 0 {
                None
            } else {
                Some(a / b)
            }
        },
        |a, b| a / b,
    )
}

// ---------------------------------------------------------------------------
// `//` modulo (infix)
// ---------------------------------------------------------------------------

/// `a // b` — remainder. Int+Int → Int (C-style `%`, sign of dividend);
/// any Float involved → Float (`%`). Zero divisor → error.
pub(crate) fn modulo(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    let a = as_number(&args[0]).ok_or_else(|| num_type_err(&args[0]))?;
    let b = as_number(&args[1]).ok_or_else(|| num_type_err(&args[1]))?;
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => {
            if y == 0 {
                return Err(math_by_zero(args, "integer modulo"));
            }
            Ok(Value::integer(x % y))
        }
        (Num::Int(x), Num::Float(y)) => {
            if y == 0.0 {
                return Err(math_by_zero(args, "float modulo"));
            }
            Ok(Value::float((x as f64) % y))
        }
        (Num::Float(x), Num::Int(y)) => {
            if y == 0 {
                return Err(math_by_zero(args, "float modulo"));
            }
            Ok(Value::float(x % (y as f64)))
        }
        (Num::Float(x), Num::Float(y)) => {
            if y == 0.0 {
                return Err(math_by_zero(args, "float modulo"));
            }
            Ok(Value::float(x % y))
        }
    }
}

// ---------------------------------------------------------------------------
// abs, negate
// ---------------------------------------------------------------------------

fn abs_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "abs", 1, args.len()));
    }
    match &args[0] {
        Value::Integer { n, .. } => Ok(Value::integer(n.wrapping_abs())),
        Value::Float { f, .. } => Ok(Value::float(f.abs())),
        // M80: abs on a percent preserves the percent wrapper.
        Value::Percent { value, .. } => Ok(Value::percent(value.abs())),
        // M80: abs on money preserves the currency; abs the cents.
        Value::Money { amount, .. } => {
            Ok(Value::money(amount.cents.abs(), amount.currency.clone()))
        }
        // M44: abs on a pair → componentwise abs.
        Value::Pair { x, y, .. } => {
            let nx = abs_one(x)?;
            let ny = abs_one(y)?;
            Ok(Value::pair(nx, ny))
        }
        Value::Tuple { .. } => Err(EvalError::TypeError {
            expected: "number! or pair!",
            found: "tuple!",
            span: args[0].span_or_default(),
        }),
        other => Err(num_type_err(other)),
    }
}

/// Helper: `abs` of a single number value (int/float). Errors on non-numbers.
fn abs_one(v: &Value) -> Result<Value, EvalError> {
    match as_number(v) {
        Some(Num::Int(n)) => Ok(Value::integer(n.wrapping_abs())),
        Some(Num::Float(f)) => Ok(Value::float(f.abs())),
        None => Err(num_type_err(v)),
    }
}

fn negate_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "negate", 1, args.len()));
    }
    match &args[0] {
        Value::Integer { n, .. } => Ok(Value::integer(n.wrapping_neg())),
        Value::Float { f, .. } => Ok(Value::float(-f)),
        // M80: negate on a percent preserves the percent wrapper.
        Value::Percent { value, .. } => Ok(Value::percent(-value)),
        // M80: negate on money preserves the currency; negate the cents.
        Value::Money { amount, .. } => Ok(Value::money(
            amount.cents.wrapping_neg(),
            amount.currency.clone(),
        )),
        // M44: negate on a pair → componentwise negate.
        Value::Pair { x, y, .. } => {
            let nx = negate_one(x)?;
            let ny = negate_one(y)?;
            Ok(Value::pair(nx, ny))
        }
        Value::Tuple { .. } => Err(EvalError::TypeError {
            expected: "number! or pair!",
            found: "tuple!",
            span: args[0].span_or_default(),
        }),
        other => Err(num_type_err(other)),
    }
}

/// Helper: negate of a single number value (int/float).
fn negate_one(v: &Value) -> Result<Value, EvalError> {
    match as_number(v) {
        Some(Num::Int(n)) => Ok(Value::integer(n.wrapping_neg())),
        Some(Num::Float(f)) => Ok(Value::float(-f)),
        None => Err(num_type_err(v)),
    }
}

// ---------------------------------------------------------------------------
// add / subtract / multiply / divide (prefix word aliases for + - * /)
// ---------------------------------------------------------------------------

/// M31: look up an infix native (`+`/`-`/`*`/`/`) by symbol, returning the
/// `NativeFn`. Was four near-identical `.expect()`/`.unwrap()` sites — a
/// registration-order bug (the infix native missing from `env.natives` when
/// the prefix alias is invoked) panicked with an opaque message. Now returns
/// a recoverable `EvalError::Compile` (VmInvariant) with the offending arg's
/// span, so a misregistration surfaces as a located error rather than a
/// release panic. Standardizes the mixed `expect`/`unwrap` style.
fn infix_lookup(env: &Env, sym: &str, span: red_core::value::Span) -> Result<NativeFn, EvalError> {
    let fd = env
        .natives
        .get(&Symbol::new(sym))
        .ok_or_else(|| EvalError::Compile {
            kind: CompileErrorKind::VmInvariant(format!(
                "{sym:?} native not registered before math prefix alias"
            )),
            span,
        })?;
    fd.native.ok_or_else(|| EvalError::Compile {
        kind: CompileErrorKind::VmInvariant(format!("{sym:?} native has no handler function")),
        span,
    })
}

fn add_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "add", 2, args.len()));
    }
    // Delegate to the infix `+` so string concatenation + numeric promotion
    // stay in one place.
    let plus = infix_lookup(env, "+", args[0].span_or_default())?;
    plus(
        &[args[0].clone(), args[1].clone()],
        &RefineArgs::empty(),
        env,
    )
}

fn subtract_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "subtract", 2, args.len()));
    }
    let f = infix_lookup(env, "-", args[0].span_or_default())?;
    f(
        &[args[0].clone(), args[1].clone()],
        &RefineArgs::empty(),
        env,
    )
}

fn multiply_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "multiply", 2, args.len()));
    }
    let f = infix_lookup(env, "*", args[0].span_or_default())?;
    f(
        &[args[0].clone(), args[1].clone()],
        &RefineArgs::empty(),
        env,
    )
}

fn divide_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "divide", 2, args.len()));
    }
    let f = infix_lookup(env, "/", args[0].span_or_default())?;
    f(
        &[args[0].clone(), args[1].clone()],
        &RefineArgs::empty(),
        env,
    )
}

// ---------------------------------------------------------------------------
// min / max
// ---------------------------------------------------------------------------

/// Numeric ordering. Returns `Less`/`Equal`/`Greater`. M38: also accepts
/// `char!` operands (compared by codepoint) so `min`/`max` work on chars.
fn num_ordering(a: &Value, b: &Value) -> Result<std::cmp::Ordering, EvalError> {
    let x = as_orderable(a).ok_or_else(|| num_type_err(a))?;
    let y = as_orderable(b).ok_or_else(|| num_type_err(b))?;
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

/// Like `as_number` but also accepts `char!` (by codepoint) — used only by
/// `num_ordering` (so `min`/`max` accept chars while `+`/`-`/`*`/`/` use the
/// dedicated `char_binop` path).
fn as_orderable(v: &Value) -> Option<Num> {
    match v {
        Value::Integer { n, .. } => Some(Num::Int(*n)),
        Value::Float { f, .. } => Some(Num::Float(*f)),
        Value::Char { c, .. } => Some(Num::Int(*c as i64)),
        _ => None,
    }
}

fn min_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "min", 2, args.len()));
    }
    // M44: pair + pair → componentwise min. Tuple → type error.
    if matches!(&args[0], Value::Tuple { .. }) || matches!(&args[1], Value::Tuple { .. }) {
        return Err(EvalError::TypeError {
            expected: "number! or pair!",
            found: "tuple!",
            span: args[0].span_or_default(),
        });
    }
    if let (Value::Pair { x: ax, y: ay, .. }, Value::Pair { x: bx, y: by, .. }) =
        (&args[0], &args[1])
    {
        let nx = match num_ordering(ax, bx)? {
            std::cmp::Ordering::Greater => (**bx).clone(),
            _ => (**ax).clone(),
        };
        let ny = match num_ordering(ay, by)? {
            std::cmp::Ordering::Greater => (**by).clone(),
            _ => (**ay).clone(),
        };
        return Ok(Value::pair(nx, ny));
    }
    if matches!(&args[0], Value::Pair { .. }) || matches!(&args[1], Value::Pair { .. }) {
        return Err(EvalError::TypeError {
            expected: "pair!",
            found: if matches!(&args[0], Value::Pair { .. }) {
                type_name(&args[1])
            } else {
                type_name(&args[0])
            },
            span: args[0].span_or_default(),
        });
    }
    Ok(match num_ordering(&args[0], &args[1])? {
        std::cmp::Ordering::Greater => args[1].clone(),
        _ => args[0].clone(),
    })
}

fn max_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "max", 2, args.len()));
    }
    // M44: pair + pair → componentwise max. Tuple → type error.
    if matches!(&args[0], Value::Tuple { .. }) || matches!(&args[1], Value::Tuple { .. }) {
        return Err(EvalError::TypeError {
            expected: "number! or pair!",
            found: "tuple!",
            span: args[0].span_or_default(),
        });
    }
    if let (Value::Pair { x: ax, y: ay, .. }, Value::Pair { x: bx, y: by, .. }) =
        (&args[0], &args[1])
    {
        let nx = match num_ordering(ax, bx)? {
            std::cmp::Ordering::Less => (**bx).clone(),
            _ => (**ax).clone(),
        };
        let ny = match num_ordering(ay, by)? {
            std::cmp::Ordering::Less => (**by).clone(),
            _ => (**ay).clone(),
        };
        return Ok(Value::pair(nx, ny));
    }
    if matches!(&args[0], Value::Pair { .. }) || matches!(&args[1], Value::Pair { .. }) {
        return Err(EvalError::TypeError {
            expected: "pair!",
            found: if matches!(&args[0], Value::Pair { .. }) {
                type_name(&args[1])
            } else {
                type_name(&args[0])
            },
            span: args[0].span_or_default(),
        });
    }
    Ok(match num_ordering(&args[0], &args[1])? {
        std::cmp::Ordering::Less => args[1].clone(),
        _ => args[0].clone(),
    })
}

// ---------------------------------------------------------------------------
// round (+ /to + /even)
// ---------------------------------------------------------------------------

/// Round half away from zero to the nearest integer (Red's default). With
/// `/even`, use banker's rounding (round half to even).
fn round_half(x: f64, even: bool) -> f64 {
    let frac = x.fract();
    let whole = x.trunc();
    if frac.abs() < 0.5 {
        return whole;
    }
    if frac.abs() > 0.5 {
        return if x >= 0.0 { whole + 1.0 } else { whole - 1.0 };
    }
    // Exactly halfway.
    if even {
        // Round to even.
        let whole_int = whole as i64;
        if whole_int & 1 == 0 {
            return whole;
        }
        return if x >= 0.0 { whole + 1.0 } else { whole - 1.0 };
    }
    // Half away from zero.
    if x >= 0.0 {
        whole + 1.0
    } else {
        whole - 1.0
    }
}

fn round_native(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "round", 1, 0));
    }
    let x = match as_number(&args[0]) {
        Some(Num::Int(n)) => n as f64,
        Some(Num::Float(f)) => f,
        None => return Err(num_type_err(&args[0])),
    };
    let even = refs.has(&Symbol::new("even"));

    // `/to scale` — round to the nearest multiple of `scale`. Returns a value
    // of the same numeric kind as the input.
    if let Some(to_args) = refs.get(&Symbol::new("to")) {
        let scale = match to_args.first() {
            Some(Value::Integer { n, .. }) => *n as f64,
            Some(Value::Float { f, .. }) => *f,
            Some(other) => return Err(num_type_err(other)),
            None => {
                return Err(EvalError::Arity {
                    native: Symbol::new("round"),
                    expected: 2,
                    got: 1,
                    span: args[0].span_or_default(),
                })
            }
        };
        if scale == 0.0 {
            return Err(native_err(&args[0], "round: /to scale must not be zero"));
        }
        let n = round_half(x / scale, even);
        let result = n * scale;
        return Ok(match &args[0] {
            Value::Integer { .. } => Value::integer(result as i64),
            _ => Value::float(result),
        });
    }

    // No scale: round to the nearest integer (Red returns an integer here).
    let n = round_half(x, even) as i64;
    Ok(Value::integer(n))
}

// ---------------------------------------------------------------------------
// random (+ /seed + /only + /secure)
// ---------------------------------------------------------------------------

thread_local! {
    /// Per-thread LCG state. Seeded from a fixed constant so output is
    /// deterministic across runs (no entropy dependency in the POC).
    static RNG: Cell<u64> = const { Cell::new(0x9E3779B97F4A7C15) };
}

/// Draw the next u64 from the LCG (xorshift64* variant).
fn rand_next() -> u64 {
    RNG.with(|cell| {
        let mut s = cell.get();
        if s == 0 {
            s = 0x9E3779B97F4A7C15;
        }
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        cell.set(s);
        s
    })
}

fn random_native(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "random", 1, 0));
    }
    // `/seed` — reseed the LCG from the (positional) value and return none.
    // In Red `random/seed value` reuses the value slot as the seed rather than
    // taking a separate refinement argument, so `/seed` declares 0 args and
    // we read the seed from `args[0]`.
    if refs.has(&Symbol::new("seed")) {
        let seed = match &args[0] {
            Value::Integer { n, .. } => *n as u64,
            Value::Float { f, .. } => f.to_bits(),
            other => return Err(num_type_err(other)),
        };
        RNG.with(|cell| cell.set(seed | 1));
        return Ok(Value::None);
    }
    // `/only` is accepted for parity with Red (selects a single element from
    // a block); for scalar inputs it's a no-op in the POC.
    // `/secure` falls back to the LCG (no entropy source wired in yet).
    match &args[0] {
        Value::Integer { n, .. } => {
            // Red: `random n` returns 1..=n for positive n.
            let n = *n;
            if n <= 0 {
                return Err(native_err(&args[0], "random: argument must be positive"));
            }
            let span = n as u64;
            let r = (rand_next() % span) + 1;
            Ok(Value::integer(r as i64))
        }
        Value::Float { f, .. } => {
            // Red: `random f` returns a float in [0, f).
            let f = *f;
            if f <= 0.0 {
                return Err(native_err(&args[0], "random: argument must be positive"));
            }
            let r = (rand_next() as f64 / u64::MAX as f64) * f;
            Ok(Value::float(r))
        }
        other => Err(EvalError::TypeError {
            expected: "integer! or float!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

// ---------------------------------------------------------------------------
// power (** / infix + prefix alias)
// ---------------------------------------------------------------------------

/// `a ** b` (and `power a b`). Both integers with non-negative exponent →
/// integer power; otherwise float via `f64::powf`. Integer overflow wraps.
pub(crate) fn power(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "power", 2, args.len()));
    }
    match (as_number(&args[0]), as_number(&args[1])) {
        (Some(Num::Int(base)), Some(Num::Int(exp))) if exp >= 0 => {
            let mut result: i64 = 1;
            for _ in 0..exp {
                result = result.wrapping_mul(base);
            }
            Ok(Value::integer(result))
        }
        (Some(Num::Int(base)), Some(Num::Int(exp))) => {
            Ok(Value::float((base as f64).powf(exp as f64)))
        }
        (Some(a), Some(b)) => {
            let af = match a {
                Num::Int(n) => n as f64,
                Num::Float(f) => f,
            };
            let bf = match b {
                Num::Int(n) => n as f64,
                Num::Float(f) => f,
            };
            Ok(Value::float(af.powf(bf)))
        }
        (None, _) => Err(num_type_err(&args[0])),
        (_, None) => Err(num_type_err(&args[1])),
    }
}

// ---------------------------------------------------------------------------
// Trig & transcendentals (M40)
// ---------------------------------------------------------------------------

/// Promote a single numeric arg to `f64`. Errors via `num_type_err` on
/// non-numeric input. `name` is the native name used in arity messages.
fn as_float_arg(args: &[Value], name: &str) -> Result<f64, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, name, 1, args.len()));
    }
    Ok(match as_number(&args[0]) {
        Some(Num::Int(n)) => n as f64,
        Some(Num::Float(f)) => f,
        None => return Err(num_type_err(&args[0])),
    })
}

fn sine(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "sin")?;
    Ok(Value::float(f64::sin(x)))
}

fn cosine(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "cos")?;
    Ok(Value::float(f64::cos(x)))
}

fn tangent(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "tan")?;
    Ok(Value::float(f64::tan(x)))
}

fn arcsine(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "asin")?;
    Ok(Value::float(f64::asin(x)))
}

fn arccosine(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "acos")?;
    Ok(Value::float(f64::acos(x)))
}

fn arctangent(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "atan")?;
    Ok(Value::float(f64::atan(x)))
}

/// `atan2 y x` — 2-arg arctangent. Note Red's argument order is `(y, x)`,
/// matching `f64::atan2(y, x)`.
fn arctangent2(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "atan2", 2, args.len()));
    }
    let y = match as_number(&args[0]) {
        Some(Num::Int(n)) => n as f64,
        Some(Num::Float(f)) => f,
        None => return Err(num_type_err(&args[0])),
    };
    let x = match as_number(&args[1]) {
        Some(Num::Int(n)) => n as f64,
        Some(Num::Float(f)) => f,
        None => return Err(num_type_err(&args[1])),
    };
    Ok(Value::float(f64::atan2(y, x)))
}

fn sqrt_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "sqrt")?;
    if x < 0.0 {
        return Err(native_err(&args[0], "math error: sqrt of negative"));
    }
    Ok(Value::float(f64::sqrt(x)))
}

fn exp_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "exp")?;
    Ok(Value::float(f64::exp(x)))
}

fn log_e(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "log-e")?;
    if x <= 0.0 {
        return Err(native_err(&args[0], "math error: log-e of non-positive"));
    }
    Ok(Value::float(f64::ln(x)))
}

fn log_10(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "log-10")?;
    if x <= 0.0 {
        return Err(native_err(&args[0], "math error: log-10 of non-positive"));
    }
    Ok(Value::float(f64::log10(x)))
}

fn log_2(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "log-2")?;
    if x <= 0.0 {
        return Err(native_err(&args[0], "math error: log-2 of non-positive"));
    }
    Ok(Value::float(f64::log2(x)))
}

fn degrees_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "degrees")?;
    Ok(Value::float(x.to_degrees()))
}

fn radians_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let x = as_float_arg(args, "radians")?;
    Ok(Value::float(x.to_radians()))
}

// ---------------------------------------------------------------------------
// even? / odd?
// ---------------------------------------------------------------------------

fn even_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "even?", 1, args.len()));
    }
    match &args[0] {
        Value::Integer { n, .. } => Ok(Value::Logic(n & 1 == 0)),
        other => Err(EvalError::TypeError {
            expected: "integer!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

fn odd_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "odd?", 1, args.len()));
    }
    match &args[0] {
        Value::Integer { n, .. } => Ok(Value::Logic(n & 1 != 0)),
        other => Err(EvalError::TypeError {
            expected: "integer!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Bitwise: xor, complement, shift-left, shift-right
// ---------------------------------------------------------------------------

/// `a xor b` — bitwise XOR on integers (infix). Logic operands fall back to
/// the truthiness-based XOR (returns logic).
pub(crate) fn xor_op(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    match (&args[0], &args[1]) {
        (Value::Integer { n: a, .. }, Value::Integer { n: b, .. }) => Ok(Value::integer(*a ^ *b)),
        (Value::Logic(a), Value::Logic(b)) => Ok(Value::Logic(*a != *b)),
        _ => Ok(Value::Logic(truthy(&args[0]) != truthy(&args[1]))),
    }
}

fn complement_native(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "complement", 1, args.len()));
    }
    match &args[0] {
        Value::Integer { n, .. } => Ok(Value::integer(!*n)),
        Value::Bitset(b) => {
            // M46: bitset complement — flip all bits in-place. Returns a new
            // bitset (the original is preserved since we clone the Rc inner).
            let new_bs = b.borrow().clone();
            new_bs.complement();
            Ok(Value::bitset(new_bs))
        }
        other => Err(EvalError::TypeError {
            expected: "integer! or bitset!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

fn shift_left(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "shift-left", 2, args.len()));
    }
    let n = match &args[0] {
        Value::Integer { n, .. } => *n,
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let by = match &args[1] {
        Value::Integer { n, .. } => *n,
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    if by < 0 {
        return Ok(Value::integer((n as u64).wrapping_shr((-by) as u32) as i64));
    }
    Ok(Value::integer(n.wrapping_shl(by as u32)))
}

fn shift_right(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "shift-right", 2, args.len()));
    }
    let n = match &args[0] {
        Value::Integer { n, .. } => *n,
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let by = match &args[1] {
        Value::Integer { n, .. } => *n,
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    if by < 0 {
        return Ok(Value::integer(n.wrapping_shl((-by) as u32)));
    }
    // Arithmetic-style: Red's shift-right is logical on the bit pattern.
    Ok(Value::integer((n as u64).wrapping_shr(by as u32) as i64))
}

// Re-export the truthiness helper from natives so xor's fallback matches
// the rest of the logic ops.
use crate::natives::truthy;

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register the M17 math + bitwise natives.
pub fn register_math_natives(env: &mut Env) {
    use std::rc::Rc;

    let reg = |env: &mut Env, name: &str, f: NF, arity: usize, infix: bool| {
        let params: Vec<Symbol> = (0..arity)
            .map(|i| Symbol::new(&format!("__arg{i}")))
            .collect();
        env.natives.insert(
            Symbol::new(name),
            Rc::new(FuncDef {
                params,
                native: Some(f),
                variadic: false,
                infix,
                ..Default::default()
            }),
        );
    };

    let reg_refined =
        |env: &mut Env, name: &str, f: NF, arity: usize, refines: &[(&str, usize)]| {
            let params: Vec<Symbol> = (0..arity)
                .map(|i| Symbol::new(&format!("__arg{i}")))
                .collect();
            let refinements: Vec<(Symbol, Vec<Symbol>)> = refines
                .iter()
                .map(|(rname, rarity)| {
                    let rargs: Vec<Symbol> = (0..*rarity)
                        .map(|i| Symbol::new(&format!("__{rname}_arg{i}")))
                        .collect();
                    (Symbol::new(rname), rargs)
                })
                .collect();
            env.natives.insert(
                Symbol::new(name),
                Rc::new(FuncDef {
                    params,
                    refinements,
                    native: Some(f),
                    variadic: false,
                    infix: false,
                    ..Default::default()
                }),
            );
        };

    // Prefix arithmetic aliases (arity 2).
    reg(env, "add", add_native as NF, 2, false);
    reg(env, "subtract", subtract_native as NF, 2, false);
    reg(env, "multiply", multiply_native as NF, 2, false);
    reg(env, "divide", divide_native as NF, 2, false);

    // abs / negate (arity 1).
    reg(env, "abs", abs_native as NF, 1, false);
    reg(env, "negate", negate_native as NF, 1, false);

    // min / max (arity 2).
    reg(env, "min", min_native as NF, 2, false);
    reg(env, "max", max_native as NF, 2, false);

    // round (+ /to + /even).
    reg_refined(
        env,
        "round",
        round_native as NF,
        1,
        &[("to", 1), ("even", 0)],
    );

    // random (+ /seed + /only + /secure). `/seed` reuses the positional value
    // as the seed (0 refinement args), matching Red's `random/seed value` form.
    reg_refined(
        env,
        "random",
        random_native as NF,
        1,
        &[("seed", 0), ("only", 0), ("secure", 0)],
    );

    // power (** infix + prefix alias).
    reg(env, "**", power as NF, 2, true);
    reg(env, "power", power as NF, 2, false);

    // even? / odd?
    reg(env, "even?", even_q as NF, 1, false);
    reg(env, "odd?", odd_q as NF, 1, false);

    // Bitwise (integer) — xor is infix; complement/shift-* are prefix.
    reg(env, "xor", xor_op as NF, 2, true);
    reg(env, "complement", complement_native as NF, 1, false);
    reg(env, "shift-left", shift_left as NF, 2, false);
    reg(env, "shift-right", shift_right as NF, 2, false);
}

/// Register the M40 trig + transcendental natives. All prefix-only, arity 1
/// except `atan2` (arity 2). Results are always `float!`; integer args are
/// promoted to float.
pub fn register_transcendental_natives(env: &mut Env) {
    use std::rc::Rc;

    let reg = |env: &mut Env, name: &str, f: NF, arity: usize| {
        let params: Vec<Symbol> = (0..arity)
            .map(|i| Symbol::new(&format!("__arg{i}")))
            .collect();
        env.natives.insert(
            Symbol::new(name),
            Rc::new(FuncDef {
                params,
                native: Some(f),
                variadic: false,
                infix: false,
                ..Default::default()
            }),
        );
    };

    reg(env, "sin", sine as NF, 1);
    reg(env, "cos", cosine as NF, 1);
    reg(env, "tan", tangent as NF, 1);
    reg(env, "asin", arcsine as NF, 1);
    reg(env, "acos", arccosine as NF, 1);
    reg(env, "atan", arctangent as NF, 1);
    reg(env, "atan2", arctangent2 as NF, 2);
    reg(env, "sqrt", sqrt_native as NF, 1);
    reg(env, "exp", exp_native as NF, 1);
    reg(env, "log-e", log_e as NF, 1);
    reg(env, "log-10", log_10 as NF, 1);
    reg(env, "log-2", log_2 as NF, 1);
    reg(env, "degrees", degrees_native as NF, 1);
    reg(env, "radians", radians_native as NF, 1);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::bind_pass;
    use crate::interp::eval;
    use crate::natives::{install_constants, register_natives};
    use red_core::context::Context;
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    struct BufferWriter(Rc<RefCell<Vec<u8>>>);
    impl Write for BufferWriter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn run_capture_val(src: &str) -> Result<(Value, Vec<u8>), String> {
        let body = load_source(src).map_err(|e| e.to_string())?;
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        let block = Value::block(body);
        let val = match eval(&block, &mut env) {
            Ok(v) => v,
            Err(EvalError::Quit(_)) => Value::None,
            Err(e) => return Err(e.to_string()),
        };
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    fn run_capture(src: &str) -> Vec<u8> {
        run_capture_val(src).unwrap().1
    }

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    // --- modulo ---

    #[test]
    fn int_modulo() {
        assert_eq!(mold_to_string(&val("7 // 3")), "1");
        assert_eq!(mold_to_string(&val("10 // 4")), "2");
    }

    #[test]
    fn float_modulo() {
        assert_eq!(mold_to_string(&val("7.0 // 3.0")), "1.0");
    }

    #[test]
    fn modulo_by_zero_errors() {
        assert!(run_capture_val("7 // 0").is_err());
    }

    // --- abs / negate ---

    #[test]
    fn abs_works() {
        assert_eq!(mold_to_string(&val("abs -5")), "5");
        assert_eq!(mold_to_string(&val("abs 5")), "5");
        assert_eq!(mold_to_string(&val("abs -2.5")), "2.5");
    }

    #[test]
    fn negate_works() {
        assert_eq!(mold_to_string(&val("negate 5")), "-5");
        assert_eq!(mold_to_string(&val("negate -3")), "3");
        assert_eq!(mold_to_string(&val("negate 2.5")), "-2.5");
    }

    // --- add/subtract/multiply/divide aliases ---

    #[test]
    fn prefix_arith_aliases() {
        assert_eq!(mold_to_string(&val("add 1 2")), "3");
        assert_eq!(mold_to_string(&val("subtract 10 4")), "6");
        assert_eq!(mold_to_string(&val("multiply 3 4")), "12");
        assert_eq!(mold_to_string(&val("divide 10 2")), "5");
    }

    #[test]
    fn add_alias_concatenates_strings() {
        assert_eq!(mold_to_string(&val("add \"ab\" \"cd\"")), "\"abcd\"");
    }

    // --- min / max ---

    #[test]
    fn min_max_int() {
        assert_eq!(mold_to_string(&val("min 3 5")), "3");
        assert_eq!(mold_to_string(&val("max 3 5")), "5");
    }

    #[test]
    fn min_max_float() {
        assert_eq!(mold_to_string(&val("min 3.5 2.5")), "2.5");
        assert_eq!(mold_to_string(&val("max 3.5 2.5")), "3.5");
    }

    // --- round ---

    #[test]
    fn round_default() {
        assert_eq!(mold_to_string(&val("round 3.6")), "4");
        assert_eq!(mold_to_string(&val("round 3.4")), "3");
        assert_eq!(mold_to_string(&val("round 3.5")), "4");
        assert_eq!(mold_to_string(&val("round -3.5")), "-4");
    }

    #[test]
    fn round_to_scale() {
        assert_eq!(mold_to_string(&val("round/to 3.14159 0.01")), "3.14");
        assert_eq!(mold_to_string(&val("round/to 3.14159 1")), "3.0");
    }

    #[test]
    fn round_even() {
        // Banker's rounding: 2.5 → 2, 3.5 → 4.
        assert_eq!(mold_to_string(&val("round/even 2.5")), "2");
        assert_eq!(mold_to_string(&val("round/even 3.5")), "4");
    }

    // --- random ---

    #[test]
    fn random_int_in_range() {
        let v = val("random 100");
        match v {
            Value::Integer { n, .. } => assert!((1..=100).contains(&n), "got {n}"),
            other => panic!("expected integer, got {other:?}"),
        }
    }

    #[test]
    fn random_seed_is_deterministic() {
        // Same seed → same first draw.
        let a = val("random/seed 42 random 100");
        let b = val("random/seed 42 random 100");
        assert_eq!(mold_to_string(&a), mold_to_string(&b));
    }

    #[test]
    fn random_float_in_range() {
        let v = val("random 1.0");
        match v {
            Value::Float { f, .. } => assert!((0.0..1.0).contains(&f), "got {f}"),
            other => panic!("expected float, got {other:?}"),
        }
    }

    // --- power ---

    #[test]
    fn power_int() {
        assert_eq!(mold_to_string(&val("2 ** 3")), "8");
        assert_eq!(mold_to_string(&val("power 2 10")), "1024");
    }

    #[test]
    fn power_float() {
        assert_eq!(mold_to_string(&val("2.0 ** 0.5")), "1.4142135623730951");
        assert_eq!(mold_to_string(&val("2 ** -1")), "0.5");
    }

    // --- even? / odd? ---

    #[test]
    fn even_odd_predicates() {
        assert_eq!(mold_to_string(&val("even? 4")), "true");
        assert_eq!(mold_to_string(&val("even? 5")), "false");
        assert_eq!(mold_to_string(&val("odd? 5")), "true");
        assert_eq!(mold_to_string(&val("odd? 4")), "false");
    }

    // --- bitwise ---

    #[test]
    fn bitwise_and() {
        assert_eq!(mold_to_string(&val("5 and 3")), "1");
        assert_eq!(mold_to_string(&val("12 and 10")), "8");
    }

    #[test]
    fn bitwise_or() {
        assert_eq!(mold_to_string(&val("5 or 3")), "7");
    }

    #[test]
    fn bitwise_xor() {
        assert_eq!(mold_to_string(&val("5 xor 3")), "6");
    }

    #[test]
    fn logic_and_or_preserved() {
        assert_eq!(mold_to_string(&val("true and false")), "false");
        assert_eq!(mold_to_string(&val("true or false")), "true");
        assert_eq!(mold_to_string(&val("true xor false")), "true");
    }

    #[test]
    fn complement_works() {
        assert_eq!(mold_to_string(&val("complement 0")), "-1");
        assert_eq!(mold_to_string(&val("complement 5")), "-6");
    }

    #[test]
    fn shift_left_right() {
        assert_eq!(mold_to_string(&val("shift-left 1 3")), "8");
        assert_eq!(mold_to_string(&val("shift-right 8 2")), "2");
    }

    // --- end-to-end via print ---

    #[test]
    fn print_modulo_and_power() {
        let out = run_capture("print 7 // 3 print 2 ** 3");
        assert_eq!(s(&out), "1\n8\n");
    }

    // --- M40 trig & transcendentals ---

    /// Extract an f64 from a `Value::Float` (panics otherwise). Used by the
    /// trig tests so float-tolerance assertions don't depend on mold format.
    fn as_f64(v: &Value) -> f64 {
        match v {
            Value::Float { f, .. } => *f,
            Value::Integer { n, .. } => *n as f64,
            other => panic!("expected number, got {other:?}"),
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn trig_basics() {
        assert!(approx(as_f64(&val("sin 0")), 0.0));
        assert!(approx(as_f64(&val("cos 0")), 1.0));
        assert!(approx(as_f64(&val("sin pi / 2")), 1.0));
        assert!(approx(as_f64(&val("cos pi")), -1.0));
    }

    #[test]
    fn inverse_trig() {
        assert!(approx(as_f64(&val("asin 1")), std::f64::consts::FRAC_PI_2));
        assert!(approx(as_f64(&val("acos 1")), 0.0));
        assert!(approx(as_f64(&val("atan 1")), std::f64::consts::FRAC_PI_4));
    }

    #[test]
    fn atan2_basic() {
        assert!(approx(
            as_f64(&val("atan2 1 1")),
            std::f64::consts::FRAC_PI_4
        ));
        assert!(approx(as_f64(&val("atan2 0 1")), 0.0));
        assert!(approx(
            as_f64(&val("atan2 1 0")),
            std::f64::consts::FRAC_PI_2
        ));
    }

    #[test]
    fn sqrt_works() {
        assert!(approx(as_f64(&val("sqrt 16")), 4.0));
        assert!(approx(as_f64(&val("sqrt 2")), std::f64::consts::SQRT_2));
        // sqrt of negative errors.
        assert!(run_capture_val("sqrt -1").is_err());
    }

    #[test]
    fn log_and_exp() {
        assert!(approx(as_f64(&val("log-e e")), 1.0));
        assert!(approx(as_f64(&val("log-10 1000")), 3.0));
        assert!(approx(as_f64(&val("log-2 8")), 3.0));
        assert!(approx(as_f64(&val("exp 1")), std::f64::consts::E));
        // log of non-positive errors.
        assert!(run_capture_val("log-e 0").is_err());
        assert!(run_capture_val("log-10 -5").is_err());
        assert!(run_capture_val("log-2 0").is_err());
    }

    #[test]
    fn degree_radian_conversion() {
        assert!(approx(as_f64(&val("degrees pi")), 180.0));
        assert!(approx(as_f64(&val("radians 180")), std::f64::consts::PI));
        assert!(approx(as_f64(&val("degrees pi / 2")), 90.0));
    }

    #[test]
    fn trig_int_promotion() {
        // Integer arg is promoted to float; result is float.
        assert!(approx(as_f64(&val("sin 0")), 0.0));
        assert!(approx(as_f64(&val("sqrt 16")), 4.0));
        match val("sin 0") {
            Value::Float { .. } => {}
            other => panic!("expected float, got {other:?}"),
        }
    }

    #[test]
    fn trig_type_errors() {
        // Non-numeric arg → type error.
        assert!(run_capture_val("sin \"a\"").is_err());
        assert!(run_capture_val("sqrt true").is_err());
    }

    #[test]
    fn pi_and_e_constants() {
        assert!(approx(as_f64(&val("pi")), std::f64::consts::PI));
        assert!(approx(as_f64(&val("e")), std::f64::consts::E));
    }

    // --- M44 pair! / tuple! arithmetic ---

    #[test]
    fn pair_add_pair() {
        assert_eq!(mold_to_string(&val("100x200 + 50x50")), "150x250");
    }

    #[test]
    fn pair_add_scalar() {
        assert_eq!(mold_to_string(&val("1x2 + 10")), "11x12");
        assert_eq!(mold_to_string(&val("10 + 1x2")), "11x12");
    }

    #[test]
    fn pair_subtract_and_multiply() {
        assert_eq!(mold_to_string(&val("100x200 - 50x50")), "50x150");
        assert_eq!(mold_to_string(&val("2x3 * 3x4")), "6x12");
        assert_eq!(mold_to_string(&val("2x3 * 2")), "4x6");
    }

    #[test]
    fn pair_divide_scalar() {
        assert_eq!(mold_to_string(&val("10x20 / 2")), "5x10");
    }

    #[test]
    fn pair_float_arith() {
        assert_eq!(mold_to_string(&val("100x200 + 1.5x2.5")), "101.5x202.5");
    }

    #[test]
    fn pair_negate_abs_min_max() {
        assert_eq!(mold_to_string(&val("negate 5x10")), "-5x-10");
        assert_eq!(mold_to_string(&val("abs -5x-10")), "5x10");
        assert_eq!(mold_to_string(&val("min 1x2 3x4")), "1x2");
        assert_eq!(mold_to_string(&val("max 1x2 3x4")), "3x4");
    }

    #[test]
    fn tuple_add_subtract() {
        assert_eq!(mold_to_string(&val("255.0.0 + 0.10.0")), "255.10.0");
        assert_eq!(mold_to_string(&val("255.0.0 - 10.20.30")), "245.0.0");
    }

    #[test]
    fn tuple_multiply_float() {
        assert_eq!(mold_to_string(&val("100.50.25 * 0.5")), "50.25.13");
        assert_eq!(mold_to_string(&val("100.50.25 * 2")), "200.100.50");
    }

    #[test]
    fn tuple_length_mismatch_errors() {
        assert!(run_capture_val("255.0.0 + 1.2.3.4").is_err());
    }

    #[test]
    fn int_minus_pair_errors() {
        // Asymmetric: `int - pair` is a type error (like `int - char`).
        assert!(run_capture_val("10 - 1x2").is_err());
    }

    #[test]
    fn tuple_division_errors() {
        assert!(run_capture_val("255.0.0 / 2").is_err());
    }
}
