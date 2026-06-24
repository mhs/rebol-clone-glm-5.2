//! Path natives (M19): predicates and conversions for path-family values.
//!
//! Predicates: `path?`, `get-path?`, `lit-path?`.
//! Conversions: `to-path`, `to-get-path`, `to-lit-path`.
//!
//! `set-path?` is intentionally not provided as a separate predicate —
//! set-paths are assignment syntax, not first-class values that persist
//! beyond the assignment (matching Red's behavior where `set-path!` exists
//! as a type but is rarely tested for at runtime).

use std::rc::Rc;

use red_core::value::{FuncDef, Span, Symbol, Value};
use red_core::{Env, EvalError, NativeFn, RefineArgs};

use crate::natives::{arity_err, type_name};

// ---------------------------------------------------------------------------
// Predicates
// ---------------------------------------------------------------------------

fn path_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "path?", 1, args.len()));
    }
    Ok(Value::Logic(matches!(args[0], Value::Path { .. })))
}

fn get_path_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "get-path?", 1, args.len()));
    }
    Ok(Value::Logic(matches!(args[0], Value::GetPath { .. })))
}

fn lit_path_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "lit-path?", 1, args.len()));
    }
    Ok(Value::Logic(matches!(args[0], Value::LitPath { .. })))
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

/// Build a path-family value from a `Vec<Value>` of parts, prefixed with the
/// given variant's marker. Used by all three `to-*` conversions.
fn path_from_parts(parts: Vec<Value>, variant: PathVariant) -> Value {
    match variant {
        PathVariant::Path => Value::Path {
            parts,
            span: Span::new(0, 0),
        },
        PathVariant::GetPath => Value::GetPath {
            parts,
            span: Span::new(0, 0),
        },
        PathVariant::LitPath => Value::LitPath {
            parts,
            span: Span::new(0, 0),
        },
    }
}

#[derive(Clone, Copy)]
enum PathVariant {
    Path,
    GetPath,
    LitPath,
}

/// Core of `to-path`/`to-get-path`/`to-lit-path`: derive a path from a
/// block, word, string, or existing path-family value.
///
/// - `block!` → parts are the block's values (each should be word/integer/
///   paren; other types are included as-is and the evaluator will reject
///   them at walk time).
/// - `word!`/`set-word!`/`get-word!`/`lit-word!`/`refinement!` → single-part
///   path with the word as its only part (demoted to a plain `Word`).
/// - `string!` → lex+parse the string; if it yields a single path-family
///   value, reclassify it; if it yields multiple values, use them as parts.
/// - `path!`/`get-path!`/`lit-path!` → reclassify to the target variant,
///   keeping the same parts.
fn to_path_kind(args: &[Value], name: &str, variant: PathVariant) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, name, 1, args.len()));
    }
    let v = &args[0];
    let parts: Vec<Value> = match v {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.data.borrow().clone(),
        Value::Path { parts, .. }
        | Value::GetPath { parts, .. }
        | Value::LitPath { parts, .. }
        | Value::SetPath { parts, .. } => parts.clone(),
        Value::Word { sym, span, .. }
        | Value::SetWord { sym, span, .. }
        | Value::GetWord { sym, span, .. }
        | Value::LitWord { sym, span } => {
            vec![Value::Word {
                sym: sym.clone(),
                binding: red_core::value::Binding::Unbound,
                span: *span,
            }]
        }
        Value::Refinement { sym, span } => {
            vec![Value::Word {
                sym: sym.clone(),
                binding: red_core::value::Binding::Unbound,
                span: *span,
            }]
        }
        Value::String { s, span } => {
            let toks = red_core::lexer::lex(s).map_err(|e| EvalError::Native {
                message: e.to_string(),
                span: *span,
            })?;
            let body = red_core::parser::load(&toks).map_err(|e| EvalError::Native {
                message: e.to_string(),
                span: *span,
            })?;
            let data = body.data.borrow();
            if data.len() == 1 {
                if let Value::Path { parts, .. }
                | Value::GetPath { parts, .. }
                | Value::LitPath { parts, .. }
                | Value::SetPath { parts, .. } = &data[0]
                {
                    parts.clone()
                } else {
                    vec![data[0].clone()]
                }
            } else {
                data.clone()
            }
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "block!, word!, string!, or path!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    Ok(path_from_parts(parts, variant))
}

fn to_path(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    to_path_kind(args, "to-path", PathVariant::Path)
}

fn to_get_path(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    to_path_kind(args, "to-get-path", PathVariant::GetPath)
}

fn to_lit_path(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    to_path_kind(args, "to-lit-path", PathVariant::LitPath)
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

pub fn register_path_natives(env: &mut Env) {
    env.natives
        .insert(Symbol::new("path?"), fixed(path_q as NativeFn, 1));
    env.natives
        .insert(Symbol::new("get-path?"), fixed(get_path_q as NativeFn, 1));
    env.natives
        .insert(Symbol::new("lit-path?"), fixed(lit_path_q as NativeFn, 1));
    env.natives
        .insert(Symbol::new("to-path"), fixed(to_path as NativeFn, 1));
    env.natives.insert(
        Symbol::new("to-get-path"),
        fixed(to_get_path as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("to-lit-path"),
        fixed(to_lit_path as NativeFn, 1),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::bind_pass;
    use crate::interp::eval;
    use crate::natives::{install_constants, register_natives};
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use red_core::Context;
    use std::cell::RefCell;
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

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
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
        let val = eval(&block, &mut env).map_err(|e| e.to_string())?;
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    #[test]
    fn path_predicates() {
        // Use lit-paths / constructed paths so the evaluator doesn't try to
        // resolve the head word (which would fail as unbound).
        assert_eq!(mold_to_string(&val("path? to-path [foo bar]")), "true");
        assert_eq!(mold_to_string(&val("path? 5")), "false");
        assert_eq!(mold_to_string(&val("path? [1 2]")), "false");
        assert_eq!(
            mold_to_string(&val("get-path? to-get-path [foo bar]")),
            "true"
        );
        assert_eq!(mold_to_string(&val("get-path? to-path [foo bar]")), "false");
        assert_eq!(mold_to_string(&val("lit-path? 'foo/bar")), "true");
        assert_eq!(mold_to_string(&val("lit-path? to-path [foo bar]")), "false");
    }

    #[test]
    fn to_path_from_block() {
        assert_eq!(mold_to_string(&val("to-path [a b c]")), "a/b/c");
    }

    #[test]
    fn to_path_from_word() {
        assert_eq!(mold_to_string(&val("to-path 'foo")), "foo");
    }

    #[test]
    fn to_get_path_from_block() {
        assert_eq!(mold_to_string(&val("to-get-path [a b]")), ":a/b");
    }

    #[test]
    fn to_lit_path_from_block() {
        assert_eq!(mold_to_string(&val("to-lit-path [a b]")), "'a/b");
    }

    #[test]
    fn to_path_from_string() {
        assert_eq!(mold_to_string(&val("to-path \"a/b/c\"")), "a/b/c");
    }

    #[test]
    fn to_path_from_existing_path() {
        assert_eq!(mold_to_string(&val("to-get-path to-path [a b]")), ":a/b");
    }
}
