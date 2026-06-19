//! Tree-walking evaluator: binding pass + `eval`.
//!
//! Milestone 5 scope: literals return themselves; `Block` is data (returned
//! as-is unless entered explicitly); `Paren` is walked eagerly in place;
//! `Word` resolves via its binding (or errors if unbound and not a native);
//! `SetWord` evaluates the next value and writes it into the bound slot;
//! `GetWord` reads the slot without calling. Native *calls* (collecting
//! arguments and invoking `f`) land in M6 — for now `resolve_word` may
//! produce a `Value::Func`, but nothing dispatches it.
//!
//! Milestone 7 adds *expression-based* evaluation: a single step evaluates a
//! prefix value followed by any chain of infix natives (`1 + 2 * 3` →
//! `((1 + 2) * 3)` = 9, Red's left-to-right no-precedence rule). SetWord
//! RHS and native arguments are both evaluated as expressions so that
//! `x: 1 + 2` and `print 1 + 2` work. Loop-variable names (`repeat 'i ...`
//! / `repeat i ...`) are pre-allocated by the binding pass so body
//! references resolve to the counter slot.
//!
//! Index contract: every evaluator function (`eval`, `eval_expression`,
//! `eval_prefix`, `dispatch_call`) leaves `*i` pointing at the *next*
//! unprocessed value — one past whatever it consumed.

use std::rc::Rc;

use red_core::lexer;
use red_core::parser::parse_program;
use red_core::value::{Binding, FuncDef, Series, Span, Symbol, Value};
use red_core::{Context, Env, Error, EvalError, RefineArgs};

use crate::binding::bind_pass;

/// Evaluate a block/paren value: walk its contents in order, returning the
/// last value. Non-block/paren values passed in are returned as-is (cloned).
///
/// This is the *block-walker*. Each step evaluates one *expression* (a
/// prefix value plus any trailing infix chain) via [`eval_expression`].
pub fn eval(block: &Value, env: &mut Env) -> Result<Value, EvalError> {
    let series = match block {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        _ => return Ok(block.clone()),
    };

    let mut last = Value::None;
    let data = series.data.borrow();
    let mut i = series.index;
    while i < data.len() {
        last = eval_expression(&data, &mut i, env)?;
    }
    Ok(last)
}

/// Evaluate a single expression starting at `data[*i]`: a prefix value
/// followed by zero or more infix native applications (Red's left-to-right,
/// no-precedence rule). Advances `*i` past the entire expression.
///
/// Examples (within a block):
///   - `5`                → `Integer(5)`        (no trailing infix)
///   - `1 + 2`            → `Integer(3)`        (one infix op)
///   - `1 + 2 * 3`        → `Integer(9)`        (two infix ops, L-to-R)
///   - `foo`              → slot value          (word resolves, no infix)
///   - `print "hi"`       → `None`              (prefix native call)
///   - `x: 1 + 2`         → `Integer(3)`        (SetWord RHS is an expression)
pub(crate) fn eval_expression(
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let mut value = eval_prefix(data, i, env)?;

    // Chain infix natives left-to-right. Each infix native uses `value` as
    // its left operand and consumes one prefix value as its right.
    while *i < data.len() {
        let infix = match infix_native_at(&data[*i], env) {
            Some(fd) => fd,
            None => break,
        };
        let infix_span = data[*i].span_or_default();
        *i += 1; // consume the infix word itself; now *i points at the right operand
        let arity = infix.params.len();
        debug_assert!(
            arity >= 1,
            "infix native must take at least one operand (the left value)"
        );
        let mut args = Vec::with_capacity(arity);
        args.push(value);
        // The first operand (left value) is already evaluated; the remaining
        // `arity - 1` operands are consumed as prefix values from the block.
        for _ in 1..arity {
            if *i >= data.len() {
                return Err(EvalError::Arity {
                    native: Symbol::new("<infix>"),
                    expected: arity,
                    got: args.len(),
                    span: infix_span,
                });
            }
            args.push(eval_prefix(data, i, env)?);
        }
        let f = infix.native.unwrap();
        value = f(&args, &RefineArgs::empty(), env)?;
    }
    Ok(value)
}

