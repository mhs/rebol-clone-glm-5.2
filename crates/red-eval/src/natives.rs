//! Native (Rust-implemented) operations.
//!
//! Milestone 6 registered the I/O natives `print`, `prin`, `probe`, plus the
//! constant words `none`, `true`, `false`, `newline`.
//!
//! Milestone 7 adds:
//!   - Arithmetic (infix): `+ - * /`
//!   - Comparison (infix): `= <> < > <= >=`
//!   - Logic: `and`, `or` (infix), `not` (prefix)
//!   - Conditionals: `if`, `either`
//!   - Loops: `loop`, `repeat`, `until`, `while`
//!   - Control flow: `break`, `continue` (via `EvalError` unwinds)
//!   - Eval: `do`, `reduce`
//!
//! String rendering note: `print`/`prin`/`probe` mold every argument
//! uniformly (including strings, which appear quoted). This diverges from
//! real Red's `form`-based printing but keeps the POC printer surface small;
//! the divergence is documented for the M12 audit pass.

use std::io::Write;
use std::rc::Rc;

use red_core::context::Context;
use red_core::printer::mold_to_string;
use red_core::value::{FuncDef, Series, Span, Symbol, Value};
use red_core::{Env, EvalError, NativeFn, RefineArgs};

use crate::interp::{eval, eval_expression};

// ---------------------------------------------------------------------------
// I/O natives (M6)
// ---------------------------------------------------------------------------

/// `print`: mold each arg, join with a single space, append a newline.
/// Returns `Value::None`.
fn print(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let joined = join_molded(args);
    let _ = writeln!(env.out, "{joined}");
    Ok(Value::None)
}

/// `prin`: like `print` but without the trailing newline.
fn prin(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let joined = join_molded(args);
    let _ = write!(env.out, "{joined}");
    Ok(Value::None)
}

/// `probe`: print `== <mold>` for each arg (joined with space), newline,
/// and return the first arg (or `none` if no args).
fn probe(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let joined = join_molded(args);
    let _ = writeln!(env.out, "== {joined}");
    Ok(args.first().cloned().unwrap_or(Value::None))
}

fn join_molded(args: &[Value]) -> String {
    let mut out = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&mold_to_string(a));
    }
    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Truthiness rule: only `false` and `none` are falsy; everything else is
/// truthy.
fn truthy(v: &Value) -> bool {
    !matches!(v, Value::None | Value::Logic(false))
}

/// Build an `Arity` error for `native` with the given expected/got counts.
/// The span falls back to the first argument's source position (if any) so
/// the user gets a `file:line:col:` pointer to the call site even though
/// natives don't receive the calling word's span directly.
fn arity_err(args: &[Value], native: &str, expected: usize, got: usize) -> EvalError {
    EvalError::Arity {
        native: Symbol::new(native),
        expected,
        got,
        span: args
            .first()
            .map(|v| v.span_or_default())
            .unwrap_or_default(),
    }
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

pub(crate) fn type_name(v: &Value) -> &'static str {
    match v {
        Value::None => "none!",
        Value::Logic(_) => "logic!",
        Value::Integer { .. } => "integer!",
        Value::Float { .. } => "float!",
        Value::String { .. } => "string!",
        Value::String8(_) => "binary!",
        Value::Word { .. } => "word!",
        Value::SetWord { .. } => "set-word!",
        Value::GetWord { .. } => "get-word!",
        Value::LitWord { .. } => "lit-word!",
        Value::Block { .. } => "block!",
        Value::Paren { .. } => "paren!",
        Value::Func(_) => "function!",
        Value::Path { .. } => "path!",
        Value::Refinement { .. } => "refinement!",
    }
}

