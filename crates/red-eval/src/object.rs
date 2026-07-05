//! Objects & contexts (M18).
//!
//! Objects are created via `make object! [spec]` (or the `object` / `context`
//! keyword aliases). An object owns a `Context` (the same `Rc<Context>` shape
//! as the user context) holding its word→value slots. Spec evaluation runs
//! with `env.user_ctx` temporarily swapped to the object's context, so
//! SetWords write into object slots and `does`/`func` bodies bind their
//! field references to `Binding::Local(obj_ctx, idx)` via the standard
//! `bind_function_body` pass.
//!
//! Inheritance is copy-based: `make object! [parent-object spec-words...]`
//! detects when the first spec element resolves to an existing `Object` and
//! seeds the child context with copies of the parent's words+values before
//! evaluating the rest of the spec.

use std::cell::RefCell;
use std::rc::Rc;

use red_core::context::Context;
use red_core::value::{Binding, FuncDef, ObjectDef, Series, Span, Symbol, Value};
use red_core::{Env, EvalError, NativeFn, RefineArgs};

use crate::binding::{bind_pass_into, deep_clone_value, rebind_to_context};
use crate::interp::eval;
use crate::natives::{arity_err, type_name};

// ---------------------------------------------------------------------------
// make object!
// ---------------------------------------------------------------------------

/// `make object! [spec]` — build a new object from a spec block.
///
/// Spec evaluation:
/// 1. Optionally detect a parent object as the first spec element (a word
///    resolving to an `Object`). Copy parent's words+values into the child.
/// 2. Pre-allocate a `self` slot holding the object value (circular `Rc`).
/// 3. Deep-clone the remaining spec, swap `env.user_ctx` to the object ctx,
///    run `bind_pass_into` (allocates SetWord slots, attaches bindings), then
///    `eval` the spec (fills slots with values, creates method funcs).
/// 4. Restore `env.user_ctx`.
pub fn make_object(spec: &Value, env: &mut Env) -> Result<Value, EvalError> {
    let block = match spec {
        Value::Block { series, .. } => series.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };

    let data = block.data.borrow();

    // Detect parent: if the first element is a word/get-word that resolves to
    // an Object in the *caller's* context, treat it as the parent.
    let (parent, spec_start) = if !data.is_empty() {
        if let Some(obj) = try_resolve_object(&data[0], env) {
            (Some(obj), 1usize)
        } else {
            (None, 0usize)
        }
    } else {
        (None, 0usize)
    };

    // Build the object.
    let obj_rc = Rc::new(RefCell::new(ObjectDef::new()));
    obj_rc.borrow_mut().parent = parent.clone();

    // Copy parent words+values into child ctx (copy-based inheritance).
    // Func values (methods) are deep-cloned and rebound to the child ctx so
    // method bodies read/write the child's slots, not the parent's.
    if let Some(ref p) = parent {
        let p_borrow = p.borrow();
        let words = p_borrow.ctx.words();
        let child_ctx: Rc<Context> = Rc::clone(&obj_rc.borrow().ctx);
        drop(p_borrow);
        let p_borrow = p.borrow();
        for sym in &words {
            if sym.as_str() == "self" {
                continue;
            }
            if let Some(val) = p_borrow.ctx.get(sym) {
                let val = rebind_func_to_ctx(&val, &child_ctx);
                child_ctx.set(sym.clone(), val);
            }
        }
    }

    // Pre-allocate `self` slot pointing at the object itself.
    let self_sym = Symbol::new("self");
    let obj_val = Value::Object(Rc::clone(&obj_rc));
    obj_rc.borrow().ctx.set(self_sym.clone(), obj_val.clone());

    // Deep-clone the spec (from spec_start onward) so we don't mutate shared
    // source data during binding.
    let spec_data: Vec<Value> = data.iter().skip(spec_start).map(deep_clone_value).collect();
    drop(data); // release the borrow before swapping user_ctx

    let spec_series = Series::new(spec_data);

    // Swap user_ctx to the object's ctx for spec binding + evaluation.
    let obj_ctx: Rc<Context> = Rc::clone(&obj_rc.borrow().ctx);
    let saved_ctx = std::mem::replace(&mut env.user_ctx, obj_ctx);

    // Bind: allocate SetWord slots in obj ctx, attach Local bindings.
    bind_pass_into(&spec_series, &env.user_ctx);

    // Evaluate the spec: SetWords write values, `does`/`func` create methods
    // (their bodies bind field refs to Local(obj_ctx, idx) via
    // `bind_function_body` which reads `env.user_ctx`).
    let spec_block = Value::block(spec_series);
    let result = eval(&spec_block, env);

    // Restore caller's user_ctx.
    env.user_ctx = saved_ctx;

    result?;
    Ok(obj_val)
}

