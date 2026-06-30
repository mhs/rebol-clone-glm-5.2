//! Modules (M61): `module`, `export`, `module?`, `make module!`.
//!
//! A module is a self-contained namespace (`ModuleDef.ctx`) with a set of
//! exported words (the public surface). The module body is evaluated with
//! `env.user_ctx` temporarily swapped to the module's ctx (mirroring
//! `make object!`), so SetWords inside the body allocate slots in the
//! module's context.
//!
//! Visibility: inside the module body all words are visible (private + public)
//! — `env.user_ctx` is the module's ctx, so bare word resolution finds them.
//! The `export` native adds a word to the module's `exports` set as a
//! side-effect; it doesn't restrict inner access. Outside the module,
//! `module/word` path resolution succeeds only for exported words (see
//! `interp_walker::select_module_path`).
//!
//! Named modules (`module 'name [...]`) are cached on `Env::modules` keyed by
//! name — a second `module 'name [different body]` returns the cached value
//! (the new body is ignored, matching Red's "module is a singleton by name").
//! Anonymous modules (`module [body]`) are not cached.
//!
//! `make module! [spec]` (the mold inverse) interprets `name:` and `exports:`
//! keyword pairs in the spec to pre-populate the module's name/exports, then
//! evaluates the remaining spec items as the body. This makes
//! `do load mold m` reconstruct a faithful (public-surface) module.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use red_core::value::{ModuleDef, Series, Symbol, Value};
use red_core::{Env, EvalError, NativeFn, RefineArgs};

use crate::binding::bind_pass_into;
use crate::interp::eval;
use crate::natives::{arity_err, type_name};

// ---------------------------------------------------------------------------
// module + export natives
// ---------------------------------------------------------------------------

/// Shared core: build a module value with the given `name` and pre-seeded
/// `exports`, by evaluating `body` in the module's context. Mirrors
/// `object::make_object` — swaps `env.user_ctx` to the module's ctx, runs
/// `bind_pass_into` + `eval`, restores. Pushes/pops `env.module_stack` so
/// the `export` native can find the current module during body eval.
///
/// The caller is responsible for `env.modules` caching (named modules).
fn build_module(
    name: Option<Symbol>,
    pre_exports: HashSet<Symbol>,
    body: Series,
    env: &mut Env,
) -> Result<Rc<RefCell<ModuleDef>>, EvalError> {
    let parent = Some(Rc::clone(&env.user_ctx));
    let mut md = ModuleDef::new();
    md.name = name;
    md.parent = parent;
    {
        let mut ex = md.exports.borrow_mut();
        for s in pre_exports {
            ex.insert(s);
        }
    }
    let module_rc = Rc::new(RefCell::new(md));
    let module_ctx: Rc<red_core::Context> = Rc::clone(&module_rc.borrow().ctx);

    // Swap user_ctx to the module's ctx, push module_stack so `export` finds
    // the current module. Restore both on the error and success paths.
    let saved_ctx = std::mem::replace(&mut env.user_ctx, module_ctx);
    env.module_stack.push(Rc::clone(&module_rc));

    bind_pass_into(&body, &env.user_ctx);
    let body_block = Value::block(body);
    let result = eval(&body_block, env);

    // Always restore user_ctx + pop module_stack, even on error.
    env.module_stack.pop();
    env.user_ctx = saved_ctx;

    result?;
    Ok(module_rc)
}

