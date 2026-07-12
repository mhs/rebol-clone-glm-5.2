//! Typesets (M89): a set of type-word symbols used for runtime type-checking
//! of function arguments.
//!
//! A `typeset!` is built via `make typeset! <block-of-type-words>` (or
//! `to-typeset`). The block is a flat series of type words (`integer!`/`float!`
//! /`any-word!`/...); group words (`any-*`/`number!`/`series!`/`any-type!`)
//! are recognized and expand at check time. The `TypesetDef::accepts(v)`
//! runtime check is wired into the `func`/`function`/`closure` call path
//! (walker + VM) via `FuncDef.param_types: Vec<Option<Rc<TypesetDef>>>`.
//!
//! M176: a typeset may carry an optional `semantic: Option<Rc<SemanticTypeDef>>`
//! field. When `Some`, `accepts_with_env(v, env)` also runs the semantic
//! type's compiled parse rule after the base-type check passes. This lets
//! `[rgb!]` in a func spec validate the semantic constraint at call time.
//!
//! The typeset *algebra* (`union`/`intersect`/`complement` of typesets) is
//! deferred to v0.8 — v0.7 ships the value type, the predicate, the
//! constructors, and the typed-function-arg headline feature only.

use std::rc::Rc;

use red_core::value::{SemanticTypeDef, Span, Symbol, TypesetDef, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::natives::{arity_err, type_name};

// ---------------------------------------------------------------------------
// make typeset! / to-typeset
// ---------------------------------------------------------------------------

/// `make typeset! <spec>` — build a new `typeset!`.
///
/// Accepted spec forms:
/// - `block!` — a flat series of type words (`[integer! float!]`). Each entry
///   must be a `Word`/`LitWord` naming a known leaf type or group word
///   (`integer!`/`float!`/`any-word!`/`number!`/...). Unknown words raise a
///   `Native` error. M176: semantic type words (registered in
///   `env.semantic_types`) are also accepted and populate the `semantic`
///   field.
/// - a single `Word`/`LitWord` — a one-element typeset (e.g. `make typeset!
///   integer!`).
/// - a `typeset!` — shallow copy (new `Rc<TypesetDef>` with the same set).
pub fn make_typeset(spec: &Value, _env: &mut Env) -> Result<Value, EvalError> {
    match spec {
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            let mut syms: Vec<Symbol> = Vec::new();
            for v in data.iter().skip(series.index) {
                push_type_word(v, &mut syms)?;
            }
            Ok(Value::typeset(TypesetDef::new(syms)))
        }
        Value::Word { sym, .. } | Value::LitWord { sym, .. } => {
            validate_type_word(sym, spec)?;
            Ok(Value::typeset(TypesetDef::new([sym.clone()])))
        }
        Value::Typeset(t) => Ok(Value::typeset(TypesetDef::new(
            t.types.borrow().iter().cloned(),
        ))),
        other => Err(EvalError::TypeError {
            expected: "block!, word!, or typeset!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `to-typeset value` — convert to a `typeset!`. Same as `make typeset!`.
pub(crate) fn to_typeset(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let spec = args.first().ok_or_else(|| EvalError::Arity {
        native: Symbol::new("to-typeset"),
        expected: 1,
        got: 0,
        span: Span::default(),
    })?;
    make_typeset(spec, env)
}

/// Push the type-word named by `v` (a `Word`/`LitWord`) into `syms` after
/// validating it is a known leaf or group word. Errors on non-word elements
/// or unknown type words.
fn push_type_word(v: &Value, syms: &mut Vec<Symbol>) -> Result<(), EvalError> {
    let sym = match v {
        Value::Word { sym, .. } | Value::LitWord { sym, .. } => sym,
        other => {
            return Err(EvalError::Native {
                message: format!(
                    "make typeset!: expected type word, got {}",
                    type_name(other)
                ),
                span: other.span_or_default(),
            });
        }
    };
    validate_type_word(sym, v)?;
    syms.push(sym.clone());
    Ok(())
}

fn validate_type_word(sym: &Symbol, span_holder: &Value) -> Result<(), EvalError> {
    if TypesetDef::is_known_type_word(sym) {
        Ok(())
    } else {
        Err(EvalError::Native {
            message: format!("make typeset!: unknown type word {}", sym.as_str()),
            span: span_holder.span_or_default(),
        })
    }
}

// ---------------------------------------------------------------------------
// Natives
// ---------------------------------------------------------------------------

/// `typeset? value` — `true` if value is a `typeset!`.
fn typeset_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "typeset?", 1, 0));
    }
    Ok(Value::Logic(matches!(args[0], Value::Typeset(_))))
}