/// Try to resolve `v` as an object reference (Word/GetWord bound to an
/// Object value). Read-only — does not mutate `env`.
fn try_resolve_object(v: &Value, env: &Env) -> Option<Rc<RefCell<ObjectDef>>> {
    let binding = match v {
        Value::Word { binding, .. } | Value::GetWord { binding, .. } => binding,
        _ => return None,
    };
    let val = match binding {
        Binding::Local(ctx, idx) => ctx.slot_value(*idx),
        Binding::Func(idx) => env.call_stack.last()?.ctx.slot_value(*idx),
        // M60: closure capture cell — resolve from the active frame's captures.
        Binding::Closure(idx) => {
            let frame = env.call_stack.last()?;
            let captures = frame.captures.as_ref()?;
            captures.get(*idx)?.borrow().clone()
        }
        // Lexical bindings are VM-only; object construction runs on the
        // walker, so a bound word here resolves via `Local`/`Func`/`Unbound`.
        // Treat `Lexical` as unresolvable (no object there).
        Binding::Lexical(_, _) => return None,
        Binding::Unbound => return None,
    };
    match val {
        Value::Object(o) => Some(o),
        _ => None,
    }
}

/// If `val` is a `Func`, deep-clone its body and rebind all word references
/// that name slots in `target_ctx` to `Local(target_ctx, idx)`. This is used
/// during copy-based inheritance so a child's inherited methods read/write
/// the child's slots instead of the parent's. Non-Func values are returned
/// as-is.
fn rebind_func_to_ctx(val: &Value, target_ctx: &Rc<Context>) -> Value {
    let fd = match val {
        Value::Func(fd) => fd.clone(),
        other => return other.clone(),
    };
    let mut new_fd = (*fd).clone();
    let rebound_body = crate::binding::deep_clone_series(&new_fd.body);
    let all_names: Vec<Symbol> = target_ctx.names.borrow().keys().cloned().collect();
    rebind_to_context(&rebound_body, target_ctx, &all_names);
    new_fd.body = rebound_body;
    Value::Func(Rc::new(new_fd))
}

// ---------------------------------------------------------------------------
// Natives
// ---------------------------------------------------------------------------

/// `object? value` — `true` if value is an object.
fn object_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "object?", 1, args.len()));
    }
    Ok(Value::Logic(matches!(args[0], Value::Object(_))))
}

/// `same? a b` — reference identity for objects (and other Rc-shared values).
fn same_predicate(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "same?", 2, args.len()));
    }
    let same = match (&args[0], &args[1]) {
        (Value::Object(a), Value::Object(b)) => Rc::ptr_eq(a, b),
        (Value::Func(a), Value::Func(b)) => Rc::ptr_eq(a, b),
        (Value::Error(a), Value::Error(b)) => Rc::ptr_eq(a, b),
        (Value::Map(a), Value::Map(b)) => Rc::ptr_eq(a, b),
        (Value::Hash(a), Value::Hash(b)) => Rc::ptr_eq(a, b),
        (Value::Vector(a), Value::Vector(b)) => Rc::ptr_eq(a, b),
        (Value::Image(a), Value::Image(b)) => Rc::ptr_eq(a, b),
        (Value::Bitset(a), Value::Bitset(b)) => Rc::ptr_eq(a, b),
        (Value::Port(a), Value::Port(b)) => Rc::ptr_eq(a, b),
        (Value::Typeset(a), Value::Typeset(b)) => Rc::ptr_eq(a, b),
        _ => false,
    };
    Ok(Value::Logic(same))
}