/// Extract a `Block` value from `args[idx]`, or raise a TypeError. The error
/// span is taken from the offending argument (its source position when
/// available).
fn expect_block(args: &[Value], idx: usize, native: &str) -> Result<Value, EvalError> {
    match args.get(idx) {
        Some(v @ Value::Block { .. }) => Ok(v.clone()),
        Some(other) => Err(EvalError::TypeError {
            expected: "block!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
        None => Err(EvalError::Arity {
            native: Symbol::new(native),
            expected: idx + 1,
            got: args.len(),
            // No argument to read a span from; fall back to the calling
            // native's first-arg span if present, else zero.
            span: args
                .first()
                .map(|v| v.span_or_default())
                .unwrap_or_default(),
        }),
    }
}

/// Apply a numeric binary operator to `args[0]` (left) and `args[1]` (right).
/// Int+Int → Int; any Float involved → Float. `op` names the operation for
/// error messages. Errors carry the offending operand's span.
fn num_binop(
    args: &[Value],
    op: &str,
    f_int: fn(i64, i64) -> Option<i64>,
    f_float: fn(f64, f64) -> f64,
) -> Result<Value, EvalError> {
    let a = match as_number(&args[0]) {
        Some(n) => n,
        None => {
            return Err(EvalError::TypeError {
                expected: "integer! or float!",
                found: type_name(&args[0]),
                span: args[0].span_or_default(),
            })
        }
    };
    let b = match as_number(&args[1]) {
        Some(n) => n,
        None => {
            return Err(EvalError::TypeError {
                expected: "integer! or float!",
                found: type_name(&args[1]),
                span: args[1].span_or_default(),
            })
        }
    };
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

// ---------------------------------------------------------------------------
// Arithmetic (infix): + - * /
// ---------------------------------------------------------------------------

fn add(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    num_binop(args, "division", |a, b| Some(a + b), |a, b| a + b)
}

fn subtract(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    num_binop(args, "division", |a, b| Some(a - b), |a, b| a - b)
}

fn multiply(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    num_binop(args, "division", |a, b| Some(a * b), |a, b| a * b)
}

fn divide(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
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
// Comparison (infix): = <> < > <= >=
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
        _ => false,
    }
}

fn equal(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::Logic(values_equal(&args[0], &args[1])))
}

fn not_equal(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::Logic(!values_equal(&args[0], &args[1])))
}

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

fn less_than(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::Logic(compare("<", num_cmp(&args[0], &args[1])?)))
}

fn greater_than(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::Logic(compare(">", num_cmp(&args[0], &args[1])?)))
}

fn less_equal(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::Logic(compare("<=", num_cmp(&args[0], &args[1])?)))
}

fn greater_equal(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::Logic(compare(">=", num_cmp(&args[0], &args[1])?)))
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

fn and_op(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::Logic(truthy(&args[0]) && truthy(&args[1])))
}

fn or_op(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::Logic(truthy(&args[0]) || truthy(&args[1])))
}

fn not_op(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::Logic(!truthy(&args[0])))
}

// ---------------------------------------------------------------------------
// Conditionals: if, either
// ---------------------------------------------------------------------------

/// `if cond block` — evaluates `block` if `cond` is truthy, else returns `none`.
fn if_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "if", 2, args.len()));
    }
    if truthy(&args[0]) {
        let body = expect_block(args, 1, "if")?;
        eval(&body, env)
    } else {
        Ok(Value::None)
    }
}

/// `either cond t-block f-block`
fn either(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 3 {
        return Err(arity_err(args, "either", 3, args.len()));
    }
    let t = expect_block(args, 1, "either")?;
    let f = expect_block(args, 2, "either")?;
    if truthy(&args[0]) {
        eval(&t, env)
    } else {
        eval(&f, env)
    }
}

// ---------------------------------------------------------------------------
// Loops: loop, repeat, until, while
// ---------------------------------------------------------------------------

/// `loop block` — evaluates `block` repeatedly until `break`. Returns the
/// break-value (or `none` if `break` had no value).
fn loop_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "loop")?;
    loop {
        match eval(&body, env) {
            Ok(_) => {}
            Err(EvalError::Break(v)) => return Ok(v.unwrap_or(Value::None)),
            Err(EvalError::Continue) => continue,
            Err(e) => return Err(e),
        }
    }
}

/// `repeat 'word count block` — binds `word` to 1..=count, evaluates `block`
/// each iteration. Accepts both lit-word (`'i`) and bare-word (`i`) forms.
fn repeat(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 3 {
        return Err(arity_err(args, "repeat", 3, args.len()));
    }
    let sym = match &args[0] {
        Value::LitWord { sym, .. } => sym.clone(),
        Value::Word { sym, .. } => sym.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let count = match &args[1] {
        Value::Integer { n, .. } => *n,
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let body = expect_block(args, 2, "repeat")?;
    let idx = env
        .user_ctx
        .index_of(&sym)
        .ok_or_else(|| EvalError::UnboundWord {
            sym: sym.clone(),
            span: args[0].span_or_default(),
        })?;
    for n in 1..=count {
        env.user_ctx.set_slot(idx, Value::integer(n));
        match eval(&body, env) {
            Ok(_) => {}
            Err(EvalError::Break(v)) => return Ok(v.unwrap_or(Value::None)),
            Err(EvalError::Continue) => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(Value::None)
}

/// `until block` — evaluates `block` repeatedly until its last value is
/// truthy. Returns `true`.
fn until(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "until")?;
    loop {
        match eval(&body, env) {
            Ok(v) => {
                if truthy(&v) {
                    return Ok(Value::Logic(true));
                }
            }
            Err(EvalError::Break(v)) => return Ok(v.unwrap_or(Value::None)),
            Err(EvalError::Continue) => continue,
            Err(e) => return Err(e),
        }
    }
}

/// `while cond-block body-block` — evaluates `cond-block`; if truthy,
/// evaluates `body-block` and repeats. Returns `none`.
fn while_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "while", 2, args.len()));
    }
    let cond = expect_block(args, 0, "while")?;
    let body = expect_block(args, 1, "while")?;
    loop {
        let c = eval(&cond, env)?;
        if !truthy(&c) {
            return Ok(Value::None);
        }
        match eval(&body, env) {
            Ok(_) => {}
            Err(EvalError::Break(v)) => return Ok(v.unwrap_or(Value::None)),
            Err(EvalError::Continue) => continue,
            Err(e) => return Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Control flow: break, continue
// ---------------------------------------------------------------------------

/// `break` — unwinds out of the enclosing loop via `EvalError::Break`.
fn break_native(_args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Err(EvalError::Break(None))
}

/// `continue` — skips to the next iteration of the enclosing loop via
/// `EvalError::Continue`.
fn continue_native(
    _args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    Err(EvalError::Continue)
}

// ---------------------------------------------------------------------------
// Eval: do, reduce
// ---------------------------------------------------------------------------

/// `do block` — evaluates a block, returning the last value.
fn do_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "do")?;
    eval(&body, env)
}

