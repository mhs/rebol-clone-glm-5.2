//! Binding/word natives: `get`, `set`, `value?`, `use`, `bind`.
//!
//! These manipulate word→slot bindings in the user context (and, for `use`,
//! a layered child context). `bind` re-binds a block's (or function's body's)
//! words to the user context, deep-copying first.

use std::rc::Rc;

use red_core::value::{Binding, FuncDef, Series, Span, Value};
use red_core::{Env, EvalError, RefineArgs, Symbol};

use super::{arity_err, expect_block, type_name};
use crate::interp::dispatch_block;

// ---------------------------------------------------------------------------
// get / set
// ---------------------------------------------------------------------------

/// `get 'word` — returns the value bound to `word`. If the word carries a
/// `Binding::Local` (e.g. the result of `in object 'word`), reads from that
/// context; otherwise falls back to `env.user_ctx`. Also accepts a block of
/// words, returning a block of their values (M18).
pub(crate) fn get_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "get", 1, args.len()));
    }
    // Block form: `get [a b c]` → `[val-a val-b val-c]`.
    if let Value::Block { series, .. } = &args[0] {
        let data = series.data.borrow();
        let results: Vec<Value> = data
            .iter()
            .map(|w| get_one(w, env, w.span_or_default()))
            .collect::<Result<_, _>>()?;
        return Ok(Value::block(Series::new(results)));
    }
    get_one(&args[0], env, args[0].span_or_default())
}

fn get_one(v: &Value, env: &mut Env, span: Span) -> Result<Value, EvalError> {
    match v {
        Value::LitWord { sym, .. } => env.user_ctx.get(sym).ok_or_else(|| EvalError::UnboundWord {
            sym: sym.clone(),
            span,
        }),
        Value::Word { sym, binding, .. } | Value::GetWord { sym, binding, .. } => match binding {
            Binding::Local(ctx, idx) => Ok(ctx.slot_value(*idx)),
            Binding::Func(idx) => {
                let frame = env
                    .call_stack
                    .last()
                    .ok_or_else(|| EvalError::UnboundWord {
                        sym: sym.clone(),
                        span,
                    })?;
                Ok(frame.ctx.slot_value(*idx))
            }
            // M60: closure capture cell — read from the active frame's captures.
            Binding::Closure(idx) => {
                let frame = env
                    .call_stack
                    .last()
                    .ok_or_else(|| EvalError::UnboundWord {
                        sym: sym.clone(),
                        span,
                    })?;
                let captures = frame.captures.as_ref().ok_or_else(|| EvalError::Native {
                    message: format!("closure: no capture cell for {:?}", sym.as_str()),
                    span,
                })?;
                // M65: bounds check (parity with the VM's LoadCapture guard).
                let cell = captures.get(*idx).ok_or_else(|| EvalError::Native {
                    message: format!("closure: capture index {idx} out of bounds"),
                    span,
                })?;
                Ok(cell.borrow().clone())
            }
            Binding::Unbound => env.user_ctx.get(sym).ok_or_else(|| EvalError::UnboundWord {
                sym: sym.clone(),
                span,
            }),
            // Lexical bindings are VM-only; `get` is a runtime native that the
            // walker executes, so reaching this arm means a block reached the
            // walker with a lexical binding it shouldn't have. Treat like
            // unbound for resolution purposes (best-effort) but surface the
            // mismatch in the error path.
            Binding::Lexical(_, _) => Err(EvalError::Native {
                message: format!(
                    "lexical binding for {:?} not yet supported in the tree-walker",
                    sym.as_str()
                ),
                span,
            }),
        },
        other => Err(EvalError::TypeError {
            expected: "word!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `set 'word value` — writes `value` into `word`'s slot. If the word carries
/// a `Binding::Local` (e.g. from `in object 'word`), writes to that context;
/// otherwise writes to `env.user_ctx`. Also accepts block operands:
/// `set [a b] [1 2]` sets each word in parallel (M18). Returns the value(s).
pub(crate) fn set_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "set", 2, args.len()));
    }
    // Block form: `set [a b c] [1 2 3]` or `set [a b c] 99` (all get 99).
    if let Value::Block { series, .. } = &args[0] {
        let words_data = series.data.borrow().clone();
        let values: Vec<Value> = if let Value::Block { series: vs, .. } = &args[1] {
            vs.data.borrow().clone()
        } else {
            vec![args[1].clone(); words_data.len()]
        };
        for (w, v) in words_data.iter().zip(values.iter()) {
            set_one(w, v.clone(), env)?;
        }
        return Ok(args[1].clone());
    }
    let val = args[1].clone();
    set_one(&args[0], val.clone(), env)?;
    Ok(val)
}