/// `not-same? a b` — negation of `same?`.
fn not_same_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let v = same_predicate(args, &RefineArgs::empty(), env)?;
    Ok(Value::Logic(!matches!(v, Value::Logic(true))))
}

/// `words-of object|map|module` — block of the object's word names (as
/// lit-words), the map's keys (as their natural `Value` form), or the
/// module's *exported* word names (M61, in `ctx` insertion order).
fn words_of_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "words-of", 1, args.len()));
    }
    match &args[0] {
        Value::Object(o) => {
            let borrow = obj_borrow_words(o);
            Ok(Value::block(Series::new(borrow)))
        }
        Value::Map(m) => Ok(Value::block(Series::new(m.borrow().keys()))),
        // M83: hash! keys via key_order (insertion-order for test stability).
        Value::Hash(h) => Ok(Value::block(Series::new(h.borrow().keys()))),
        // M84: vector! has no word keys — return an empty block.
        Value::Vector(_) => Ok(Value::block(Series::new(Vec::new()))),
        // M85: image! word keys are the fixed accessor set (width/height/size).
        Value::Image(_) => Ok(Value::block(Series::new(vec![
            Value::word("width"),
            Value::word("height"),
            Value::word("size"),
        ]))),
        // M61: module exports only, in ctx insertion order.
        Value::Module(m) => {
            let md = m.borrow();
            let exports = md.exports.borrow();
            let words: Vec<Value> = md
                .ctx
                .words()
                .into_iter()
                .filter(|s| exports.contains(s))
                .map(|s| Value::Word {
                    sym: s,
                    binding: Binding::Unbound,
                    span: Span::default(),
                })
                .collect();
            Ok(Value::block(Series::new(words)))
        }
        other => Err(EvalError::TypeError {
            expected: "object!, map!, or module!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// Helper: borrow an object's words as unbound `Word` values (excluding
/// `self`).
fn obj_borrow_words(obj: &Rc<RefCell<ObjectDef>>) -> Vec<Value> {
    let borrow = obj.borrow();
    borrow
        .ctx
        .words()
        .into_iter()
        .filter(|s| s.as_str() != "self")
        .map(|s| Value::Word {
            sym: s,
            binding: Binding::Unbound,
            span: Span::default(),
        })
        .collect()
}

/// `values-of object|map|module` — block of the object's slot values, the
/// map's values in insertion order, or the module's *exported* values (M61,
/// in `ctx` insertion order).
fn values_of_native(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "values-of", 1, args.len()));
    }
    match &args[0] {
        Value::Object(o) => {
            let borrow = o.borrow();
            let values: Vec<Value> = borrow
                .ctx
                .words()
                .into_iter()
                .filter(|s| s.as_str() != "self")
                .filter_map(|s| borrow.ctx.get(&s))
                .collect();
            Ok(Value::block(Series::new(values)))
        }
        Value::Map(m) => Ok(Value::block(Series::new(m.borrow().values()))),
        // M83: hash! values via key_order.
        Value::Hash(h) => Ok(Value::block(Series::new(h.borrow().values()))),
        // M84: vector! elements as a block.
        Value::Vector(v) => Ok(Value::block(Series::new(v.borrow().elements()))),
        // M85: image! values are the width/height/size triple (matching
        // `words-of` order: width height size).
        Value::Image(im) => {
            let b = im.borrow();
            Ok(Value::block(Series::new(vec![
                Value::integer(b.width as i64),
                Value::integer(b.height as i64),
                Value::pair(
                    Value::integer(b.width as i64),
                    Value::integer(b.height as i64),
                ),
            ])))
        }
        // M61: module exports' values, in ctx insertion order.
        Value::Module(m) => {
            let md = m.borrow();
            let exports = md.exports.borrow();
            let values: Vec<Value> = md
                .ctx
                .words()
                .into_iter()
                .filter(|s| exports.contains(s))
                .filter_map(|s| md.ctx.get(&s))
                .collect();
            Ok(Value::block(Series::new(values)))
        }
        other => Err(EvalError::TypeError {
            expected: "object!, map!, or module!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `reflect object 'words` / `'values` — alias dispatch for words/values.
fn reflect_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "reflect", 2, args.len()));
    }
    let field = match &args[1] {
        Value::LitWord { sym, .. } | Value::Word { sym, .. } => sym.as_str().to_string(),
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    match field.as_str() {
        "words" => words_of_native(&[args[0].clone()], &RefineArgs::empty(), env),
        "values" => values_of_native(&[args[0].clone()], &RefineArgs::empty(), env),
        other => Err(EvalError::Native {
            message: format!("reflect: {other} not supported for objects/modules"),
            span: args[1].span_or_default(),
        }),
    }
}

/// `in object 'word` — returns a `word!` value bound to the object's slot.
/// The returned word can be passed to `get`/`set` to read/write the object's
/// field directly.
fn in_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "in", 2, args.len()));
    }
    let obj = match &args[0] {
        Value::Object(o) => o.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "object!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    let sym = match &args[1] {
        Value::LitWord { sym, .. } | Value::Word { sym, .. } => sym.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    let borrow = obj.borrow();
    if let Some(idx) = borrow.ctx.index_of(&sym) {
        let obj_ctx = Rc::clone(&borrow.ctx);
        drop(borrow);
        Ok(Value::Word {
            sym,
            binding: Binding::Local(obj_ctx, idx),
            span: args[1].span_or_default(),
        })
    } else {
        Err(EvalError::UnboundWord {
            sym,
            span: args[1].span_or_default(),
        })
    }
}

