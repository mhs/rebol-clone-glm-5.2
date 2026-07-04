//! Vectors (M84): a numeric series with a typed element kind
//! (`integer!`/`float!`/`i8!`/`i16!`/`i32!`/`i64!`/`f32!`/`f64!`).
//!
//! The first "container with a typed payload" type. Stored as
//! `Vec<Value>` of `Integer`/`Float`; the `kind` field drives narrow-on-write
//! (clamp ints, round floats) and `vec/integer` path access (returns the
//! kind word).
//!
//! Vectors are created via `make vector! <spec>` (or `to-vector`). The spec
//! may be:
//! - a `block!` with an explicit kind lead: `[integer! 1 2 3]`,
//!   `[float! 1.0 2.0]`, `[i8! 1 2 3]`. The first element is a word/lit-word
//!   naming the kind; the rest are numeric values (evaluated in the caller's
//!   context — `make vector! [integer! 1 + 2]` stores `3`).
//! - a `block!` of bare numerics: `[1 2 3]` → infer `integer!`; `[1.0 2.0]`
//!   → `float!`; `[1 2.0 3]` → `float!` (ints promoted to f64).
//! - a `block!` of the form `[integer! N]` where N is a single integer and
//!   `integer!` is a kind word → N-element zero vector of that kind.
//! - an `integer!` count → N-element zero vector (default `integer!`).
//! - a `vector!` → identity (clone the contents into a fresh `VectorDef`).
//!
//! Path resolution (`vec/1` → first element, `vec/integer` → the kind word,
//! `vec/1: 99` → poke) is in `interp_walker.rs`; series natives
//! (`length?`/`pick`/`poke`/`first`/`last`/`append`/`insert`/`change`/
//! `remove`/`take`/`clear`/`copy`/`select`/`find`/`foreach` and the cursor
//! ops `next`/`back`/`at`/`skip`/`head`/`tail`/`index?`) are wired in
//! `series.rs`; equality is in `natives/compare.rs`; `same?`/`not-same?`/
//! `values-of` are in `object.rs`; arithmetic (`+ - * /` componentwise +
//! scalar broadcast) is in `math.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use red_core::value::{infer_vector_kind, Span, Symbol, Value, VectorDef};
use red_core::{Env, EvalError, RefineArgs};

use crate::interp_walker::eval_expression;
use crate::natives::{arity_err, type_name};

// ---------------------------------------------------------------------------
// make vector! / to-vector
// ---------------------------------------------------------------------------