fn set_one(v: &Value, val: Value, env: &mut Env) -> Result<(), EvalError> {
    match v {
        Value::LitWord { sym, .. } => {
            if let Some(idx) = env.user_ctx.index_of(sym) {
                env.user_ctx.set_slot(idx, val);
                Ok(())
            } else {
                Err(EvalError::UnboundWord {
                    sym: sym.clone(),
                    span: v.span_or_default(),
                })
            }
        }
        Value::Word { sym, binding, .. } | Value::SetWord { sym, binding, .. } => match binding {
            Binding::Local(ctx, idx) => {
                ctx.set_slot(*idx, val);
                Ok(())
            }
            Binding::Func(idx) => {
                let frame = env
                    .call_stack
                    .last_mut()
                    .ok_or_else(|| EvalError::UnboundWord {
                        sym: sym.clone(),
                        span: v.span_or_default(),
                    })?;
                frame.ctx.set_slot(*idx, val);
                Ok(())
            }
            // M60: closure capture cell — write to the active frame's captures.
            Binding::Closure(idx) => {
                let frame = env
                    .call_stack
                    .last_mut()
                    .ok_or_else(|| EvalError::UnboundWord {
                        sym: sym.clone(),
                        span: v.span_or_default(),
                    })?;
                let captures = frame.captures.as_ref().ok_or_else(|| EvalError::Native {
                    message: format!("closure: no capture cell for {:?}", sym.as_str()),
                    span: v.span_or_default(),
                })?;
                // M65: bounds check (parity with the VM's SetCapture guard).
                let cell = captures.get(*idx).ok_or_else(|| EvalError::Native {
                    message: format!("closure: capture index {idx} out of bounds"),
                    span: v.span_or_default(),
                })?;
                *cell.borrow_mut() = val;
                Ok(())
            }
            Binding::Unbound => {
                if let Some(idx) = env.user_ctx.index_of(sym) {
                    env.user_ctx.set_slot(idx, val);
                    Ok(())
                } else {
                    Err(EvalError::UnboundWord {
                        sym: sym.clone(),
                        span: v.span_or_default(),
                    })
                }
            }
            // Lexical bindings are VM-only; `set` is a runtime native run by
            // the walker, so this arm should not be reached. Surface as an
            // error to catch any routing bug.
            Binding::Lexical(_, _) => Err(EvalError::Native {
                message: format!(
                    "lexical binding for {:?} not yet supported in the tree-walker",
                    sym.as_str()
                ),
                span: v.span_or_default(),
            }),
        },
        other => Err(EvalError::TypeError {
            expected: "word!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

// ---------------------------------------------------------------------------
// value? / use / bind
// ---------------------------------------------------------------------------

/// `value? 'word` — `true` if `word` has a value in the user context, else
/// `false`.
pub(crate) fn value_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
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

/// `char? value` — `true` if `value` is a `char!`, else `false`. (M38)
pub(crate) fn char_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "char?", 1, 0));
    }
    Ok(Value::Logic(matches!(args[0], Value::Char { .. })))
}

// ---------------------------------------------------------------------------
// M39 type predicates + type?/types-of
// ---------------------------------------------------------------------------

/// Helper: arity-1 predicate that returns `Value::Logic(matches!(args[0], ..))`.
fn pred1(args: &[Value], name: &str, f: impl Fn(&Value) -> bool) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, name, 1, 0));
    }
    Ok(Value::Logic(f(&args[0])))
}

pub(crate) fn integer_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "integer?", |v| matches!(v, Value::Integer { .. }))
}

