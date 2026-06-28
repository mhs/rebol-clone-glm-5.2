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
use crate::interp::{dispatch_block, eval_expression, resolve_compiled_block};
use red_core::printer::mold_to_string;
use red_core::value::{Symbol, Value};
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

/// `either cond t-block f-block`
pub(crate) fn either(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
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
// Loops: loop, repeat, until, while
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
            match crate::vm::run((*compiled).clone(), env) {
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
pub(crate) fn repeat(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
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
    // M30.2.E: resolve the compiled block once. If VM-mode + cacheable,
    // run a tight `vm::run` loop (no per-iteration dispatch overhead).
    if let Some(compiled) = resolve_compiled_block(&body, env) {
        for n in 1..=count {
            env.user_ctx.set_slot(idx, Value::integer(n));
            match crate::vm::run((*compiled).clone(), env) {
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
        env.user_ctx.set_slot(idx, Value::integer(n));
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
            match crate::vm::run((*compiled).clone(), env) {
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
            let c = crate::vm::run((*cond_c).clone(), env)?;
            if !truthy(&c) {
                return Ok(Value::None);
            }
            match crate::vm::run((*body_c).clone(), env) {
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
pub(crate) fn all_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
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
pub(crate) fn any_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
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
/// catchable error, return a `Value::Error` carrying the message. Control-
/// flow unwinds (`Return`/`Break`/`Continue`/`Throw`/`Quit`) propagate.
pub(crate) fn try_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
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
        Err(e) => Ok(Value::error(e.to_string())),
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
/// value. Other errors propagate.
pub(crate) fn catch_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "catch")?;
    match dispatch_block(&body, env) {
        Ok(v) => Ok(v),
        Err(EvalError::Throw(v)) => Ok(v),
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

/// `cause-error err-type err-code args-block` (POC: variadic; builds a
/// message and raises `EvalError::Native`). Real Red constructs a structured
/// error value; the full error-value model is deferred to v0.3.
pub(crate) fn cause_error(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    let mut parts: Vec<String> = Vec::new();
    for a in args {
        parts.push(mold_to_string(a));
    }
    let message = if parts.is_empty() {
        "cause-error".to_string()
    } else {
        parts.join(" ")
    };
    Err(EvalError::Native {
        message,
        span: args
            .first()
            .map(|v| v.span_or_default())
            .unwrap_or_default(),
    })
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