/// `reduce block` — evaluates each expression in the block, returning a new
/// block of the results.
fn reduce(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "reduce")?;
    let series = match &body {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        _ => return Ok(Value::None),
    };
    let data = series.data.borrow();
    let mut results = Vec::new();
    let mut i = series.index;
    while i < data.len() {
        results.push(eval_expression(&data, &mut i, env)?);
    }
    drop(data);
    Ok(Value::block(Series::new(results)))
}

// ---------------------------------------------------------------------------
// Functions: func, does, make, function?, return (M9)
// ---------------------------------------------------------------------------

/// `func [spec] [body]` — create a user-defined function value. `spec` is a
/// block of word/lit-word parameter names; `body` is the body block. The body
/// is bound at creation time to a fresh function-local context (params +
/// body-local SetWords become `Binding::Func`), with outer user-context words
/// (recursion, globals) bound as `Binding::Local`. Returns `Value::Func`.
fn func_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "func", 2, args.len()));
    }
    let spec_block = expect_block(args, 0, "func")?;
    let body_block = expect_block(args, 1, "func")?;
    let spec = extract_spec(&spec_block)?;
    let body_series = match &body_block {
        Value::Block { series, .. } => series.clone(),
        _ => unreachable!("expect_block guarantees Block"),
    };
    let mut fd = FuncDef {
        params: spec.params,
        refinements: spec.refinements,
        body: body_series,
        native: None,
        variadic: false,
        infix: false,
        ..Default::default()
    };
    crate::binding::bind_function_body(&mut fd, &env.user_ctx);
    Ok(Value::Func(Rc::new(fd)))
}

/// `does [body]` — zero-argument `func`. Returns `Value::Func`.
fn does_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "does", 1, args.len()));
    }
    let body_block = expect_block(args, 0, "does")?;
    let body_series = match &body_block {
        Value::Block { series, .. } => series.clone(),
        _ => unreachable!("expect_block guarantees Block"),
    };
    let mut fd = FuncDef {
        params: Vec::new(),
        body: body_series,
        native: None,
        variadic: false,
        infix: false,
        ..Default::default()
    };
    crate::binding::bind_function_body(&mut fd, &env.user_ctx);
    Ok(Value::Func(Rc::new(fd)))
}