pub(crate) fn float_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "float?", |v| matches!(v, Value::Float { .. }))
}

/// `percent?` — true for `percent!` (`Value::Percent`). M80. Red parity: a
/// distinct scalar type; NOT a member of `number!` (per Red's `types-of`).
pub(crate) fn percent_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "percent?", |v| matches!(v, Value::Percent { .. }))
}

/// `number?` — true for `integer!` or `float!`.
pub(crate) fn number_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "number?", |v| {
        matches!(v, Value::Integer { .. } | Value::Float { .. })
    })
}

pub(crate) fn string_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "string?", |v| matches!(v, Value::String { .. }))
}

pub(crate) fn logic_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "logic?", |v| matches!(v, Value::Logic(_)))
}

pub(crate) fn none_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "none?", |v| matches!(v, Value::None))
}

/// `binary?` — true for `binary!` (`Value::String8`). The variant exists
/// (M16 stub); M41 wires the lexer/parser/converters to make it reachable
/// from source. The predicate is real today.
pub(crate) fn binary_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "binary?", |v| matches!(v, Value::String8 { .. }))
}

/// `pair?` — true for `pair!` (`Value::Pair`). M44.
pub(crate) fn pair_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "pair?", |v| matches!(v, Value::Pair { .. }))
}

/// `tuple?` — true for `tuple!` (`Value::Tuple`). M44.
pub(crate) fn tuple_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "tuple?", |v| matches!(v, Value::Tuple { .. }))
}

/// `date?` — true for `date!` (`Value::Date`). M45.
pub(crate) fn date_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "date?", |v| matches!(v, Value::Date { .. }))
}

/// `bitset?` — true for `bitset!` (`Value::Bitset`). M46.
pub(crate) fn bitset_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "bitset?", |v| matches!(v, Value::Bitset(_)))
}

/// `time?` — true for `date!` values that have a time component (non-midnight
/// `dt`) OR a zone (`zone != None`). Matches Red's `time?` semantics broadly:
/// any `date!` that isn't date-only. M45.
pub(crate) fn time_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "time?", |v| match v {
        Value::Date { dt, .. } => dt.has_time() || dt.zone.is_some(),
        _ => false,
    })
}

/// `error?` — true for `error!` (`Value::Error`). The variant exists (M16
/// basic `try`/`catch`/`throw`); M42 extends the field set, not the variant.
pub(crate) fn error_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "error?", |v| matches!(v, Value::Error(_)))
}

/// `attempted?` — M42 alias of `error?`. True if the value is an `error!`
/// (i.e. `attempt`-shaped: a prior call returned an error value). Red
/// parity; `none?` is the conventional `attempt`-failure check, but Red
/// also exposes `attempted?` for explicit error-value testing.
pub(crate) fn attempted_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "attempted?", |v| matches!(v, Value::Error(_)))
}

pub(crate) fn word_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "word?", |v| matches!(v, Value::Word { .. }))
}

pub(crate) fn set_word_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "set-word?", |v| matches!(v, Value::SetWord { .. }))
}

pub(crate) fn get_word_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "get-word?", |v| matches!(v, Value::GetWord { .. }))
}

pub(crate) fn lit_word_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "lit-word?", |v| matches!(v, Value::LitWord { .. }))
}

pub(crate) fn refinement_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "refinement?", |v| {
        matches!(v, Value::Refinement { .. })
    })
}

/// `any-word?` — true for any word-family value (`word!`/`set-word!`/
/// `get-word!`/`lit-word!`/`refinement!`).
pub(crate) fn any_word_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "any-word?", |v| {
        matches!(
            v,
            Value::Word { .. }
                | Value::SetWord { .. }
                | Value::GetWord { .. }
                | Value::LitWord { .. }
                | Value::Refinement { .. }
        )
    })
}

/// `any-path?` — true for any path-family value (`path!`/`get-path!`/
/// `lit-path!`/`set-path!`).
pub(crate) fn any_path_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "any-path?", |v| {
        matches!(
            v,
            Value::Path { .. }
                | Value::GetPath { .. }
                | Value::LitPath { .. }
                | Value::SetPath { .. }
        )
    })
}

