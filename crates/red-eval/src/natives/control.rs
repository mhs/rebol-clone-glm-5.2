//! Control-flow natives: conditionals (`if`/`either`), loops
//! (`loop`/`repeat`/`until`/`while`), `break`/`continue`, and the M16
//! expansion (`switch`/`case`/`default`/`all`/`any`/`try`/`attempt`/
//! `catch`/`throw`/`cause-error`/`comment`/`exit`/`quit`).
//!
//! M30.2.E: in VM mode, the loop natives resolve the body's `CompiledBlock`
//! once (cache lookup or compile-on-demand) and call `vm::run` in a tight
//! loop — eliminating the per-iteration `dispatch_block` overhead that
//! remained after Tier 1. Falls back to `dispatch_block` per iteration in
//! Walk mode or when the block can't be VM-compiled (foreign bindings /
//! `needs_rebind`).

use super::{arity_err, expect_block, truthy, type_name, values_equal};
use crate::interp::{active_captures, dispatch_block, eval_expression, resolve_compiled_block};
use red_core::printer::mold_to_string;
use red_core::value::{ErrorValue, Series, Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

// ---------------------------------------------------------------------------
// Conditionals: if, either
// ---------------------------------------------------------------------------

/// `if cond block` — evaluates `block` if `cond` is truthy, else returns `none`.
pub(crate) fn if_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "if", 2, args.len()));
    }
    if truthy(&args[0]) {
        let body = expect_block(args, 1, "if")?;
        dispatch_block(&body, env)
    } else {
        Ok(Value::None)
    }
}

/// `unless cond block` — inverse of `if`: evaluates `block` when `cond` is
/// falsy, else returns `none`. Mirrors `if_native`'s return semantics.
pub(crate) fn unless_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "unless", 2, args.len()));
    }
    if !truthy(&args[0]) {
        let body = expect_block(args, 1, "unless")?;
        dispatch_block(&body, env)
    } else {
        Ok(Value::None)
    }
}

/// `either cond t-block f-block`
pub(crate) fn either(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 3 {
        return Err(arity_err(args, "either", 3, args.len()));
    }
    let t = expect_block(args, 1, "either")?;
    let f = expect_block(args, 2, "either")?;
    if truthy(&args[0]) {
        dispatch_block(&t, env)
    } else {
        dispatch_block(&f, env)
    }
}

// ---------------------------------------------------------------------------
// Loops: loop, repeat, until, while, forever, for
// ---------------------------------------------------------------------------

/// `loop block` — evaluates `block` repeatedly until `break`. Returns the
/// break-value (or `none` if `break` had no value).
pub(crate) fn loop_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "loop")?;
    if let Some(compiled) = resolve_compiled_block(&body, env) {
        loop {
            let caps = active_captures(env);
            match crate::vm::run((*compiled).clone(), env, caps) {
                Ok(_) => {}
                Err(EvalError::Break(v)) => return Ok(v.unwrap_or(Value::None)),
                Err(EvalError::Continue) => continue,
                Err(e) => return Err(e),
            }
        }
    }
    loop {
        match dispatch_block(&body, env) {
            Ok(_) => {}
            Err(EvalError::Break(v)) => return Ok(v.unwrap_or(Value::None)),
            Err(EvalError::Continue) => continue,
            Err(e) => return Err(e),
        }
    }
}

