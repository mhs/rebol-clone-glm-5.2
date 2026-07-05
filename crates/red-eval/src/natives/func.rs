//! Function-creation natives: `func`, `does`, `function` (auto-locals),
//! `function?`, `return`, and the shared `extract_spec` helper used by the
//! VM compiler (`vm::lex`/`vm::compiler`/`vm::vm`) to parse function spec
//! blocks.

use std::cell::RefCell;
use std::rc::Rc;

use red_core::value::{Binding, FuncDef, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

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
        param_types: spec.param_types,
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
        param_types: spec.param_types,
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
/// names), explicit `<local>` words (recognized by `function` — `func`
/// ignores them), and per-param runtime typesets (M89 — `None` = unchecked).
pub(crate) struct FuncSpec {
    pub params: Vec<Symbol>,
    pub refinements: Vec<(Symbol, Vec<Symbol>)>,
    pub locals: Vec<Symbol>,
    /// M89: parallel to `params`. `param_types[i] = Some(ts)` iff the spec
    /// block has a `[type! ...]` annotation block immediately following
    /// `params[i]`. Refinement-arg types are not checked in v0.7 (entries
    /// for refinement args stay `None` — the vec is sized only for params).
    pub param_types: Vec<Option<Rc<red_core::value::TypesetDef>>>,
}

/// Extract parameter symbols and refinements from a func spec block.
///
/// Spec grammar (POC subset):
///   spec := item*
///   item := word | lit-word | refinement | <local> | type-annotation
///   refinement := `/name` word*     — `/name` introduces a refinement; the
///                                       following words (until the next
///                                       refinement, `<local>`, or end) are
///                                       its argument words.
///   <local> := `<local>` word*      — the `<local>` marker (a Word whose
///                                       symbol is `<local>`) introduces
///                                       function-local words; following words
///                                       (until the next refinement, another
///                                       `<local>`, or end) are collected as
///                                       locals.
///   type-annotation := block        — a `[type! ...]` block immediately
///                                       following a positional param word
///                                       (M89) becomes the param's runtime
///                                       typeset. Skipped in refinement/local
///                                       sections (refinement-arg types
///                                       deferred to v0.8).
///
/// Words become positional params (in order) unless inside a refinement or
/// `<local>` section.
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
    let mut param_types: Vec<Option<Rc<red_core::value::TypesetDef>>> = Vec::new();
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
            // M81: `<local>` now lexes as a `tag!` (`Tag("local")`). Accept
            // both the new `Tag` form and the legacy `Word("<local>")` form
            // (the latter is unreachable from source post-M81 but kept for
            // synthetic/programmatic spec blocks).
            Value::Tag { text, .. } if text.as_ref() == "local" => {
                section = Section::Local;
            }
            Value::Word { sym, .. } if sym.as_str() == "<local>" => {
                section = Section::Local;
            }
            Value::Refinement { sym, .. } => {
                refinements.push((sym.clone(), Vec::new()));
                section = Section::Refinement;
            }
            Value::Word { sym, .. } | Value::LitWord { sym, .. } => match section {
                Section::Params => {
                    params.push(sym.clone());
                    param_types.push(None);
                }
                Section::Refinement => {
                    if let Some(last) = refinements.last_mut() {
                        last.1.push(sym.clone());
                    }
                }
                Section::Local => locals.push(sym.clone()),
            },
            // M89: a `block!` immediately following a param word in the
            // Params section is the param's runtime typeset annotation
            // (`[integer! float!]`). Refinement/local-section blocks stay
            // skipped (refinement-arg types deferred to v0.8).
            Value::Block { .. } if matches!(section, Section::Params) => {
                if let Some(last) = param_types.last_mut() {
                    *last = Some(crate::typeset::parse_typeset_block(v)?);
                }
            }
            _ => {
                // Skip type annotations / other markers in other sections.
            }
        }
    }
    Ok(FuncSpec {
        params,
        refinements,
        locals,
        param_types,
    })
}

/// `function? value` — `true` if value is a `function!` or `closure!`, else
/// `false`. M60: a closure is a function (subset relation: `closure?` →
/// `function?` but not vice versa). M87: the broad umbrella — true on
/// `native!`/`op!`/`function!`/`closure!` alike (kept back-compat so existing
/// fixtures like `function? :square` keep passing).
pub(crate) fn function_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "function?", 1, 0));
    }
    Ok(Value::Logic(matches!(
        args[0],
        Value::Func(_) | Value::Closure(_)
    )))
}

