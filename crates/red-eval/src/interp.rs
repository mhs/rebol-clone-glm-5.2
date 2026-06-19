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
use red_core::{Context, Env, Error, EvalError};

/// Walk `body` and attach `Binding::Local` to every word whose name matches a
/// slot allocated for a `SetWord` or a `repeat` loop variable. Recurses into
/// nested `Block`/`Paren` contents so that words inside data blocks are also
/// bound (matches Red semantics: `foo: 5 [foo]` later `do`ne yields `[5]`).
///
/// Returns the `Rc<Context>` shared by all attached bindings. The caller
/// installs it into `Env.user_ctx` so eval-time writes flow through the same
/// slots.
pub fn bind_pass(body: &Series, user_ctx: Context) -> Rc<Context> {
    let mut ctx = user_ctx;
    collect_setwords(body, &mut ctx);
    collect_loop_vars(body, &mut ctx);
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

/// Phase 1b: allocate a slot for every word introduced as a loop variable by
/// `repeat`, `foreach`, or `forall`. Each is recognized in either of two
/// forms:
/// - `repeat 'i <count> <body>`  (lit-word counter, Red canonical form)
/// - `repeat i <count> <body>`   (bare-word counter, accepted by the POC)
/// - `foreach 'word <series> <body>` / `forall 'word <series> <body>`
///
/// The lit-word/bare-word value itself is *not* a SetWord, so without this
/// pass the loop name would never get a slot and body references would
/// resolve as unbound. Recurses into nested `Block`/`Paren`.
fn collect_loop_vars(series: &Series, ctx: &mut Context) {
    let data = series.data.borrow();
    let n = data.len();
    let mut i = 0;
    while i < n {
        match &data[i] {
            Value::Word {
                sym,
                binding: Binding::Unbound,
            } if matches!(sym.as_str(), "repeat" | "foreach" | "forall") => {
                if i + 1 < n {
                    let name = match &data[i + 1] {
                        Value::LitWord(sym) => Some(sym.clone()),
                        Value::Word {
                            sym,
                            binding: Binding::Unbound,
                        } => Some(sym.clone()),
                        _ => None,
                    };
                    if let Some(sym) = name {
                        ctx.slot_index(sym);
                    }
                }
                i += 1;
            }
            Value::Block { series: s, .. } | Value::Paren { series: s, .. } => {
                let child = s.clone();
                collect_loop_vars(&child, ctx);
                i += 1;
            }
            _ => i += 1,
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
                    span: Span::new(0, 0),
                });
            }
            args.push(eval_prefix(data, i, env)?);
        }
        let f = infix.native.unwrap();
        value = f(&args, env)?;
    }
    Ok(value)
}

/// If `v` is an unbound `Word` naming an infix native, return its `FuncDef`.
/// Infix natives are never invoked through the normal `dispatch_call` path —
/// they're consumed by [`eval_expression`] before the prefix evaluator ever
/// sees them.
fn infix_native_at(v: &Value, env: &Env) -> Option<Rc<FuncDef>> {
    let sym = match v {
        Value::Word { sym, binding } | Value::GetWord { sym, binding } => {
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
    let span = data[*i].span().unwrap_or(Span::new(0, 0));
    let cur = data[*i].clone();
    *i += 1; // consume the prefix value itself
    match &cur {
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
        | Value::Path(_) => Ok(cur),

        // Paren: walked eagerly in place. The recursion borrows the child
        // series's `RefCell`, distinct from the outer borrow.
        Value::Paren { series: p, .. } => {
            let p = p.clone();
            eval(&Value::Paren { series: p, span }, env)
        }

        Value::Word { sym, binding } => {
            let resolved = resolve_word(sym, binding, env, span)?;
            // `dispatch_call` collects args starting at the new `*i`.
            dispatch_call(resolved, sym, data, i, env, span)
        }

        Value::SetWord { sym, binding } => {
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

        Value::GetWord { sym, binding } => {
            // GetWord returns the slot value (or native Func) without
            // invoking it — no argument collection, no dispatch.
            resolve_word(sym, binding, env, span)
        }
    }
}

/// If `resolved` is a native-bearing `Func`, collect arguments from `data`
/// (advancing `i`) and invoke the native. Otherwise return `resolved` as-is
/// (user-defined funcs land in M9). `sym` is the calling word's symbol, used
/// for arity-error messages.
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
    let fd = match &resolved {
        Value::Func(fd) if fd.native.is_some() => fd.clone(),
        _ => return Ok(resolved),
    };
    let f = fd.native.unwrap();
    let mut args = Vec::new();
    if fd.variadic {
        // Consume remaining expressions until the next native word or end of
        // block.
        while *i < data.len() && !is_native_word(&data[*i], env) {
            args.push(eval_expression(data, i, env)?);
        }
    } else {
        let arity = fd.params.len();
        for n in 0..arity {
            if *i >= data.len() {
                return Err(EvalError::Arity {
                    native: sym.clone(),
                    expected: arity,
                    got: args.len(),
                    span,
                });
            }
            // `repeat`/`foreach`/`forall`'s first argument is a word/lit-word
            // *name*, not a value to evaluate. Pass it through unevaluated so
            // the native can bind it as the loop counter / iterator. (Red uses
            // a lit-word here; the POC also accepts a bare word for
            // ergonomics.)
            if n == 0 && matches!(sym.as_str(), "repeat" | "foreach" | "forall") {
                args.push(data[*i].clone());
                *i += 1;
            } else {
                args.push(eval_expression(data, i, env)?);
            }
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