/// If `v` is an unbound `Word` naming an infix native, return its `FuncDef`.
/// Infix natives are never invoked through the normal `dispatch_call` path —
/// they're consumed by [`eval_expression`] before the prefix evaluator ever
/// sees them.
fn infix_native_at(v: &Value, env: &Env) -> Option<Rc<FuncDef>> {
    let sym = match v {
        Value::Word { sym, binding, .. } | Value::GetWord { sym, binding, .. } => {
            if !matches!(binding, Binding::Unbound) {
                return None;
            }
            sym
        }
        _ => return None,
    };
    env.natives.get(sym).filter(|fd| fd.infix).cloned()
}

/// Evaluate a single prefix value (no infix chaining): literals are cloned,
/// `Paren` is walked eagerly, `Word` resolves and dispatches a native call,
/// `SetWord` consumes one trailing expression as its RHS, `GetWord` reads
/// the slot without invoking, `Block` is returned as data. Advances `*i`
/// past the consumed value (and any native args / SetWord RHS).
fn eval_prefix(
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let span = data[*i].span_or_default();
    let cur = data[*i].clone();
    *i += 1; // consume the prefix value itself
    match &cur {
        // Data / literals: returned as-is.
        Value::None
        | Value::Logic(_)
        | Value::Integer { .. }
        | Value::Float { .. }
        | Value::String { .. }
        | Value::String8(_)
        | Value::LitWord { .. }
        | Value::Block { .. }
        | Value::Func(_)
        | Value::Refinement { .. }
        | Value::Error(_) => Ok(cur),

        // Path: a function-headed path is a refined call (`copy/part`,
        // `find/case`); anything else is a data-path select (`block/2`,
        // `obj/field`) which lands in M19. Resolve the head; if it's a Func,
        // dispatch a refined call with the path tail as leading refinement
        // flags. Otherwise stub-error (M19).
        Value::Path {
            parts,
            span: path_span,
        } => dispatch_path_call(parts, *path_span, data, i, env),

        // Paren: walked eagerly in place. The recursion borrows the child
        // series's `RefCell`, distinct from the outer borrow.
        Value::Paren { series: p, .. } => {
            let p = p.clone();
            eval(&Value::Paren { series: p, span }, env)
        }

        Value::Word { sym, binding, .. } => {
            let resolved = resolve_word(sym, binding, env, span)?;
            // `dispatch_call` collects args starting at the new `*i`.
            dispatch_call(resolved, sym, data, i, env, span)
        }

        Value::SetWord { sym, binding, .. } => {
            // Evaluate the next *expression* as the RHS (so `x: 1 + 2` works).
            if *i >= data.len() {
                return Err(EvalError::Arity {
                    native: sym.clone(),
                    expected: 1,
                    got: 0,
                    span,
                });
            }
            let rhs = eval_expression(data, i, env)?;
            write_setword(sym, binding, rhs.clone(), env, span)?;
            // `foo: 5` evaluates to the written value (Red semantics).
            Ok(rhs)
        }

        Value::GetWord { sym, binding, .. } => {
            // GetWord returns the slot value (or native Func) without
            // invoking it — no argument collection, no dispatch.
            resolve_word(sym, binding, env, span)
        }
    }
}

/// If `resolved` is a `Func`, collect arguments from `data` (advancing `i`)
/// and invoke it. Native funcs (M6+) call their `NativeFn` directly; user
/// funcs (M9: created via `func`/`does`/`make function!`) push a `CallFrame`
/// with a per-call context clone, evaluate the body, and catch
/// `EvalError::Return` as the return value. Otherwise return `resolved`
/// as-is. `sym` is the calling word's symbol, used for arity-error messages.
///
/// Pre: `*i` points at the first potential argument (the calling word has
/// already been consumed by [`eval_prefix`]). Each argument is evaluated as a
/// full *expression* (so `print 1 + 2` passes `3`), not just a single prefix
/// value. Post: `*i` points past the last consumed argument.
fn dispatch_call(
    resolved: Value,
    sym: &Symbol,
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
    span: Span,
) -> Result<Value, EvalError> {
    dispatch_call_with_refs(resolved, sym, &[], data, i, env, span)
}