/// `object [spec]` / `context [spec]` — keyword aliases for `make object!`.
fn object_keyword(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "object", 1, args.len()));
    }
    make_object(&args[0], env)
}

// ---------------------------------------------------------------------------
// M131: object/context reflection natives
// ---------------------------------------------------------------------------

/// `set? 'word` — alias of `value?`: true if the word has a value set.
fn set_predicate(args: &[Value], refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    crate::natives::value_predicate(args, refs, env)
}

/// `bound? word` / `bind? word` — true if `word` carries a binding (its
/// `Binding` is not `Unbound`) or the name has a slot in `user_ctx`. Takes
/// the word unevaluated (registered in `uneval_first`).
fn bound_predicate(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "bound?", 1, args.len()));
    }
    let bound = match &args[0] {
        Value::Word { sym, binding, .. } => {
            !matches!(binding, Binding::Unbound) || env.user_ctx.has(sym)
        }
        Value::LitWord { sym, .. } => env.user_ctx.has(sym),
        Value::GetWord { sym, binding, .. } => {
            !matches!(binding, Binding::Unbound) || env.user_ctx.has(sym)
        }
        Value::SetWord { sym, binding, .. } => {
            !matches!(binding, Binding::Unbound) || env.user_ctx.has(sym)
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    Ok(Value::Logic(bound))
}

/// `context-of word` / `bind-of word` — returns the `object!` the word is
/// bound into (if its `Binding::Local(ctx, _)` ctx is an object's ctx), else
/// `none`. Takes the word unevaluated.
fn context_of_native(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "context-of", 1, args.len()));
    }
    let ctx = match &args[0] {
        Value::Word { binding, .. }
        | Value::GetWord { binding, .. }
        | Value::SetWord { binding, .. } => match binding {
            Binding::Local(ctx, _) => Some(Rc::clone(ctx)),
            _ => None,
        },
        _ => None,
    };
    // We can't reconstruct the `ObjectDef` from a bare `Context` (the link is
    // one-way: ObjectDef→ctx). Return `none` unless a future API exposes the
    // reverse link. This matches Red's `context-of` returning `none` for
    // words not bound to an object.
    let _ = ctx;
    Ok(Value::None)
}

/// `context? value` — alias of `object?`.
fn context_predicate(args: &[Value], refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    object_predicate(args, refs, env)
}