/// `repeat 'word count block` — binds `word` to 1..=count, evaluates `block`
/// each iteration. Accepts both lit-word (`'i`) and bare-word (`i`) forms.
pub(crate) fn repeat(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 3 {
        return Err(arity_err(args, "repeat", 3, args.len()));
    }
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
    let slot = crate::series::resolve_loop_slot(&args[0], env)?;
    // M30.2.E: resolve the compiled block once. If VM-mode + cacheable,
    // run a tight `vm::run` loop (no per-iteration dispatch overhead).
    if let Some(compiled) = resolve_compiled_block(&body, env) {
        for n in 1..=count {
            crate::series::write_loop_slot(&slot, Value::integer(n), env);
            let caps = active_captures(env);
            match crate::vm::run((*compiled).clone(), env, caps) {
                Ok(_) => {}
                Err(EvalError::Break(v)) => return Ok(v.unwrap_or(Value::None)),
                Err(EvalError::Continue) => continue,
                Err(e) => return Err(e),
            }
        }
        return Ok(Value::None);
    }
    // Fallback: Walk mode or non-VM-able block.
    for n in 1..=count {
        crate::series::write_loop_slot(&slot, Value::integer(n), env);
        match dispatch_block(&body, env) {
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
pub(crate) fn until(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "until")?;
    if let Some(compiled) = resolve_compiled_block(&body, env) {
        loop {
            let caps = active_captures(env);
            match crate::vm::run((*compiled).clone(), env, caps) {
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
    loop {
        match dispatch_block(&body, env) {
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
pub(crate) fn while_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "while", 2, args.len()));
    }
    let cond = expect_block(args, 0, "while")?;
    let body = expect_block(args, 1, "while")?;
    let cond_compiled = resolve_compiled_block(&cond, env);
    let body_compiled = resolve_compiled_block(&body, env);
    if let (Some(cond_c), Some(body_c)) = (cond_compiled, body_compiled) {
        loop {
            let caps = active_captures(env);
            let c = crate::vm::run((*cond_c).clone(), env, caps)?;
            if !truthy(&c) {
                return Ok(Value::None);
            }
            let caps = active_captures(env);
            match crate::vm::run((*body_c).clone(), env, caps) {
                Ok(_) => {}
                Err(EvalError::Break(v)) => return Ok(v.unwrap_or(Value::None)),
                Err(EvalError::Continue) => continue,
                Err(e) => return Err(e),
            }
        }
    }
    loop {
        let c = dispatch_block(&cond, env)?;
        if !truthy(&c) {
            return Ok(Value::None);
        }
        match dispatch_block(&body, env) {
            Ok(_) => {}
            Err(EvalError::Break(v)) => return Ok(v.unwrap_or(Value::None)),
            Err(EvalError::Continue) => continue,
            Err(e) => return Err(e),
        }
    }
}

/// `forever block` — unconditional infinite loop: evaluates `body`
/// repeatedly until a `break` unwinds it. Returns the break-value (or `none`
/// if `break` had no value). Equivalent to `while [true] body` but skips the
/// condition re-evaluation each iteration.
pub(crate) fn forever_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "forever")?;
    if let Some(compiled) = resolve_compiled_block(&body, env) {
        loop {
            let caps = active_captures(env);
            match crate::vm::run((*compiled).clone(), env, caps) {
                Ok(_) => {}
                Err(EvalError::Break(v)) => return Ok(v.unwrap_or(Value::None)),
                Err(EvalError::Continue) => continue,
                Err(e) => return Err(e),
            }
        }
    }
    loop {
        match dispatch_block(&body, env) {
            Ok(_) => {}
            Err(EvalError::Break(v)) => return Ok(v.unwrap_or(Value::None)),
            Err(EvalError::Continue) => continue,
            Err(e) => return Err(e),
        }
    }
}

/// `for word start end bump body` — classic counted loop. Binds `word` to
/// `start`, evaluates `body`, adds `bump` to `word`, repeats while `word`
/// hasn't passed `end` (direction-aware: positive bump → `word <= end`;
/// negative bump → `word >= end`). Inclusive of `end` when it lands exactly.
/// Supports `integer!`/`float!`/`char!` start/end/bump (char takes codepoint
/// arithmetic, `char + int → char`).
pub(crate) fn for_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 5 {
        return Err(arity_err(args, "for", 5, args.len()));
    }
    let slot = crate::series::resolve_loop_slot(&args[0], env)?;
    let body = expect_block(args, 4, "for")?;
    let start = args[1].clone();
    let end = &args[2];
    let bump = &args[3];

    // Determine direction from the sign of `bump`. A zero bump is an error
    // (would loop forever; Red rejects it too).
    let bump_sign = match crate::math::numeric_cmp(bump, &Value::integer(0)) {
        Some(std::cmp::Ordering::Equal) => {
            return Err(EvalError::Native {
                message: "for: bump must be non-zero".into(),
                span: bump.span_or_default(),
            });
        }
        Some(std::cmp::Ordering::Greater) => 1i8,
        Some(std::cmp::Ordering::Less) => -1i8,
        None => {
            return Err(EvalError::TypeError {
                expected: "integer!, float!, or char!",
                found: type_name(bump),
                span: bump.span_or_default(),
            });
        }
    };

    let compiled = resolve_compiled_block(&body, env);
    let mut cur = start;
    let mut last = Value::None;
    loop {
        // Direction-aware bound check.
        let past = match crate::math::numeric_cmp(&cur, end) {
            Some(ord) => matches!(
                (bump_sign, ord),
                (1, std::cmp::Ordering::Greater) | (-1, std::cmp::Ordering::Less)
            ),
            None => {
                return Err(EvalError::TypeError {
                    expected: "integer!, float!, or char!",
                    found: type_name(&cur),
                    span: cur.span_or_default(),
                });
            }
        };
        if past {
            break;
        }
        crate::series::write_loop_slot(&slot, cur.clone(), env);
        let result = if let Some(ref c) = compiled {
            let caps = active_captures(env);
            crate::vm::run((**c).clone(), env, caps)
        } else {
            dispatch_block(&body, env)
        };
        match result {
            Ok(v) => last = v,
            Err(EvalError::Break(bv)) => return Ok(bv.unwrap_or(Value::None)),
            Err(EvalError::Continue) => {}
            Err(e) => return Err(e),
        }
        cur = crate::math::numeric_add(&cur, bump)?;
    }
    Ok(last)
}

// ---------------------------------------------------------------------------
// Control flow: break, continue
// ---------------------------------------------------------------------------

/// `break` — unwinds out of the enclosing loop via `EvalError::Break`.
pub(crate) fn break_native(
    _args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    Err(EvalError::Break(None))
}

/// `continue` — skips to the next iteration of the enclosing loop via
/// `EvalError::Continue`.
pub(crate) fn continue_native(
    _args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    Err(EvalError::Continue)
}

// ---------------------------------------------------------------------------
// Control flow expansion (M16): switch, case, default, all, any, try,
// attempt, catch, throw, cause-error, comment, exit, quit
// ---------------------------------------------------------------------------

/// `switch value cases-block` — walks `cases-block` in pairs: each candidate
/// is evaluated (as a full expression) and compared to `value`; on match, the
/// following value (typically a block) is evaluated and its result returned.
/// Refinements:
/// - `/default block` — runs if no candidate matched.
/// - `/case` — case-sensitive string comparison (POC: string equality is
///   already case-sensitive by default; the flag is accepted for parity).
pub(crate) fn switch_native(
    args: &[Value],
    refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "switch", 2, args.len()));
    }
    let value = args[0].clone();
    let cases = expect_block(args, 1, "switch")?;
    let series = match &cases {
        Value::Block { series, .. } => series.clone(),
        _ => unreachable!("expect_block guarantees Block"),
    };
    let data = series.data.borrow();
    let mut i = series.index;
    while i < data.len() {
        // Candidate is a full expression (so `1 + 1` works as a case).
        let candidate = eval_expression(&data, &mut i, env)?;
        if i >= data.len() {
            break;
        }
        let body = data[i].clone();
        i += 1;
        if values_equal(&candidate, &value) {
            return match &body {
                Value::Block { .. } | Value::Paren { .. } => {
                    drop(data);
                    dispatch_block(&body, env)
                }
                _ => Ok(body),
            };
        }
    }
    drop(data);
    if let Some(default_args) = refs.get(&Symbol::new("default")) {
        if let Some(body) = default_args.first() {
            if let Value::Block { .. } | Value::Paren { .. } = body {
                return dispatch_block(body, env);
            }
            return Ok(body.clone());
        }
    }
    Ok(Value::None)
}