/// Like [`dispatch_call`] but the caller has already consumed some refinement
/// flags from a path (`copy/part` → head `copy`, leading ref `["part"]`).
/// `leading_refs` are refinement names already activated; they're merged with
/// any inline `/ref` tokens discovered at the call site during spec-order
/// collection.
fn dispatch_call_with_refs(
    resolved: Value,
    sym: &Symbol,
    leading_refs: &[Symbol],
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
    span: Span,
) -> Result<Value, EvalError> {
    let fd = match &resolved {
        Value::Func(fd) if fd.native.is_some() => fd.clone(),
        Value::Func(fd) => {
            let (args, refs) = collect_call_args(sym, fd, leading_refs, data, i, env, span)?;
            return call_user_func(fd, args, &refs, env);
        }
        _ => return Ok(resolved),
    };
    let (args, refs) = collect_call_args(sym, &fd, leading_refs, data, i, env, span)?;
    let f = fd.native.unwrap();
    f(&args, &refs, env)
}

/// A function-headed path (`copy/part [1 2 3] 2`) dispatches as a refined
/// call: resolve the head word; if it's a Func, treat the path tail as
/// leading refinement flags and delegate to [`dispatch_call_with_refs`].
/// Anything else (data-path select: `block/2`, `obj/field`) is a stub error
/// until M19.
fn dispatch_path_call(
    parts: &[Value],
    path_span: Span,
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if parts.is_empty() {
        return Err(EvalError::Native {
            message: "empty path".into(),
            span: path_span,
        });
    }
    // Resolve the head word.
    let (head_sym, head_binding) = match &parts[0] {
        Value::Word { sym, binding, .. } => (sym.clone(), binding.clone()),
        Value::GetWord { sym, binding, .. } => (sym.clone(), binding.clone()),
        Value::LitWord { sym, .. } => (sym.clone(), Binding::Unbound),
        other => {
            return Err(EvalError::Native {
                message: format!(
                    "path head must be a word, found {}",
                    crate::natives::type_name(other)
                ),
                span: path_span,
            });
        }
    };
    // Tail parts are stored as `Value::Word` by the parser; extract their
    // symbol names as leading refinement flags.
    let leading_refs: Vec<Symbol> = parts[1..]
        .iter()
        .filter_map(|p| match p {
            Value::Word { sym, .. } => Some(sym.clone()),
            _ => None,
        })
        .collect();
    let resolved = resolve_word(&head_sym, &head_binding, env, path_span)?;
    match resolved {
        Value::Func(_) => {
            dispatch_call_with_refs(resolved, &head_sym, &leading_refs, data, i, env, path_span)
        }
        _ => Err(EvalError::Native {
            message: "path select not implemented (M19)".into(),
            span: path_span,
        }),
    }
}

/// Collect positional args + refinement args in spec order (Red's
/// refinement semantics). Walks `fd.params` then `fd.refinements` in
/// declaration order; for each refinement, if it's in `leading_refs` (path
/// tail) or the next inline token is a matching `Value::Refinement`, the
/// refinement is active and its `arity` expressions are collected. Returns
/// the positional `args` and a `RefineArgs` of active refinements + their
/// collected arg values.
///
/// The special-case natives that take their first argument unevaluated
/// (`repeat`/`foreach`/`forall`/`make`/`to` — a word/name, not a value) are
/// honored only for the *positional* params; refinements on those natives
/// aren't supported (none declare any).
fn collect_call_args(
    sym: &Symbol,
    fd: &Rc<FuncDef>,
    leading_refs: &[Symbol],
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
    span: Span,
) -> Result<(Vec<Value>, RefineArgs), EvalError> {
    // Variadic natives (e.g. `return`, `print`) collect all remaining
    // expressions up to the next native word. They don't take refinements.
    if fd.variadic {
        let mut args = Vec::new();
        while *i < data.len() && !is_native_word(&data[*i], env) {
            args.push(eval_expression(data, i, env)?);
        }
        return Ok((args, RefineArgs::default()));
    }

    let arity = fd.params.len();
    let mut args: Vec<Value> = Vec::with_capacity(arity);
    let uneval_first = matches!(
        sym.as_str(),
        "repeat" | "foreach" | "forall" | "make" | "to" | "default"
    );

    // Positional params.
    for n in 0..arity {
        if *i >= data.len() {
            return Err(EvalError::Arity {
                native: sym.clone(),
                expected: arity,
                got: args.len(),
                span,
            });
        }
        if n == 0 && uneval_first {
            args.push(data[*i].clone());
            *i += 1;
        } else {
            args.push(eval_expression(data, i, env)?);
        }
    }

    // Refinements in spec order.
    let mut ref_pairs: Vec<(Symbol, Vec<Value>)> = Vec::new();
    for (ref_name, ref_args_spec) in &fd.refinements {
        let already_leading = leading_refs.iter().any(|r| r == ref_name);
        let mut active = already_leading;
        // Inline refinement flag? Peek the next value; if it's a matching
        // Refinement token, consume it and activate.
        if !active {
            if let Some(Value::Refinement { sym: rname, .. }) = data.get(*i) {
                if rname == ref_name {
                    *i += 1;
                    active = true;
                }
            }
        }
        if active {
            let mut collected = Vec::with_capacity(ref_args_spec.len());
            for _ in 0..ref_args_spec.len() {
                if *i >= data.len() {
                    return Err(EvalError::Arity {
                        native: sym.clone(),
                        expected: arity + ref_args_spec.len(),
                        got: args.len() + collected.len(),
                        span,
                    });
                }
                collected.push(eval_expression(data, i, env)?);
            }
            ref_pairs.push((ref_name.clone(), collected));
        }
    }

    Ok((args, RefineArgs::from_pairs(ref_pairs)))
}