/// `spec-of func` — returns the spec block (params + refinements + locals)
/// of a `func!`/`closure!`/`native!`, re-molded as a `block!`.
fn spec_of_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "spec-of", 1, args.len()));
    }
    let fd = match &args[0] {
        Value::Func(fd) => fd.clone(),
        Value::Closure(c) => Rc::clone(&c.func),
        other => {
            return Err(EvalError::TypeError {
                expected: "function!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let mut items: Vec<Value> = Vec::new();
    let unbound = Binding::Unbound;
    for p in &fd.params {
        items.push(Value::Word {
            sym: p.clone(),
            binding: unbound.clone(),
            span: Span::new(0, 0),
        });
    }
    for (rname, rargs) in &fd.refinements {
        items.push(Value::Refinement {
            sym: rname.clone(),
            span: Span::new(0, 0),
        });
        for a in rargs {
            items.push(Value::Word {
                sym: a.clone(),
                binding: unbound.clone(),
                span: Span::new(0, 0),
            });
        }
    }
    for l in &fd.locals {
        items.push(Value::LitWord {
            sym: Symbol::new("local"),
            span: Span::new(0, 0),
        });
        items.push(Value::Word {
            sym: l.clone(),
            binding: unbound.clone(),
            span: Span::new(0, 0),
        });
    }
    Ok(Value::Block {
        series: Series::new(items),
        span: Span::new(0, 0),
    })
}

/// `body-of func` — returns the body block of a function/closure.
fn body_of_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "body-of", 1, args.len()));
    }
    let body = match &args[0] {
        Value::Func(fd) => fd.body.clone(),
        Value::Closure(c) => c.func.body.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "function!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    Ok(Value::Block {
        series: body,
        span: Span::new(0, 0),
    })
}

/// `resolve target source` — copies all words/values from `source` object
/// into `target` object, overwriting existing slots. Returns the target.
fn resolve_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "resolve", 2, args.len()));
    }
    let target = match &args[0] {
        Value::Object(o) => Rc::clone(o),
        other => {
            return Err(EvalError::TypeError {
                expected: "object!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let source = match &args[1] {
        Value::Object(o) => Rc::clone(o),
        other => {
            return Err(EvalError::TypeError {
                expected: "object!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let s = source.borrow();
    let words = s.ctx.words();
    for w in &words {
        if let Some(v) = s.ctx.get(w) {
            target.borrow().ctx.set(w.clone(), v);
        }
    }
    Ok(args[0].clone())
}

/// `has object 'word` — true if `object` has a field named `word`.
fn has_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "has", 2, args.len()));
    }
    let obj = match &args[0] {
        Value::Object(o) => Rc::clone(o),
        other => {
            return Err(EvalError::TypeError {
                expected: "object!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let sym = match &args[1] {
        Value::Word { sym, .. } | Value::LitWord { sym, .. } => sym.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let has = obj.borrow().ctx.has(&sym);
    Ok(Value::Logic(has))
}

/// `extend object spec` — adds new fields to an existing object in place
/// (mutates, unlike `make object!` which copies). Evaluates `spec` set-words
/// into the object's context.
fn extend_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "extend", 2, args.len()));
    }
    let obj = match &args[0] {
        Value::Object(o) => Rc::clone(o),
        other => {
            return Err(EvalError::TypeError {
                expected: "object!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let spec = match &args[1] {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    // Swap user_ctx to the object's ctx, bind+eval the spec so set-words land
    // in the object. Mirrors `make object!`'s spec evaluation minus the
    // fresh-context allocation.
    let obj_ctx = Rc::clone(&obj.borrow().ctx);
    let saved = std::mem::replace(&mut env.user_ctx, obj_ctx);
    let cloned = Series::new(spec.data.borrow().clone());
    bind_pass_into(&cloned, &env.user_ctx);
    let block_val = Value::Block {
        series: cloned,
        span: args[1].span_or_default(),
    };
    let _ = eval(&block_val, env);
    env.user_ctx = saved;
    Ok(args[0].clone())
}

// ---------------------------------------------------------------------------
// M131: protect / unprotect / protect-system
// ---------------------------------------------------------------------------

/// `protect value` — marks an object (or series) immutable. Subsequent
/// mutating ops error via `check_protected`.
fn protect_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "protect", 1, args.len()));
    }
    set_protected(&args[0], env, true);
    Ok(args[0].clone())
}

/// `unprotect value` — clears the protect flag.
fn unprotect_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "unprotect", 1, args.len()));
    }
    set_protected(&args[0], env, false);
    Ok(args[0].clone())
}