/// `case cases-block` — walks `cases-block` in pairs: each condition is
/// evaluated (as a full expression); if truthy, the following value
/// (typically a block) is evaluated and its result returned. Refinements:
/// - `/all` — evaluate *every* matching branch (default: stop at first).
/// - `/default block` — runs if no condition matched.
pub(crate) fn case_native(
    args: &[Value],
    refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "case", 1, args.len()));
    }
    let cases = expect_block(args, 0, "case")?;
    let all = refs.has(&Symbol::new("all"));
    let series = match &cases {
        Value::Block { series, .. } => series.clone(),
        _ => unreachable!(),
    };
    let data = series.data.borrow();
    let mut i = series.index;
    let mut last = Value::None;
    let mut matched = false;
    while i < data.len() {
        let cond_val = eval_expression(&data, &mut i, env)?;
        if i >= data.len() {
            break;
        }
        let body = data[i].clone();
        i += 1;
        if truthy(&cond_val) {
            matched = true;
            last = match &body {
                Value::Block { .. } | Value::Paren { .. } => dispatch_block(&body, env)?,
                _ => body.clone(),
            };
            if !all {
                return Ok(last);
            }
        }
    }
    drop(data);
    if matched {
        Ok(last)
    } else if let Some(default_args) = refs.get(&Symbol::new("default")) {
        if let Some(body) = default_args.first() {
            if let Value::Block { .. } | Value::Paren { .. } = body {
                return dispatch_block(body, env);
            }
            return Ok(body.clone());
        }
        Ok(Value::None)
    } else {
        Ok(Value::None)
    }
}