/// Invoke a user-defined function: clone its `FuncDef.ctx` (fresh slot
/// storage per call so recursion is safe), fill param slots in order, fill
/// refinement flag + arg slots, push a `CallFrame`, evaluate the body, then
/// pop the frame. `EvalError::Return(v)` is caught and converted to
/// `Ok(v)` — that's how the `return` native exits a function. Any other
/// error propagates.
///
/// Slot layout (established by `bind_function_body`):
///   `[param_0 .. param_{n-1}] [ref_0_flag] [ref_0_arg_0 ..] [ref_1_flag] ...`
fn call_user_func(
    fd: &Rc<FuncDef>,
    args: Vec<Value>,
    refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let call_ctx = fd.ctx.clone();
    let mut slot = 0;
    for arg in args.iter() {
        call_ctx.set_slot(slot, arg.clone());
        slot += 1;
    }
    // Refinement slots follow param slots in spec order: for each declared
    // refinement, a logic flag slot then its arg-word slots.
    for (ref_name, ref_args_spec) in &fd.refinements {
        let active = refs.has(ref_name);
        call_ctx.set_slot(slot, Value::Logic(active));
        slot += 1;
        if active {
            if let Some(collected) = refs.get(ref_name) {
                for v in collected {
                    call_ctx.set_slot(slot, v.clone());
                    slot += 1;
                }
            }
        } else {
            // Inactive refinement: arg slots default to `none`.
            for _ in 0..ref_args_spec.len() {
                call_ctx.set_slot(slot, Value::None);
                slot += 1;
            }
        }
    }
    env.call_stack.push(crate::context::CallFrame {
        ctx: call_ctx,
        func: Some(Rc::clone(fd)),
    });
    let body_block = Value::Block {
        series: fd.body.clone(),
        span: Span::new(0, 0),
    };
    let result = eval(&body_block, env);
    env.call_stack.pop();
    match result {
        Ok(v) => Ok(v),
        Err(EvalError::Return(v)) => Ok(v),
        Err(e) => Err(e),
    }
}

/// True if `v` is an unbound `Word`/`GetWord` whose name is a registered
/// native. Used to stop variadic argument collection at the next native call.
fn is_native_word(v: &Value, env: &Env) -> bool {
    let sym = match v {
        Value::Word { sym, binding, .. } | Value::GetWord { sym, binding, .. } => {
            if !matches!(binding, Binding::Unbound) {
                return false;
            }
            sym
        }
        _ => return false,
    };
    env.natives.contains_key(sym)
}

/// Resolve a `Word`/`GetWord` to its value via the binding, or via the native
/// registry if unbound. `GetWord` shares this path in M5 since no natives are
/// registered yet; M6+ may diverge if `GetWord` should return a function
/// value without invoking it (it does here — invocation is the caller's job).
fn resolve_word(
    sym: &Symbol,
    binding: &Binding,
    env: &mut Env,
    span: Span,
) -> Result<Value, EvalError> {
    match binding {
        Binding::Local(ctx, idx) => Ok(ctx.slot_value(*idx)),
        Binding::Func(idx) => {
            // Function-local slot: read from the current call frame's
            // per-call context clone. `call_stack` is non-empty whenever
            // a function body is being evaluated.
            let frame = env
                .call_stack
                .last()
                .ok_or_else(|| EvalError::UnboundWord {
                    sym: sym.clone(),
                    span,
                })?;
            Ok(frame.ctx.slot_value(*idx))
        }
        Binding::Unbound => {
            if let Some(fd) = env.natives.get(sym) {
                Ok(Value::Func(Rc::clone(fd)))
            } else {
                Err(EvalError::UnboundWord {
                    sym: sym.clone(),
                    span,
                })
            }
        }
    }
}