/// `any-object?` — true for `object!`. Umbrella category (only one member
/// today; future types like `module!` would join).
pub(crate) fn any_object_predicate(
    args: &[Value],
    _r: &RefineArgs,
    _e: &mut Env,
) -> Result<Value, EvalError> {
    pred1(args, "any-object?", |v| matches!(v, Value::Object(_)))
}

/// `type? value` — returns the type word for `value` (e.g. `integer!`,
/// `string!`, `char!`). Mirrors Red's `type?` native (distinct from the `?`
/// predicate family, which returns `logic!`).
pub(crate) fn type_q(args: &[Value], _r: &RefineArgs, _e: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "type?", 1, 0));
    }
    Ok(Value::word(type_name(&args[0])))
}

/// `types-of value` — returns a block of all type words the value matches,
/// including umbrella categories (`number!`/`any-word!`/`any-path!`/
/// `any-block!`/`any-object!`/`series!`). E.g. `types-of 5` →
/// `[integer! number!]`; `types-of 'foo` → `[word! any-word!]`.
pub(crate) fn types_of(args: &[Value], _r: &RefineArgs, _e: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "types-of", 1, 0));
    }
    let v = &args[0];
    let mut out: Vec<Value> = Vec::new();
    let primary = type_name(v);
    out.push(Value::word(primary));

    // Umbrella categories.
    let is_number = matches!(v, Value::Integer { .. } | Value::Float { .. });
    let is_any_word = matches!(
        v,
        Value::Word { .. }
            | Value::SetWord { .. }
            | Value::GetWord { .. }
            | Value::LitWord { .. }
            | Value::Refinement { .. }
    );
    let is_any_path = matches!(
        v,
        Value::Path { .. } | Value::GetPath { .. } | Value::LitPath { .. } | Value::SetPath { .. }
    );
    let is_any_block = matches!(
        v,
        Value::Block { .. }
            | Value::Paren { .. }
            | Value::Path { .. }
            | Value::GetPath { .. }
            | Value::LitPath { .. }
            | Value::SetPath { .. }
    );
    let is_any_object = matches!(v, Value::Object(_));
    // `series!` covers blocks, parens, paths, strings, binary, files, urls.
    let is_series = matches!(
        v,
        Value::Block { .. }
            | Value::Paren { .. }
            | Value::Path { .. }
            | Value::GetPath { .. }
            | Value::LitPath { .. }
            | Value::SetPath { .. }
            | Value::String { .. }
            | Value::String8 { .. }
            | Value::File { .. }
            | Value::Url { .. }
    );
    let is_any_string = matches!(v, Value::String { .. } | Value::String8 { .. });

    if is_number {
        out.push(Value::word("number!"));
    }
    if is_any_word {
        out.push(Value::word("any-word!"));
    }
    if is_any_path {
        out.push(Value::word("any-path!"));
    }
    if is_any_block {
        out.push(Value::word("any-block!"));
    }
    if is_any_string {
        out.push(Value::word("any-string!"));
    }
    if is_any_object {
        out.push(Value::word("any-object!"));
    }
    if is_series {
        out.push(Value::word("series!"));
    }

    Ok(Value::block(Series::new(out)))
}

// ---------------------------------------------------------------------------
// Registration for the M39 type-predicate natives.
// ---------------------------------------------------------------------------

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