/// `make vector! <spec>` — build a new `vector!`. See module docs for spec
/// forms.
pub fn make_vector(spec: &Value, env: &mut Env) -> Result<Value, EvalError> {
    match spec {
        Value::Block { series, span } => {
            let data = series.data.borrow();
            build_from_block(&data, series.index, *span, env)
        }
        Value::Integer { n, .. } => {
            // N-element zero vector, default integer!.
            let len = (*n).max(0) as usize;
            let kind = Symbol::new("integer!");
            Ok(Value::vector(VectorDef::new(
                kind,
                vec![Value::integer(0); len],
            )))
        }
        Value::Float { f, .. } => {
            // N-element zero vector, float! (counts given as floats still
            // work — `make vector! 3.0`).
            let len = (*f).max(0.0) as usize;
            let kind = Symbol::new("float!");
            Ok(Value::vector(VectorDef::new(
                kind,
                vec![Value::float(0.0); len],
            )))
        }
        Value::Vector(v) => {
            // Shallow copy: new VectorDef with cloned kind + elems + reset cursor.
            let b = v.borrow();
            Ok(Value::vector(VectorDef::new(b.kind(), b.elements())))
        }
        other => Err(EvalError::TypeError {
            expected: "block!, integer!, or vector!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

fn build_from_block(
    data: &std::cell::Ref<Vec<Value>>,
    start: usize,
    span: Span,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if data.len() == start {
        // Empty block — default to integer! kind, no elements.
        return Ok(Value::vector(VectorDef::empty(Symbol::new("integer!"))));
    }
    let head = &data[start];
    // Try the explicit-kind lead: `[integer! 1 2 3]` / `[i8! ...]` /
    // `[float! 1.0 2.0]`. The head may be a `Word` or `LitWord` naming a kind.
    let kind_sym = match head {
        Value::Word { sym, .. } | Value::LitWord { sym, .. } => VectorDef::kind_word(sym.as_str()),
        _ => None,
    };
    if let Some(kind) = kind_sym {
        let rest = &data[start + 1..];
        // Special case: `[integer! N]` (single integer after the kind lead)
        // → N-element zero vector of that kind.
        if rest.len() == 1 {
            if let Value::Integer { n, .. } = &rest[0] {
                let len = (*n).max(0) as usize;
                let zero = if kind.as_str().contains("float") {
                    Value::float(0.0)
                } else {
                    Value::integer(0)
                };
                return Ok(Value::vector(VectorDef::new(kind, vec![zero; len])));
            }
        }
        // General: evaluate the rest as numeric expressions, narrow to `kind`.
        let mut elems = Vec::with_capacity(rest.len());
        let mut i = start + 1;
        while i < data.len() {
            let v = eval_expression(data, &mut i, env)?;
            if !is_numeric(&v) {
                return Err(EvalError::Native {
                    message: format!(
                        "make vector!: element type {} is not numeric",
                        type_name(&v)
                    ),
                    span: v.span_or_default(),
                });
            }
            // Build a temp VectorDef so we can use its `narrow`.
            let tmp = VectorDef::new(kind.clone(), Vec::new());
            elems.push(tmp.narrow(&v));
        }
        return Ok(Value::vector(VectorDef::new(kind, elems)));
    }
    // No explicit kind — infer from the elements. Evaluate each, then
    // `infer_vector_kind` decides `integer!` vs `float!` and promotes.
    let mut evaluated: Vec<Value> = Vec::with_capacity(data.len() - start);
    let mut i = start;
    while i < data.len() {
        let v = eval_expression(data, &mut i, env)?;
        if !is_numeric(&v) {
            return Err(EvalError::Native {
                message: format!(
                    "make vector!: element type {} is not numeric",
                    type_name(&v)
                ),
                span: v.span_or_default(),
            });
        }
        evaluated.push(v);
    }
    let (kind, narrowed) =
        infer_vector_kind(&evaluated).map_err(|m| EvalError::Native { message: m, span })?;
    Ok(Value::vector(VectorDef::new(kind, narrowed)))
}

fn is_numeric(v: &Value) -> bool {
    matches!(
        v,
        Value::Integer { .. } | Value::Float { .. } | Value::Percent { .. }
    )
}

/// `to-vector value` — convert to a `vector!`. Same shape as `make vector!`
/// minus the identity/copy-on-vector case (a `vector!` arg still copies).
pub(crate) fn to_vector(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let spec = args.first().ok_or_else(|| EvalError::Arity {
        native: Symbol::new("to-vector"),
        expected: 1,
        got: 0,
        span: Span::default(),
    })?;
    make_vector(spec, env)
}

// ---------------------------------------------------------------------------
// Natives
// ---------------------------------------------------------------------------

/// `vector? value` — `true` if value is a `vector!`.
fn vector_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "vector?", 1, 0));
    }
    Ok(Value::Logic(matches!(args[0], Value::Vector(_))))
}

// ---------------------------------------------------------------------------
// Shared helpers used by path resolution + series natives + arithmetic
// ---------------------------------------------------------------------------