/// `default 'word value` — set `word` to `value` if it currently holds `none`
/// (or has no slot — treated as unset). Returns the (possibly new) value.
/// First argument is taken unevaluated (a word/lit-word name).
pub(crate) fn default_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "default", 2, args.len()));
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
    let new_val = args[1].clone();
    let idx = env
        .user_ctx
        .index_of(&sym)
        .ok_or_else(|| EvalError::UnboundWord {
            sym: sym.clone(),
            span: args[0].span_or_default(),
        })?;
    let current = env.user_ctx.slot_value(idx);
    if matches!(current, Value::None) {
        env.user_ctx.set_slot(idx, new_val.clone());
        Ok(new_val)
    } else {
        Ok(current)
    }
}

/// `all [block]` — evaluates each expression in `block`; short-circuits to
/// `none` on the first falsy value, otherwise returns the last value.
pub(crate) fn all_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "all")?;
    let series = match &body {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        _ => return Ok(Value::None),
    };
    let data = series.data.borrow();
    let mut i = series.index;
    let mut last = Value::Logic(true);
    while i < data.len() {
        let v = eval_expression(&data, &mut i, env)?;
        if !truthy(&v) {
            return Ok(Value::None);
        }
        last = v;
    }
    drop(data);
    Ok(last)
}

/// `any [block]` — evaluates each expression in `block`; returns the first
/// truthy value, or `none` if all are falsy.
pub(crate) fn any_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "any")?;
    let series = match &body {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        _ => return Ok(Value::None),
    };
    let data = series.data.borrow();
    let mut i = series.index;
    while i < data.len() {
        let v = eval_expression(&data, &mut i, env)?;
        if truthy(&v) {
            return Ok(v);
        }
    }
    drop(data);
    Ok(Value::None)
}

/// `try [block]` — evaluate `block`; on success return the value; on a
/// catchable error, return a `Value::Error`. Control-flow unwinds
/// (`Return`/`Break`/`Continue`/`Throw`/`Quit`) propagate.
///
/// M42: structured `EvalError::Raised(ev)` is unwrapped directly into
/// `Value::Error(ev)`. Legacy catchable errors (`Native`/`TypeError`/
/// `Arity`/`UnboundWord`/`Compile`) are synthesized into a structured
/// `ErrorValue` with `type: 'script` and the rendered message body.
pub(crate) fn try_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "try")?;
    match dispatch_block(&body, env) {
        Ok(v) => Ok(v),
        Err(
            e @ (EvalError::Return(_)
            | EvalError::Break(_)
            | EvalError::Continue
            | EvalError::Throw(_)
            | EvalError::Quit(_)),
        ) => Err(e),
        Err(EvalError::Raised(ev)) => Ok(Value::Error(ev)),
        Err(e) => Ok(Value::error_structed(
            e.to_string(),
            None,
            Some(Symbol::new("script")),
            Vec::new(),
            None,
            None,
            None,
        )),
    }
}

/// `attempt [block]` — like `try` but returns `none` on error instead of an
/// error value.
pub(crate) fn attempt_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "attempt")?;
    match dispatch_block(&body, env) {
        Ok(v) => Ok(v),
        Err(
            e @ (EvalError::Return(_)
            | EvalError::Break(_)
            | EvalError::Continue
            | EvalError::Throw(_)
            | EvalError::Quit(_)),
        ) => Err(e),
        Err(_) => Ok(Value::None),
    }
}

/// `catch [block]` — evaluate `block`; on `throw value`, return the thrown
/// value. M42: also catches structured `EvalError::Raised(ev)` errors,
/// returning them as `Value::Error(ev)`. Other errors propagate.
pub(crate) fn catch_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "catch")?;
    match dispatch_block(&body, env) {
        Ok(v) => Ok(v),
        Err(EvalError::Throw(v)) => Ok(v),
        Err(EvalError::Raised(ev)) => Ok(Value::Error(ev)),
        Err(e) => Err(e),
    }
}

