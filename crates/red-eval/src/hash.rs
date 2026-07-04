//! Hashes (M83): an unordered heterogeneous key→value table backed by a real
//! `HashMap` (not `IndexMap`).
//!
//! Distinct from `map!` (M43) in two ways: (1) iteration order is unspecified
//! (HashMap order — a `key_order` vec is kept for stable mold/`keys-of` output
//! in tests only, a documented deviation from Red); (2) `hash!` IS a
//! `series!` — indexable/sliceable as alternating key/value pairs.
//!
//! Hashes are created via `make hash! <spec>` (or `to-hash`). The spec may be:
//! - a `block!` of set-word/value pairs (`[a: 1 b: 2]`), word/value pairs
//!   (`[a 1 b 2]`), pair-blocks (`[[a 1] [b 2]]`), or any mix of literal
//!   key values (`[1 "one" #"c" 3]`). Keys are taken literally (the hashable
//!   subset of `Value`); values are *evaluated* in the caller's context so
//!   `make hash! [a: 1 + 2]` stores `3` under `'a`.
//! - an `object!` → each word/value slot (except `self`) becomes a `Sym`-keyed
//!   entry.
//! - a `map!` → copy entries into a new `HashDef`.
//! - a `hash!` → shallow copy.
//!
//! Path resolution (`h/word`, `h/2`, `h/key: value` — Integer parts are *key
//! lookups*, not positions, matching `map!`) and the series natives
//! `length?`/`empty?`/`clear`/`select`/`find`/`copy`/`pick`/`poke`/etc. are
//! extended in `interp_walker.rs` and `series.rs`; equality lives in
//! `natives/compare.rs`; `same?`/`not-same?`/`words-of`/`values-of`/`reflect`
//! are extended in `object.rs`.

use std::rc::Rc;