/// `pick`-style 1-based element access (negative counts from tail;
/// out-of-range returns `None`). Ignores the cursor.
#[allow(dead_code)] // wired through `VectorDef::pick` directly in series.rs
pub(crate) fn select_vector_element(v: &Rc<RefCell<VectorDef>>, idx: i64) -> Option<Value> {
    v.borrow().pick(idx)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

pub fn register_vector_natives(env: &mut Env) {
    use red_core::value::FuncDef;

    let reg = |env: &mut Env, name: &str, f: NF, arity: usize| {
        let params: Vec<Symbol> = (0..arity)
            .map(|i| Symbol::new(&format!("__arg{i}")))
            .collect();
        env.natives.insert(
            Symbol::new(name),
            Rc::new(FuncDef {
                params,
                native: Some(f),
                variadic: false,
                infix: false,
                ..Default::default()
            }),
        );
    };

    reg(env, "vector?", vector_predicate as NF, 1);
    reg(env, "to-vector", to_vector as NF, 1);
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
        run_capture_val(src).unwrap_err()
    }

    #[test]
    fn make_vector_explicit_kind_molds_back() {
        assert_eq!(
            mold_to_string(&val("make vector! [integer! 1 2 3]")),
            "make vector! [integer! 1 2 3]"
        );
        assert_eq!(
            mold_to_string(&val("make vector! [float! 1.0 2.0 3.0]")),
            "make vector! [float! 1.0 2.0 3.0]"
        );
    }

    #[test]
    fn make_vector_infer_integer() {
        assert_eq!(
            mold_to_string(&val("make vector! [1 2 3]")),
            "make vector! [integer! 1 2 3]"
        );
    }

    #[test]
    fn make_vector_infer_float() {
        assert_eq!(
            mold_to_string(&val("make vector! [1.0 2.0]")),
            "make vector! [float! 1.0 2.0]"
        );
    }

    #[test]
    fn make_vector_kind_promote_mixed() {
        // 1 2.0 3 → all promoted to float!.
        assert_eq!(
            mold_to_string(&val("make vector! [1 2.0 3]")),
            "make vector! [float! 1.0 2.0 3.0]"
        );
    }

    #[test]
    fn make_vector_from_count_int() {
        assert_eq!(
            mold_to_string(&val("make vector! 3")),
            "make vector! [integer! 0 0 0]"
        );
    }

    #[test]
    fn make_vector_from_count_block_form() {
        // `[integer! 3]` → 3-element zero vector.
        assert_eq!(
            mold_to_string(&val("make vector! [integer! 3]")),
            "make vector! [integer! 0 0 0]"
        );
        assert_eq!(
            mold_to_string(&val("make vector! [float! 2]")),
            "make vector! [float! 0.0 0.0]"
        );
    }

    #[test]
    fn make_vector_empty() {
        assert_eq!(
            mold_to_string(&val("make vector! []")),
            "make vector! [integer!]"
        );
    }

    #[test]
    fn make_vector_eval_elems() {
        // `make vector! [integer! 1 + 2]` → stores 3 (expression evaluated).
        assert_eq!(
            mold_to_string(&val("make vector! [integer! 1 + 2]")),
            "make vector! [integer! 3]"
        );
    }

    #[test]
    fn vector_predicate_true_false() {
        assert_eq!(mold_to_string(&val("vector? make vector! []")), "true");
        assert_eq!(mold_to_string(&val("vector? [1 2 3]")), "false");
        assert_eq!(mold_to_string(&val("vector? 5")), "false");
    }

    #[test]
    fn to_vector_from_block() {
        assert_eq!(
            mold_to_string(&val("to-vector [1 2 3]")),
            "make vector! [integer! 1 2 3]"
        );
    }

    #[test]
    fn vector_is_a_series() {
        assert_eq!(mold_to_string(&val("series? make vector! []")), "true");
    }

    #[test]
    fn vector_length() {
        assert_eq!(
            mold_to_string(&val("length? make vector! [integer! 1 2 3]")),
            "3"
        );
        assert_eq!(mold_to_string(&val("length? make vector! []")), "0");
    }

    #[test]
    fn vector_pick_1_based() {
        assert_eq!(
            mold_to_string(&val("pick (make vector! [integer! 10 20 30]) 1")),
            "10"
        );
        assert_eq!(
            mold_to_string(&val("pick (make vector! [integer! 10 20 30]) 2")),
            "20"
        );
        assert_eq!(
            mold_to_string(&val("pick (make vector! [integer! 10 20 30]) -1")),
            "30"
        );
    }

    #[test]
    fn vector_poke_round_trip() {
        assert_eq!(
            mold_to_string(&val(
                "v: make vector! [integer! 1 2 3] poke v 2 99 pick v 2"
            )),
            "99"
        );
    }

    #[test]
    fn vector_poke_narrows_float_to_int() {
        // Poke 2.5 into an integer! vector → narrows to 2.
        assert_eq!(
            mold_to_string(&val(
                "v: make vector! [integer! 1 2 3] poke v 1 2.5 pick v 1"
            )),
            "2"
        );
    }

    #[test]
    fn vector_poke_rejects_non_numeric() {
        let msg = err_src("v: make vector! [integer! 1 2 3] poke v 1 \"x\"");
        assert!(
            msg.contains("vector") || msg.contains("numeric") || msg.contains("poke"),
            "got: {msg}"
        );
    }

    #[test]
    fn vector_first_second_last() {
        assert_eq!(
            mold_to_string(&val("first make vector! [integer! 10 20 30]")),
            "10"
        );
        assert_eq!(
            mold_to_string(&val("second make vector! [integer! 10 20 30]")),
            "20"
        );
        assert_eq!(
            mold_to_string(&val("last make vector! [integer! 10 20 30]")),
            "30"
        );
    }

    #[test]
    fn vector_append_narrows() {
        assert_eq!(
            mold_to_string(&val("v: make vector! [integer! 1 2] append v 3 v")),
            "make vector! [integer! 1 2 3]"
        );
    }

    #[test]
    fn vector_append_rejects_string() {
        let msg = err_src("v: make vector! [integer! 1 2] append v \"x\"");
        assert!(
            msg.contains("vector") || msg.contains("numeric"),
            "got: {msg}"
        );
    }

    #[test]
    fn vector_insert_at_index() {
        // `insert` inserts at the cursor (default 0). On a fresh vector
        // (cursor 0), inserts at the head. (Cursor ops on a vector! return
        // a positioned Block view — documented deviation.)
        assert_eq!(
            mold_to_string(&val("v: make vector! [integer! 1 3] insert v 2 v")),
            "make vector! [integer! 2 1 3]"
        );
    }

    #[test]
    fn vector_remove() {
        // `remove` removes at the cursor. On a fresh vector (cursor 0),
        // removes element 1.
        assert_eq!(
            mold_to_string(&val("v: make vector! [integer! 1 2 3] remove v v")),
            "make vector! [integer! 2 3]"
        );
    }

    #[test]
    fn vector_take_returns_removed() {
        // `take` removes and returns the value at the cursor. On a fresh
        // vector (cursor 0), takes element 1. (Cursor ops on a vector!
        // return a positioned Block view — documented deviation — so we
        // test take at cursor 0 directly.)
        assert_eq!(
            mold_to_string(&val("v: make vector! [integer! 1 2 3] r: take v r")),
            "1"
        );
        // The vector now lacks that element (take mutates shared storage
        // when operating on the vector directly).
        assert_eq!(
            mold_to_string(&val("v: make vector! [integer! 1 2 3] take v v")),
            "make vector! [integer! 2 3]"
        );
    }

    #[test]
    fn vector_clear() {
        assert_eq!(
            mold_to_string(&val("v: make vector! [integer! 1 2 3] clear v v")),
            "make vector! [integer!]"
        );
    }

    #[test]
    fn vector_copy_is_shallow() {
        assert_eq!(
            mold_to_string(&val("copy make vector! [integer! 1 2 3]")),
            "make vector! [integer! 1 2 3]"
        );
    }

    #[test]
    fn vector_select_by_value() {
        // Red's `select` returns the value *after* the match.
        assert_eq!(
            mold_to_string(&val("select make vector! [integer! 10 20 30] 20")),
            "30"
        );
    }

    #[test]
    fn vector_find_position() {
        // `find` returns the matched value (the positioned series at the
        // match; the value at the cursor is the match).
        assert_eq!(
            mold_to_string(&val("first find make vector! [integer! 10 20 30] 20")),
            "20"
        );
    }

    #[test]
    fn vector_foreach() {
        let src = "v: make vector! [integer! 1 2 3] total: 0 foreach x v [total: total + x] total";
        assert_eq!(mold_to_string(&val(src)), "6");
    }

    #[test]
    fn vector_path_kind_word() {
        // `vec/integer` returns the kind word `'integer!` (as a LitWord,
        // which molds as `integer!`).
        assert_eq!(
            mold_to_string(&val("v: make vector! [integer! 1 2 3] v/integer")),
            "integer!"
        );
        assert_eq!(
            mold_to_string(&val("v: make vector! [float! 1.0] v/float")),
            "float!"
        );
    }

    #[test]
    fn vector_path_index_pick() {
        assert_eq!(
            mold_to_string(&val("v: make vector! [integer! 10 20 30] v/2")),
            "20"
        );
    }

    #[test]
    fn vector_path_set_poke() {
        assert_eq!(
            mold_to_string(&val("v: make vector! [integer! 1 2 3] v/2: 99 v")),
            "make vector! [integer! 1 99 3]"
        );
    }

    #[test]
    fn vector_equality_deep() {
        assert_eq!(
            mold_to_string(&val(
                "(make vector! [integer! 1 2 3]) = (make vector! [integer! 1 2 3])"
            )),
            "true"
        );
        assert_eq!(
            mold_to_string(&val(
                "(make vector! [integer! 1 2 3]) = (make vector! [integer! 1 2 4])"
            )),
            "false"
        );
        assert_eq!(
            mold_to_string(&val(
                "(make vector! [integer! 1 2 3]) = (make vector! [float! 1.0 2.0 3.0])"
            )),
            "false"
        );
    }

    #[test]
    fn vector_same_identity() {
        assert_eq!(
            mold_to_string(&val("v: make vector! [integer! 1 2 3] same? v v")),
            "true"
        );
        assert_eq!(
            mold_to_string(&val(
                "same? (make vector! [integer! 1]) (make vector! [integer! 1])"
            )),
            "false"
        );
    }

    #[test]
    fn vector_arith_componentwise() {
        assert_eq!(
            mold_to_string(&val(
                "(make vector! [integer! 1 2 3]) + (make vector! [integer! 4 5 6])"
            )),
            "make vector! [integer! 5 7 9]"
        );
        assert_eq!(
            mold_to_string(&val(
                "(make vector! [integer! 5 5 5]) - (make vector! [integer! 1 2 3])"
            )),
            "make vector! [integer! 4 3 2]"
        );
    }

    #[test]
    fn vector_arith_broadcast_scalar() {
        assert_eq!(
            mold_to_string(&val("(make vector! [integer! 1 2 3]) + 5")),
            "make vector! [integer! 6 7 8]"
        );
        assert_eq!(
            mold_to_string(&val("5 - (make vector! [integer! 1 2 3])")),
            "make vector! [integer! 4 3 2]"
        );
        assert_eq!(
            mold_to_string(&val("(make vector! [integer! 1 2 3]) * 2")),
            "make vector! [integer! 2 4 6]"
        );
        assert_eq!(
            mold_to_string(&val("(make vector! [integer! 4 6 8]) / 2")),
            "make vector! [float! 2.0 3.0 4.0]"
        );
    }

    #[test]
    fn vector_arith_length_mismatch_errors() {
        let msg = err_src("(make vector! [integer! 1 2 3]) + (make vector! [integer! 1 2])");
        assert!(
            msg.contains("length") || msg.contains("vector"),
            "got: {msg}"
        );
    }

    #[test]
    fn vector_values_of() {
        assert_eq!(
            mold_to_string(&val("values-of make vector! [integer! 1 2 3]")),
            "[1 2 3]"
        );
    }

    #[test]
    fn vector_to_vector_copies() {
        assert_eq!(
            mold_to_string(&val("v: make vector! [integer! 1 2 3] to-vector v")),
            "make vector! [integer! 1 2 3]"
        );
    }
}
