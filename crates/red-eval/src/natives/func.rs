//! Function-creation natives: `func`, `does`, `function` (auto-locals),
//! `function?`, `return`, and the shared `extract_spec` helper used by the
//! VM compiler (`vm::lex`/`vm::compiler`/`vm::vm`) to parse function spec
//! blocks.

use std::rc::Rc;

use red_core::value::{FuncDef, Value};
use red_core::{Env, EvalError, RefineArgs, Symbol};

use super::{arity_err, expect_block, type_name};

// ---------------------------------------------------------------------------
// Functions: `function` (auto-locals) — M16
// ---------------------------------------------------------------------------

/// `function [spec] [body]` — like `func` but the spec block recognizes a
/// `<local>` marker: any words following it (until the next refinement or
/// end) are declared as explicit function-local words. They get slots even
/// if never assigned by a body `SetWord`, so the body can reference them
/// before assignment without an unbound-word error. Body SetWords also still
/// auto-local (same as `func`).
pub(crate) fn function_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "function", 2, args.len()));
    }
    let spec_block = expect_block(args, 0, "function")?;
    let body_block = expect_block(args, 1, "function")?;
    let spec = extract_spec(&spec_block)?;
    let body_series = match &body_block {
        Value::Block { series, .. } => series.clone(),
        _ => unreachable!("expect_block guarantees Block"),
    };
    let mut fd = FuncDef {
        params: spec.params,
        refinements: spec.refinements,
        locals: spec.locals,
        body: body_series,
        native: None,
        variadic: false,
        infix: false,
        ..Default::default()
    };
    crate::binding::bind_function_body(&mut fd, &env.user_ctx);
    Ok(Value::Func(Rc::new(fd)))
}

// ---------------------------------------------------------------------------
// Functions: func, does, make, function?, return (M9)
// ---------------------------------------------------------------------------

/// `func [spec] [body]` — create a user-defined function value. `spec` is a
/// block of word/lit-word parameter names; `body` is the body block. The body
/// is bound at creation time to a fresh function-local context (params +
/// body-local SetWords become `Binding::Func`), with outer user-context words
/// (recursion, globals) bound as `Binding::Local`. Returns `Value::Func`.
pub(crate) fn func_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
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
pub(crate) fn does_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
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

// `make <type> <spec>` and `to <type> <value>` live in `crate::convert`
// (M14). `function?` and `return` remain here.

/// Result of parsing a `func`/`does`/`function` spec block: positional
/// parameter names, declared refinements (each a name + its argument-word
/// names), and explicit `<local>` words (recognized by `function` — `func`
/// ignores them).
pub(crate) struct FuncSpec {
    pub params: Vec<Symbol>,
    pub refinements: Vec<(Symbol, Vec<Symbol>)>,
    pub locals: Vec<Symbol>,
}

/// Extract parameter symbols and refinements from a func spec block.
///
/// Spec grammar (POC subset):
///   spec := item*
///   item := word | lit-word | refinement | <local>
///   refinement := `/name` word*    — `/name` introduces a refinement; the
///                                     following words (until the next
///                                     refinement, `<local>`, or end) are
///                                     its argument words.
///   <local> := `<local>` word*     — the `<local>` marker (a Word whose
///                                     symbol is `<local>`) introduces
///                                     function-local words; following words
///                                     (until the next refinement, another
///                                     `<local>`, or end) are collected as
///                                     locals.
///
/// Words become positional params (in order) unless inside a refinement or
/// `<local>` section. Type annotations are skipped.
pub(crate) fn extract_spec(spec_block: &Value) -> Result<FuncSpec, EvalError> {
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
    let mut locals: Vec<Symbol> = Vec::new();
    // Section state: which collector following words go into.
    #[derive(Clone, Copy)]
    enum Section {
        Params,
        Refinement,
        Local,
    }
    let mut section = Section::Params;
    for v in data.iter() {
        match v {
            Value::Word { sym, .. } if sym.as_str() == "<local>" => {
                section = Section::Local;
            }
            Value::Refinement { sym, .. } => {
                refinements.push((sym.clone(), Vec::new()));
                section = Section::Refinement;
            }
            Value::Word { sym, .. } | Value::LitWord { sym, .. } => match section {
                Section::Params => params.push(sym.clone()),
                Section::Refinement => {
                    if let Some(last) = refinements.last_mut() {
                        last.1.push(sym.clone());
                    }
                }
                Section::Local => locals.push(sym.clone()),
            },
            _ => {
                // Skip type annotations / other markers.
            }
        }
    }
    Ok(FuncSpec {
        params,
        refinements,
        locals,
    })
}

/// `function? value` — `true` if value is a `function!`, else `false`.
pub(crate) fn function_predicate(
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
pub(crate) fn return_native(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    let v = args.first().cloned().unwrap_or(Value::None);
    Err(EvalError::Return(v))
}