use red_core::value::{HashDef, MapKey, Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::interp_walker::eval_expression;
use crate::natives::{arity_err, type_name};

// ---------------------------------------------------------------------------
// make hash! / to-hash
// ---------------------------------------------------------------------------

/// `make hash! <spec>` — build a new hash!.
///
/// Accepted spec forms (mirror `make map!`):
/// - `block!` — walked raw; keys taken literally (the hashable subset),
///   values evaluated in the caller's context. Each step:
///   - `SetWord k:` → key `Sym(k)`, value = eval(next).
///   - `Block [k v]` → pair-block: key = `k` (literal), value = eval(`v`).
///   - Other (Word/Integer/String/Char/Logic/None) → key = `from_value`,
///     value = eval(next).
/// - `object!` → word/value slots (excluding `self`) as `Sym` keys.
/// - `map!` → copy each entry into a new `HashDef` (in `map!` iteration order).
/// - `hash!` → shallow copy (new `HashDef` with cloned entries + key_order).
pub fn make_hash(spec: &Value, env: &mut Env) -> Result<Value, EvalError> {
    match spec {
        Value::Block { series, .. } => {
            let hash = HashDef::new();
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
                    hash.set(MapKey::Sym(sym.clone()), val);
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
                            message: "make hash!: pair block needs key and value".into(),
                            span: key_val.span_or_default(),
                        });
                    }
                    let key = MapKey::from_value(&pair[0])
                        .ok_or_else(|| unhashable_key_error(&pair[0]))?;
                    let mut j = 1;
                    let val = eval_expression(&pair, &mut j, env)?;
                    hash.set(key, val);
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
                hash.set(key, val);
            }
            Ok(Value::hash(hash))
        }
        Value::Object(obj) => {
            let hash = HashDef::new();
            let borrow = obj.borrow();
            for sym in borrow.ctx.words() {
                if sym.as_str() == "self" {
                    continue;
                }
                if let Some(val) = borrow.ctx.get(&sym) {
                    hash.set(MapKey::Sym(sym), val);
                }
            }
            Ok(Value::hash(hash))
        }
        Value::Map(m) => {
            // Copy each entry into a new HashDef. Map iteration order is
            // preserved (IndexMap), so `key_order` follows the map's order.
            let hash = HashDef::new();
            for (k, v) in m.borrow().entries.borrow().iter() {
                hash.set(k.clone(), v.clone());
            }
            Ok(Value::hash(hash))
        }
        Value::Hash(h) => {
            // Shallow copy: new HashDef with cloned entries + key_order.
            let copy = h.borrow().clone();
            Ok(Value::hash(copy))
        }
        other => Err(EvalError::TypeError {
            expected: "block!, object!, map!, or hash!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `to-hash value` — convert to a `hash!`. Same shape as `make hash!` minus
/// the identity/copy-on-hash case (a `hash!` arg still copies, matching
/// `to-*` semantics).
pub(crate) fn to_hash(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let spec = args.first().ok_or_else(|| EvalError::Arity {
        native: Symbol::new("to-hash"),
        expected: 1,
        got: 0,
        span: Span::default(),
    })?;
    make_hash(spec, env)
}

/// Build the "key type X is not hashable" error for an unhashable key value.
fn unhashable_key_error(key: &Value) -> EvalError {
    EvalError::Native {
        message: format!("hash: key type {} is not hashable", type_name(key)),
        span: key.span_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Natives
// ---------------------------------------------------------------------------

/// `hash? value` — `true` if value is a `hash!`.
fn hash_predicate(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "hash?", 1, 0));
    }
    Ok(Value::Logic(matches!(args[0], Value::Hash(_))))
}

// ---------------------------------------------------------------------------
// Shared helpers used by path resolution + series natives
// ---------------------------------------------------------------------------

/// Select a field from a hash by symbol, with the Red-parity string fall-back:
/// if the `Sym` lookup misses, retry with `MapKey::Str(sym.as_str())`. Returns
/// `none` if neither hits. (Mirrors `map::select_map_field`.)
pub(crate) fn select_hash_field(hash: &Rc<std::cell::RefCell<HashDef>>, sym: &Symbol) -> Value {
    let b = hash.borrow();
    if let Some(v) = b.get(&MapKey::Sym(sym.clone())) {
        return v;
    }
    let str_key: Rc<str> = Rc::from(sym.as_str());
    b.get(&MapKey::Str(str_key)).unwrap_or(Value::None)
}

/// Select a hash entry by an arbitrary key. Returns `none` if absent.
pub(crate) fn select_hash_key(hash: &Rc<std::cell::RefCell<HashDef>>, key: &MapKey) -> Value {
    hash.borrow().get(key).unwrap_or(Value::None)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

pub fn register_hash_natives(env: &mut Env) {
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

    reg(env, "hash?", hash_predicate as NF, 1);
    reg(env, "to-hash", to_hash as NF, 1);
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

    #[test]
    fn make_hash_setword_molds_back() {
        assert_eq!(
            mold_to_string(&val("make hash! [a: 1 b: 2]")),
            "make hash! [a: 1 b: 2]"
        );
    }

    #[test]
    fn make_hash_word_pair_form() {
        assert_eq!(
            mold_to_string(&val("make hash! [a 1 b 2]")),
            "make hash! [a: 1 b: 2]"
        );
    }

    #[test]
    fn make_hash_empty() {
        assert_eq!(mold_to_string(&val("make hash! []")), "make hash! []");
    }

    #[test]
    fn hash_field_access_word_key() {
        assert_eq!(mold_to_string(&val("h: make hash! [a: 1] h/a")), "1");
    }

    #[test]
    fn hash_field_set_path() {
        assert_eq!(mold_to_string(&val("h: make hash! [a: 1] h/b: 2 h/b")), "2");
    }

    #[test]
    fn hash_is_a_series() {
        // The headline discriminator vs map! (which is NOT a series).
        assert_eq!(mold_to_string(&val("series? make hash! []")), "true");
        assert_eq!(mold_to_string(&val("series? make map! []")), "false");
    }

    #[test]
    fn hash_length_is_pair_count() {
        assert_eq!(mold_to_string(&val("length? make hash! [a 1 b 2]")), "4");
    }

    #[test]
    fn hash_pick_alternating() {
        // pick 1 → first key, pick 2 → first value, pick 3 → second key.
        assert_eq!(mold_to_string(&val("pick (make hash! [a 1 b 2]) 1")), "a");
        assert_eq!(mold_to_string(&val("pick (make hash! [a 1 b 2]) 2")), "1");
        assert_eq!(mold_to_string(&val("pick (make hash! [a 1 b 2]) 3")), "b");
        assert_eq!(mold_to_string(&val("pick (make hash! [a 1 b 2]) 4")), "2");
    }

    #[test]
    fn hash_first_second_last() {
        assert_eq!(mold_to_string(&val("first make hash! [a 1 b 2]")), "a");
        assert_eq!(mold_to_string(&val("second make hash! [a 1 b 2]")), "1");
        assert_eq!(mold_to_string(&val("last make hash! [a 1 b 2]")), "2");
    }

    #[test]
    fn hash_equality_order_independent() {
        // Two hashes with the same entries in different insertion order are
        // equal (unlike map!, which is order-sensitive in equality — actually
        // map! equality is also order-independent in the POC; the headline
        // discriminator is series? and the unspecified iteration order).
        assert_eq!(
            mold_to_string(&val("(make hash! [a 1 b 2]) = (make hash! [b 2 a 1])")),
            "true"
        );
    }

    #[test]
    fn hash_predicate() {
        assert_eq!(mold_to_string(&val("hash? make hash! []")), "true");
        assert_eq!(mold_to_string(&val("hash? make map! []")), "false");
    }

    #[test]
    fn hash_to_hash_copies() {
        assert_eq!(
            mold_to_string(&val("to-hash make hash! [a 1]")),
            "make hash! [a: 1]"
        );
    }

    #[test]
    fn hash_from_map() {
        assert_eq!(
            mold_to_string(&val("make hash! make map! [a: 1 b: 2]")),
            "make hash! [a: 1 b: 2]"
        );
    }

    #[test]
    fn hash_foreach_destructuring() {
        // foreach [k v] hash iterates the alternating key/value view.
        let out = val("out: copy [] foreach [k v] make hash! [a 1 b 2] [append out v] out");
        assert_eq!(mold_to_string(&out), "[1 2]");
    }

    #[test]
    fn hash_foreach_destructuring_keys() {
        let out = val("out: copy [] foreach [k v] make hash! [a 1 b 2] [append out k] out");
        assert_eq!(mold_to_string(&out), "[a b]");
    }

    #[test]
    fn hash_append_pair() {
        assert_eq!(
            mold_to_string(&val("h: make hash! [a 1] append h [b 2]")),
            "make hash! [a: 1 b: 2]"
        );
    }

    #[test]
    fn hash_poke_value_slot() {
        // poke at position 2 (the value of the first pair) updates it.
        assert_eq!(
            mold_to_string(&val("h: make hash! [a 1 b 2] poke h 2 99 h/a")),
            "99"
        );
    }

    #[test]
    fn hash_clear() {
        assert_eq!(
            mold_to_string(&val("clear make hash! [a 1 b 2]")),
            "make hash! []"
        );
    }

    #[test]
    fn hash_select_and_find() {
        assert_eq!(
            mold_to_string(&val("select (make hash! [a 1 b 2]) 'b")),
            "2"
        );
        assert_eq!(mold_to_string(&val("find (make hash! [a 1 b 2]) 'a")), "a");
        assert_eq!(
            mold_to_string(&val("find (make hash! [a 1 b 2]) 'z")),
            "none"
        );
    }

    #[test]
    fn hash_copy() {
        assert_eq!(
            mold_to_string(&val("copy make hash! [a 1 b 2]")),
            "make hash! [a: 1 b: 2]"
        );
    }

    #[test]
    fn hash_words_of_values_of() {
        assert_eq!(
            mold_to_string(&val("words-of make hash! [a 1 b 2]")),
            "[a b]"
        );
        assert_eq!(
            mold_to_string(&val("values-of make hash! [a 1 b 2]")),
            "[1 2]"
        );
    }

    #[test]
    fn hash_same_is_identity() {
        // same? is reference identity; two separate makes are not same.
        assert_eq!(
            mold_to_string(&val("same? (make hash! [a 1]) (make hash! [a 1])")),
            "false"
        );
        assert_eq!(
            mold_to_string(&val("h: make hash! [a 1] same? h h")),
            "true"
        );
    }

    #[test]
    fn hash_integer_path_is_key_lookup() {
        // h/2 looks up MapKey::Int(2), NOT the second position.
        assert_eq!(
            mold_to_string(&val("h: make hash! [2 \"twenty\"] h/2")),
            "\"twenty\""
        );
    }
}