/// `make <type> <spec>` — currently only `make function! [[spec][body]]` is
/// supported. The single spec block must contain exactly two sub-blocks:
/// the parameter spec and the body.
fn make_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "make", 2, args.len()));
    }
    let type_sym = match &args[0] {
        Value::LitWord { sym, .. } => sym.clone(),
        Value::Word { sym, .. } => sym.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    if type_sym.as_str() != "function!" && type_sym.as_str() != "function" {
        return Err(EvalError::Native {
            message: format!("make: {:?} type not supported in POC", type_sym.as_str()),
            span: args[0].span_or_default(),
        });
    }
    let packed = expect_block(args, 1, "make")?;
    let packed_series = match &packed {
        Value::Block { series, .. } => series.clone(),
        _ => unreachable!("expect_block guarantees Block"),
    };
    let data = packed_series.data.borrow();
    if data.len() != 2 {
        return Err(EvalError::Native {
            message: "make function!: packed block must be [[spec][body]]".to_string(),
            span: args[1].span_or_default(),
        });
    }
    let spec_block = match &data[0] {
        Value::Block { .. } => data[0].clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let body_block = match &data[1] {
        Value::Block { .. } => data[1].clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    drop(data);
    func_native(&[spec_block, body_block], &RefineArgs::empty(), env)
}

/// Result of parsing a `func`/`does` spec block: positional parameter
/// names plus declared refinements (each a name + its argument-word names).
struct FuncSpec {
    params: Vec<Symbol>,
    refinements: Vec<(Symbol, Vec<Symbol>)>,
}

/// Extract parameter symbols and refinements from a func spec block.
///
/// Spec grammar (POC subset):
///   spec := item*
///   item := word | lit-word | refinement
///   refinement := `/name` word*    — `/name` introduces a refinement; the
///                                     following words (until the next
///                                     refinement or end) are its argument
///                                     words.
///
/// Words become positional params (in order). A refinement and its args are
/// recorded in `refinements` in declaration order. Type annotations and
/// locals markers (e.g. `<local>`) are skipped.
fn extract_spec(spec_block: &Value) -> Result<FuncSpec, EvalError> {
    let series = match spec_block {
        Value::Block { series, .. } => series.clone(),
        _ => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(spec_block),
                span: spec_block.span_or_default(),
            })
        }
    };
    let data = series.data.borrow();
    let mut params = Vec::new();
    let mut refinements: Vec<(Symbol, Vec<Symbol>)> = Vec::new();
    for v in data.iter() {
        match v {
            Value::Word { sym, .. } | Value::LitWord { sym, .. } => {
                if let Some(last) = refinements.last_mut() {
                    last.1.push(sym.clone());
                } else {
                    params.push(sym.clone());
                }
            }
            Value::Refinement { sym, .. } => {
                refinements.push((sym.clone(), Vec::new()));
            }
            _ => {
                // Skip type annotations / locals markers.
            }
        }
    }
    Ok(FuncSpec {
        params,
        refinements,
    })
}

/// `function? value` — `true` if value is a `function!`, else `false`.
fn function_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "function?", 1, 0));
    }
    Ok(Value::Logic(matches!(args[0], Value::Func(_))))
}

/// `return [value]` — unwinds out of the enclosing function via
/// `EvalError::Return`. With no argument, returns `none`. Caught by
/// `call_user_func` in `interp.rs`.
fn return_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let v = args.first().cloned().unwrap_or(Value::None);
    Err(EvalError::Return(v))
}

// ---------------------------------------------------------------------------
// Binding natives: get, set, value?, use, bind (M9)
// ---------------------------------------------------------------------------

/// `get 'word` — returns the value bound to `word` in the user context.
/// Errors if the word has no value. The word operand is a lit-word (`'foo`)
/// or an unbound word (`foo`).
fn get_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "get", 1, args.len()));
    }
    let sym = match &args[0] {
        Value::LitWord { sym, .. } => sym.clone(),
        Value::Word { sym, .. } => sym.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    env.user_ctx
        .get(&sym)
        .ok_or_else(|| EvalError::UnboundWord {
            sym,
            span: args[0].span_or_default(),
        })
}

/// `set 'word value` — writes `value` into `word`'s slot in the user context
/// (the word must have been pre-allocated by `bind_pass`). Returns the value.
fn set_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "set", 2, args.len()));
    }
    let sym = match &args[0] {
        Value::LitWord { sym, .. } => sym.clone(),
        Value::Word { sym, .. } => sym.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let val = args[1].clone();
    if let Some(idx) = env.user_ctx.index_of(&sym) {
        env.user_ctx.set_slot(idx, val.clone());
        Ok(val)
    } else {
        Err(EvalError::UnboundWord {
            sym,
            span: args[0].span_or_default(),
        })
    }
}

/// `value? 'word` — `true` if `word` has a value in the user context, else
/// `false`.
fn value_predicate(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "value?", 1, args.len()));
    }
    let sym = match &args[0] {
        Value::LitWord { sym, .. } => sym.clone(),
        Value::Word { sym, .. } => sym.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    Ok(Value::Logic(env.user_ctx.has(&sym)))
}