/// `native? value` — `true` iff `value` is a built-in (`Value::Func` with
/// `native.is_some()`) that is NOT an infix operator. Red parity: `op?` and
/// `native?` are disjoint — an infix native like `+` is an `op!`, not a
/// `native!`. Returns `false` on closures and user-defined funcs.
pub(crate) fn native_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "native?", 1, 0));
    }
    Ok(Value::Logic(matches!(
        &args[0],
        Value::Func(fd) if fd.native.is_some() && !fd.infix
    )))
}

/// `op? value` — `true` iff `value` is an infix operator (`Value::Func` with
/// `fd.infix == true`). Returns `false` on closures and non-infix natives.
pub(crate) fn op_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "op?", 1, 0));
    }
    Ok(Value::Logic(
        matches!(&args[0], Value::Func(fd) if fd.infix),
    ))
}

/// `any-function? value` — `true` iff `value` is any function-kind value
/// (`function!`/`native!`/`op!`/`closure!`). M87 open-q #2 decision: add the
/// umbrella predicate for completeness; mirrors `function?` (the existing
/// broad predicate) but named to match Red's `any-function?`.
pub(crate) fn any_function_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "any-function?", 1, 0));
    }
    Ok(Value::Logic(matches!(
        args[0],
        Value::Func(_) | Value::Closure(_)
    )))
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

// ---------------------------------------------------------------------------
// Closures (M60)
// ---------------------------------------------------------------------------

/// `closure [spec] [body]` — like `func` but captures freevar values into a
/// `ClosureDef.captures` cell at creation time (snapshot semantics). Returns
/// `Value::Closure`. The walker path: the VM path uses `Instr::MakeClosure`.
///
/// Capture rules (walker):
/// - `Binding::Local(user_ctx, idx)` words (globals): capture their current
///   value, EXCEPT words whose current value is `none` (heuristic: the
///   closure's own name is `none` at creation time because the SetWord hasn't
///   fired yet — skip it for late-binding recursion).
/// - `Binding::Unbound` words: walk `env.call_stack` (ancestor func frames).
///   If found, capture the value from that frame's ctx. If not found, leave
///   unbound (will error at call time if truly unbound).
/// - `Binding::Func(idx)` words: function-local, don't capture.
pub(crate) fn closure_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "closure", 2, args.len()));
    }
    let spec_block = expect_block(args, 0, "closure")?;
    let body_block = expect_block(args, 1, "closure")?;
    let spec = extract_spec(&spec_block)?;
    // Deep-clone the body so each `closure_native` invocation starts from
    // pristine source bindings. Without this, the first call mutates the
    // shared source-tree body (via `bind_function_body` +
    // `set_closure_bindings`), leaving `Binding::Closure(idx)` on the
    // freevar words. The second call's capture scanner then skips them
    // (`_ => {}` arm in `try_capture_word`), producing an empty captures
    // cell while the body still references index 0 → panic. Mirrors the
    // pattern used by `dispatch_block` (interp_walker.rs:143) and
    // `ensure_compiled` (vm.rs:1480).
    let body_series = match &body_block {
        Value::Block { series, .. } => crate::binding::deep_clone_series(series),
        _ => unreachable!("expect_block guarantees Block"),
    };
    let mut fd = FuncDef {
        params: spec.params,
        refinements: spec.refinements,
        param_types: spec.param_types,
        body: body_series,
        native: None,
        variadic: false,
        infix: false,
        ..Default::default()
    };
    // Bind params/locals (same as `func_native`).
    crate::binding::bind_function_body(&mut fd, &env.user_ctx);

    // Scan the body for words to capture. Build a map: name → capture index.
    let mut captures_map: std::collections::HashMap<Symbol, usize> =
        std::collections::HashMap::new();
    let mut capture_vals: Vec<Value> = Vec::new();
    scan_closure_captures(&fd.body, env, &mut captures_map, &mut capture_vals);

    // Record freevar names on the FuncDef for introspection.
    fd.freevars = captures_map.keys().cloned().collect();

    // Overlay Binding::Closure(idx) on matching body words.
    crate::binding::set_closure_bindings(&fd.body, &captures_map);

    // Build the captures cell (RefCell per value for interior mutability).
    let captures: Vec<RefCell<Value>> = capture_vals.into_iter().map(RefCell::new).collect();
    Ok(Value::closure(Rc::new(fd), Rc::new(captures)))
}

/// Walk a closure body and collect words to capture. For each candidate:
/// - `Binding::Local(user_ctx, idx)` with a non-`none` current value → capture.
/// - `Binding::Unbound` that resolves in `env.call_stack` → capture.
///
/// Populates `captures_map` (name → index) and `capture_vals` (parallel Vec).
fn scan_closure_captures(
    series: &red_core::value::Series,
    env: &Env,
    captures_map: &mut std::collections::HashMap<Symbol, usize>,
    capture_vals: &mut Vec<Value>,
) {
    let data = series.data.borrow();
    scan_closure_captures_inner(&data, env, captures_map, capture_vals);
}