pub(crate) fn register_word_predicate_natives(env: &mut Env) {
    use red_core::value::FuncDef;
    use std::rc::Rc as StdRc;

    let reg = |env: &mut Env, name: &str, f: NF| {
        let params: Vec<Symbol> = vec![Symbol::new("__arg0")];
        env.natives.insert(
            Symbol::new(name),
            StdRc::new(FuncDef {
                params,
                native: Some(f),
                variadic: false,
                infix: false,
                ..Default::default()
            }),
        );
    };

    // Scalar type predicates.
    reg(env, "integer?", integer_predicate);
    reg(env, "float?", float_predicate);
    reg(env, "percent?", percent_predicate);
    reg(env, "number?", number_predicate);
    reg(env, "string?", string_predicate);
    reg(env, "logic?", logic_predicate);
    reg(env, "none?", none_predicate);
    reg(env, "binary?", binary_predicate);
    reg(env, "pair?", pair_predicate);
    reg(env, "tuple?", tuple_predicate);
    reg(env, "date?", date_predicate);
    reg(env, "time?", time_predicate);
    reg(env, "bitset?", bitset_predicate);
    reg(env, "error?", error_predicate);
    reg(env, "attempted?", attempted_predicate);

    // Word-family predicates.
    reg(env, "word?", word_predicate);
    reg(env, "set-word?", set_word_predicate);
    reg(env, "get-word?", get_word_predicate);
    reg(env, "lit-word?", lit_word_predicate);
    reg(env, "refinement?", refinement_predicate);
    reg(env, "any-word?", any_word_predicate);

    // Path-family predicates.
    reg(env, "any-path?", any_path_predicate);

    // Object predicates.
    reg(env, "any-object?", any_object_predicate);

    // Introspection.
    reg(env, "type?", type_q);
    reg(env, "types-of", types_of);
}

/// `use [words] block` — evaluates `block` with the listed words bound as
/// locals in a fresh child context layered over the user context. Body
/// SetWords and loop vars inside `block` are also collected as use-locals
/// (scoped to the child), so `use` provides a self-contained local scope.
/// Outer user-context words remain visible. The locals do not persist after
/// `use` returns. Returns the block's last value.
pub(crate) fn use_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
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
    let block = Value::block(rebound);
    // `use`'s rebound block carries `Binding::Local(child_rc, _)` for the
    // declared locals — foreign w.r.t. `env.user_ctx`. `dispatch_block`
    // detects that via `has_foreign_bindings` and routes to the walker,
    // so this is correct in both `Walk` and `Vm` modes. (The walker reads
    // `env.user_ctx`, which we just swapped for `child_rc`, so the child
    // bindings resolve correctly.)
    let result = dispatch_block(&block, env);
    env.user_ctx = saved;
    result
}

/// `bind block 'word` — rebinds words in `block` to the user context (the
/// context where `word` is bound). For the POC, the second argument names a
/// word in the user context (the canonical Red form takes a context value;
/// objects are out of scope, so we accept a word/lit-word and bind to the
/// user context it lives in). Returns the rebound block (a deep copy).
///
/// M27: also accepts a `function!` as the first argument (`bind :func 'word`),
/// returning a new function whose body words are rebound to the user
/// context. The original function's VM compiled-block cache entry is
/// invalidated so the next call recompiles against the new bindings.
pub(crate) fn bind_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "bind", 2, args.len()));
    }
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
    let all_names: Vec<Symbol> = env.user_ctx.names.borrow().keys().cloned().collect();
    match &args[0] {
        // Block form: deep-copy the block so we don't mutate shared data,
        // then rebind every word whose name is in the user context to a
        // `Binding::Local` pointing at it.
        Value::Block { series, .. } => {
            let rebound = crate::binding::deep_clone_series(series);
            crate::binding::rebind_to_context(&rebound, &env.user_ctx, &all_names);
            Ok(Value::block(rebound))
        }
        // Function form (M27): clone the FuncDef, deep-clone its body, rebind
        // body words, invalidate the original's VM cache entry. Returns a new
        // `Value::Func` (a deep copy — the original is untouched).
        Value::Func(fd) => {
            let mut new_fd: FuncDef = (**fd).clone();
            new_fd.body = crate::binding::deep_clone_series(&fd.body);
            crate::binding::rebind_to_context(&new_fd.body, &env.user_ctx, &all_names);
            new_fd.invalidate_compiled();
            // The original `fd`'s Env-level cache entry is now stale (its
            // body's bindings don't match what was compiled, if it was ever
            // called from the VM). Invalidate so a subsequent call on the
            // original recompiles. (In practice `bind` returns a *new* func,
            // so the original may still be called with its existing bindings —
            // but if the caller intended to mutate in place, this is safe.)
            env.invalidate_func_cache(fd);
            Ok(Value::Func(Rc::new(new_fd)))
        }
        other => Err(EvalError::TypeError {
            expected: "block! or function!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}