// ---------------------------------------------------------------------------
// Spec-block parsing (used by `func`/`function`/`closure` to populate
// `FuncDef.param_types` from a `[integer! float!]` annotation block)
// ---------------------------------------------------------------------------

/// Parse a typeset annotation block (e.g. `[integer! float!]`) into a
/// `Rc<TypesetDef>`. Used by `extract_spec` in `natives/func.rs` when a param
/// is followed by a `Block` of type words. Returns an error for an empty
/// block or one containing unknown/non-word entries.
///
/// M176: if a word in the block is NOT a known builtin type word but IS a
/// registered semantic type (in `env.semantic_types`), the typeset's
/// `semantic` field is set to the semantic type def, and the base type word
/// is added to the `types` set. A typeset may contain at most one semantic
/// type word (mixing `[rgb! integer!]` is an error). `env` is `Option`:
/// `None` for compile-time paths (which only need arity, not semantic
/// checking); `Some` for runtime func creation.
pub(crate) fn parse_typeset_block(
    block: &Value,
    env: Option<&Env>,
) -> Result<Rc<TypesetDef>, EvalError> {
    let series = match block {
        Value::Block { series, .. } => series.clone(),
        other => {
            return Err(EvalError::Native {
                message: format!("func: type spec must be a block, got {}", type_name(other)),
                span: other.span_or_default(),
            });
        }
    };
    let data = series.data.borrow();
    let mut syms: Vec<Symbol> = Vec::new();
    let mut semantic: Option<Rc<SemanticTypeDef>> = None;
    for v in data.iter().skip(series.index) {
        let sym = match v {
            Value::Word { sym, .. } | Value::LitWord { sym, .. } => sym,
            other => {
                return Err(EvalError::Native {
                    message: format!(
                        "make typeset!: expected type word, got {}",
                        type_name(other)
                    ),
                    span: other.span_or_default(),
                });
            }
        };
        // M176: check semantic types FIRST (a registered semantic type
        // `port!` takes precedence over the builtin `port!` type word).
        let mut found_semantic = false;
        if let Some(env) = env {
            if let Some(def) = env.lookup_semantic_type(sym) {
                if semantic.is_some() || syms.len() > 1 {
                    return Err(EvalError::Native {
                        message: format!(
                            "semantic type {} cannot be mixed with other type words in a typeset",
                            sym.as_str()
                        ),
                        span: v.span_or_default(),
                    });
                }
                syms.push(def.base.clone());
                semantic = Some(def);
                found_semantic = true;
            }
        }
        if !found_semantic {
            if TypesetDef::is_known_type_word(sym) {
                if semantic.is_some() {
                    return Err(EvalError::Native {
                        message: "semantic type cannot be mixed with other type words in a typeset".into(),
                        span: v.span_or_default(),
                    });
                }
                syms.push(sym.clone());
            } else {
                return Err(EvalError::Native {
                    message: format!("make typeset!: unknown type word {}", sym.as_str()),
                    span: v.span_or_default(),
                });
            }
        }
    }
    if syms.is_empty() {
        return Err(EvalError::Native {
            message: "func: type spec block is empty".into(),
            span: block.span_or_default(),
        });
    }
    let ts = TypesetDef::new(syms);
    *ts.semantic.borrow_mut() = semantic;
    Ok(Rc::new(ts))
}

/// Format the expected-typeset portion of a TypeError-style message. Used by
/// the walker/VM call paths when `param_types[i].accepts(arg)` is false.
pub(crate) fn typeset_label(ts: &TypesetDef) -> String {
    let words = ts.sorted_words();
    let mut s = String::from("[");
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            s.push_str(" | ");
        }
        s.push_str(w.as_str());
    }
    s.push(']');
    s
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

