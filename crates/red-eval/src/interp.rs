//! Tree-walking evaluator: binding pass + `eval`.
//!
//! Milestone 5 scope: literals return themselves; `Block` is data (returned
//! as-is unless entered explicitly); `Paren` is walked eagerly in place;
//! `Word` resolves via its binding (or errors if unbound and not a native);
//! `SetWord` evaluates the next value and writes it into the bound slot;
//! `GetWord` reads the slot without calling. Native *calls* (collecting
//! arguments and invoking `f`) land in M6 — for now `resolve_word` may
//! produce a `Value::Func`, but nothing dispatches it.

use std::rc::Rc;

use red_core::lexer;
use red_core::parser::parse_program;
use red_core::value::{Binding, Series, Span, Symbol, Value};
use red_core::{Context, Env, Error, EvalError};

/// Walk `body` and attach `Binding::Local` to every word whose name matches a
/// slot allocated for a `SetWord`. Recurses into nested `Block`/`Paren`
/// contents so that words inside data blocks are also bound (matches Red
/// semantics: `foo: 5 [foo]` later `do`ne yields `[5]`).
///
/// Returns the `Rc<Context>` shared by all attached bindings. The caller
/// installs it into `Env.user_ctx` so eval-time writes flow through the same
/// slots.
pub fn bind_pass(body: &Series, user_ctx: Context) -> Rc<Context> {
    let mut ctx = user_ctx;
    collect_setwords(body, &mut ctx);
    let ctx_rc = Rc::new(ctx);
    attach_bindings(body, &ctx_rc);
    ctx_rc
}

/// Phase 1: allocate a slot in `ctx` for every `SetWord` encountered anywhere
/// in the tree. The slots are populated during eval, not here.
fn collect_setwords(series: &Series, ctx: &mut Context) {
    let data = series.data.borrow();
    for v in data.iter() {
        match v {
            Value::SetWord { sym, .. } => {
                ctx.slot_index(sym.clone());
            }
            Value::Block { series: s, .. } | Value::Paren { series: s, .. } => {
                collect_setwords(s, ctx);
            }
            _ => {}
        }
    }
}

/// Phase 2: for every `Word`/`SetWord`/`GetWord` whose name is now in `ctx`,
/// replace its `binding` with `Binding::Local(Rc::clone(ctx), idx)`. Words
/// with no matching slot stay `Unbound` (function locals / natives resolved
/// at eval time).
fn attach_bindings(series: &Series, ctx: &Rc<Context>) {
    let mut data = series.data.borrow_mut();
    for i in 0..data.len() {
        match &mut data[i] {
            Value::Block { series, .. } | Value::Paren { series, .. } => {
                let child = series.clone();
                // Recurse into the child series — a different `RefCell`, so
                // the outer `borrow_mut` above stays valid.
                attach_bindings(&child, ctx);
            }
            Value::Word { sym, binding }
            | Value::SetWord { sym, binding }
            | Value::GetWord { sym, binding } => {
                if let Some(idx) = ctx.index_of(sym) {
                    *binding = Binding::Local(Rc::clone(ctx), idx);
                }
            }
            _ => {}
        }
    }
}

/// Evaluate a block/paren value: walk its contents in order, returning the
/// last value. Non-block/paren values passed in are returned as-is (cloned).
///
/// This is the *block-walker*. To evaluate a single value as if it were the
/// sole element of a block (e.g. the RHS of a `SetWord`), use `eval_value`.
pub fn eval(block: &Value, env: &mut Env) -> Result<Value, EvalError> {
    let series = match block {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        _ => return Ok(block.clone()),
    };

    let mut last = Value::None;
    let data = series.data.borrow();
    let mut i = series.index;
    while i < data.len() {
        let span = data[i].span().unwrap_or(Span::new(0, 0));
        let cur = &data[i];
        last = match cur {
            // Data / literals: returned as-is.
            Value::None
            | Value::Logic(_)
            | Value::Integer(_)
            | Value::Float(_)
            | Value::String(_)
            | Value::String8(_)
            | Value::LitWord(_)
            | Value::Block { .. }
            | Value::Func(_)
            | Value::Path(_) => cur.clone(),

            // Paren: walked eagerly in place. The recursion borrows the
            // child series's `RefCell`, distinct from the outer borrow.
            Value::Paren { series: p, .. } => {
                let p = p.clone();
                eval(&Value::Paren { series: p, span }, env)?
            }

            Value::Word { sym, binding } => {
                let resolved = resolve_word(sym, binding, env, span)?;
                dispatch_call(resolved, sym, &data, &mut i, env, span)?
            }

            Value::SetWord { sym, binding } => {
                // Evaluate the next value as the RHS.
                i += 1;
                if i >= data.len() {
                    return Err(EvalError::Arity {
                        native: sym.clone(),
                        expected: 1,
                        got: 0,
                        span,
                    });
                }
                let rhs_span = data[i].span().unwrap_or(Span::new(0, 0));
                let rhs = eval_value(&data[i], env)?;
                write_setword(sym, binding, rhs.clone(), env, rhs_span)?;
                // `foo: 5` evaluates to the written value (Red semantics).
                rhs
            }

            Value::GetWord { sym, binding } => {
                // GetWord returns the slot value (or native Func) without
                // invoking it — no argument collection, no dispatch.
                resolve_word(sym, binding, env, span)?
            }
        };
        i += 1;
    }
    Ok(last)
}