/// `module [body]` / `module 'name [body]` — build a module value.
///
/// Form 1 (`module [body]`): anonymous module; not cached.
/// Form 2 (`module 'name [body]`): named module; cached in `env.modules[name]`.
///   Re-evaluating `module 'name [different body]` returns the cached module
///   (the new body is ignored — matches Red's "module is a singleton by
///   name").
///
/// Registered as variadic so the collector gathers the 1 or 2 args; the
/// handler dispatches on `args.len()`. (Variadic collection is required
/// because the two forms have different arities and the native registry
/// keys a single name to one `FuncDef`. The variadic stop condition — next
/// native word / end of block — naturally terminates after `[body]` in the
/// common `m: module ... <next-statement>` shapes; the handler also
/// tolerates >2 args from over-collection when a non-native statement like
/// `m2: ...` immediately follows, by using `args[0]`/`args[1]` and ignoring
/// the trailing over-collected values, which were already evaluated for
/// their side effects.)
pub fn module_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    // Dispatch on the leading args. `args[0]` is either the body block
    // (anonymous: 1-arg form) or the name word (named: 2+-arg form).
    if args.is_empty() {
        return Err(arity_err(args, "module", 1, 0));
    }
    let (name, body_block) = match &args[0] {
        Value::Block { .. } => (None, &args[0]),
        Value::Word { .. }
        | Value::GetWord { .. }
        | Value::LitWord { .. }
        | Value::SetWord { .. } => {
            // Named form: args[0] is the name, args[1] is the body.
            let name = match &args[0] {
                Value::Word { sym, .. }
                | Value::GetWord { sym, .. }
                | Value::LitWord { sym, .. }
                | Value::SetWord { sym, .. } => sym.clone(),
                _ => unreachable!("checked above"),
            };
            let body = args.get(1).ok_or_else(|| arity_err(args, "module", 2, 1))?;
            (Some(name), body)
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "word! or block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };

    // Named-module cache check: return the cached value, body ignored.
    if let Some(name) = &name {
        if let Some(cached) = env.modules.get(name) {
            return Ok(Value::Module(Rc::clone(cached)));
        }
    }

    let body_series = match body_block {
        Value::Block { series, .. } => series.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };

    let module_rc = build_module(name.clone(), HashSet::new(), body_series, env)?;

    // Cache named modules for later `import`/re-eval.
    if let Some(name) = name {
        env.modules.insert(name, Rc::clone(&module_rc));
    }

    Ok(Value::Module(module_rc))
}

/// `export 'word` / `export [w1 w2 ...]` — mark words as public in the
/// current module's `exports` set. Only valid inside a `module` body
/// (uses `env.module_stack` to find the current module). Returns `none`.
pub fn export_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "export", 1, args.len()));
    }
    let module_rc = match env.current_module() {
        Some(m) => Rc::clone(m),
        None => {
            return Err(EvalError::Native {
                message: "export used outside module".into(),
                span: args
                    .first()
                    .map(|v| v.span_or_default())
                    .unwrap_or_default(),
            });
        }
    };
    let module_borrow = module_rc.borrow();
    let mut exports = module_borrow.exports.borrow_mut();
    match &args[0] {
        Value::Word { sym, .. }
        | Value::GetWord { sym, .. }
        | Value::LitWord { sym, .. }
        | Value::SetWord { sym, .. } => {
            exports.insert(sym.clone());
        }
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            for v in data.iter() {
                let sym = match v {
                    Value::Word { sym, .. }
                    | Value::GetWord { sym, .. }
                    | Value::LitWord { sym, .. }
                    | Value::SetWord { sym, .. } => sym.clone(),
                    other => {
                        return Err(EvalError::TypeError {
                            expected: "word! in export block",
                            found: type_name(other),
                            span: other.span_or_default(),
                        });
                    }
                };
                exports.insert(sym);
            }
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "word! or block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    }
    Ok(Value::None)
}

/// `module? value` — true iff `value` is a `module!`. (M61.)
pub fn module_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "module?", 1, 0));
    }
    Ok(Value::Logic(matches!(args[0], Value::Module(_))))
}

// ---------------------------------------------------------------------------
// make module!
// ---------------------------------------------------------------------------

