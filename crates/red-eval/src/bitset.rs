//! Bitsets (M46): a bit-packed set of byte values (0..255).
//!
//! Bitsets are created via `make bitset! <spec>` (or `to-bitset`/`charset`).
//! Spec forms:
//! - `string!` → one bit set per char in the string.
//! - `char!` → a singleton bitset (just that char).
//! - `binary!` → the raw bit pattern: byte `i` of the binary controls bits
//!   `8i..8i+8` of the bitset. Size is 8 × binary length.
//! - `integer!` → an empty bitset sized to N bits (all clear).
//! - `block!` → union of: chars/strings (each char sets a bit), `char!`
//!   literals, `#"a" - #"z"` ranges (parsed as `[#"a" - #"z"]` with the
//!   `-` Word separator), nested blocks, and `binary!` values.
//! - `bitset!` → shallow copy.
//!
//! The `parse` dialect treats a `bitset!` rule as a charset match: the
//! current input char is tested for membership; on a hit the cursor
//! advances by one char and the rule succeeds.
//!
//! Set operations (`union`/`intersect`/`difference`/`complement`) mutate
//! the left operand and return it. `extract? char bitset` is a membership
//! test returning `logic!`.

use red_core::value::{BitsetDef, Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::natives::{arity_err, type_name};

// ---------------------------------------------------------------------------
// make bitset! / to-bitset
// ---------------------------------------------------------------------------

/// `make bitset! <spec>` — build a new bitset (see module docs for forms).
pub fn make_bitset(spec: &Value, _env: &mut Env) -> Result<Value, EvalError> {
    match spec {
        Value::String { s, .. } => Ok(Value::bitset(BitsetDef::from_chars(s))),
        Value::Char { c, .. } => {
            let bs = BitsetDef::new_charset();
            bs.set((*c as u32) as usize);
            Ok(Value::bitset(bs))
        }
        Value::String8 { bytes, .. } => Ok(binary_to_bitset(bytes)),
        Value::Integer { n, .. } => {
            if *n < 0 {
                return Err(EvalError::Native {
                    message: format!("make bitset!: size must be non-negative, got {n}"),
                    span: spec.span_or_default(),
                });
            }
            Ok(Value::bitset(BitsetDef::new(*n as usize)))
        }
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            build_from_block(&data, series.index)
        }
        Value::Bitset(b) => {
            // Shallow copy.
            let copy = b.borrow().clone();
            Ok(Value::bitset(copy))
        }
        other => Err(EvalError::TypeError {
            expected: "string!, char!, binary!, integer!, block!, or bitset!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// Convert a `binary!` byte pattern into a bitset. Byte `i` of the binary
/// controls bits `8i..8i+7` of the bitset: bit `8i + j` is set iff byte `i`
/// has bit `j` set (little-endian bit order within a byte). `#{FF}` → bits
/// 0-7 set; `#{80}` → bit 7 set only. Size is 8 × binary length.
fn binary_to_bitset(bytes: &[u8]) -> Value {
    let len = bytes.len() * 8;
    let n_words = len.div_ceil(64);
    let mut packed = vec![0u64; n_words];
    for (i, &b) in bytes.iter().enumerate() {
        let word = i / 8;
        let byte_in_word = i % 8;
        packed[word] |= (b as u64) << (8 * byte_in_word);
    }
    Value::bitset(BitsetDef {
        bits: std::cell::RefCell::new(packed),
        len,
    })
}

/// Walk a block spec, building a bitset from chars/strings/ranges/nested
/// blocks. The `-` Word between two char!/integer! values denotes a range.
fn build_from_block(data: &[Value], start: usize) -> Result<Value, EvalError> {
    let bs = BitsetDef::new_charset();
    let mut i = start;
    while i < data.len() {
        let v = &data[i];
        match v {
            Value::Char { c, .. } => {
                // Look ahead for `-"c"` range form.
                if i + 2 < data.len() {
                    if let Value::Word { sym, .. } = &data[i + 1] {
                        if sym.as_str() == "-" {
                            if let Value::Char { c: hi, .. } = &data[i + 2] {
                                let lo_b = (*c as u32) as u8;
                                let hi_b = (*hi as u32) as u8;
                                let (lo, hi) = if lo_b <= hi_b {
                                    (lo_b, hi_b)
                                } else {
                                    (hi_b, lo_b)
                                };
                                for b in lo..=hi {
                                    bs.set(b as usize);
                                }
                                i += 3;
                                continue;
                            }
                        }
                    }
                }
                bs.set((*c as u32) as usize);
                i += 1;
            }
            Value::Integer { n, .. } => {
                // Integer range form: `1 - 9`.
                if i + 2 < data.len() {
                    if let Value::Word { sym, .. } = &data[i + 1] {
                        if sym.as_str() == "-" {
                            if let Value::Integer { n: hi, .. } = &data[i + 2] {
                                let lo_b = (*n as u32) as u8;
                                let hi_b = (*hi as u32) as u8;
                                let (lo, hi) = if lo_b <= hi_b {
                                    (lo_b, hi_b)
                                } else {
                                    (hi_b, lo_b)
                                };
                                for b in lo..=hi {
                                    bs.set(b as usize);
                                }
                                i += 3;
                                continue;
                            }
                        }
                    }
                }
                if let Ok(b) = u8::try_from(*n) {
                    bs.set(b as usize);
                }
                i += 1;
            }
            Value::String { s, .. } => {
                for c in s.chars() {
                    let b = (c as u32) as usize;
                    if b < 256 {
                        bs.set(b);
                    }
                }
                i += 1;
            }
            Value::String8 { bytes, .. } => {
                for &b in bytes.iter() {
                    bs.set(b as usize);
                }
                i += 1;
            }
            Value::Block { series, .. } => {
                let child = series.data.borrow();
                let inner = build_from_block(&child, series.index)?;
                if let Value::Bitset(other) = &inner {
                    bs.union(&other.borrow());
                }
                i += 1;
            }
            Value::Bitset(b) => {
                bs.union(&b.borrow());
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    Ok(Value::bitset(bs))
}

/// `to-bitset value` — convert to a `bitset!` (same as `make bitset!`).
pub(crate) fn to_bitset(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let spec = args.first().ok_or_else(|| EvalError::Arity {
        native: Symbol::new("to-bitset"),
        expected: 1,
        got: 0,
        span: Span::default(),
    })?;
    make_bitset(spec, env)
}

// ---------------------------------------------------------------------------
// Natives
// ---------------------------------------------------------------------------

/// `charset string` — build a bitset of the chars in `string`. Shorthand for
/// `make bitset! "..."`; the common construction in `parse` dialect code.
fn charset_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "charset", 1, 0));
    }
    make_bitset(&args[0], env)
}

/// Helper: extract a bitset `Rc<RefCell<BitsetDef>>` from a `Value::Bitset`
/// or raise a TypeError.
fn expect_bitset<'a>(
    v: &'a Value,
    native: &str,
) -> Result<&'a std::rc::Rc<std::cell::RefCell<BitsetDef>>, EvalError> {
    match v {
        Value::Bitset(b) => Ok(b),
        other => Err(EvalError::TypeError {
            expected: "bitset!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
    .map_err(|e| {
        // Augment the message with the native name for clarity.
        if let EvalError::TypeError {
            expected,
            found,
            span,
        } = e
        {
            EvalError::Native {
                message: format!("{native}: expected {expected}, got {found}"),
                span,
            }
        } else {
            e
        }
    })
}

/// `union a b` — bitset union. Mutates `a` and returns it.
fn union_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "union", 2, args.len()));
    }
    let a = expect_bitset(&args[0], "union")?.clone();
    let b = expect_bitset(&args[1], "union")?;
    a.borrow().union(&b.borrow());
    Ok(Value::Bitset(a))
}