/// Write `val` into the slot bound to a `SetWord`. For M5 all top-level
/// `SetWord`s are bound by `bind_pass`, so the `Unbound` arm only fires for
/// malformed trees and surfaces as an error (runtime slot allocation on a
/// shared `Rc<Context>` would require a `RefCell` name map, deferred).
fn write_setword(
    sym: &Symbol,
    binding: &Binding,
    val: Value,
    env: &mut Env,
    span: Span,
) -> Result<(), EvalError> {
    match binding {
        Binding::Local(ctx, idx) => {
            ctx.set_slot(*idx, val);
            Ok(())
        }
        Binding::Func(idx) => {
            // Write to the current call frame's function-local slot.
            let frame = env
                .call_stack
                .last()
                .ok_or_else(|| EvalError::UnboundWord {
                    sym: sym.clone(),
                    span,
                })?;
            frame.ctx.set_slot(*idx, val);
            Ok(())
        }
        Binding::Unbound => Err(EvalError::UnboundWord {
            sym: sym.clone(),
            span,
        }),
    }
}

/// End-to-end: lex → parse → bind → eval. Handles both bare bodies and
/// `Red [...] <body>` programs (the header is discarded for the POC).
pub fn run_source(src: &str) -> Result<Value, Error> {
    run_source_with_output(src, Box::new(std::io::stdout()))
}

/// Like `run_source` but with a custom output sink. Used by golden program
/// tests to capture native output into an in-memory buffer.
pub fn run_source_with_output(src: &str, out: Box<dyn std::io::Write>) -> Result<Value, Error> {
    let tokens = lexer::lex(src)?;
    let body = if tokens.is_empty() {
        Series::empty()
    } else {
        let (_header, body) = parse_program(&tokens)?;
        body
    };
    run_series_with_output(body, out)
}

/// End-to-end run that also returns the requested exit code (from `exit`/
/// `quit`). Mirrors `run_source` but yields `(last_value, exit_code)`. Used
/// by the CLI to propagate the script's exit status to the process.
pub fn run_source_with_exit(src: &str) -> Result<(Value, i32), Error> {
    run_source_with_exit_output(src, Box::new(std::io::stdout()))
}

/// Like `run_source_with_exit` but with a custom output sink.
pub fn run_source_with_exit_output(
    src: &str,
    out: Box<dyn std::io::Write>,
) -> Result<(Value, i32), Error> {
    let tokens = lexer::lex(src)?;
    let body = if tokens.is_empty() {
        Series::empty()
    } else {
        let (_header, body) = parse_program(&tokens)?;
        body
    };
    run_series_with_exit_output(body, out)
}

/// Evaluate an already-parsed body series with a fresh environment.
/// Constants (`none`/`true`/`false`/`newline`) are installed into the user
/// context before the binding pass, and natives (`print`/`prin`/`probe`) are
/// registered before eval.
pub fn run_series(body: Series) -> Result<Value, Error> {
    run_series_with_output(body, Box::new(std::io::stdout()))
}

/// Like `run_series` but with a custom output sink.
pub fn run_series_with_output(body: Series, out: Box<dyn std::io::Write>) -> Result<Value, Error> {
    Ok(run_series_inner(body, out)?.0)
}

/// Like `run_series` but returns the exit code from `exit`/`quit`. The CLI
/// uses this to set the process exit status.
pub fn run_series_with_exit_output(
    body: Series,
    out: Box<dyn std::io::Write>,
) -> Result<(Value, i32), Error> {
    run_series_inner(body, out)
}