fn scan_closure_captures_inner(
    data: &[Value],
    env: &Env,
    captures_map: &mut std::collections::HashMap<Symbol, usize>,
    capture_vals: &mut Vec<Value>,
) {
    let mut i = 0;
    while i < data.len() {
        // Skip nested func/does/function/closure bodies — their words belong
        // to their own scopes.
        if crate::binding::use_body_index(data, i).is_some() {
            i += 3;
            continue;
        }
        if crate::binding::func_form_skip(data, i).is_some() {
            i += crate::binding::func_form_skip(data, i).unwrap();
            continue;
        }
        match &data[i] {
            Value::Block { series, .. } | Value::Paren { series, .. } => {
                let child = series.clone();
                let child_data = child.data.borrow();
                scan_closure_captures_inner(&child_data, env, captures_map, capture_vals);
            }
            Value::Word { sym, binding, .. }
            | Value::SetWord { sym, binding, .. }
            | Value::GetWord { sym, binding, .. } => {
                try_capture_word(sym, binding, env, captures_map, capture_vals);
            }
            Value::Path { parts, .. }
            | Value::GetPath { parts, .. }
            | Value::LitPath { parts, .. }
            | Value::SetPath { parts, .. } => {
                if let Some(Value::Word { sym, binding, .. })
                | Some(Value::GetWord { sym, binding, .. }) = parts.first()
                {
                    try_capture_word(sym, binding, env, captures_map, capture_vals);
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// Check if `sym`/`binding` should be captured. If so, record it in
/// `captures_map` + `capture_vals`.
fn try_capture_word(
    sym: &Symbol,
    binding: &Binding,
    env: &Env,
    captures_map: &mut std::collections::HashMap<Symbol, usize>,
    capture_vals: &mut Vec<Value>,
) {
    // Don't double-capture a word that's already in the map.
    if captures_map.contains_key(sym) {
        return;
    }
    match binding {
        Binding::Local(ctx, idx) => {
            // Global (user_ctx) reference. Capture its current value, UNLESS
            // it's `none` — the closure's own name is `none` at creation time
            // (the SetWord hasn't fired), and we want late-binding recursion.
            let val = ctx.slot_value(*idx);
            if !matches!(val, Value::None) {
                let idx_cap = capture_vals.len();
                captures_map.insert(sym.clone(), idx_cap);
                capture_vals.push(val);
            }
        }
        Binding::Unbound | Binding::Closure(_) => {
            // `Binding::Unbound`: the word wasn't bound by `bind_function_body`
            // (not a param/local and not in `user_ctx`). Check ancestor func
            // frames (the walker's call stack) — if found, capture its value.
            //
            // `Binding::Closure(idx)` (M60 bug fix): the VM's lexical analyzer
            // (`vm/lex.rs`) may have already set `Binding::Closure(idx)` on
            // closure-body freevars during `ensure_compiled`. When the body
            // then falls back to the walker (`needs_rebind` →
            // `invoke_via_walker` → `closure_native`), the deep-cloned body
            // PRESERVES these analyzer bindings. Without this arm,
            // `try_capture_word` would skip them, producing an empty captures
            // cell while the body still references the index → panic at call
            // time. Treating `Binding::Closure` the same as `Unbound` here
            // re-scans the enclosing scope and populates the cell correctly.
            // The subsequent `set_closure_bindings` call overwrites the
            // analyzer's stale index with the correct one.
            for frame in env.call_stack.iter().rev() {
                if let Some(slot) = frame.ctx.index_of(sym) {
                    let val = frame.ctx.slot_value(slot);
                    let idx_cap = capture_vals.len();
                    captures_map.insert(sym.clone(), idx_cap);
                    capture_vals.push(val);
                    return;
                }
            }
            // Not found in any frame — truly unbound, don't capture.
        }
        // `Binding::Func` (function-local param/local — the closure's own
        // params, not freevars) and `Binding::Lexical` (VM-only lexical
        // addressing, not used in the walker path) — don't capture.
        Binding::Func(_) | Binding::Lexical(_, _) => {}
    }
}

/// `closure? value` — `true` if value is a `closure!`, else `false`.
pub(crate) fn closure_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "closure?", 1, 0));
    }
    Ok(Value::Logic(matches!(args[0], Value::Closure(_))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::bind_pass;
    use crate::natives::install_constants;
    use crate::{interp::eval, EvalError};
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use red_core::{Context, Env, Error};
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

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

    fn run_capture_val(src: &str) -> Result<(Value, Vec<u8>), String> {
        let body = load_source(src).map_err(|e| e.to_string())?;
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        crate::natives::register_natives(&mut env);
        let block = Value::block(body);
        let val = eval(&block, &mut env).map_err(|e| e.to_string())?;
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    fn run_err(src: &str) -> Error {
        let body = load_source(src).expect("parse failed");
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        crate::natives::register_natives(&mut env);
        let block = Value::block(body);
        let err = eval(&block, &mut env).expect_err("expected error");
        Error::Eval(err)
    }

    // --- closure? predicate ---

    #[test]
    fn closure_predicate_on_closure() {
        assert_eq!(mold_to_string(&val("closure? closure [] []")), "true");
    }

    #[test]
    fn closure_predicate_on_func() {
        assert_eq!(mold_to_string(&val("closure? func [] []")), "false");
    }

    #[test]
    fn closure_predicate_on_non_func() {
        assert_eq!(mold_to_string(&val("closure? 5")), "false");
    }

    // --- function? extension ---

    #[test]
    fn function_predicate_on_closure() {
        // M60: a closure is a function.
        assert_eq!(mold_to_string(&val("function? closure [] []")), "true");
    }

    // --- closure basic capture ---

    #[test]
    fn closure_captures_value_at_creation() {
        // Snapshot: `y: 10` is captured; later `y: 99` doesn't affect the
        // closure.
        assert_eq!(
            mold_to_string(&val("y: 10 f: closure [x][x + y] y: 99 f 5")),
            "15"
        );
    }

    // --- closure escape (v0.3 bug regression) ---

    #[test]
    fn closure_escapes_defining_frame() {
        let src = "make-adder: func [n][closure [x][x + n]] add5: make-adder 5 add5 10";
        assert_eq!(mold_to_string(&val(src)), "15");
    }

    // --- two closures from the same factory (Bug 1 regression) ---

    #[test]
    fn two_closures_from_same_factory() {
        // Without deep-cloning the body, the second `make-adder` call sees
        // stale `Binding::Closure(0)` from the first call, skips capture,
        // and produces an empty captures cell → panic on invocation.
        let src = "make-adder: func [n][closure [x][x + n]] add5: make-adder 5 add10: make-adder 10 add5 100 add10 100";
        assert_eq!(mold_to_string(&val(src)), "110");
    }

    // --- closure internal mutation ---

    #[test]
    fn closure_internal_mutation_persists() {
        // The RefCell cell allows interior mutability across invocations
        // of the same closure.
        let src = "counter: 0 inc: closure [][counter: counter + 1 counter] inc inc";
        assert_eq!(mold_to_string(&val(src)), "2");
    }

    // --- closure recursive ---

    #[test]
    fn closure_recursive_via_outer_slot() {
        let src = "fact: closure [n][either n <= 1 [1][n * fact n - 1]] fact 5";
        assert_eq!(mold_to_string(&val(src)), "120");
    }

    // --- closure in object ---

    #[test]
    fn closure_in_object_captures_field() {
        let src = "o: object [base: 100 adder: closure [x][x + base]] o/adder 5";
        assert_eq!(mold_to_string(&val(src)), "105");
    }

    // --- closure unbound capture errors ---

    #[test]
    fn closure_unbound_word_errors() {
        let err = run_err("c: closure [x][x + undefined-word] c 5");
        assert!(
            matches!(err, Error::Eval(EvalError::UnboundWord { .. })),
            "expected UnboundWord, got {err:?}"
        );
    }

    // --- closure returns Value::Closure ---

    #[test]
    fn closure_returns_closure_value() {
        let v = val("closure [] []");
        assert!(matches!(v, Value::Closure(_)));
    }

    // --- mold ---

    #[test]
    fn closure_molds_as_placeholder() {
        assert_eq!(mold_to_string(&val("closure [] []")), "#[closure]");
    }

    // --- Bug 3: control-flow inside closure bodies (capture propagation) ---

    #[test]
    fn closure_with_do_inside_body() {
        let src = "count: 0 c: closure [] [do [count: count + 1] count] c c c";
        assert_eq!(mold_to_string(&val(src)), "3");
    }

    #[test]
    fn closure_with_if_inside_body() {
        let src = "count: 0 c: closure [] [if true [count: count + 1] count] c c c";
        assert_eq!(mold_to_string(&val(src)), "3");
    }

    #[test]
    fn closure_method_with_if_in_module() {
        // Module method dispatch routes through the walker's
        // `call_closure_func`, which invokes `eval` on the body; the `if`
        // native then calls `dispatch_block` → fresh `vm::run`. Without
        // capture propagation, `LoadCapture` fails.
        let src = "m: module 'm [count: 0 bump: closure [] [if true [count: count + 1] count] export 'bump] m/bump m/bump m/bump";
        assert_eq!(mold_to_string(&val(src)), "3");
    }
}