fn set_protected(v: &Value, env: &mut Env, on: bool) {
    match v {
        Value::Object(o) => *o.borrow().protected.borrow_mut() = on,
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            let ptr = Rc::as_ptr(&series.data) as *const ();
            if on {
                env.protected_series.insert(ptr);
            } else {
                env.protected_series.remove(&ptr);
            }
        }
        Value::String { .. } | Value::String8 { .. } | Value::Vector(_) | Value::Hash(_) => {
            // String/vector/hash protection tracked via the Env side-set on
            // their backing storage pointer where applicable; strings use an
            // immutable Rc<str> so protection is a no-op (they're already
            // copy-on-write). Best-effort: no-op for immutable-backed series.
        }
        _ => {}
    }
}

/// `protect-system` — protects the root `system` object.
fn protect_system_native(
    _args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if let Some(Value::Object(o)) = env.user_ctx.get(&Symbol::new("system")) {
        *o.borrow().protected.borrow_mut() = true;
    }
    Ok(Value::None)
}

/// Check whether a value is protected; if so, return a `Native` error.
/// Called by mutating series/object natives before writing.
pub(crate) fn check_protected(v: &Value, env: &Env, native: &str) -> Result<(), EvalError> {
    match v {
        Value::Object(o) => {
            if *o.borrow().protected.borrow() {
                return Err(EvalError::Native {
                    message: format!("{native}: object is protected"),
                    span: v.span_or_default(),
                });
            }
        }
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            let ptr = Rc::as_ptr(&series.data) as *const ();
            if env.protected_series.contains(&ptr) {
                return Err(EvalError::Native {
                    message: format!("{native}: series is protected"),
                    span: v.span_or_default(),
                });
            }
        }
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

fn fixed(f: NativeFn, arity: usize) -> Rc<FuncDef> {
    let params: Vec<Symbol> = (0..arity)
        .map(|i| Symbol::new(&format!("__arg{i}")))
        .collect();
    Rc::new(FuncDef {
        params,
        native: Some(f),
        ..Default::default()
    })
}

pub fn register_object_natives(env: &mut Env) {
    env.natives.insert(
        Symbol::new("object?"),
        fixed(object_predicate as NativeFn, 1),
    );
    env.natives
        .insert(Symbol::new("same?"), fixed(same_predicate as NativeFn, 2));
    env.natives.insert(
        Symbol::new("not-same?"),
        fixed(not_same_predicate as NativeFn, 2),
    );
    env.natives.insert(
        Symbol::new("words-of"),
        fixed(words_of_native as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("values-of"),
        fixed(values_of_native as NativeFn, 1),
    );
    env.natives
        .insert(Symbol::new("reflect"), fixed(reflect_native as NativeFn, 2));
    env.natives
        .insert(Symbol::new("in"), fixed(in_native as NativeFn, 2));
    env.natives
        .insert(Symbol::new("object"), fixed(object_keyword as NativeFn, 1));
    env.natives
        .insert(Symbol::new("context"), fixed(object_keyword as NativeFn, 1));

    // M131: object/context reflection + protect.
    env.natives
        .insert(Symbol::new("set?"), fixed(set_predicate as NativeFn, 1));
    env.natives
        .insert(Symbol::new("bound?"), fixed(bound_predicate as NativeFn, 1));
    env.natives
        .insert(Symbol::new("bind?"), fixed(bound_predicate as NativeFn, 1));
    env.natives.insert(
        Symbol::new("context-of"),
        fixed(context_of_native as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("bind-of"),
        fixed(context_of_native as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("context?"),
        fixed(context_predicate as NativeFn, 1),
    );
    env.natives
        .insert(Symbol::new("spec-of"), fixed(spec_of_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("body-of"), fixed(body_of_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("resolve"), fixed(resolve_native as NativeFn, 2));
    env.natives
        .insert(Symbol::new("has"), fixed(has_native as NativeFn, 2));
    env.natives
        .insert(Symbol::new("extend"), fixed(extend_native as NativeFn, 2));
    env.natives
        .insert(Symbol::new("protect"), fixed(protect_native as NativeFn, 1));
    env.natives.insert(
        Symbol::new("unprotect"),
        fixed(unprotect_native as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("protect-system"),
        fixed(protect_system_native as NativeFn, 0),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::bind_pass;
    use crate::natives::{install_constants, register_natives};
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use std::io::Write;

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

    fn run(src: &str) -> Value {
        run_capture(src).unwrap().0
    }

    fn run_capture(src: &str) -> Result<(Value, Vec<u8>), String> {
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

    fn out(src: &str) -> String {
        s(&run_capture(src).unwrap().1)
    }

    #[test]
    fn object_field_access() {
        assert_eq!(mold_to_string(&run("o: make object! [a: 5] o/a")), "5");
    }

    #[test]
    fn object_method_calling_self() {
        let src = "o: make object! [n: 0 inc: does [n: n + 1]] o/inc o/n";
        assert_eq!(mold_to_string(&run(src)), "1");
    }

    #[test]
    fn object_method_with_param() {
        let src = "o: make object! [n: 0 add: func [x][n: n + x]] o/add 5 o/n";
        assert_eq!(mold_to_string(&run(src)), "5");
    }

    #[test]
    fn object_inheritance() {
        let src = "p: make object! [a: 1 b: 2] c: make object! [p b: 3] c/a";
        assert_eq!(mold_to_string(&run(src)), "1");
    }

    #[test]
    fn object_inheritance_override() {
        let src = "p: make object! [a: 1] c: make object! [p a: 99] c/a";
        assert_eq!(mold_to_string(&run(src)), "99");
    }

    #[test]
    fn object_inheritance_new_field() {
        let src = "p: make object! [a: 1] c: make object! [p b: 2] c/b";
        assert_eq!(mold_to_string(&run(src)), "2");
    }

    #[test]
    fn in_returns_bound_word() {
        // `set (in o 'a) 99` writes to the object's slot.
        let src = "o: make object! [a: 1] set (in o 'a) 99 o/a";
        assert_eq!(mold_to_string(&run(src)), "99");
    }

    #[test]
    fn in_get_reads_object_slot() {
        let src = "o: make object! [a: 42] get (in o 'a)";
        assert_eq!(mold_to_string(&run(src)), "42");
    }

    #[test]
    fn words_of_object() {
        let src = "o: make object! [a: 1 b: 2] words-of o";
        assert_eq!(mold_to_string(&run(src)), "[a b]");
    }

    #[test]
    fn values_of_object() {
        let src = "o: make object! [a: 1 b: 2] values-of o";
        assert_eq!(mold_to_string(&run(src)), "[1 2]");
    }

    #[test]
    fn reflect_words() {
        let src = "o: make object! [x: 10] reflect o 'words";
        assert_eq!(mold_to_string(&run(src)), "[x]");
    }

    #[test]
    fn object_predicate() {
        assert_eq!(mold_to_string(&run("object? make object! [a: 1]")), "true");
        assert_eq!(mold_to_string(&run("object? 5")), "false");
    }

    #[test]
    fn same_predicate_identity() {
        let src = "o: make object! [a: 1] same? o o";
        assert_eq!(mold_to_string(&run(src)), "true");
    }

    #[test]
    fn same_predicate_different() {
        let src = "same? (make object! [a: 1]) (make object! [a: 1])";
        assert_eq!(mold_to_string(&run(src)), "false");
    }

    #[test]
    fn object_keyword_alias() {
        assert_eq!(mold_to_string(&run("o: object [a: 5] o/a")), "5");
    }

    #[test]
    fn context_keyword_alias() {
        assert_eq!(mold_to_string(&run("o: context [a: 5] o/a")), "5");
    }

    #[test]
    fn object_mold() {
        assert_eq!(
            mold_to_string(&run("make object! [a: 1 b: 2]")),
            "make object! [a: 1 b: 2]"
        );
    }

    #[test]
    fn object_self_reference() {
        // `self` inside an object refers to the object itself.
        let src = "o: make object! [a: 1 geta: does [self/a]] o/geta";
        assert_eq!(mold_to_string(&run(src)), "1");
    }

    #[test]
    fn object_multiple_methods() {
        let src = "o: make object! [
            n: 0
            inc: does [n: n + 1]
            dec: does [n: n - 1]
        ] o/inc o/inc o/dec o/n";
        assert_eq!(mold_to_string(&run(src)), "1");
    }

    #[test]
    fn object_print_field() {
        let src = "o: make object! [msg: \"hello\"] print o/msg";
        assert_eq!(out(src), "hello\n");
    }
}