/// Shared core: runs the body, catching `EvalError::Quit(code)` as a normal
/// termination with the given exit code. Other errors propagate as `Error`.
fn run_series_inner(body: Series, out: Box<dyn std::io::Write>) -> Result<(Value, i32), Error> {
    let ctx = Context::new();
    crate::natives::install_constants(&ctx);
    let ctx_rc = bind_pass(&body, ctx);
    let mut env = Env::new_with_output(ctx_rc, out);
    crate::natives::register_natives(&mut env);
    let block = Value::Block {
        series: body,
        span: Span::new(0, 0),
    };
    match eval(&block, &mut env) {
        Ok(v) => Ok((v, 0)),
        Err(EvalError::Quit(code)) => Ok((Value::None, code)),
        Err(e) => Err(Error::Eval(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use red_core::printer::mold_to_string;

    fn run(src: &str) -> Value {
        run_source(src).expect("run_source failed")
    }

    fn run_err(src: &str) -> Error {
        run_source(src).expect_err("expected error")
    }

    #[test]
    fn integer_literal() {
        assert_eq!(mold_to_string(&run("5")), "5");
    }

    #[test]
    fn setword_then_word() {
        assert_eq!(mold_to_string(&run("foo: 5 foo")), "5");
    }

    #[test]
    fn unbound_word_errors() {
        let err = run_err("foo");
        assert!(matches!(err, Error::Eval(EvalError::UnboundWord { .. })));
    }

    #[test]
    fn paren_eager() {
        // Paren walks eagerly; last value is the result.
        assert_eq!(mold_to_string(&run("(1 2 3)")), "3");
    }

    #[test]
    fn block_returns_as_data() {
        assert_eq!(mold_to_string(&run("[1 2 3]")), "[1 2 3]");
    }

    #[test]
    fn setword_returns_written_value() {
        // `foo: 5` itself evaluates to 5 (Red semantics).
        assert_eq!(mold_to_string(&run("foo: 5")), "5");
    }

    #[test]
    fn nested_block_data_preserved() {
        // Blocks inside the body are data, not walked.
        assert_eq!(mold_to_string(&run("[a [b c] d]")), "[a [b c] d]");
    }

    #[test]
    fn setword_then_word_in_nested_block_data() {
        // The inner `[foo]` is data here; the outer eval doesn't enter it.
        // `foo: 5 [foo]` returns the block `[foo]` (last value of the body).
        assert_eq!(mold_to_string(&run("foo: 5 [foo]")), "[foo]");
    }

    #[test]
    fn word_in_paren_resolves() {
        // Paren is walked eagerly, so `foo` inside resolves to 5.
        assert_eq!(mold_to_string(&run("foo: 5 (foo)")), "5");
    }

    #[test]
    fn multiple_assignments() {
        assert_eq!(mold_to_string(&run("a: 1 b: 2 a")), "1");
        assert_eq!(mold_to_string(&run("a: 1 b: 2 b")), "2");
    }

    #[test]
    fn getword_reads_slot() {
        assert_eq!(mold_to_string(&run("foo: 7 :foo")), "7");
    }

    #[test]
    fn litword_returns_as_data() {
        assert_eq!(mold_to_string(&run("'foo")), "'foo");
    }

    #[test]
    fn empty_source_returns_none() {
        assert_eq!(mold_to_string(&run("")), "none");
    }

    #[test]
    fn header_program_evaluates_body() {
        assert_eq!(mold_to_string(&run("Red [] foo: 42 foo")), "42");
    }

    #[test]
    fn setword_at_eof_errors() {
        let err = run_err("foo:");
        assert!(matches!(
            err,
            Error::Eval(EvalError::Arity { native, expected: 1, got: 0, .. })
            if native.as_str() == "foo"
        ));
    }

    // --- Milestone 7: expression / infix evaluation ---

    #[test]
    fn infix_addition() {
        assert_eq!(mold_to_string(&run("1 + 2")), "3");
    }

    #[test]
    fn infix_left_to_right_no_precedence() {
        // Red: `1 + 2 * 3` = `(1 + 2) * 3` = 9.
        assert_eq!(mold_to_string(&run("1 + 2 * 3")), "9");
    }

    #[test]
    fn setword_rhs_is_full_expression() {
        assert_eq!(mold_to_string(&run("x: 1 + 2 x")), "3");
    }

    #[test]
    fn native_arg_is_full_expression() {
        // `print 1 + 2` should pass 3 to print. The block's last value is
        // print's return: none.
        assert_eq!(mold_to_string(&run("print 1 + 2")), "none");
    }
}