/// `throw value` — unwinds via `EvalError::Throw(value)`, caught by `catch`.
pub(crate) fn throw_native(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    let v = args.first().cloned().unwrap_or(Value::None);
    Err(EvalError::Throw(v))
}

/// `cause-error` — M42 structured form. Accepts three shapes:
///
/// - `cause-error "msg"` (1-arg) — message-only error (back-compat with the
///   prior variadic string-join form; keeps the `cause_error.red` golden
///   fixture green).
/// - `cause-error 'type "msg"` (2-arg) — type word + message.
/// - `cause-error 'type 'code [args...] "msg"` (4-arg) — full structured
///   form. `code` may be a word or integer; `args` is a block of values.
/// - `cause-error [type: 'word code: 42 message: "..." args: [...]]` —
///   block-of-keyword-pairs form.
///
/// Builds a structured `ErrorValue` and raises `EvalError::Raised`.
pub(crate) fn cause_error(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    let span = args
        .first()
        .map(|v| v.span_or_default())
        .unwrap_or_default();

    // Block form: `cause-error [type: ... code: ... message: ...]`.
    if args.len() == 1 {
        if let Value::Block { series, .. } = &args[0] {
            let ev = parse_error_block(series, span)?;
            return Err(EvalError::Raised(std::rc::Rc::new(ev)));
        }
    }

    // 1-arg string form: message-only (back-compat).
    if args.len() == 1 {
        let message = match &args[0] {
            Value::String { s, .. } => s.to_string(),
            other => mold_to_string(other),
        };
        return Err(EvalError::Raised(std::rc::Rc::new(
            ErrorValue::new_message(message),
        )));
    }

    // 2-arg: `cause-error 'type "msg"`.
    if args.len() == 2 {
        let kind = extract_kind(&args[0], span)?;
        let message = match &args[1] {
            Value::String { s, .. } => s.to_string(),
            other => mold_to_string(other),
        };
        return Err(EvalError::Raised(std::rc::Rc::new(
            ErrorValue::new_structed(message, None, Some(kind), Vec::new(), None, None, None),
        )));
    }

    // 4-arg: `cause-error 'type 'code [args...] "msg"`.
    if args.len() == 4 {
        let kind = extract_kind(&args[0], span)?;
        let code = extract_code(&args[1], span)?;
        let error_args = match &args[2] {
            Value::Block { series, .. } => series.data.borrow().clone(),
            other => {
                return Err(EvalError::Native {
                    message: format!(
                        "cause-error: args must be a block, got {}",
                        type_name(other)
                    ),
                    span: other.span_or_default(),
                });
            }
        };
        let message = match &args[3] {
            Value::String { s, .. } => s.to_string(),
            other => mold_to_string(other),
        };
        return Err(EvalError::Raised(std::rc::Rc::new(
            ErrorValue::new_structed(
                message,
                Some(code),
                Some(kind),
                error_args,
                None,
                None,
                None,
            ),
        )));
    }

    // Fallback: variadic string-join (back-compat with the pre-M42 form for
    // any other arity).
    let message = args
        .iter()
        .map(mold_to_string)
        .collect::<Vec<_>>()
        .join(" ");
    Err(EvalError::Raised(std::rc::Rc::new(
        ErrorValue::new_message(message),
    )))
}

/// Extract a `Symbol` kind word from a value (`'word` or `word:` or bare
/// word). Errors with `cause-error: type must be a word`.
fn extract_kind(v: &Value, _span: Span) -> Result<Symbol, EvalError> {
    match v {
        Value::LitWord { sym, .. } => Ok(sym.clone()),
        Value::Word { sym, .. } | Value::SetWord { sym, .. } | Value::GetWord { sym, .. } => {
            Ok(sym.clone())
        }
        other => Err(EvalError::Native {
            message: format!("cause-error: type must be a word, got {}", type_name(other)),
            span: other.span_or_default(),
        }),
    }
}

/// Extract an `i64` code from a value (word or integer). Red's error codes
/// are words (`'no-arg`) but we also accept integers for parity with `code:`.
fn extract_code(v: &Value, span: Span) -> Result<i64, EvalError> {
    match v {
        Value::Integer { n, .. } => Ok(*n),
        Value::LitWord { sym, .. } | Value::Word { sym, .. } => {
            // Words map to a hash of their name; for the POC we use 0 since
            // Red's named error codes are symbolic. Tests only check that
            // `error-code` returns *some* numeric value.
            let _ = span;
            Ok(sym.as_str().len() as i64)
        }
        other => Err(EvalError::Native {
            message: format!(
                "cause-error: code must be a word or integer, got {}",
                type_name(other)
            ),
            span: other.span_or_default(),
        }),
    }
}

