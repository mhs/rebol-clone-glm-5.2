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

use red_core::value::{FuncDef, Symbol, Value};
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
        _ => None,
    }
}

fn num_type_err(v: &Value) -> EvalError {
    EvalError::TypeError {
        expected: "integer! or float!",
        found: type_name(v),
        span: v.span_or_default(),
    }
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
            None => Err(EvalError::Native {
                message: format!("math error: {op} by zero"),
                span: args[0].span_or_default(),
            }),
        },
        (Num::Int(x), Num::Float(y)) => Ok(Value::float(f_float(x as f64, y))),
        (Num::Float(x), Num::Int(y)) => Ok(Value::float(f_float(x, y as f64))),
        (Num::Float(x), Num::Float(y)) => Ok(Value::float(f_float(x, y))),
    }
}

/// `+` infix — numeric addition, with string concatenation when both operands
/// are strings (M15), and char arithmetic (M38: `char + int → char`,
/// `char + char → int`). Falls through to numeric addition otherwise.
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
    num_binop(args, "add", |a, b| Some(a + b), |a, b| a + b)
}

/// `-` infix — numeric subtraction, with char arithmetic (M38:
/// `char - char → int`, `char - int → char`). Falls through to numeric
/// subtraction otherwise. `int - char` is a type error in Red.
pub(crate) fn subtract(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    // `int - char` is not allowed (asymmetric). Block it before `char_binop`.
    if as_codepoint(&args[0]).is_none() && as_codepoint(&args[1]).is_some() {
        return Err(EvalError::TypeError {
            expected: "char! or float!",
            found: type_name(&args[0]),
            span: args[0].span_or_default(),
        });
    }
    if let Some(r) = char_binop(args, "subtract", |a, b| a - b, |a, b| a - b)? {
        return Ok(r);
    }
    num_binop(args, "subtraction", |a, b| Some(a - b), |a, b| a - b)
}

/// `*` infix — numeric multiplication.
pub(crate) fn multiply(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    num_binop(args, "division", |a, b| Some(a * b), |a, b| a * b)
}

/// `/` infix — numeric division. Integer division by zero → error.
pub(crate) fn divide(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
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
                return Err(EvalError::Native {
                    message: "math error: integer modulo by zero".into(),
                    span: args[0].span_or_default(),
                });
            }
            Ok(Value::integer(x % y))
        }
        (Num::Int(x), Num::Float(y)) => {
            if y == 0.0 {
                return Err(EvalError::Native {
                    message: "math error: float modulo by zero".into(),
                    span: args[0].span_or_default(),
                });
            }
            Ok(Value::float((x as f64) % y))
        }
        (Num::Float(x), Num::Int(y)) => {
            if y == 0 {
                return Err(EvalError::Native {
                    message: "math error: float modulo by zero".into(),
                    span: args[0].span_or_default(),
                });
            }
            Ok(Value::float(x % (y as f64)))
        }
        (Num::Float(x), Num::Float(y)) => {
            if y == 0.0 {
                return Err(EvalError::Native {
                    message: "math error: float modulo by zero".into(),
                    span: args[0].span_or_default(),
                });
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
    match as_number(&args[0]) {
        Some(Num::Int(n)) => Ok(Value::integer(n.wrapping_abs())),
        Some(Num::Float(f)) => Ok(Value::float(f.abs())),
        None => Err(num_type_err(&args[0])),
    }
}

fn negate_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "negate", 1, args.len()));
    }
    match as_number(&args[0]) {
        Some(Num::Int(n)) => Ok(Value::integer(n.wrapping_neg())),
        Some(Num::Float(f)) => Ok(Value::float(-f)),
        None => Err(num_type_err(&args[0])),
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
    Ok(match num_ordering(&args[0], &args[1])? {
        std::cmp::Ordering::Greater => args[1].clone(),
        _ => args[0].clone(),
    })
}

fn max_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "max", 2, args.len()));
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
        other => Err(EvalError::TypeError {
            expected: "integer!",
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
        assert!(approx(as_f64(&val("atan2 1 1")), std::f64::consts::FRAC_PI_4));
        assert!(approx(as_f64(&val("atan2 0 1")), 0.0));
        assert!(approx(as_f64(&val("atan2 1 0")), std::f64::consts::FRAC_PI_2));
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
}