/// `use [words] block` — evaluates `block` with the listed words bound as
/// locals in a fresh child context layered over the user context. Body
/// SetWords and loop vars inside `block` are also collected as use-locals
/// (scoped to the child), so `use` provides a self-contained local scope.
/// Outer user-context words remain visible. The locals do not persist after
/// `use` returns. Returns the block's last value.
fn use_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "use", 2, args.len()));
    }
    let words_block = expect_block(args, 0, "use")?;
    let body_block = expect_block(args, 1, "use")?;

    // Collect the word names declared in the words block.
    let words_series = match &words_block {
        Value::Block { series, .. } => series.clone(),
        _ => unreachable!(),
    };
    let local_names: Vec<Symbol> = {
        let data = words_series.data.borrow();
        data.iter()
            .filter_map(crate::binding::loop_word_name)
            .collect()
    };

    // Build a fresh child context seeded from the current user ctx (so outer
    // words are visible), then allocate the listed locals (overriding any
    // inherited slots so writes go to the child, not the user ctx).
    let child = (*env.user_ctx).clone();
    for sym in &local_names {
        child.slot_index(sym.clone());
    }

    // Collect body-local SetWords and loop vars into the child so they're
    // scoped to the `use` and don't leak to the user context.
    let body_series = match &body_block {
        Value::Block { series, .. } => series.clone(),
        _ => unreachable!(),
    };
    crate::binding::collect_setwords(&body_series, &child);
    crate::binding::collect_loop_vars(&body_series, &child);

    let child_rc = Rc::new(child);
    // Deep-copy the body so rebinding doesn't mutate the shared source tree,
    // then bind words: child-locals first (shadow outer), then outer
    // user-ctx words, else leave Unbound.
    let rebound = crate::binding::deep_clone_series(&body_series);
    crate::binding::attach_use_bindings(&rebound, &child_rc, &env.user_ctx);

    let saved = std::mem::replace(&mut env.user_ctx, child_rc);
    let block = Value::Block {
        series: rebound,
        span: Span::new(0, 0),
    };
    let result = eval(&block, env);
    env.user_ctx = saved;
    result
}

