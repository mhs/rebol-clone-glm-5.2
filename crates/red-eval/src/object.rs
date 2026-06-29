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

/// `words-of object|map` — block of the object's word names (as lit-words),
/// or the map's keys (as their natural `Value` form).
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
        other => Err(EvalError::TypeError {
            expected: "object! or map!",
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

/// `values-of object|map` — block of the object's slot values, or the map's
/// values in insertion order.
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
        other => Err(EvalError::TypeError {
            expected: "object! or map!",
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
            message: format!("reflect: {other} not supported for objects"),
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
        assert_eq!(out(src), "\"hello\"\n");
    }
}