/// If `resolved` is a native-bearing `Func`, collect arguments from `data`
/// (advancing `i`) and invoke the native. Otherwise return `resolved` as-is
/// (user-defined funcs land in M9). `sym` is the calling word's symbol, used
/// for arity-error messages.
fn dispatch_call(
    resolved: Value,
    sym: &Symbol,
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
    span: Span,
) -> Result<Value, EvalError> {
    let fd = match &resolved {
        Value::Func(fd) if fd.native.is_some() => fd.clone(),
        _ => return Ok(resolved),
    };
    let f = fd.native.unwrap();
    let mut args = Vec::new();
    if fd.variadic {
        // Consume remaining values until the next native word or end of block.
        while *i + 1 < data.len() && !is_native_word(&data[*i + 1], env) {
            *i += 1;
            args.push(eval_value(&data[*i], env)?);
        }
    } else {
        let arity = fd.params.len();
        for _ in 0..arity {
            *i += 1;
            if *i >= data.len() {
                return Err(EvalError::Arity {
                    native: sym.clone(),
                    expected: arity,
                    got: args.len(),
                    span,
                });
            }
            args.push(eval_value(&data[*i], env)?);
        }
    }
    f(&args, env)
}

/// True if `v` is an unbound `Word`/`GetWord` whose name is a registered
/// native. Used to stop variadic argument collection at the next native call.
fn is_native_word(v: &Value, env: &Env) -> bool {
    let sym = match v {
        Value::Word { sym, binding } | Value::GetWord { sym, binding } => {
            if !matches!(binding, Binding::Unbound) {
                return false;
            }
            sym
        }
        _ => return false,
    };
    env.natives.contains_key(sym)
}

/// Evaluate a single value as if it were the sole element of a block.
/// `Block` → returned as data; `Paren` → walked eagerly; `Word`/`GetWord` →
/// resolved; literals → cloned. Used for `SetWord` RHS and any other place
/// we need to reduce one value.
fn eval_value(v: &Value, env: &mut Env) -> Result<Value, EvalError> {
    match v {
        Value::Paren { .. } => eval(v, env),
        Value::Word { sym, binding } | Value::GetWord { sym, binding } => {
            resolve_word(sym, binding, env, v.span().unwrap_or(Span::new(0, 0)))
        }
        _ => Ok(v.clone()),
    }
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
        Binding::Func => {
            // Reserved for M9 (function-parameter binding).
            Err(EvalError::UnboundWord {
                sym: sym.clone(),
                span,
            })
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
    _env: &mut Env,
    span: Span,
) -> Result<(), EvalError> {
    match binding {
        Binding::Local(ctx, idx) => {
            ctx.set_slot(*idx, val);
            Ok(())
        }
        Binding::Unbound => Err(EvalError::UnboundWord {
            sym: sym.clone(),
            span,
        }),
        Binding::Func => Err(EvalError::UnboundWord {
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

/// Evaluate an already-parsed body series with a fresh environment.
/// Constants (`none`/`true`/`false`/`newline`) are installed into the user
/// context before the binding pass, and natives (`print`/`prin`/`probe`) are
/// registered before eval.
pub fn run_series(body: Series) -> Result<Value, Error> {
    run_series_with_output(body, Box::new(std::io::stdout()))
}

/// Like `run_series` but with a custom output sink.
pub fn run_series_with_output(body: Series, out: Box<dyn std::io::Write>) -> Result<Value, Error> {
    let mut ctx = Context::new();
    crate::natives::install_constants(&mut ctx);
    let ctx_rc = bind_pass(&body, ctx);
    let mut env = Env::new_with_output(ctx_rc, out);
    crate::natives::register_natives(&mut env);
    let block = Value::Block {
        series: body,
        span: Span::new(0, 0),
    };
    Ok(eval(&block, &mut env)?)
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
}