/// `bind block 'word` — rebinds words in `block` to the user context (the
/// context where `word` is bound). For the POC, the second argument names a
/// word in the user context (the canonical Red form takes a context value;
/// objects are out of scope, so we accept a word/lit-word and bind to the
/// user context it lives in). Returns the rebound block (a deep copy).
fn bind_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "bind", 2, args.len()));
    }
    let block = expect_block(args, 0, "bind")?;
    // Verify the word operand is bound in the user context (POC: the only
    // context available). The operand itself is otherwise unused — `bind`
    // always rebinds to the user context in the POC.
    let word_sym = match &args[1] {
        Value::LitWord { sym, .. } => sym.clone(),
        Value::Word { sym, .. } => sym.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    if !env.user_ctx.has(&word_sym) {
        return Err(EvalError::UnboundWord {
            sym: word_sym,
            span: args[1].span_or_default(),
        });
    }
    // Deep-copy the block so we don't mutate shared data, then rebind every
    // word whose name is in the user context to a `Binding::Local` pointing
    // at it.
    let series = match &block {
        Value::Block { series, .. } => series.clone(),
        _ => unreachable!(),
    };
    let rebound = crate::binding::deep_clone_series(&series);
    let all_names: Vec<Symbol> = env.user_ctx.names.borrow().keys().cloned().collect();
    crate::binding::rebind_to_context(&rebound, &env.user_ctx, &all_names);
    Ok(Value::Block {
        series: rebound,
        span: Span::new(0, 0),
    })
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

fn fixed_native(f: NativeFn, arity: usize) -> Rc<FuncDef> {
    let params: Vec<Symbol> = (0..arity)
        .map(|i| Symbol::new(&format!("__arg{i}")))
        .collect();
    Rc::new(FuncDef {
        params,
        native: Some(f),
        variadic: false,
        infix: false,
        ..Default::default()
    })
}

fn infix_native(f: NativeFn, arity: usize) -> Rc<FuncDef> {
    let params: Vec<Symbol> = (0..arity)
        .map(|i| Symbol::new(&format!("__arg{i}")))
        .collect();
    Rc::new(FuncDef {
        params,
        native: Some(f),
        variadic: false,
        infix: true,
        ..Default::default()
    })
}

/// Build a variadic native: collects all remaining expressions up to the next
/// native word. Used by `make` (which accepts 2 or 3 args depending on form).
fn variadic_native(f: NativeFn) -> Rc<FuncDef> {
    Rc::new(FuncDef {
        params: Vec::new(),
        native: Some(f),
        variadic: true,
        infix: false,
        ..Default::default()
    })
}

/// Register all native words (M6 I/O + M7 arithmetic/comparison/logic/
/// control-flow/loops/eval) into `env.natives`.
pub fn register_natives(env: &mut Env) {
    // I/O (M6)
    env.natives
        .insert(Symbol::new("print"), fixed_native(print as NativeFn, 1));
    env.natives
        .insert(Symbol::new("prin"), fixed_native(prin as NativeFn, 1));
    env.natives
        .insert(Symbol::new("probe"), fixed_native(probe as NativeFn, 1));

    // Arithmetic (M7, infix)
    env.natives
        .insert(Symbol::new("+"), infix_native(add as NativeFn, 2));
    env.natives
        .insert(Symbol::new("-"), infix_native(subtract as NativeFn, 2));
    env.natives
        .insert(Symbol::new("*"), infix_native(multiply as NativeFn, 2));
    env.natives
        .insert(Symbol::new("/"), infix_native(divide as NativeFn, 2));

    // Comparison (M7, infix)
    env.natives
        .insert(Symbol::new("="), infix_native(equal as NativeFn, 2));
    env.natives
        .insert(Symbol::new("<>"), infix_native(not_equal as NativeFn, 2));
    env.natives
        .insert(Symbol::new("<"), infix_native(less_than as NativeFn, 2));
    env.natives
        .insert(Symbol::new(">"), infix_native(greater_than as NativeFn, 2));
    env.natives
        .insert(Symbol::new("<="), infix_native(less_equal as NativeFn, 2));
    env.natives.insert(
        Symbol::new(">="),
        infix_native(greater_equal as NativeFn, 2),
    );

    // Logic (M7)
    env.natives
        .insert(Symbol::new("and"), infix_native(and_op as NativeFn, 2));
    env.natives
        .insert(Symbol::new("or"), infix_native(or_op as NativeFn, 2));
    env.natives
        .insert(Symbol::new("not"), fixed_native(not_op as NativeFn, 1));

    // Conditionals (M7)
    env.natives
        .insert(Symbol::new("if"), fixed_native(if_native as NativeFn, 2));
    env.natives
        .insert(Symbol::new("either"), fixed_native(either as NativeFn, 3));

    // Loops (M7)
    env.natives.insert(
        Symbol::new("loop"),
        fixed_native(loop_native as NativeFn, 1),
    );
    env.natives
        .insert(Symbol::new("repeat"), fixed_native(repeat as NativeFn, 3));
    env.natives
        .insert(Symbol::new("until"), fixed_native(until as NativeFn, 1));
    env.natives.insert(
        Symbol::new("while"),
        fixed_native(while_native as NativeFn, 2),
    );

    // Control flow (M7)
    env.natives.insert(
        Symbol::new("break"),
        fixed_native(break_native as NativeFn, 0),
    );
    env.natives.insert(
        Symbol::new("continue"),
        fixed_native(continue_native as NativeFn, 0),
    );

    // Eval (M7)
    env.natives
        .insert(Symbol::new("do"), fixed_native(do_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("reduce"), fixed_native(reduce as NativeFn, 1));

    // Functions (M9)
    env.natives.insert(
        Symbol::new("func"),
        fixed_native(func_native as NativeFn, 2),
    );
    env.natives.insert(
        Symbol::new("does"),
        fixed_native(does_native as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("make"),
        fixed_native(make_native as NativeFn, 2),
    );
    env.natives.insert(
        Symbol::new("function?"),
        fixed_native(function_predicate as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("return"),
        variadic_native(return_native as NativeFn),
    );

    // Binding (M9)
    env.natives
        .insert(Symbol::new("get"), fixed_native(get_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("set"), fixed_native(set_native as NativeFn, 2));
    env.natives.insert(
        Symbol::new("value?"),
        fixed_native(value_predicate as NativeFn, 1),
    );
    env.natives
        .insert(Symbol::new("use"), fixed_native(use_native as NativeFn, 2));
    env.natives.insert(
        Symbol::new("bind"),
        fixed_native(bind_native as NativeFn, 2),
    );

    // Series (M8)
    crate::series::register_series_natives(env);

    // Parse dialect (M10)
    env.natives.insert(
        Symbol::new("parse"),
        fixed_native(crate::parse::parse_native as NativeFn, 2),
    );
}

/// Install the predefined constant words (`none`, `true`, `false`, `newline`)
/// into a user context. Must be called before `bind_pass` so references to
/// these words get `Local` bindings to the constant slots.
pub fn install_constants(ctx: &Context) {
    ctx.set(Symbol::new("none"), Value::None);
    ctx.set(Symbol::new("true"), Value::Logic(true));
    ctx.set(Symbol::new("false"), Value::Logic(false));
    ctx.set(
        Symbol::new("newline"),
        Value::string(std::rc::Rc::from("\n")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use red_core::parser::load_source;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// In-memory `Write` sink that records bytes into a shared `Rc<RefCell<Vec<u8>>>`.
    struct BufferWriter(Rc<RefCell<Vec<u8>>>);

    impl Write for BufferWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Run `src` with a fresh env (constants + natives) and capture stdout.
    fn run_capture(src: &str) -> Result<Vec<u8>, String> {
        run_capture_val(src).map(|(_, out)| out)
    }

    fn run_capture_val(src: &str) -> Result<(Value, Vec<u8>), String> {
        use crate::binding::bind_pass;
        let body = load_source(src).map_err(|e| e.to_string())?;
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        let block = Value::block(body);
        let val = eval(&block, &mut env).map_err(|e| e.to_string())?;
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    // --- M6 I/O tests (preserved) ---

    #[test]
    fn print_integer() {
        assert_eq!(s(&run_capture("print 5").unwrap()), "5\n");
    }

    #[test]
    fn prin_concat() {
        // mold-everything: strings render quoted, so `prin "a" prin "b"`
        // yields `"a""b"`. Each `prin` takes exactly one argument.
        assert_eq!(
            s(&run_capture("prin \"a\" prin \"b\"").unwrap()),
            "\"a\"\"b\""
        );
    }

    #[test]
    fn print_block() {
        assert_eq!(s(&run_capture("print [1 2 3]").unwrap()), "[1 2 3]\n");
    }

    #[test]
    fn print_string_molded() {
        assert_eq!(
            s(&run_capture("print \"Hello, World!\"").unwrap()),
            "\"Hello, World!\"\n"
        );
    }

    #[test]
    fn probe_value() {
        assert_eq!(s(&run_capture("probe 42").unwrap()), "== 42\n");
    }

    #[test]
    fn print_returns_none() {
        let (v, _) = run_capture_val("print 5").unwrap();
        assert_eq!(mold_to_string(&v), "none");
    }

    // --- M7 arithmetic ---

    #[test]
    fn add_integers() {
        assert_eq!(mold_to_string(&val("1 + 2")), "3");
    }

    #[test]
    fn subtract_integers() {
        assert_eq!(mold_to_string(&val("10 - 4")), "6");
    }

    #[test]
    fn multiply_integers() {
        assert_eq!(mold_to_string(&val("3 * 4")), "12");
    }

    #[test]
    fn divide_integers() {
        assert_eq!(mold_to_string(&val("10 / 3")), "3");
    }

    #[test]
    fn division_by_zero_errors() {
        let err = run_capture("10 / 0").unwrap_err();
        assert!(err.contains("division by zero"));
    }

    #[test]
    fn mixed_int_float_promotes_to_float() {
        assert_eq!(mold_to_string(&val("1 + 2.0")), "3.0");
    }

    #[test]
    fn left_to_right_no_precedence() {
        // `1 + 2 * 3` = `(1 + 2) * 3` = 9
        assert_eq!(mold_to_string(&val("1 + 2 * 3")), "9");
    }

    // --- M7 comparison ---

    #[test]
    fn equal_returns_logic() {
        assert_eq!(mold_to_string(&val("3 = 3")), "true");
        assert_eq!(mold_to_string(&val("3 = 4")), "false");
    }

    #[test]
    fn not_equal_returns_logic() {
        assert_eq!(mold_to_string(&val("3 <> 4")), "true");
    }

    #[test]
    fn less_than() {
        assert_eq!(mold_to_string(&val("1 < 2")), "true");
        assert_eq!(mold_to_string(&val("2 < 1")), "false");
    }

    #[test]
    fn greater_than() {
        assert_eq!(mold_to_string(&val("2 > 1")), "true");
    }

    #[test]
    fn less_equal() {
        assert_eq!(mold_to_string(&val("2 <= 2")), "true");
    }

    #[test]
    fn greater_equal() {
        assert_eq!(mold_to_string(&val("3 >= 2")), "true");
    }

    #[test]
    fn one_plus_two_equals_three() {
        // The milestone test: `1 + 2 = 3` evaluates left-to-right to `true`.
        assert_eq!(mold_to_string(&val("1 + 2 = 3")), "true");
    }

    // --- M7 logic ---

    #[test]
    fn and_or_not() {
        assert_eq!(mold_to_string(&val("true and false")), "false");
        assert_eq!(mold_to_string(&val("true or false")), "true");
        assert_eq!(mold_to_string(&val("not true")), "false");
        assert_eq!(mold_to_string(&val("not false")), "true");
    }

    #[test]
    fn none_is_falsy() {
        assert_eq!(mold_to_string(&val("not none")), "true");
    }

    // --- M7 conditionals ---

    #[test]
    fn if_true_evaluates_block() {
        assert_eq!(mold_to_string(&val("if true [42]")), "42");
    }

    #[test]
    fn if_false_returns_none() {
        assert_eq!(mold_to_string(&val("if false [42]")), "none");
    }

    #[test]
    fn either_true_branch() {
        assert_eq!(mold_to_string(&val("either 1 > 0 [\"y\"][\"n\"]")), "\"y\"");
    }

    #[test]
    fn either_false_branch() {
        assert_eq!(mold_to_string(&val("either 1 < 0 [\"y\"][\"n\"]")), "\"n\"");
    }

    // --- M7 loops ---

    #[test]
    fn repeat_prints_counter() {
        let out = run_capture("repeat i 3 [print i]").unwrap();
        assert_eq!(s(&out), "1\n2\n3\n");
    }

    #[test]
    fn repeat_litword_form() {
        let out = run_capture("repeat 'i 3 [print i]").unwrap();
        assert_eq!(s(&out), "1\n2\n3\n");
    }

    #[test]
    fn until_terminates() {
        // `i: 0 until [i: i + 1 i > 3]` → true, i == 4
        let v = val("i: 0 until [i: i + 1 i > 3]");
        assert_eq!(mold_to_string(&v), "true");
        // Verify i ended at 4.
        assert_eq!(mold_to_string(&val("i: 0 until [i: i + 1 i > 3] i")), "4");
    }

    #[test]
    fn while_terminates() {
        // `a: 0 while [a < 3][a: a + 1]` → terminates; a == 3
        let v = val("a: 0 while [a < 3][a: a + 1]");
        assert_eq!(mold_to_string(&v), "none");
        assert_eq!(mold_to_string(&val("a: 0 while [a < 3][a: a + 1] a")), "3");
    }

    #[test]
    fn loop_with_break() {
        // `i: 0 loop [i: i + 1 if i > 3 [break]] i` → i == 4
        let v = val("i: 0 loop [i: i + 1 if i > 3 [break]] i");
        assert_eq!(mold_to_string(&v), "4");
    }

    #[test]
    fn loop_break_returns_none() {
        assert_eq!(mold_to_string(&val("loop [break]")), "none");
    }

    #[test]
    fn continue_skips_rest() {
        // Sum 1..5 skipping 3: i: 0 sum: 0 repeat 5 [if i = 2 [continue] sum: sum + i] sum
        // Actually with continue, the `sum: sum + i` after `continue` won't run.
        // i goes 1..5. When i=2, continue skips the rest. sum = 0+1+3+4+5 = 13.
        // Wait, i=2 is skipped but the repeat counter is the loop var...
        // Let me use a clearer test: repeat 5 [if i = 3 [continue] print i]
        // → prints 1, 2, 4, 5 (skips 3)
        let out = run_capture("repeat i 5 [if i = 3 [continue] print i]").unwrap();
        assert_eq!(s(&out), "1\n2\n4\n5\n");
    }

    // --- M7 eval ---

    #[test]
    fn do_evaluates_block() {
        assert_eq!(mold_to_string(&val("do [1 + 2]")), "3");
    }

    #[test]
    fn reduce_collects_results() {
        assert_eq!(mold_to_string(&val("reduce [1 + 1 2 + 2]")), "[2 4]");
    }

    #[test]
    fn reduce_empty_block() {
        assert_eq!(mold_to_string(&val("reduce []")), "[]");
    }

    // --- M7 truthiness edge cases ---

    #[test]
    fn if_with_integer_condition() {
        // Non-false, non-none values are truthy.
        assert_eq!(mold_to_string(&val("if 5 [42]")), "42");
    }

    #[test]
    fn if_with_zero_is_truthy() {
        // In Red, 0 is truthy (only false and none are falsy).
        assert_eq!(mold_to_string(&val("if 0 [42]")), "42");
    }

    #[test]
    fn if_with_none_is_falsy() {
        assert_eq!(mold_to_string(&val("if none [42]")), "none");
    }

    // --- M13: user-function refinements ---

    #[test]
    fn func_with_only_refinement_callable_with_and_without() {
        // `func [x /only][...]` — callable both ways. The body reads `only`
        // as a logic flag (true when `/only` supplied, false otherwise).
        let src = r#"
            f: func [x /only][
                either only [x * 10][x]
            ]
            print f 5
            print f/only 5
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "5\n50\n");
    }

    #[test]
    fn func_refinement_with_argument() {
        // `func [x /with y][...]` — `/with` takes one arg `y`. The inactive
        // branch must not reference `y` (it's `none` when `/with` is unused).
        let src = r#"
            f: func [x /with y][
                if with [return x + y]
                x
            ]
            print f 5
            print f/with 5 7
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "5\n12\n");
    }

    #[test]
    fn func_refinement_inline_spaced_form() {
        // The spaced form `f 5 /with 7` (refinement as a standalone token
        // after the positional args) also works — spec-order dispatch
        // consumes positional args first, then the refinement flag + its
        // args. (Refinements may not skip required positionals.)
        let src = r#"
            f: func [x /with y][
                if with [return x + y]
                x
            ]
            print f 5 /with 7
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "12\n");
    }

    #[test]
    fn func_refinement_arg_defaults_to_none_when_inactive() {
        // When `/with` isn't supplied, `y` is `none` in the body. The body
        // must guard against using `y` in the inactive path.
        let src = r#"
            f: func [x /with y][
                if with [return y]
                x
            ]
            print f 5
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "5\n");
    }

    #[test]
    fn func_multiple_refinements() {
        // Two refinements, both usable independently and together.
        let src = r#"
            f: func [x /double /add n][
                if double [x: x * 2]
                if add [x: x + n]
                x
            ]
            print f 5
            print f/double 5
            print f/add 5 3
            print f/double/add 5 3
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "5\n10\n8\n13\n");
    }
}