/// `make module! [spec]` — the mold inverse. The spec may contain `name:`/
/// `exports:` keyword pairs (interpreted specially) plus ordinary
/// `word: value` slot assignments and `export 'word` calls. Builds a module
/// by extracting the keywords, then evaluating the remaining items as the
/// body.
///
/// Mold form: `make module! [name: foo exports: [a b] a: 1 b: 2]`.
/// `do load mold m` reconstructs a module with name `foo`, exports `{a,b}`,
/// and slots `a=1, b=2`.
pub fn make_module(spec: &Value, env: &mut Env) -> Result<Value, EvalError> {
    let series = match spec {
        Value::Block { series, .. } => series.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    let data = series.data.borrow();

    let mut name: Option<Symbol> = None;
    let mut pre_exports: HashSet<Symbol> = HashSet::new();
    let mut body_items: Vec<Value> = Vec::with_capacity(data.len());

    let mut i = 0;
    while i < data.len() {
        let cur = &data[i];
        // Detect `name: <word>` and `exports: <block-of-words>` keyword pairs
        // at the top level of the spec. Other items pass through to the body.
        if let Value::SetWord { sym, .. } = cur {
            if sym.as_str() == "name" && i + 1 < data.len() {
                match &data[i + 1] {
                    Value::Word { sym: w, .. }
                    | Value::GetWord { sym: w, .. }
                    | Value::LitWord { sym: w, .. }
                    | Value::SetWord { sym: w, .. } => {
                        name = Some(w.clone());
                        i += 2;
                        continue;
                    }
                    _ => {}
                }
            }
            if sym.as_str() == "exports" && i + 1 < data.len() {
                if let Value::Block { series: ex_s, .. } = &data[i + 1] {
                    let ex_data = ex_s.data.borrow();
                    let mut ok = true;
                    let mut syms: Vec<Symbol> = Vec::new();
                    for v in ex_data.iter() {
                        match v {
                            Value::Word { sym: w, .. }
                            | Value::GetWord { sym: w, .. }
                            | Value::LitWord { sym: w, .. }
                            | Value::SetWord { sym: w, .. } => syms.push(w.clone()),
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    drop(ex_data);
                    if ok {
                        for s in syms {
                            pre_exports.insert(s);
                        }
                        i += 2;
                        continue;
                    }
                }
            }
        }
        body_items.push(cur.clone());
        i += 1;
    }
    drop(data);

    let body_series = Series::new(body_items);
    let module_rc = build_module(name, pre_exports, body_series, env)?;
    Ok(Value::Module(module_rc))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

fn fixed(f: NativeFn, arity: usize) -> Rc<red_core::value::FuncDef> {
    let params: Vec<Symbol> = (0..arity)
        .map(|i| Symbol::new(&format!("__arg{i}")))
        .collect();
    Rc::new(red_core::value::FuncDef {
        params,
        native: Some(f),
        ..Default::default()
    })
}

/// Register the M61 module natives. Called from `natives::registry`.
pub fn register_module_natives(env: &mut Env) {
    // `module` is registered with arity 1 (the common anonymous form
    // `module [body]`). The named form `module 'name [body]` (2 args) is
    // handled by a variable-arity peek in the walker/compiler collectors:
    // when the next value is a Word-family (the name), the collector
    // gathers 2 args instead of 1. Both args are pushed as-is (the name
    // is a word-kind literal, not evaluated). See
    // `interp_walker::collect_call_args` and `vm::compiler::collect_args`.
    env.natives
        .insert(Symbol::new("module"), fixed(module_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("export"), fixed(export_native as NativeFn, 1));
    env.natives.insert(
        Symbol::new("module?"),
        fixed(module_predicate as NativeFn, 1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::bind_pass;
    use crate::natives::install_constants;
    use crate::EvalError;
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use red_core::{Context, Env, Error};
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

    fn out(src: &str) -> String {
        let bytes = run_capture_val(src).unwrap().1;
        String::from_utf8(bytes).unwrap()
    }

    // --- module returns Value::Module ---

    #[test]
    fn module_returns_module_value() {
        let v = val("module []");
        assert!(matches!(v, Value::Module(_)));
    }

    // --- module? predicate ---

    #[test]
    fn module_predicate_on_module() {
        assert_eq!(mold_to_string(&val("module? module []")), "true");
    }

    #[test]
    fn module_predicate_on_object() {
        assert_eq!(mold_to_string(&val("module? object []")), "false");
    }

    #[test]
    fn module_predicate_on_non_object() {
        assert_eq!(mold_to_string(&val("module? 5")), "false");
    }

    // --- export outside module errors ---

    #[test]
    fn export_outside_module_errors() {
        // The VM/walker enriches `Native` errors into `Raised` (M42); match
        // either form so the test is robust to the enrichment path.
        let err = run_err("export 'foo");
        match err {
            Error::Eval(EvalError::Native { message, .. }) => {
                assert!(message.contains("export used outside module"), "{message}");
            }
            Error::Eval(EvalError::Raised(ev)) => {
                assert!(
                    ev.message.contains("export used outside module"),
                    "{:?}",
                    ev.message
                );
            }
            other => panic!("expected Native/Raised error, got {other:?}"),
        }
    }

    // --- words-of returns exports only (insertion order) ---

    #[test]
    fn words_of_module_returns_exports_only() {
        assert_eq!(
            mold_to_string(&val("m: module [priv: 1 pub: 2 export 'pub] words-of m")),
            "[pub]"
        );
    }

    #[test]
    fn words_of_module_export_block() {
        assert_eq!(
            mold_to_string(&val("m: module [a: 1 b: 2 c: 3 export [a c]] words-of m")),
            "[a c]"
        );
    }

    // --- module/word path resolution ---

    #[test]
    fn module_word_resolves_export() {
        assert_eq!(
            mold_to_string(&val("m: module [a: 1 b: 2 export 'a] m/a")),
            "1"
        );
    }

    #[test]
    fn module_word_private_unbound_from_outside() {
        let err = run_err("m: module [priv: 42 pub: 2 export 'pub] print m/priv");
        match err {
            Error::Eval(EvalError::UnboundWord { sym, .. }) => {
                assert_eq!(sym.as_str(), "priv");
            }
            other => panic!("expected UnboundWord, got {other:?}"),
        }
    }

    // --- named modules cached by name ---

    #[test]
    fn module_named_cached() {
        // Second `module 'once` returns the cached module; the new body
        // (x: 999) is ignored. `x` is exported so `m2/x` is accessible from
        // outside (visibility rule: only exports are reachable from outside).
        assert_eq!(
            mold_to_string(&val(
                "m1: module 'once [x: 1 export 'x] m2: module 'once [x: 999] m2/x"
            )),
            "1"
        );
    }

    #[test]
    fn module_named_export_callable() {
        assert_eq!(
            mold_to_string(&val(
                "m: module 'utils [helper: func [n][n * 2] export 'helper] m/helper 5"
            )),
            "10"
        );
    }

    // --- module body sees its own private words ---

    #[test]
    fn module_body_sees_private_words() {
        assert_eq!(out("module [priv: 42 print priv]").trim(), "42");
    }

    // --- module_basic golden (print m/a + words-of m) ---

    #[test]
    fn module_basic() {
        // `print [m/a words-of m]` prints the literal block; print the two
        // values separately to get `1 [a]` (matches plan6's fixture intent).
        assert_eq!(
            out("m: module [a: 1 b: 2 export 'a] print m/a print words-of m").trim(),
            "1\n[a]"
        );
    }

    // --- mold round-trip ---

    #[test]
    fn module_mold_form() {
        let m = val("module 'foo [a: 1 b: 2 export [a b]]");
        assert_eq!(
            mold_to_string(&m),
            "make module! [name: foo exports: [a b] a: 1 b: 2]"
        );
    }

    #[test]
    fn module_mold_load_roundtrips() {
        // `load mold m` must parse without error (reparseable).
        let m = val("module [a: 1 export 'a]");
        let molded = mold_to_string(&m);
        let _ = red_core::parser::load(&red_core::lexer::lex(&molded).unwrap()).unwrap();
    }

    #[test]
    fn module_make_reconstructs() {
        // `make module! [name: foo exports: [a] a: 1]` reconstructs a module
        // with the named exports. (`do load mold m` would require `mold` as
        // a script-level native, which it isn't — `mold` is a red-core
        // printer function. The round-trip is exercised via `load mold m`
        // in the `module_mold_load_roundtrips` test above.)
        assert_eq!(
            mold_to_string(&val(
                "m: make module! [name: foo exports: [a] a: 1] words-of m"
            )),
            "[a]"
        );
    }
}