pub fn register_typeset_natives(env: &mut Env) {
    use red_core::value::FuncDef;
    use std::rc::Rc as StdRc;

    let reg = |env: &mut Env, name: &str, f: NF, arity: usize| {
        let params: Vec<Symbol> = (0..arity)
            .map(|i| Symbol::new(&format!("__arg{i}")))
            .collect();
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

    reg(env, "typeset?", typeset_predicate as NF, 1);
    reg(env, "to-typeset", to_typeset as NF, 1);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::bind_pass;
    use crate::natives::{install_constants, register_natives};
    use red_core::context::Context;
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use std::cell::RefCell;
    use std::io::Write;

    struct BufferWriter(Rc<RefCell<Vec<u8>>>);

    impl Write for BufferWriter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
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
        register_natives(&mut env);
        let block = Value::block(body);
        let val = crate::interp::eval(&block, &mut env).map_err(|e| e.to_string())?;
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    fn err_src(src: &str) -> String {
        match run_capture_val(src) {
            Ok(_) => "<no error>".into(),
            Err(e) => e,
        }
    }

    #[test]
    fn make_typeset_molds_back() {
        assert_eq!(
            mold_to_string(&val("make typeset! [integer! float!]")),
            "make typeset! [float! integer!]"
        );
    }

    #[test]
    fn make_typeset_single_word() {
        // Single lit-word form (`'integer!` self-evaluates; a bare `integer!`
        // would be an unbound word).
        assert_eq!(
            mold_to_string(&val("make typeset! 'integer!")),
            "make typeset! [integer!]"
        );
    }

    #[test]
    fn make_typeset_empty() {
        assert_eq!(mold_to_string(&val("make typeset! []")), "make typeset! []");
    }

    #[test]
    fn typeset_predicate() {
        assert_eq!(
            mold_to_string(&val("typeset? make typeset! [integer!]")),
            "true"
        );
        assert_eq!(mold_to_string(&val("typeset? 5")), "false");
    }

    #[test]
    fn typeset_group_word_any_function() {
        // `any-function!` covers `function!`/`native!`/`op!`/`closure!`.
        // Verified via the func typed-arg path: a func with `[x [any-function!]]`
        // accepts `:print` (a native) as its argument. `:x` (GetWord) fetches
        // the value without invoking it (walker auto-invokes a bare word
        // bound to a function; the VM does not — a pre-existing parity gap
        // we avoid here).
        let v = val("f: func [x [any-function!]][type? :x] f :print");
        assert_eq!(mold_to_string(&v), "native!");
    }

    #[test]
    fn typeset_group_word_number() {
        // `number!` covers `integer!`/`float!`/`percent!` (red-core broad).
        let v = val("f: func [x [number!]][x + 1] f 5");
        assert_eq!(mold_to_string(&v), "6");
        let v2 = val("f: func [x [number!]][x + 1] f 5.0");
        assert_eq!(mold_to_string(&v2), "6.0");
    }

    #[test]
    fn typeset_group_word_any_word() {
        // `any-word!` covers word!/set-word!/get-word!/lit-word!/refinement!.
        let v = val("f: func [x [any-word!]][type? x] f 'foo");
        assert_eq!(mold_to_string(&v), "lit-word!");
    }

    #[test]
    fn typeset_group_word_any_type() {
        // `any-type!` matches everything — verify via a func that accepts any.
        let v = val("f: func [x [any-type!]][type? x] f 5");
        assert_eq!(mold_to_string(&v), "integer!");
        let v2 = val("f: func [x [any-type!]][type? x] f \"hi\"");
        assert_eq!(mold_to_string(&v2), "string!");
        let v3 = val("f: func [x [any-type!]][type? x] f none");
        assert_eq!(mold_to_string(&v3), "none!");
    }

    #[test]
    fn func_with_typed_arg_accepts_int() {
        let v = val("f: func [x [integer!]][x + 1] f 3");
        assert_eq!(mold_to_string(&v), "4");
    }

    #[test]
    fn func_with_typed_arg_accepts_float_too() {
        let v = val("f: func [x [integer! float!]][x + 1] f 3.5");
        assert_eq!(mold_to_string(&v), "4.5");
    }

    #[test]
    fn func_with_typed_arg_rejects_string() {
        let e = err_src("f: func [x [integer!]][x + 1] f \"hi\"");
        assert!(
            e.contains("type error") && e.contains("integer!"),
            "got: {e}"
        );
    }

    #[test]
    fn func_untyped_arg_backcompat() {
        // Pre-M89 funcs (no type spec) accept any type — regression guard.
        let v = val("f: func [x][x] f \"hi\"");
        // mold of a string! adds quotes.
        assert_eq!(mold_to_string(&v), "\"hi\"");
    }

    #[test]
    fn function_native_with_typed_arg() {
        let v = val("f: function [x [integer!]][x + 1] f 10");
        assert_eq!(mold_to_string(&v), "11");
    }

    #[test]
    fn closure_native_with_typed_arg() {
        let v = val("f: closure [x [integer!]][x + 1] f 10");
        assert_eq!(mold_to_string(&v), "11");
    }

    #[test]
    fn make_typeset_unknown_type_errors() {
        let e = err_src("make typeset! [bogus!]");
        assert!(e.contains("unknown type word"), "got: {e}");
    }

    #[test]
    fn typeset_to_typeset_identity() {
        let v = val("t: make typeset! [integer!] to-typeset t");
        assert_eq!(mold_to_string(&v), "make typeset! [integer!]");
    }

    // ---- M176: func-spec with semantic types ----

    #[test]
    fn func_spec_with_semantic_type_rgb() {
        let src = "define-type 'rgb! 'tuple! [r: byte g: byte b: byte] ";
        let v = val(&format!(
            "{}{}",
            src, "f: func [c [rgb!]] [c] f 255.0.0"
        ));
        assert_eq!(mold_to_string(&v), "255.0.0");
    }

    #[test]
    fn func_spec_with_semantic_type_rejects_wrong_arity() {
        let src = "define-type 'rgb! 'tuple! [r: byte g: byte b: byte] ";
        let e = err_src(&format!(
            "{}{}",
            src, "f: func [c [rgb!]] [c] f 192.168.1.10"
        ));
        assert!(e.contains("type error"), "got: {e}");
    }

    #[test]
    fn func_spec_with_semantic_type_rejects_wrong_base() {
        let src = "define-type 'rgb! 'tuple! [r: byte g: byte b: byte] ";
        let e = err_src(&format!(
            "{}{}",
            src, "f: func [c [rgb!]] [c] f \"red\""
        ));
        assert!(e.contains("type error"), "got: {e}");
    }

    #[test]
    fn func_spec_with_semantic_type_port() {
        let src = "define-type 'port! 'integer! [range 1 65535] ";
        let v = val(&format!(
            "{}{}",
            src, "f: func [p [port!]] [p] f 443"
        ));
        assert_eq!(mold_to_string(&v), "443");
        let e = err_src(&format!(
            "{}{}",
            src, "f: func [p [port!]] [p] f 99999"
        ));
        assert!(e.contains("type error"), "got: {e}");
    }

    #[test]
    fn func_spec_with_semantic_type_slug() {
        let src = "define-type 'slug! 'string! [some slug-char] ";
        let v = val(&format!(
            "{}{}",
            src, "f: func [s [slug!]] [s] f \"user-42\""
        ));
        assert_eq!(mold_to_string(&v), "\"user-42\"");
        let e = err_src(&format!(
            "{}{}",
            src, "f: func [s [slug!]] [s] f \"bad slug\""
        ));
        assert!(e.contains("type error"), "got: {e}");
    }

    #[test]
    fn func_spec_multiple_semantic_params() {
        let src = concat!(
            "define-type 'ipv4! 'tuple! [a: byte b: byte c: byte d: byte] ",
            "define-type 'port! 'integer! [range 1 65535] ",
        );
        let v = val(&format!(
            "{}{}",
            src, "f: func [addr [ipv4!] p [port!]] [addr] f 192.168.1.10 443"
        ));
        assert_eq!(mold_to_string(&v), "192.168.1.10");
    }

    #[test]
    fn func_spec_backcompat_no_semantic() {
        // Existing funcs with builtin type specs still work (no semantic ref).
        let v = val("f: func [x [integer!]] [x + 1] f 5");
        assert_eq!(mold_to_string(&v), "6");
    }
}