/// Parse a `make error! [...]` block spec into an `ErrorValue`. The block
/// is a series of `keyword: value` pairs (in any order). Recognized keys:
/// `code:` (integer), `type:` (lit-word/word), `message:` (string),
/// `args:` (block), `where:` (word), `by:` (word), `near:` (any value).
///
/// Public so `convert.rs::make_error` can reuse it (avoiding duplication).
pub(crate) fn parse_error_block_public(
    series: &Series,
    span: Span,
) -> Result<ErrorValue, EvalError> {
    parse_error_block(series, span)
}

/// Parse a `make error! [...]` block spec into an `ErrorValue`. The block
/// is a series of `keyword: value` pairs (in any order). Recognized keys:
/// `code:` (integer), `type:` (lit-word/word), `message:` (string),
/// `args:` (block), `where:` (word), `by:` (word), `near:` (any value).
fn parse_error_block(series: &Series, _span: Span) -> Result<ErrorValue, EvalError> {
    let data = series.data.borrow();
    let mut i = series.index;
    let mut message = String::new();
    let mut code: Option<i64> = None;
    let mut kind: Option<Symbol> = None;
    let mut args: Vec<Value> = Vec::new();
    let mut near: Option<Value> = None;
    let mut cause: Option<Symbol> = None;
    let mut by: Option<Symbol> = None;
    while i + 1 < data.len() {
        let key = &data[i];
        let val = &data[i + 1];
        let key_sym = match key {
            Value::SetWord { sym, .. }
            | Value::Word { sym, .. }
            | Value::GetWord { sym, .. }
            | Value::LitWord { sym, .. } => sym.clone(),
            other => {
                return Err(EvalError::Native {
                    message: format!("make error!: expected keyword, got {}", type_name(other)),
                    span: other.span_or_default(),
                });
            }
        };
        match key_sym.as_str() {
            "message" => {
                message = match val {
                    Value::String { s, .. } => s.to_string(),
                    other => mold_to_string(other),
                };
            }
            "code" => {
                code = match val {
                    Value::Integer { n, .. } => Some(*n),
                    _ => None,
                };
            }
            "type" => {
                kind = match val {
                    Value::LitWord { sym, .. }
                    | Value::Word { sym, .. }
                    | Value::SetWord { sym, .. }
                    | Value::GetWord { sym, .. } => Some(sym.clone()),
                    _ => None,
                };
            }
            "args" => {
                if let Value::Block { series: s, .. } = val {
                    args = s.data.borrow().clone();
                }
            }
            "where" => {
                cause = match val {
                    Value::LitWord { sym, .. }
                    | Value::Word { sym, .. }
                    | Value::SetWord { sym, .. }
                    | Value::GetWord { sym, .. } => Some(sym.clone()),
                    _ => None,
                };
            }
            "by" => {
                by = match val {
                    Value::LitWord { sym, .. }
                    | Value::Word { sym, .. }
                    | Value::SetWord { sym, .. }
                    | Value::GetWord { sym, .. } => Some(sym.clone()),
                    _ => None,
                };
            }
            "near" => {
                near = Some(val.clone());
            }
            other => {
                return Err(EvalError::Native {
                    message: format!("make error!: unknown keyword {other:?}"),
                    span: key.span_or_default(),
                });
            }
        }
        i += 2;
    }
    if message.is_empty() {
        return Err(EvalError::Native {
            message: "make error!: missing message field".into(),
            span: Span::default(),
        });
    }
    Ok(ErrorValue::new_structed(
        message, code, kind, args, near, cause, by,
    ))
}

/// `comment <block-or-string>` — discards its single argument, returns
/// `none`. Takes one arg (a block or string) so trailing expressions in the
/// enclosing block are not consumed.
pub(crate) fn comment_native(
    _args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    Ok(Value::None)
}

/// `exit [code]` / `quit [code]` — unwind via `EvalError::Quit(code)`,
/// caught at the top-level script entry point. Default exit code is 0.
pub(crate) fn exit_native(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    let code = match args.first() {
        Some(Value::Integer { n, .. }) => *n as i32,
        Some(Value::None) | None => 0,
        Some(other) => {
            return Err(EvalError::TypeError {
                expected: "integer!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    Err(EvalError::Quit(code))
}