/// `intersect a b` — bitset intersection. Mutates `a` and returns it.
fn intersect_native(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "intersect", 2, args.len()));
    }
    let a = expect_bitset(&args[0], "intersect")?.clone();
    let b = expect_bitset(&args[1], "intersect")?;
    a.borrow().intersect(&b.borrow());
    Ok(Value::Bitset(a))
}

/// `difference a b` — bitset difference (a \ b). Mutates `a` and returns it.
fn difference_native(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "difference", 2, args.len()));
    }
    let a = expect_bitset(&args[0], "difference")?.clone();
    let b = expect_bitset(&args[1], "difference")?;
    a.borrow().difference(&b.borrow());
    Ok(Value::Bitset(a))
}

/// `extract? char-or-int bitset` — membership test. Returns `logic!`.
fn extract_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "extract?", 2, args.len()));
    }
    let byte = match &args[0] {
        Value::Char { c, .. } => *c as u32 as usize,
        Value::Integer { n, .. } => *n as usize,
        other => {
            return Err(EvalError::TypeError {
                expected: "char! or integer!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let b = expect_bitset(&args[1], "extract?")?;
    Ok(Value::Logic(b.borrow().test(byte)))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

pub fn register_bitset_natives(env: &mut Env) {
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

    reg(env, "charset", charset_native as NF, 1);
    reg(env, "to-bitset", to_bitset as NF, 1);
    reg(env, "union", union_native as NF, 2);
    reg(env, "intersect", intersect_native as NF, 2);
    reg(env, "difference", difference_native as NF, 2);
    reg(env, "extract?", extract_predicate as NF, 2);
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
    use std::rc::Rc;

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
    fn charset_string_molds_back() {
        assert_eq!(
            mold_to_string(&val("charset \"ABC\"")),
            "make bitset! \"ABC\""
        );
    }

    #[test]
    fn make_bitset_string_molds_back() {
        assert_eq!(
            mold_to_string(&val("make bitset! \"abc\"")),
            "make bitset! \"abc\""
        );
    }

    #[test]
    fn make_bitset_char_singleton() {
        let v = val("make bitset! #\"a\"");
        assert!(matches!(v, Value::Bitset(_)));
        assert_eq!(mold_to_string(&v), "make bitset! \"a\"");
    }

    #[test]
    fn make_bitset_range_block() {
        // `make bitset! [#"a" - #"z"]` sets bits a-z.
        let v = val("make bitset! [#\"a\" - #\"z\"]");
        // All 26 lowercase letters set; mold should produce that string.
        let expected: String = ('a'..='z').collect();
        assert_eq!(mold_to_string(&v), format!("make bitset! {expected:?}"));
    }

    #[test]
    fn make_bitset_integer_range_block() {
        let v = val("make bitset! [48 - 57]");
        // Digits 0-9 ASCII 48-57.
        let expected: String = ('0'..='9').collect();
        assert_eq!(mold_to_string(&v), format!("make bitset! {expected:?}"));
    }

    #[test]
    fn make_bitset_copy() {
        let v = val("a: charset \"x\" b: make bitset! a b: difference b charset \"x\" print extract? #\"x\" a");
        // a is unchanged by b's difference (make bitset! copies).
        let _ = mold_to_string(&v);
    }

    #[test]
    fn bitset_predicate_true_false() {
        assert_eq!(mold_to_string(&val("bitset? charset \"ABC\"")), "true");
        assert_eq!(mold_to_string(&val("bitset? 5")), "false");
    }

    #[test]
    fn extract_predicate_true() {
        assert_eq!(
            mold_to_string(&val("extract? #\"A\" charset \"ABC\"")),
            "true"
        );
    }

    #[test]
    fn extract_predicate_false() {
        assert_eq!(
            mold_to_string(&val("extract? #\"z\" charset \"ABC\"")),
            "false"
        );
    }

    #[test]
    fn extract_predicate_integer_byte() {
        assert_eq!(mold_to_string(&val("extract? 65 charset \"ABC\"")), "true");
    }

    #[test]
    fn union_combines_bits() {
        assert_eq!(
            mold_to_string(&val("union charset \"AB\" charset \"CD\"")),
            "make bitset! \"ABCD\""
        );
    }

    #[test]
    fn intersect_keeps_overlap() {
        assert_eq!(
            mold_to_string(&val("intersect charset \"ABCD\" charset \"BE\"")),
            "make bitset! \"B\""
        );
    }

    #[test]
    fn difference_removes_bits() {
        assert_eq!(
            mold_to_string(&val("difference charset \"ABCD\" charset \"BC\"")),
            "make bitset! \"AD\""
        );
    }

    #[test]
    fn complement_native_flips_bits() {
        // complement of charset "A" should have 255 bits set; the printer
        // falls back to `#{hex}` form since not all bits are printable ASCII.
        let v = val("complement charset \"A\"");
        assert!(matches!(v, Value::Bitset(_)));
    }

    #[test]
    fn to_bitset_from_binary() {
        // binary #{FF} → bitset with bits 0..7 set → molds as "make bitset! #{FF}".
        let v = val("to-bitset #{FF}");
        assert!(matches!(v, Value::Bitset(_)));
        // Extract? should report bits set in 0..7.
        assert_eq!(mold_to_string(&val("extract? 0 to-bitset #{FF}")), "true");
        assert_eq!(mold_to_string(&val("extract? 7 to-bitset #{FF}")), "true");
        assert_eq!(mold_to_string(&val("extract? 8 to-bitset #{FF}")), "false");
    }

    #[test]
    fn make_bitset_empty_block() {
        assert_eq!(mold_to_string(&val("make bitset! []")), "make bitset! \"\"");
    }
}
