//! Maps (M43): an insertion-ordered heterogeneous key→value table.
//!
//! Maps are created via `make map! <spec>` (or `to-map`). The spec may be:
//! - a `block!` of set-word/value pairs (`[a: 1 b: 2]`), word/value pairs
//!   (`[a 1 b 2]`), pair-blocks (`[[a 1] [b 2]]`), or any mix of literal
//!   key values (`[1 "one" #"c" 3]`). Keys are taken literally (the hashable
//!   subset of `Value`); values are *evaluated* in the caller's context so
//!   `make map! [a: 1 + 2]` stores `3` under `'a`.
//! - an `object!` → each word/value slot (except `self`) becomes a `Sym`-keyed
//!   entry.
//! - a `map!` → shallow copy.
//!
//! Path resolution (`m/word`, `m/2`, `m/key: value`) and the series natives
//! `length?`/`empty?`/`clear`/`select`/`find`/`copy` are extended in
//! `interp_walker.rs` and `series.rs`; equality lives in
//! `natives/compare.rs`; `same?`/`not-same?`/`words-of`/`values-of`/`reflect`
//! are extended in `object.rs`.

use std::rc::Rc;

use red_core::value::{MapDef, MapKey, Series, Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::interp_walker::eval_expression;
use crate::natives::{arity_err, type_name};

// ---------------------------------------------------------------------------
// make map! / to-map
// ---------------------------------------------------------------------------

/// `make map! <spec>` — build a new map.
///
/// Accepted spec forms:
/// - `block!` — walked raw; keys taken literally (the hashable subset),
///   values evaluated in the caller's context. Each step:
///   - `SetWord k:` → key `Sym(k)`, value = eval(next).
///   - `Block [k v]` → pair-block: key = `k` (literal), value = eval(`v`).
///   - Other (Word/Integer/String/Char/Logic/None) → key = `from_value`,
///     value = eval(next).
/// - `object!` → word/value slots (excluding `self`) as `Sym` keys.
/// - `map!` → shallow copy (new `MapDef` with cloned entries).
pub fn make_map(spec: &Value, env: &mut Env) -> Result<Value, EvalError> {
    match spec {
        Value::Block { series, .. } => {
            let map = MapDef::new();
            let data = series.data.borrow();
            let mut i = series.index;
            while i < data.len() {
                let key_val = &data[i];
                // SetWord key: `a: <value>`.
                if let Value::SetWord { sym, .. } = key_val {
                    i += 1;
                    let val = if i < data.len() {
                        eval_expression(&data, &mut i, env)?
                    } else {
                        Value::None
                    };
                    map.set(MapKey::Sym(sym.clone()), val);
                    continue;
                }
                // Pair-block key: `[key value]`.
                if let Value::Block {
                    series: pair_series,
                    ..
                } = key_val
                {
                    let pair = pair_series.data.borrow();
                    if pair.len() < 2 {
                        return Err(EvalError::Native {
                            message: "make map!: pair block needs key and value".into(),
                            span: key_val.span_or_default(),
                        });
                    }
                    let key = MapKey::from_value(&pair[0])
                        .ok_or_else(|| unhashable_key_error(&pair[0]))?;
                    // Evaluate the value expression starting at index 1 of
                    // the pair block (a distinct series from the outer spec,
                    // so the outer `data` borrow stays undisturbed).
                    let mut j = 1;
                    let val = eval_expression(&pair, &mut j, env)?;
                    map.set(key, val);
                    i += 1;
                    continue;
                }
                // Literal key (word/int/string/char/bool/none): `<key> <value>`.
                let key =
                    MapKey::from_value(key_val).ok_or_else(|| unhashable_key_error(key_val))?;
                i += 1;
                let val = if i < data.len() {
                    eval_expression(&data, &mut i, env)?
                } else {
                    Value::None
                };
                map.set(key, val);
            }
            Ok(Value::map(map))
        }
        Value::Object(obj) => {
            let map = MapDef::new();
            let borrow = obj.borrow();
            for sym in borrow.ctx.words() {
                if sym.as_str() == "self" {
                    continue;
                }
                if let Some(val) = borrow.ctx.get(&sym) {
                    map.set(MapKey::Sym(sym), val);
                }
            }
            Ok(Value::map(map))
        }
        Value::Map(m) => {
            // Shallow copy: new MapDef with cloned entries.
            let copy = m.borrow().clone();
            Ok(Value::map(copy))
        }
        other => Err(EvalError::TypeError {
            expected: "block!, object!, or map!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `to-map value` — convert to a `map!`. Same shape as `make map!` minus the
/// identity/copy-on-map case (a `map!` arg still copies, matching `to-*`
/// semantics).
pub(crate) fn to_map(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let spec = args.first().ok_or_else(|| EvalError::Arity {
        native: Symbol::new("to-map"),
        expected: 1,
        got: 0,
        span: Span::default(),
    })?;
    make_map(spec, env)
}

/// Build the "key type X is not hashable" error for an unhashable key value.
fn unhashable_key_error(key: &Value) -> EvalError {
    EvalError::Native {
        message: format!("map: key type {} is not hashable", type_name(key)),
        span: key.span_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Natives
// ---------------------------------------------------------------------------

/// `map? value` — `true` if value is a `map!`.
fn map_predicate(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "map?", 1, 0));
    }
    Ok(Value::Logic(matches!(args[0], Value::Map(_))))
}

/// `keys-of map` — block of the map's keys (as `Value`s) in insertion order.
fn keys_of_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "keys-of", 1, args.len()));
    }
    match &args[0] {
        Value::Map(m) => Ok(Value::block(Series::new(m.borrow().keys()))),
        other => Err(EvalError::TypeError {
            expected: "map!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `put map key value` — insert/replace an entry. Returns the previous value
/// (or `none` if the key was new).
fn put_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 3 {
        return Err(arity_err(args, "put", 3, args.len()));
    }
    let map = match &args[0] {
        Value::Map(m) => m.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "map!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let key = MapKey::from_value(&args[1]).ok_or_else(|| unhashable_key_error(&args[1]))?;
    let prev = map.borrow().set(key, args[2].clone());
    Ok(prev.unwrap_or(Value::None))
}

// ---------------------------------------------------------------------------
// Shared helpers used by path resolution + series natives
// ---------------------------------------------------------------------------

/// Resolve a path part to a `MapKey`. Handles the Word→Sym fall-back-to-Str
/// convention: a `Word` part maps to `MapKey::Sym` (and `select_map_field`
/// will additionally try `MapKey::Str` if the Sym lookup misses). Integer,
/// string, char, logic, none map directly to their `MapKey` forms.
#[allow(dead_code)]
pub(crate) fn part_to_map_key(part: &Value) -> Result<MapKey, EvalError> {
    MapKey::from_value(part).ok_or_else(|| EvalError::Native {
        message: format!("map: key type {} is not hashable", type_name(part)),
        span: part.span_or_default(),
    })
}

/// Select a field from a map by symbol, with the Red-parity string fall-back:
/// if the `Sym` lookup misses, retry with `MapKey::Str(sym.as_str())`. Returns
/// `none` if neither hits.
pub(crate) fn select_map_field(map: &Rc<std::cell::RefCell<MapDef>>, sym: &Symbol) -> Value {
    let b = map.borrow();
    if let Some(v) = b.get(&MapKey::Sym(sym.clone())) {
        return v;
    }
    let str_key: Rc<str> = Rc::from(sym.as_str());
    b.get(&MapKey::Str(str_key)).unwrap_or(Value::None)
}

/// Select a map entry by an arbitrary key. Returns `none` if absent.
pub(crate) fn select_map_key(map: &Rc<std::cell::RefCell<MapDef>>, key: &MapKey) -> Value {
    map.borrow().get(key).unwrap_or(Value::None)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

pub fn register_map_natives(env: &mut Env) {
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

    reg(env, "map?", map_predicate as NF, 1);
    reg(env, "to-map", to_map as NF, 1);
    reg(env, "keys-of", keys_of_native as NF, 1);
    reg(env, "put", put_native as NF, 3);
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

    fn run_capture(src: &str) -> Result<Vec<u8>, String> {
        run_capture_val(src).map(|(_, o)| o)
    }

    #[test]
    fn make_map_setword_molds_back() {
        assert_eq!(
            mold_to_string(&val("make map! [a: 1 b: 2]")),
            "make map! [a: 1 b: 2]"
        );
    }

    #[test]
    fn make_map_word_pair_form() {
        assert_eq!(
            mold_to_string(&val("make map! [a 1 b 2]")),
            "make map! [a: 1 b: 2]"
        );
    }

    #[test]
    fn make_map_empty() {
        assert_eq!(mold_to_string(&val("make map! []")), "make map! []");
    }

    #[test]
    fn map_field_access_word_key() {
        assert_eq!(mold_to_string(&val("m: make map! [a: 1] m/a")), "1");
    }

    #[test]
    fn map_field_set_path() {
        assert_eq!(mold_to_string(&val("m: make map! [a: 1] m/b: 2 m/b")), "2");
    }

    #[test]
    fn map_set_path_updates_existing() {
        assert_eq!(mold_to_string(&val("m: make map! [a: 1] m/a: 9 m/a")), "9");
    }

    #[test]
    fn map_heterogeneous_keys_round_trip() {
        let v = val("make map! [a 1 2 \"two\" #\"c\" 3]");
        let molded = mold_to_string(&v);
        assert_eq!(molded, "make map! [a: 1 2 \"two\" #\"c\" 3]");
    }

    #[test]
    fn map_predicate() {
        assert_eq!(mold_to_string(&val("map? make map! []")), "true");
        assert_eq!(mold_to_string(&val("map? []")), "false");
    }

    #[test]
    fn map_length() {
        assert_eq!(mold_to_string(&val("length? make map! [a 1 b 2]")), "2");
    }

    #[test]
    fn map_empty_predicate() {
        assert_eq!(mold_to_string(&val("empty? make map! []")), "true");
        assert_eq!(mold_to_string(&val("empty? make map! [a 1]")), "false");
    }

    #[test]
    fn map_insertion_order_preserved() {
        assert_eq!(
            mold_to_string(&val("keys-of make map! [a 1 2 \"two\" #\"c\" 3]")),
            "[a 2 #\"c\"]"
        );
    }

    #[test]
    fn map_integer_key_path() {
        assert_eq!(
            mold_to_string(&val("m: make map! [1 \"one\"] m/1")),
            "\"one\""
        );
    }

    #[test]
    fn map_string_key_via_select() {
        assert_eq!(
            mold_to_string(&val("select make map! [\"k\" 99] \"k\"")),
            "99"
        );
    }

    #[test]
    fn map_char_key_via_select() {
        assert_eq!(
            mold_to_string(&val("select make map! [#\"x\" 7] #\"x\"")),
            "7"
        );
    }

    #[test]
    fn map_missing_key_returns_none() {
        assert_eq!(mold_to_string(&val("m: make map! [a 1] m/missing")), "none");
    }

    #[test]
    fn map_select() {
        assert_eq!(mold_to_string(&val("select make map! [a 99] 'a")), "99");
        assert_eq!(
            mold_to_string(&val("select make map! [a 99] 'missing")),
            "none"
        );
    }

    #[test]
    fn map_find() {
        assert_eq!(mold_to_string(&val("find make map! [a 1 b 2] 'b")), "b");
        assert_eq!(mold_to_string(&val("find make map! [a 1 b 2] 'z")), "none");
    }

    #[test]
    fn map_copy() {
        let src = "m: make map! [a 1] c: copy m c/a";
        assert_eq!(mold_to_string(&val(src)), "1");
        // Copy is independent.
        let src2 = "m: make map! [a 1] c: copy m c/b: 2 m/b";
        assert_eq!(mold_to_string(&val(src2)), "none");
    }

    #[test]
    fn map_clear() {
        assert_eq!(
            mold_to_string(&val("clear make map! [a 1 b 2]")),
            "make map! []"
        );
    }

    #[test]
    fn map_put_inserts_and_returns_none() {
        assert_eq!(mold_to_string(&val("m: make map! [] put m 'a 1 m/a")), "1");
        let src = "m: make map! [a 1] print put m 'a 9";
        let _ = run_capture(src).unwrap();
    }

    #[test]
    fn map_put_replace_returns_old() {
        assert_eq!(mold_to_string(&val("m: make map! [a 1] put m 'a 9")), "1");
    }

    #[test]
    fn map_values_of() {
        assert_eq!(
            mold_to_string(&val("values-of make map! [a 1 b 2]")),
            "[1 2]"
        );
    }

    #[test]
    fn map_words_of() {
        assert_eq!(
            mold_to_string(&val("words-of make map! [a 1 b 2]")),
            "[a b]"
        );
    }

    #[test]
    fn map_reflect_words() {
        assert_eq!(
            mold_to_string(&val("reflect make map! [a 1] 'words")),
            "[a]"
        );
        assert_eq!(
            mold_to_string(&val("reflect make map! [a 1] 'values")),
            "[1]"
        );
    }

    #[test]
    fn map_same_identity() {
        assert_eq!(mold_to_string(&val("m: make map! [a 1] same? m m")), "true");
        assert_eq!(
            mold_to_string(&val("same? (make map! [a 1]) (make map! [a 1])")),
            "false"
        );
    }

    #[test]
    fn map_to_map_from_object() {
        let src = "o: make object! [a: 1 b: 2] m: to-map o m/a";
        assert_eq!(mold_to_string(&val(src)), "1");
    }

    #[test]
    fn map_make_from_object() {
        let src = "o: make object! [a: 1] m: make map! o m/a";
        assert_eq!(mold_to_string(&val(src)), "1");
    }

    #[test]
    fn map_equality() {
        assert_eq!(
            mold_to_string(&val("(make map! [a 1]) = (make map! [a 1])")),
            "true"
        );
        assert_eq!(
            mold_to_string(&val("(make map! [a 1]) = (make map! [a 2])")),
            "false"
        );
    }

    #[test]
    fn map_unhashable_key_errors() {
        // A block can't be a map key. `put` surfaces the error directly.
        let r = run_capture("m: make map! [] put m [] 1");
        assert!(r.is_err(), "expected error for block key");
        let err = r.unwrap_err();
        assert!(
            err.contains("not hashable") || err.contains("hashable"),
            "got: {err}"
        );
    }

    #[test]
    fn map_pair_block_form() {
        assert_eq!(
            mold_to_string(&val("make map! [[a 1] [b 2]]")),
            "make map! [a: 1 b: 2]"
        );
    }

    #[test]
    fn map_type_name() {
        assert_eq!(mold_to_string(&val("type? make map! []")), "map!");
    }

    #[test]
    fn map_types_of_includes_map() {
        assert_eq!(mold_to_string(&val("types-of make map! []")), "[map!]");
    }
}
