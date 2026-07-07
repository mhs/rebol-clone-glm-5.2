//! Type-conversion natives + `make`/`to` dispatcher + `form` (Milestone 14).
//!
//! Covers the Red `to-*` family (`to-integer`/`to-float`/`to-string`/
//! `to-block`/`to-word`/`to-set-word`/`to-get-word`/`to-lit-word`/`to-logic`),
//! the `make <type> <spec>` constructor (extended beyond the original
//! `make function!` form to support `integer!`/`float!`/`string!`/`block!`),
//! the `to <type> <value>` conversion alias, and `form` as a native (returns
//! the human-readable `form` of a value as a `string!`).
//!
//! Semantics follow Red/Rebol:
//! - `to-string` is `form` (no quotes, no escapes for strings, space-joined
//!   blocks, bare word names).
//! - `make string! n` returns `""` (the integer is a capacity hint, not a
//!   fill length — matches Red's documented behavior).
//! - `make block! n` returns `[]` (capacity hint).
//! - `to string! 5` returns `"5"` (conversion, not construction — `to` always
//!   renders the value's representation).
//! - `to-word` from a non-string/non-word value raises a `TypeError`.

use std::rc::Rc;

use red_core::lexer;
use red_core::parser::load;
use red_core::value::{Series, Span, Symbol, Value};
use red_core::{form_to_string, mold_to_string};
use red_core::{Env, EvalError, RefineArgs};

use crate::natives::{arity_err, expect_block, func_native, truthy, type_name};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a word-family symbol from a value (any of Word/SetWord/GetWord/
/// LitWord/Refinement). Returns `None` for non-word values.
fn word_sym(v: &Value) -> Option<Symbol> {
    match v {
        Value::Word { sym, .. }
        | Value::SetWord { sym, .. }
        | Value::GetWord { sym, .. }
        | Value::LitWord { sym, .. }
        | Value::Refinement { sym, .. } => Some(sym.clone()),
        _ => None,
    }
}

/// Type-word operand (`integer!`/`string!`/etc.): accept either a `word!`
/// (`integer`) or `lit-word!` (`'integer!`) form. Returns the lowercase type
/// name string (with trailing `!` preserved).
fn type_name_operand(v: &Value) -> Result<String, EvalError> {
    let s = match v {
        Value::LitWord { sym, .. } | Value::Word { sym, .. } => sym.as_str().to_string(),
        Value::Refinement { sym, .. } => sym.as_str().to_string(),
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    Ok(s)
}

/// `EvalError::Native` with a span sourced from `from` (the offending value).
fn native_err(from: &Value, msg: impl Into<String>) -> EvalError {
    EvalError::Native {
        message: msg.into(),
        span: from.span_or_default(),
    }
}

/// Parse a string as `i64`. Errors carry the string's span.
fn parse_i64(s: &str, span_src: &Value) -> Result<i64, EvalError> {
    s.trim().parse::<i64>().map_err(|_| {
        native_err(
            span_src,
            format!("to-integer: cannot parse {s:?} as integer"),
        )
    })
}

/// Parse a string as `f64`. Errors carry the string's span.
fn parse_f64(s: &str, span_src: &Value) -> Result<f64, EvalError> {
    s.trim()
        .parse::<f64>()
        .map_err(|_| native_err(span_src, format!("to-float: cannot parse {s:?} as float")))
}

// ---------------------------------------------------------------------------
// to-* family (all arity 1)
// ---------------------------------------------------------------------------

/// `to-integer value` — coerce to `integer!`.
/// - `integer!` → identity
/// - `float!`   → truncate toward zero
/// - `logic!`   → 1 if true, 0 if false
/// - `none!`    → 0
/// - `string!`  → parse as i64 (error on unparseable, span from the string)
/// - `char!`    → codepoint as integer (M38)
fn to_integer(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-integer", 1, args.len()));
    }
    let v = &args[0];
    Ok(match v {
        Value::Integer { n, .. } => Value::integer(*n),
        Value::Float { f, .. } => Value::integer(*f as i64),
        Value::Decimal { d, .. } => Value::integer((*d).try_into().unwrap_or(0)),
        Value::Logic(b) => Value::integer(if *b { 1 } else { 0 }),
        Value::None => Value::integer(0),
        Value::String { s, .. } => Value::integer(parse_i64(s, v)?),
        Value::Char { c, .. } => Value::integer(*c as i64),
        // M80: money → its cents value (e.g. $10.00 → 1000).
        Value::Money { amount, .. } => Value::integer(amount.cents),
        // M140: duration → total seconds, truncated.
        Value::Duration { d, .. } => Value::integer(d.num_seconds()),
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!, float!, logic!, none!, char!, or string!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    })
}

/// `to-char value` — coerce to `char!`. (M38)
/// - `integer!` → codepoint (truncate to u32; error if not a valid char)
/// - `float!`   → codepoint (truncate toward zero)
/// - `logic!`   → `#"^(01)"` if true, `#"^(00)"` if false
/// - `string!`  → first char of the string (error if empty/multi-char)
/// - `char!`    → identity
fn to_char(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-char", 1, args.len()));
    }
    let v = &args[0];
    Ok(match v {
        Value::Char { c, .. } => Value::char(*c),
        Value::Integer { n, .. } => {
            let cp = *n as u32;
            let c = char::from_u32(cp).ok_or_else(|| EvalError::Native {
                message: format!("to-char: integer {n} is not a valid codepoint"),
                span: v.span_or_default(),
            })?;
            Value::char(c)
        }
        Value::Float { f, .. } => {
            let cp = (*f as i64) as u32;
            let c = char::from_u32(cp).ok_or_else(|| EvalError::Native {
                message: format!("to-char: float {f} is not a valid codepoint"),
                span: v.span_or_default(),
            })?;
            Value::char(c)
        }
        Value::Logic(b) => Value::char(if *b { '\u{1}' } else { '\u{0}' }),
        Value::String { s, .. } => {
            let mut chars = s.chars();
            let c = chars.next().ok_or_else(|| EvalError::Native {
                message: "to-char: empty string has no char".into(),
                span: v.span_or_default(),
            })?;
            if chars.next().is_some() {
                return Err(EvalError::Native {
                    message: "to-char: string has more than one char".into(),
                    span: v.span_or_default(),
                });
            }
            Value::char(c)
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "char!, integer!, float!, logic!, or single-char string!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    })
}

/// `to-float value` — coerce to `float!`.
/// - `integer!` → as f64
/// - `float!`   → identity
/// - `string!`  → parse as f64 (error on unparseable)
fn to_float(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-float", 1, args.len()));
    }
    let v = &args[0];
    Ok(match v {
        Value::Integer { n, .. } => Value::float(*n as f64),
        Value::Float { f, .. } => Value::float(*f),
        Value::Decimal { d, .. } => Value::float((*d).try_into().unwrap_or(f64::NAN)),
        // M80: percent promotes to its fractional float value.
        Value::Percent { value, .. } => Value::float(*value),
        // M140: duration → total seconds as f64.
        Value::Duration { d, .. } => {
            let ns = d.num_nanoseconds().unwrap_or(0);
            Value::float(ns as f64 / 1e9)
        }
        Value::String { s, .. } => Value::float(parse_f64(s, v)?),
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!, float!, or string!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    })
}

/// `to-decimal value` — M150. Coerce to `decimal!` (rust_decimal).
/// - `integer!` → exact Decimal
/// - `float!`   → Decimal::try_from (errors on NaN/Inf)
/// - `decimal!` → identity
/// - `percent!` → fractional value as Decimal
/// - `money!`   → cents as Decimal with 2 decimal places (currency discarded)
/// - `string!`  → parse as Decimal (error on unparseable)
fn to_decimal(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-decimal", 1, args.len()));
    }
    let v = &args[0];
    let span = v.span_or_default();
    Ok(match v {
        Value::Integer { n, .. } => Value::decimal(rust_decimal::Decimal::from(*n)),
        Value::Float { f, .. } => rust_decimal::Decimal::try_from(*f)
            .map(Value::decimal)
            .map_err(|_| EvalError::Native {
                message: format!(
                    "to-decimal: cannot convert {f:?} (NaN/Inf not representable as decimal!)"
                ),
                span,
            })?,
        Value::Decimal { d, .. } => Value::decimal(*d),
        Value::Percent { value, .. } => rust_decimal::Decimal::try_from(*value)
            .map(Value::decimal)
            .map_err(|_| EvalError::Native {
                message: "to-decimal: percent value out of decimal! range".into(),
                span,
            })?,
        Value::Money { amount, .. } => Value::decimal(rust_decimal::Decimal::new(amount.cents, 2)),
        Value::String { s, .. } => s
            .parse::<rust_decimal::Decimal>()
            .map(Value::decimal)
            .map_err(|_| EvalError::Native {
                message: format!("to-decimal: cannot parse {:?} as decimal!", s),
                span,
            })?,
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!, float!, decimal!, percent!, money!, or string!",
                found: type_name(other),
                span,
            })
        }
    })
}

/// `to-string value` — Red's `to-string` is `form` for most types, but
/// decodes UTF-8 bytes for `binary!` (M41 — Red parity).
fn to_string(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-string", 1, args.len()));
    }
    // `to-string` of a binary! decodes UTF-8 bytes (Red behavior). Error on
    // invalid UTF-8 sequences.
    if let Value::String8 { bytes, span } = &args[0] {
        let s = std::str::from_utf8(bytes).map_err(|_| EvalError::Native {
            message: "to-string: invalid UTF-8 in binary!".into(),
            span: *span,
        })?;
        return Ok(Value::string(std::rc::Rc::from(s)));
    }
    Ok(Value::string(std::rc::Rc::from(
        form_to_string(&args[0]).as_str(),
    )))
}

/// `to-block value` — coerce to `block!`.
/// - `string!` → `load` the string and wrap the resulting body series in a
///   `Block` (errors propagate with the string's span).
/// - word-family → single-element block `[word]`
/// - `block!` → identity
/// - `paren!` → same contents as a `block!`
fn to_block(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-block", 1, args.len()));
    }
    let v = &args[0];
    Ok(match v {
        Value::Block { series, .. } => Value::block(series.clone()),
        Value::Paren { series, .. } => Value::block(series.clone()),
        Value::String { s, .. } => {
            let toks = lexer::lex(s).map_err(|e| native_err(v, e.to_string()))?;
            let series = load(&toks).map_err(|e| native_err(v, e.to_string()))?;
            Value::block(series)
        }
        w if word_sym(w).is_some() => {
            let sym = word_sym(w).unwrap();
            Value::block(Series::new(vec![Value::word(sym.as_str())]))
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "string!, block!, paren!, or word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    })
}

/// Build an unbound word value of the requested kind from a symbol.
fn mk_word(kind: WordKind, sym: Symbol) -> Value {
    match kind {
        WordKind::Word => Value::Word {
            sym,
            binding: red_core::value::Binding::Unbound,
            span: Span::default(),
        },
        WordKind::SetWord => Value::SetWord {
            sym,
            binding: red_core::value::Binding::Unbound,
            span: Span::default(),
        },
        WordKind::GetWord => Value::GetWord {
            sym,
            binding: red_core::value::Binding::Unbound,
            span: Span::default(),
        },
        WordKind::LitWord => Value::LitWord {
            sym,
            span: Span::default(),
        },
    }
}

#[derive(Clone, Copy)]
enum WordKind {
    Word,
    SetWord,
    GetWord,
    LitWord,
}

/// Core of `to-word`/`to-set-word`/`to-get-word`/`to-lit-word`: derive a
/// word of the given kind from a `string!` (body becomes the symbol) or a
/// word-family value (symbol carried over). Other types → TypeError.
fn to_word_kind(args: &[Value], native: &str, kind: WordKind) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, native, 1, args.len()));
    }
    let v = &args[0];
    let sym = match v {
        Value::String { s, .. } => Symbol::new(s.as_ref()),
        w if word_sym(w).is_some() => word_sym(w).unwrap(),
        other => {
            return Err(EvalError::TypeError {
                expected: "string! or word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    Ok(mk_word(kind, sym))
}

fn to_word(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    to_word_kind(args, "to-word", WordKind::Word)
}

fn to_set_word(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    to_word_kind(args, "to-set-word", WordKind::SetWord)
}

fn to_get_word(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    to_word_kind(args, "to-get-word", WordKind::GetWord)
}

fn to_lit_word(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    to_word_kind(args, "to-lit-word", WordKind::LitWord)
}

/// `to-logic value` — Red truthiness: only `false` and `none` are falsy.
fn to_logic(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-logic", 1, args.len()));
    }
    Ok(Value::Logic(truthy(&args[0])))
}

/// `to-file value` — coerce to `file!`. From `string!` (body becomes path),
/// `file!` (id), `url!` (body becomes path), or word-family (name becomes
/// path).
fn to_file(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-file", 1, args.len()));
    }
    let v = &args[0];
    let path: Rc<str> = match v {
        Value::String { s, .. } => s.clone(),
        Value::File { path, .. } => path.clone(),
        Value::Url { url, .. } => url.clone(),
        w if word_sym(w).is_some() => {
            let sym = word_sym(w).unwrap();
            Rc::from(sym.as_str())
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "string!, file!, url!, or word!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    Ok(Value::file(path))
}

/// `to-url value` — coerce to `url!`. From `string!` (body becomes url),
/// `url!` (id), `file!` (body becomes url).
fn to_url(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-url", 1, args.len()));
    }
    let v = &args[0];
    let url: Rc<str> = match v {
        Value::String { s, .. } => s.clone(),
        Value::Url { url, .. } => url.clone(),
        Value::File { path, .. } => path.clone(),
        w if word_sym(w).is_some() => {
            let sym = word_sym(w).unwrap();
            Rc::from(sym.as_str())
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "string!, file!, url!, or word!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    Ok(Value::url(url))
}

// ---------------------------------------------------------------------------
// make <type> <spec>
// ---------------------------------------------------------------------------

/// `make <type> <spec>` — constructor. Dispatches on the type word (accepted
/// as `word!` or `lit-word!`; the trailing `!` is optional but conventional).
///
/// Supported types:
/// - `integer!` — from `float!` (truncates), `integer!` (id), `logic!` (0/1),
///   `none!` (0), `string!` (parse i64).
/// - `float!` — from `integer!`, `float!` (id), `string!` (parse f64).
/// - `string!` — from `integer! n` → `""` (capacity hint, matches Red);
///   `string!` (id); `block!`/`paren!` → `form`; word-family → `form`.
/// - `block!` — from `integer! n` → `[]` (capacity hint); `block!` (id);
///   `paren!` (as block); `string!` (load).
/// - `function!`/`function` — original packed-block `[[spec][body]]` form
///   (delegates to `func_native`).
fn make_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "make", 2, args.len()));
    }
    let type_str = type_name_operand(&args[0])?;
    let spec = &args[1];
    let t = type_str.as_str();
    Ok(match t {
        "integer!" | "integer" => make_integer(spec)?,
        "float!" | "float" => make_float(spec)?,
        // M150: decimal! — route through to-decimal.
        "decimal!" | "decimal" => {
            to_decimal(std::slice::from_ref(spec), &RefineArgs::empty(), env)?
        }
        "percent!" | "percent" => make_percent(spec)?,
        "money!" | "money" => make_money(spec)?,
        "issue!" | "issue" => make_issue(spec)?,
        "email!" | "email" => make_email(spec)?,
        "tag!" | "tag" => make_tag(spec)?,
        "string!" | "string" => make_string(spec)?,
        "block!" | "block" => make_block(spec)?,
        "file!" | "file" => make_file(spec)?,
        "url!" | "url" => make_url(spec)?,
        "char!" | "char" => make_char(spec)?,
        "binary!" | "binary" => make_binary(spec)?,
        "pair!" | "pair" => make_pair(spec)?,
        "tuple!" | "tuple" => make_tuple(spec)?,
        "date!" | "date" => make_date(spec)?,
        "duration!" | "duration" => make_duration(spec)?,
        "error!" | "error" => return make_error(spec),
        "object!" | "object" => return crate::object::make_object(spec, env),
        "module!" | "module" => return crate::module::make_module(spec, env),
        "map!" | "map" => return crate::map::make_map(spec, env),
        "hash!" | "hash" => return crate::hash::make_hash(spec, env),
        "vector!" | "vector" => return crate::vector::make_vector(spec, env),
        "image!" | "image" => return crate::image::make_image(spec, env),
        "bitset!" | "bitset" => return crate::bitset::make_bitset(spec, env),
        "typeset!" | "typeset" => return crate::typeset::make_typeset(spec, env),
        "function!" | "function" => {
            // Original behavior: spec is a packed `[[spec][body]]` block.
            let packed = expect_block(&[args[0].clone(), spec.clone()], 1, "make")?;
            let packed_series = match &packed {
                Value::Block { series, .. } => series.clone(),
                _ => unreachable!("expect_block guarantees Block"),
            };
            let data = packed_series.data.borrow();
            if data.len() != 2 {
                return Err(EvalError::Native {
                    message: "make function!: packed block must be [[spec][body]]".to_string(),
                    span: spec.span_or_default(),
                });
            }
            let spec_block = match &data[0] {
                Value::Block { .. } => data[0].clone(),
                other => {
                    return Err(EvalError::TypeError {
                        expected: "block!",
                        found: type_name(other),
                        span: other.span_or_default(),
                    })
                }
            };
            let body_block = match &data[1] {
                Value::Block { .. } => data[1].clone(),
                other => {
                    return Err(EvalError::TypeError {
                        expected: "block!",
                        found: type_name(other),
                        span: other.span_or_default(),
                    })
                }
            };
            drop(data);
            return func_native(&[spec_block, body_block], &RefineArgs::empty(), env);
        }
        other => {
            return Err(EvalError::Native {
                message: format!("make: {other:?} type not supported in POC"),
                span: args[0].span_or_default(),
            })
        }
    })
}

fn make_integer(spec: &Value) -> Result<Value, EvalError> {
    Ok(match spec {
        Value::Integer { n, .. } => Value::integer(*n),
        Value::Float { f, .. } => Value::integer(*f as i64),
        Value::Logic(b) => Value::integer(if *b { 1 } else { 0 }),
        Value::None => Value::integer(0),
        Value::String { s, .. } => Value::integer(parse_i64(s, spec)?),
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!, float!, logic!, none!, or string!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    })
}

fn make_float(spec: &Value) -> Result<Value, EvalError> {
    Ok(match spec {
        Value::Integer { n, .. } => Value::float(*n as f64),
        Value::Float { f, .. } => Value::float(*f),
        Value::String { s, .. } => Value::float(parse_f64(s, spec)?),
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!, float!, or string!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    })
}

/// `make percent! spec` (M80) / `to-percent`:
/// - `integer!` → `n` as the fractional value (`50` ⇒ 5000%, mirroring Red's
///   `to-percent` which treats the input as the fraction).
/// - `float!` → `f` as the fractional value (`0.5` ⇒ 50%).
/// - `percent!` → identity.
/// - `string!` → parse `"NN%"` (the lexer divides by 100; `"50%"` ⇒ 0.5).
fn make_percent(spec: &Value) -> Result<Value, EvalError> {
    match spec {
        Value::Percent { value, .. } => Ok(Value::percent(*value)),
        Value::Integer { n, .. } => Ok(Value::percent(*n as f64)),
        Value::Float { f, .. } => Ok(Value::percent(*f)),
        Value::String { s, .. } => {
            // Lex the string; the first token should be a Percent (or a number
            // we treat as a fractional value).
            let toks = lexer::lex(s).map_err(|e| native_err(spec, e.to_string()))?;
            match toks.first() {
                Some(t) => match &t.kind {
                    lexer::TokenKind::Percent(p) => Ok(Value::percent(*p)),
                    lexer::TokenKind::Integer(n) => Ok(Value::percent(*n as f64)),
                    lexer::TokenKind::Float(f) => Ok(Value::percent(*f)),
                    _ => Err(native_err(spec, format!("cannot parse {s:?} as percent"))),
                },
                None => Err(native_err(spec, "empty string for percent")),
            }
        }
        other => Err(EvalError::TypeError {
            expected: "integer!, float!, percent!, or string!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `make money! spec` (M80) / `to-money`:
/// - `integer!` → `n` cents (USD). `make money! 1000` ⇒ `$10.00`.
/// - `float!` → rounds to nearest cent with banker's rounding (USD).
/// - `money!` → identity.
/// - `string!` → parse `"$10.00"` / `"$10.00:EUR"` via the lexer's money
///   scanner.
/// - `block!` → `[cents]` or `[cents currency]` (currency is a 3-letter word
///   or string; default USD).
fn make_money(spec: &Value) -> Result<Value, EvalError> {
    match spec {
        Value::Money { amount, .. } => Ok(Value::money(amount.cents, amount.currency.clone())),
        Value::Integer { n, .. } => Ok(Value::money(*n, "USD")),
        Value::Float { f, .. } => {
            // Banker's rounding to the nearest cent.
            let cents = (*f * 100.0).round_ties_even() as i64;
            Ok(Value::money(cents, "USD"))
        }
        Value::String { s, .. } => {
            let toks = lexer::lex(s).map_err(|e| native_err(spec, e.to_string()))?;
            match toks.first() {
                Some(t) => match &t.kind {
                    lexer::TokenKind::Money(mv) => Ok(Value::money(mv.cents, mv.currency.clone())),
                    _ => Err(native_err(spec, format!("cannot parse {s:?} as money"))),
                },
                None => Err(native_err(spec, "empty string for money")),
            }
        }
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            let mut iter = data.iter();
            let cents = match iter.next() {
                Some(Value::Integer { n, .. }) => *n,
                Some(other) => {
                    return Err(EvalError::TypeError {
                        expected: "integer! (cents)",
                        found: type_name(other),
                        span: other.span_or_default(),
                    });
                }
                None => {
                    return Err(EvalError::TypeError {
                        expected: "integer! (cents)",
                        found: "none!",
                        span: Span::default(),
                    });
                }
            };
            let currency: std::rc::Rc<str> = match iter.next() {
                None => std::rc::Rc::from("USD"),
                Some(Value::String { s, .. }) => s.clone(),
                Some(Value::Word { sym, .. }) | Some(Value::LitWord { sym, .. }) => {
                    std::rc::Rc::from(sym.as_str())
                }
                Some(other) => {
                    return Err(EvalError::TypeError {
                        expected: "string! or word! (currency)",
                        found: type_name(other),
                        span: other.span_or_default(),
                    });
                }
            };
            Ok(Value::money(cents, currency))
        }
        other => Err(EvalError::TypeError {
            expected: "integer!, float!, money!, string!, or block!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// M140: `make duration!` / `to-duration`. Forms accepted:
/// - `integer!` → duration of N seconds (`make duration! 30` → `30s`).
/// - `float!` → duration of N seconds (fractional; `make duration! 1.5` →
///   `1.5s`, which molds as `1500ms`).
/// - `string!` → parse the unit-suffix form (`"30s"`, `"1.5h"`, `"250ms"`,
///   `"-5m"`, `"1d1h"` compound). Reuses the lexer's duration scanner.
/// - `block!` → `[N]` (N seconds), `[h m s]`, `[h m s ms]`, or
///   `[d h m s ms]` (positional; missing trailing components default to 0).
///   All elements must be integers.
/// - `duration!` → identity.
fn make_duration(spec: &Value) -> Result<Value, EvalError> {
    use red_core::Duration;
    match spec {
        Value::Duration { d, .. } => Ok(Value::duration(*d)),
        Value::Integer { n, .. } => Ok(Value::duration(Duration::seconds(*n))),
        Value::Float { f, .. } => {
            let secs = *f;
            if !secs.is_finite() {
                return Err(native_err(spec, "float for duration is not finite"));
            }
            let nanos = (secs * 1e9) as i128;
            let ns = if nanos > i64::MAX as i128 {
                i64::MAX
            } else if nanos < i64::MIN as i128 {
                i64::MIN
            } else {
                nanos as i64
            };
            Ok(Value::duration(Duration::nanoseconds(ns)))
        }
        Value::String { s, .. } => {
            let toks = lexer::lex(s).map_err(|e| native_err(spec, e.to_string()))?;
            match toks.first() {
                Some(t) => match &t.kind {
                    lexer::TokenKind::Duration(d) => Ok(Value::duration(*d)),
                    _ => Err(native_err(spec, format!("cannot parse {s:?} as duration"))),
                },
                None => Err(native_err(spec, "empty string for duration")),
            }
        }
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            let ints: Result<Vec<i64>, EvalError> = data
                .iter()
                .map(|v| match v {
                    Value::Integer { n, .. } => Ok(*n),
                    other => Err(EvalError::TypeError {
                        expected: "integer!",
                        found: type_name(other),
                        span: other.span_or_default(),
                    }),
                })
                .collect();
            let v = ints?;
            let d = match v.len() {
                1 => Duration::seconds(v[0]),
                3 => Duration::hours(v[0]) + Duration::minutes(v[1]) + Duration::seconds(v[2]),
                4 => {
                    Duration::hours(v[0])
                        + Duration::minutes(v[1])
                        + Duration::seconds(v[2])
                        + Duration::milliseconds(v[3])
                }
                5 => {
                    Duration::days(v[0])
                        + Duration::hours(v[1])
                        + Duration::minutes(v[2])
                        + Duration::seconds(v[3])
                        + Duration::milliseconds(v[4])
                }
                n => {
                    return Err(native_err(
                        spec,
                        format!("duration block must have 1, 3, 4, or 5 elements (got {n})"),
                    ));
                }
            };
            Ok(Value::duration(d))
        }
        other => Err(EvalError::TypeError {
            expected: "integer!, float!, duration!, string!, or block!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `make string!`:
/// - from `integer! n` → `""` (n is a capacity hint; matches Red).
/// - from `string!` → identity
/// - from `block!`/`paren!` → `form` (space-joined)
/// - from word-family → `form` (bare name)
fn make_string(spec: &Value) -> Result<Value, EvalError> {
    Ok(match spec {
        Value::Integer { .. } => Value::string(std::rc::Rc::from("")),
        Value::String { s, .. } => Value::string(s.clone()),
        Value::Block { .. } | Value::Paren { .. } => {
            Value::string(std::rc::Rc::from(form_to_string(spec).as_str()))
        }
        w if word_sym(w).is_some() => {
            Value::string(std::rc::Rc::from(form_to_string(spec).as_str()))
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!, string!, block!, paren!, or word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    })
}

/// `make block!`:
/// - from `integer! n` → `[]` (n is a capacity hint).
/// - from `block!` → identity
/// - from `paren!` → same contents as a `block!`
/// - from `string!` → load and wrap
fn make_block(spec: &Value) -> Result<Value, EvalError> {
    Ok(match spec {
        Value::Integer { .. } => Value::block(Series::empty()),
        Value::Block { series, .. } => Value::block(series.clone()),
        Value::Paren { series, .. } => Value::block(series.clone()),
        Value::String { s, .. } => {
            let toks = lexer::lex(s).map_err(|e| native_err(spec, e.to_string()))?;
            let series = load(&toks).map_err(|e| native_err(spec, e.to_string()))?;
            Value::block(series)
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!, block!, paren!, or string!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    })
}

/// `make file!`:
/// - from `string!` → file with that path
/// - from `file!`/`url!` → identity (path carried over)
/// - from `block!`/`paren!`/word-family → `form` (joined/bare name)
fn make_file(spec: &Value) -> Result<Value, EvalError> {
    let path: Rc<str> = match spec {
        Value::String { s, .. } => s.clone(),
        Value::File { path, .. } => path.clone(),
        Value::Url { url, .. } => url.clone(),
        Value::Block { .. } | Value::Paren { .. } => Rc::from(form_to_string(spec).as_str()),
        w if word_sym(w).is_some() => Rc::from(form_to_string(spec).as_str()),
        other => {
            return Err(EvalError::TypeError {
                expected: "string!, file!, url!, block!, or word!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    Ok(Value::file(path))
}

/// `make url!`: same shape as `make file!` but produces a `url!`.
fn make_url(spec: &Value) -> Result<Value, EvalError> {
    let url: Rc<str> = match spec {
        Value::String { s, .. } => s.clone(),
        Value::Url { url, .. } => url.clone(),
        Value::File { path, .. } => path.clone(),
        Value::Block { .. } | Value::Paren { .. } => Rc::from(form_to_string(spec).as_str()),
        w if word_sym(w).is_some() => Rc::from(form_to_string(spec).as_str()),
        other => {
            return Err(EvalError::TypeError {
                expected: "string!, file!, url!, block!, or word!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    Ok(Value::url(url))
}

/// `make char! <spec>` — construct a char! value. (M38)
/// - `integer!` → codepoint (truncate to u32; error if invalid char)
/// - `string!`  → first char (error if empty/multi-char)
/// - `char!`    → identity
fn make_char(spec: &Value) -> Result<Value, EvalError> {
    match spec {
        Value::Char { c, .. } => Ok(Value::char(*c)),
        Value::Integer { n, .. } => {
            let cp = *n as u32;
            let c = char::from_u32(cp).ok_or_else(|| EvalError::Native {
                message: format!("make char!: integer {n} is not a valid codepoint"),
                span: spec.span_or_default(),
            })?;
            Ok(Value::char(c))
        }
        Value::String { s, .. } => {
            let mut chars = s.chars();
            let c = chars.next().ok_or_else(|| EvalError::Native {
                message: "make char!: empty string".into(),
                span: spec.span_or_default(),
            })?;
            if chars.next().is_some() {
                return Err(EvalError::Native {
                    message: "make char!: string has more than one char".into(),
                    span: spec.span_or_default(),
                });
            }
            Ok(Value::char(c))
        }
        other => Err(EvalError::TypeError {
            expected: "integer!, char!, or single-char string!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `make binary! <spec>` — construct a `binary!` value (M41).
/// - `binary!` → identity
/// - `string!` → UTF-8 bytes
/// - `integer!` → big-endian 8 bytes
/// - `block!`/`paren!` → bytes from each element (each int mod 256; chars →
///   their codepoint byte; strings → their UTF-8 bytes concatenated)
fn make_binary(spec: &Value) -> Result<Value, EvalError> {
    match spec {
        Value::String8 { bytes, .. } => Ok(Value::binary(bytes.clone())),
        Value::String { s, .. } => Ok(Value::binary(s.as_bytes().to_vec())),
        Value::Integer { n, .. } => Ok(Value::binary(n.to_be_bytes().to_vec())),
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            let data = series.data.borrow();
            let mut out: Vec<u8> = Vec::new();
            for v in data.iter().skip(series.index) {
                match v {
                    Value::Integer { n, .. } => out.push((*n & 0xFF) as u8),
                    Value::Char { c, .. } => {
                        let mut buf = [0u8; 4];
                        let s = c.encode_utf8(&mut buf);
                        out.extend_from_slice(s.as_bytes());
                    }
                    Value::String { s, .. } => out.extend_from_slice(s.as_bytes()),
                    Value::String8 { bytes, .. } => out.extend_from_slice(bytes),
                    other => {
                        return Err(EvalError::TypeError {
                            expected: "integer!, char!, string!, or binary!",
                            found: type_name(other),
                            span: other.span_or_default(),
                        })
                    }
                }
            }
            Ok(Value::binary(out))
        }
        other => Err(EvalError::TypeError {
            expected: "binary!, string!, integer!, or block!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `make pair! <spec>` — M44. Construct a `pair!` value.
/// - `pair!` → identity
/// - `block!` of 2 elements → `make pair! [x y]`
/// - `integer!`/`float!` → `Nx0` (single value as x, y zeroed)
fn make_pair(spec: &Value) -> Result<Value, EvalError> {
    match spec {
        Value::Pair { .. } => Ok(spec.clone()),
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            let data = series.data.borrow();
            let items: Vec<Value> = data.iter().skip(series.index).cloned().collect();
            if items.len() != 2 {
                return Err(EvalError::Native {
                    message: format!(
                        "make pair!: block must have exactly 2 elements, got {}",
                        items.len()
                    ),
                    span: spec.span_or_default(),
                });
            }
            let mut iter = items.into_iter();
            let x = iter.next().unwrap();
            let y = iter.next().unwrap();
            Ok(Value::pair(x, y))
        }
        Value::Integer { .. } | Value::Float { .. } => {
            Ok(Value::pair(spec.clone(), Value::integer(0)))
        }
        other => Err(EvalError::TypeError {
            expected: "pair!, block!, integer!, or float!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `make tuple! <spec>` — M44. Construct a `tuple!` (RGB or RGBA bytes).
/// - `tuple!` → identity
/// - `integer!` N → all-zero tuple of N components (3 or 4; else error)
/// - `block!` of 3–4 integers → tuple from those bytes (each 0–255)
fn make_tuple(spec: &Value) -> Result<Value, EvalError> {
    match spec {
        Value::Tuple { .. } => Ok(spec.clone()),
        Value::Integer { n, .. } => {
            let n = *n;
            if n != 3 && n != 4 {
                return Err(EvalError::Native {
                    message: format!(
                        "make tuple!: integer must be 3 or 4 (component count), got {n}"
                    ),
                    span: spec.span_or_default(),
                });
            }
            Ok(Value::tuple(vec![0u8; n as usize]))
        }
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            let data = series.data.borrow();
            let items: Vec<Value> = data.iter().skip(series.index).cloned().collect();
            if items.len() < 3 || items.len() > 4 {
                return Err(EvalError::Native {
                    message: format!(
                        "make tuple!: block must have 3 or 4 elements, got {}",
                        items.len()
                    ),
                    span: spec.span_or_default(),
                });
            }
            let mut bytes: Vec<u8> = Vec::with_capacity(items.len());
            for v in &items {
                let n = match v {
                    Value::Integer { n, .. } => *n,
                    Value::Char { c, .. } => *c as i64,
                    other => {
                        return Err(EvalError::TypeError {
                            expected: "integer! or char!",
                            found: type_name(other),
                            span: other.span_or_default(),
                        })
                    }
                };
                if !(0..=255).contains(&n) {
                    return Err(EvalError::Native {
                        message: format!("make tuple!: component {n} out of range 0-255"),
                        span: v.span_or_default(),
                    });
                }
                bytes.push(n as u8);
            }
            Ok(Value::tuple(bytes))
        }
        other => Err(EvalError::TypeError {
            expected: "tuple!, integer!, or block!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `make error! <value>` — M42. Two forms:
fn make_error(spec: &Value) -> Result<Value, EvalError> {
    match spec {
        Value::String { s, .. } => Ok(Value::error(s.to_string())),
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            let ev = crate::natives::parse_error_block_public(series, spec.span_or_default())?;
            Ok(Value::Error(std::rc::Rc::new(ev)))
        }
        // Allow molding an existing error through `make error! err` (id).
        Value::Error(_) => Ok(spec.clone()),
        other => Err(EvalError::TypeError {
            expected: "string! or block!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `to-error value` — M42. Coerce to `error!`. From a string → message-only;
/// from an existing `error!` → identity; from a block → parse keyword pairs.
fn to_error(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let spec = args.first().ok_or_else(|| EvalError::Arity {
        native: Symbol::new("to-error"),
        expected: 1,
        got: 0,
        span: Span::default(),
    })?;
    make_error(spec)
}

/// `to-binary value` — coerce to `binary!`. Same shape as `make_binary` but
/// a conversion (no capacity-hint form for `integer!`).
fn to_binary(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-binary", 1, args.len()));
    }
    make_binary(&args[0])
}

/// `to-pair value` — M44. Coerce to `pair!`. Same as `make pair!` (no
/// distinct conversion semantics in the POC).
fn to_pair(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-pair", 1, args.len()));
    }
    make_pair(&args[0])
}

/// `to-tuple value` — M44. Coerce to `tuple!`. Same as `make tuple!`.
fn to_tuple(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-tuple", 1, args.len()));
    }
    make_tuple(&args[0])
}

/// `to-date value` — M45. Coerce to `date!`. Same as `make date!`.
fn to_date(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-date", 1, args.len()));
    }
    make_date(&args[0])
}

/// `to-percent value` — M80. Coerce to `percent!`. Same as `make percent!`.
fn to_percent(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-percent", 1, args.len()));
    }
    make_percent(&args[0])
}

/// `to-money value` — M80. Coerce to `money!`. Same as `make money!`.
fn to_money(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-money", 1, args.len()));
    }
    make_money(&args[0])
}

/// `to-duration value` — M140. Coerce to `duration!`. Same as
/// `make duration!` for the non-block forms.
fn to_duration(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-duration", 1, args.len()));
    }
    make_duration(&args[0])
}

/// `to-issue value` — M80. Coerce to `issue!`. Same as `make issue!`.
/// - `string!` → `#<string>` (the string body becomes the issue body).
/// - `integer!` → `#<decimal>` (e.g. `1234` ⇒ `#1234`).
/// - `issue!` → identity.
/// - `block!` of integers → `#<concat of int chars>`.
/// - word-family → `#<word name>`.
fn to_issue(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-issue", 1, args.len()));
    }
    make_issue(&args[0])
}

/// `to-email value` — M80. Coerce to `email!`. Same as `make email!`.
/// - `string!` → parse the string as an email address.
/// - `email!` → identity.
/// - `block!` → `[user host]` (two strings/words joined with `@`).
fn to_email(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-email", 1, args.len()));
    }
    make_email(&args[0])
}

/// `to-tag value` — M81. Coerce to `tag!`. Same as `make tag!`.
/// - `string!` → `<string>`.
/// - `tag!` → identity.
/// - `block!` → `<joined elements>`.
/// - word-family → `<word name>`.
fn to_tag(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-tag", 1, args.len()));
    }
    make_tag(&args[0])
}

/// `make email! spec` (M80) / `to-email`:
/// - `string!` → the string as the address.
/// - `email!` → identity.
/// - `block!` → `[user host]` (two strings/words joined with `@`; host must
///   contain a dot, matching the lexer's validation).
fn make_email(spec: &Value) -> Result<Value, EvalError> {
    match spec {
        Value::Email { addr, .. } => Ok(Value::email(addr.clone())),
        Value::String { s, .. } => {
            // Validate by lexing the string — if the lexer produces an Email
            // token, use it; otherwise accept the raw string (the lexer may
            // reject some forms that make email! should accept from string).
            let toks = lexer::lex(s).map_err(|e| native_err(spec, e.to_string()))?;
            if let Some(t) = toks.first() {
                if let lexer::TokenKind::Email(addr) = &t.kind {
                    return Ok(Value::email(addr.clone()));
                }
            }
            // Fallback: accept the raw string as an email address.
            Ok(Value::email(s.clone()))
        }
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            if data.len() != 2 {
                return Err(EvalError::Native {
                    message: "make email!: block must be [user host]".into(),
                    span: spec.span_or_default(),
                });
            }
            let to_str = |v: &Value| -> Result<String, EvalError> {
                match v {
                    Value::String { s, .. } => Ok(s.to_string()),
                    w if word_sym(w).is_some() => Ok(word_sym(w).unwrap().as_str().to_string()),
                    _ => Err(EvalError::TypeError {
                        expected: "string! or word! (email component)",
                        found: type_name(v),
                        span: v.span_or_default(),
                    }),
                }
            };
            let user = to_str(&data[0])?;
            let host = to_str(&data[1])?;
            Ok(Value::email(std::rc::Rc::from(
                format!("{user}@{host}").as_str(),
            )))
        }
        other => Err(EvalError::TypeError {
            expected: "string!, email!, or block!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `make tag! spec` (M81) / `to-tag`:
/// - `string!` → `<string>` (the string body becomes the tag body).
/// - `tag!` → identity.
/// - `block!` → `<joined>` (each element stringified and space-joined, so
///   `to-tag [a b]` ⇒ `<a b>` and `to-tag [img src "x"]` ⇒ `<img src x>`).
/// - word-family → `<word name>` (so `to-tag 'b` ⇒ `<b>`).
fn make_tag(spec: &Value) -> Result<Value, EvalError> {
    match spec {
        Value::Tag { text, .. } => Ok(Value::tag(text.clone())),
        Value::String { s, .. } => Ok(Value::tag(s.clone())),
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            let mut out = String::new();
            let mut first = true;
            for v in data.iter() {
                if !first {
                    out.push(' ');
                }
                first = false;
                match v {
                    Value::String { s, .. } => out.push_str(s),
                    Value::Integer { n, .. } => out.push_str(&n.to_string()),
                    Value::Char { c, .. } => out.push(*c),
                    w if word_sym(w).is_some() => out.push_str(word_sym(w).unwrap().as_str()),
                    _ => {
                        return Err(EvalError::TypeError {
                            expected: "string!, integer!, char!, or word! (tag element)",
                            found: type_name(v),
                            span: v.span_or_default(),
                        });
                    }
                }
            }
            Ok(Value::tag(std::rc::Rc::from(out.as_str())))
        }
        w if word_sym(w).is_some() => {
            let sym = word_sym(w).unwrap();
            Ok(Value::tag(std::rc::Rc::from(sym.as_str())))
        }
        other => Err(EvalError::TypeError {
            expected: "string!, tag!, block!, or word!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `make issue! spec` (M80) / `to-issue`:
/// - `string!` → `#<string>`.
/// - `integer!` → `#<decimal>`.
/// - `issue!` → identity.
/// - `block!` of integers → `#<concat>`.
/// - word-family → `#<word name>`.
fn make_issue(spec: &Value) -> Result<Value, EvalError> {
    match spec {
        Value::Issue { s, .. } => Ok(Value::issue(s.clone())),
        Value::String { s, .. } => Ok(Value::issue(s.clone())),
        Value::Integer { n, .. } => Ok(Value::issue(std::rc::Rc::from(n.to_string().as_str()))),
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            let mut out = String::new();
            for v in data.iter() {
                match v {
                    Value::Integer { n, .. } => out.push_str(&n.to_string()),
                    Value::Char { c, .. } => out.push(*c),
                    Value::String { s, .. } => out.push_str(s),
                    _ => {
                        return Err(EvalError::TypeError {
                            expected: "integer!, char!, or string! (issue element)",
                            found: type_name(v),
                            span: v.span_or_default(),
                        });
                    }
                }
            }
            Ok(Value::issue(std::rc::Rc::from(out.as_str())))
        }
        w if word_sym(w).is_some() => {
            let sym = word_sym(w).unwrap();
            Ok(Value::issue(std::rc::Rc::from(sym.as_str())))
        }
        other => Err(EvalError::TypeError {
            expected: "string!, integer!, issue!, block!, or word!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `make date! spec` — M45. Construct a `date!` from:
/// - `string!` → parse via the lexer's date scanner (e.g. `"29-Jun-2024"`).
/// - `date!` → identity (clone).
/// - `integer!` → epoch seconds (UTC, `zone = Some(0)`).
/// - `block!` → `[year month day]` or `[year month day hour min sec]`.
fn make_date(spec: &Value) -> Result<Value, EvalError> {
    match spec {
        Value::Date { dt, .. } => Ok(Value::Date {
            dt: dt.clone(),
            span: Span::default(),
        }),
        Value::String { s, .. } => {
            // Lex the string; the first token should be a Date.
            let toks = lexer::lex(s).map_err(|e| native_err(spec, e.to_string()))?;
            match toks.first() {
                Some(t) => match &t.kind {
                    lexer::TokenKind::Date(dv) => Ok(Value::date(dv.clone())),
                    _ => Err(native_err(spec, format!("cannot parse {s:?} as date"))),
                },
                None => Err(native_err(spec, "empty string for date")),
            }
        }
        Value::Integer { n, .. } => {
            // Epoch seconds (UTC). `zone = Some(0)`.
            let dv = red_core::DateValue::from_epoch(*n)
                .ok_or_else(|| native_err(spec, format!("epoch {} out of range", n)))?;
            Ok(Value::date(dv))
        }
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            let nums: Vec<i64> = data
                .iter()
                .map(|v| match v {
                    Value::Integer { n, .. } => Ok(*n),
                    _ => Err(EvalError::TypeError {
                        expected: "integer!",
                        found: type_name(v),
                        span: v.span_or_default(),
                    }),
                })
                .collect::<Result<Vec<_>, _>>()?;
            match nums.len() {
                3 => {
                    let y = nums[0] as i32;
                    let mo = nums[1] as u32;
                    let d = nums[2] as u32;
                    let date = red_core::NaiveDate::from_ymd_opt(y, mo, d)
                        .ok_or_else(|| native_err(spec, "invalid date"))?;
                    Ok(Value::date(red_core::DateValue::date_only(date)))
                }
                6 => {
                    let y = nums[0] as i32;
                    let mo = nums[1] as u32;
                    let d = nums[2] as u32;
                    let h = nums[3] as u32;
                    let mi = nums[4] as u32;
                    let s = nums[5] as u32;
                    let date = red_core::NaiveDate::from_ymd_opt(y, mo, d)
                        .ok_or_else(|| native_err(spec, "invalid date"))?;
                    let time = red_core::NaiveTime::from_hms_opt(h, mi, s)
                        .ok_or_else(|| native_err(spec, "invalid time"))?;
                    Ok(Value::date(red_core::DateValue::from_local(
                        date.and_time(time),
                        None,
                    )))
                }
                _ => Err(native_err(
                    spec,
                    "make date!: block must be [y m d] or [y m d h mi s]",
                )),
            }
        }
        other => Err(EvalError::TypeError {
            expected: "string!, integer!, date!, or block!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

// ---------------------------------------------------------------------------
// to <type> <value>
// ---------------------------------------------------------------------------

/// `to <type> <value>` — conversion (distinct from `make`'s constructor
/// semantics). Dispatches on the type word; differs from `make` only for
/// `string!` (`to string! 5` → `"5"`, while `make string! 5` → `""`).
fn to_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "to", 2, args.len()));
    }
    let type_str = type_name_operand(&args[0])?;
    let val = &args[1];
    let t = type_str.as_str();
    let one = std::slice::from_ref(val);
    match t {
        "integer!" | "integer" => to_integer(one, &RefineArgs::empty(), env),
        "float!" | "float" => to_float(one, &RefineArgs::empty(), env),
        "decimal!" | "decimal" => to_decimal(one, &RefineArgs::empty(), env),
        "percent!" | "percent" => to_percent(one, &RefineArgs::empty(), env),
        "money!" | "money" => to_money(one, &RefineArgs::empty(), env),
        "issue!" | "issue" => to_issue(one, &RefineArgs::empty(), env),
        "email!" | "email" => to_email(one, &RefineArgs::empty(), env),
        "tag!" | "tag" => to_tag(one, &RefineArgs::empty(), env),
        "string!" | "string" => to_string(one, &RefineArgs::empty(), env),
        "block!" | "block" => to_block(one, &RefineArgs::empty(), env),
        "word!" | "word" => to_word_kind(one, "to", WordKind::Word),
        "set-word!" | "set-word" => to_word_kind(one, "to", WordKind::SetWord),
        "get-word!" | "get-word" => to_word_kind(one, "to", WordKind::GetWord),
        "lit-word!" | "lit-word" => to_word_kind(one, "to", WordKind::LitWord),
        "logic!" | "logic" => to_logic(one, &RefineArgs::empty(), env),
        "file!" | "file" => to_file(one, &RefineArgs::empty(), env),
        "url!" | "url" => to_url(one, &RefineArgs::empty(), env),
        "char!" | "char" => to_char(one, &RefineArgs::empty(), env),
        "binary!" | "binary" => to_binary(one, &RefineArgs::empty(), env),
        "pair!" | "pair" => to_pair(one, &RefineArgs::empty(), env),
        "tuple!" | "tuple" => to_tuple(one, &RefineArgs::empty(), env),
        "date!" | "date" => to_date(one, &RefineArgs::empty(), env),
        "duration!" | "duration" => to_duration(one, &RefineArgs::empty(), env),
        "error!" | "error" => to_error(one, &RefineArgs::empty(), env),
        "map!" | "map" => crate::map::to_map(one, &RefineArgs::empty(), env),
        "hash!" | "hash" => crate::hash::to_hash(one, &RefineArgs::empty(), env),
        "vector!" | "vector" => crate::vector::to_vector(one, &RefineArgs::empty(), env),
        "image!" | "image" => crate::image::to_image(one, &RefineArgs::empty(), env),
        "bitset!" | "bitset" => crate::bitset::to_bitset(one, &RefineArgs::empty(), env),
        "typeset!" | "typeset" => crate::typeset::to_typeset(one, &RefineArgs::empty(), env),
        other => Err(EvalError::Native {
            message: format!("to: {other:?} type not supported in POC"),
            span: args[0].span_or_default(),
        }),
    }
}

// ---------------------------------------------------------------------------
// form (native)
// ---------------------------------------------------------------------------

/// `form value` — returns the human-readable form as a `string!`. Mirrors
/// `to-string` (Red treats them as equivalent for the value-returning case).
fn form_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "form", 1, args.len()));
    }
    Ok(Value::string(std::rc::Rc::from(
        form_to_string(&args[0]).as_str(),
    )))
}

// ---------------------------------------------------------------------------
// mold (native) — M111
// ---------------------------------------------------------------------------

/// `mold value` / `mold/only value` — returns the Red source form as a
/// `string!`. Mirrors `printer::mold_to_string` (which `probe`/REPL already
/// use directly); this is the script-callable wrapper. `mold/only` strips
/// the outer `[...]` when the input is a `block!` (no-op on other types).
fn mold_native(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "mold", 1, args.len()));
    }
    let mut s = mold_to_string(&args[0]);
    if refs.has(&Symbol::new("only")) && matches!(args[0], Value::Block { .. }) {
        // `mold_to_string` of a Block always emits `[`...`]`; strip both ends.
        s.remove(0);
        s.pop();
    }
    Ok(Value::string(std::rc::Rc::from(s.as_str())))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

/// Register the M14 conversion natives (`to-*` family, `make`, `to`, `form`).
pub fn register_convert_natives(env: &mut Env) {
    use red_core::value::FuncDef;
    use std::rc::Rc;

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

    // to-* family (arity 1)
    reg(env, "to-integer", to_integer as NF, 1);
    reg(env, "to-float", to_float as NF, 1);
    reg(env, "to-decimal", to_decimal as NF, 1);
    reg(env, "to-percent", to_percent as NF, 1);
    reg(env, "to-money", to_money as NF, 1);
    reg(env, "to-issue", to_issue as NF, 1);
    reg(env, "to-email", to_email as NF, 1);
    reg(env, "to-tag", to_tag as NF, 1);
    reg(env, "to-string", to_string as NF, 1);
    reg(env, "to-block", to_block as NF, 1);
    reg(env, "to-word", to_word as NF, 1);
    reg(env, "to-set-word", to_set_word as NF, 1);
    reg(env, "to-get-word", to_get_word as NF, 1);
    reg(env, "to-lit-word", to_lit_word as NF, 1);
    reg(env, "to-logic", to_logic as NF, 1);
    reg(env, "to-file", to_file as NF, 1);
    reg(env, "to-url", to_url as NF, 1);
    reg(env, "to-char", to_char as NF, 1);
    reg(env, "to-binary", to_binary as NF, 1);
    reg(env, "to-pair", to_pair as NF, 1);
    reg(env, "to-tuple", to_tuple as NF, 1);
    reg(env, "to-date", to_date as NF, 1);
    reg(env, "to-duration", to_duration as NF, 1);
    reg(env, "to-error", to_error as NF, 1);

    // make / to (arity 2)
    reg(env, "make", make_native as NF, 2);
    reg(env, "to", to_native as NF, 2);

    // form (arity 1)
    reg(env, "form", form_native as NF, 1);

    // mold (arity 1, with /only refinement — M111)
    env.natives.insert(
        Symbol::new("mold"),
        Rc::new(FuncDef {
            params: vec![Symbol::new("__arg0")],
            refinements: vec![(Symbol::new("only"), vec![])],
            native: Some(mold_native as NF),
            variadic: false,
            infix: false,
            ..Default::default()
        }),
    );

    // M42 error accessors (arity 1): return the field or `none`.
    reg(env, "error-type", error_type_native as NF, 1);
    reg(env, "error-code", error_code_native as NF, 1);
    reg(env, "error-args", error_args_native as NF, 1);
    reg(env, "error-near", error_near_native as NF, 1);
}

/// `error-type err` → the `type` word (as a `LitWord`), or `none`.
fn error_type_native(args: &[Value], _r: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "error-type", 1, args.len()));
    }
    match &args[0] {
        Value::Error(ev) => match &ev.kind {
            Some(sym) => Ok(Value::LitWord {
                sym: sym.clone(),
                span: Span::default(),
            }),
            None => Ok(Value::None),
        },
        other => Err(EvalError::TypeError {
            expected: "error!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `error-code err` → the numeric code as an `integer!`, or `none`.
fn error_code_native(args: &[Value], _r: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "error-code", 1, args.len()));
    }
    match &args[0] {
        Value::Error(ev) => match ev.code {
            Some(n) => Ok(Value::integer(n)),
            None => Ok(Value::None),
        },
        other => Err(EvalError::TypeError {
            expected: "error!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `error-args err` → the `args` block, or empty block.
fn error_args_native(args: &[Value], _r: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "error-args", 1, args.len()));
    }
    match &args[0] {
        Value::Error(ev) => Ok(Value::Block {
            series: Series::new(ev.args.clone()),
            span: Span::default(),
        }),
        other => Err(EvalError::TypeError {
            expected: "error!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `error-near err` → the `near` value, or `none`.
fn error_near_native(args: &[Value], _r: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "error-near", 1, args.len()));
    }
    match &args[0] {
        Value::Error(ev) => match &ev.near {
            Some(v) => Ok(v.clone()),
            None => Ok(Value::None),
        },
        other => Err(EvalError::TypeError {
            expected: "error!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
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
        let val = eval(&block, &mut env).map_err(|e| e.to_string())?;
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    fn run_capture(src: &str) -> Vec<u8> {
        run_capture_val(src).unwrap().1
    }

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    use crate::interp::eval;

    // --- to-integer ---

    #[test]
    fn to_integer_from_float_truncates() {
        assert_eq!(mold_to_string(&val("to-integer 3.7")), "3");
        assert_eq!(mold_to_string(&val("to-integer -2.9")), "-2");
    }

    #[test]
    fn to_integer_from_string() {
        assert_eq!(mold_to_string(&val("to-integer \"42\"")), "42");
        assert_eq!(mold_to_string(&val("to-integer \"-7\"")), "-7");
    }

    #[test]
    fn to_integer_from_logic() {
        assert_eq!(mold_to_string(&val("to-integer true")), "1");
        assert_eq!(mold_to_string(&val("to-integer false")), "0");
    }

    #[test]
    fn to_integer_from_none() {
        assert_eq!(mold_to_string(&val("to-integer none")), "0");
    }

    #[test]
    fn to_integer_unparseable_string_errors() {
        assert!(run_capture_val("to-integer \"abc\"").is_err());
    }

    #[test]
    fn to_integer_identity() {
        assert_eq!(mold_to_string(&val("to-integer 5")), "5");
    }

    // --- to-float ---

    #[test]
    fn to_float_from_integer() {
        assert_eq!(mold_to_string(&val("to-float 5")), "5.0");
    }

    #[test]
    fn to_float_from_string() {
        assert_eq!(mold_to_string(&val("to-float \"3.14\"")), "3.14");
    }

    #[test]
    fn to_float_identity() {
        assert_eq!(mold_to_string(&val("to-float 2.5")), "2.5");
    }

    #[test]
    fn to_float_unparseable_string_errors() {
        assert!(run_capture_val("to-float \"xyz\"").is_err());
    }

    // --- to-string (== form) ---

    #[test]
    fn to_string_from_integer() {
        assert_eq!(mold_to_string(&val("to-string 42")), "\"42\"");
    }

    #[test]
    fn to_string_from_block_is_form() {
        // Space-joined, no brackets, no inner quotes.
        assert_eq!(mold_to_string(&val("to-string [1 2 3]")), "\"1 2 3\"");
    }

    #[test]
    fn to_string_from_word_is_bare_name() {
        assert_eq!(mold_to_string(&val("to-string 'foo")), "\"foo\"");
    }

    #[test]
    fn to_string_from_string_is_identity() {
        assert_eq!(mold_to_string(&val("to-string \"hi\"")), "\"hi\"");
    }

    #[test]
    fn to_string_from_logic() {
        assert_eq!(mold_to_string(&val("to-string true")), "\"true\"");
    }

    // --- to-block ---

    #[test]
    fn to_block_from_string_loads() {
        assert_eq!(mold_to_string(&val("to-block \"1 2 3\"")), "[1 2 3]");
    }

    #[test]
    fn to_block_from_word() {
        assert_eq!(mold_to_string(&val("to-block 'foo")), "[foo]");
    }

    #[test]
    fn to_block_identity() {
        assert_eq!(mold_to_string(&val("to-block [1 2]")), "[1 2]");
    }

    // --- to-word family ---

    #[test]
    fn to_word_from_string() {
        assert_eq!(mold_to_string(&val("to-word \"abc\"")), "abc");
    }

    #[test]
    fn to_word_from_word() {
        assert_eq!(mold_to_string(&val("to-word 'foo")), "foo");
    }

    #[test]
    fn to_set_word_from_string() {
        assert_eq!(mold_to_string(&val("to-set-word \"x\"")), "x:");
    }

    #[test]
    fn to_get_word_from_string() {
        assert_eq!(mold_to_string(&val("to-get-word \"x\"")), ":x");
    }

    #[test]
    fn to_lit_word_from_string() {
        assert_eq!(mold_to_string(&val("to-lit-word \"x\"")), "'x");
    }

    #[test]
    fn to_word_from_integer_type_errors() {
        assert!(run_capture_val("to-word 5").is_err());
    }

    // --- to-logic ---

    #[test]
    fn to_logic_truthiness() {
        // Red: only false and none are falsy.
        assert_eq!(mold_to_string(&val("to-logic 0")), "true");
        assert_eq!(mold_to_string(&val("to-logic false")), "false");
        assert_eq!(mold_to_string(&val("to-logic none")), "false");
        assert_eq!(mold_to_string(&val("to-logic \"\"")), "true");
        assert_eq!(mold_to_string(&val("to-logic 1")), "true");
    }

    // --- to-file / to-url (M20) ---

    #[test]
    fn to_file_from_string() {
        assert_eq!(
            mold_to_string(&val("to-file \"foo/bar.txt\"")),
            "%foo/bar.txt"
        );
    }

    #[test]
    fn to_file_from_url() {
        assert_eq!(mold_to_string(&val("to-url \"http://x/y\"")), "http://x/y");
    }

    #[test]
    fn to_file_from_word() {
        assert_eq!(mold_to_string(&val("to-file 'foo")), "%foo");
    }

    #[test]
    fn to_file_identity() {
        assert_eq!(mold_to_string(&val("to-file %foo/bar.txt")), "%foo/bar.txt");
    }

    #[test]
    fn make_file_from_string() {
        assert_eq!(mold_to_string(&val("make file! \"a/b.txt\"")), "%a/b.txt");
    }

    #[test]
    fn make_url_from_string() {
        assert_eq!(mold_to_string(&val("make url! \"https://x\"")), "https://x");
    }

    #[test]
    fn to_type_dispatcher_for_file_url() {
        assert_eq!(mold_to_string(&val("to file! \"p.txt\"")), "%p.txt");
        assert_eq!(mold_to_string(&val("to url! \"ftp://h\"")), "ftp://h");
    }

    // --- make ---

    #[test]
    fn make_integer_from_float() {
        assert_eq!(mold_to_string(&val("make integer! 3.5")), "3");
    }

    #[test]
    fn make_integer_from_string() {
        assert_eq!(mold_to_string(&val("make integer! \"42\"")), "42");
    }

    #[test]
    fn make_string_from_integer_is_empty() {
        // Red: the integer is a capacity hint, not a fill length.
        assert_eq!(mold_to_string(&val("make string! 5")), "\"\"");
    }

    #[test]
    fn make_string_from_block_forms() {
        assert_eq!(mold_to_string(&val("make string! [1 2 3]")), "\"1 2 3\"");
    }

    #[test]
    fn make_block_from_integer_is_empty() {
        assert_eq!(mold_to_string(&val("make block! 3")), "[]");
    }

    #[test]
    fn make_block_from_string_loads() {
        assert_eq!(mold_to_string(&val("make block! \"1 2 3\"")), "[1 2 3]");
    }

    #[test]
    fn make_function_regression() {
        // The original `make function! [[spec][body]]` form must still work.
        let v = val("make function! [[a][a + 1]]");
        assert!(matches!(v, Value::Func(_)));
        assert_eq!(
            mold_to_string(&val("f: make function! [[a][a + 1]] f 5")),
            "6"
        );
    }

    #[test]
    fn make_float_from_integer() {
        assert_eq!(mold_to_string(&val("make float! 5")), "5.0");
    }

    #[test]
    fn make_unsupported_type_errors() {
        // `make <unknown-type!> 5` errors (vector! is now supported as of M84;
        // pick a still-unsupported type).
        assert!(run_capture_val("make foobar! 5").is_err());
    }

    // --- to (alias) ---

    #[test]
    fn to_integer_via_to() {
        assert_eq!(mold_to_string(&val("to integer! 3.5")), "3");
    }

    #[test]
    fn to_string_via_to_converts_value() {
        // `to string! 5` renders the value (unlike `make string! 5` → "").
        assert_eq!(mold_to_string(&val("to string! 5")), "\"5\"");
    }

    #[test]
    fn to_block_via_to() {
        assert_eq!(mold_to_string(&val("to block! \"1 2\"")), "[1 2]");
    }

    #[test]
    fn to_word_via_to() {
        assert_eq!(mold_to_string(&val("to word! \"abc\"")), "abc");
    }

    // --- form (native) ---

    #[test]
    fn form_block_returns_string() {
        assert_eq!(mold_to_string(&val("form [1 2 3]")), "\"1 2 3\"");
    }

    #[test]
    fn form_string_is_raw() {
        // form of a string returns the same string (no quoting).
        assert_eq!(mold_to_string(&val("form \"hi\"")), "\"hi\"");
    }

    #[test]
    fn form_word_is_bare_name() {
        assert_eq!(mold_to_string(&val("form 'foo")), "\"foo\"");
    }

    #[test]
    fn form_integer() {
        assert_eq!(mold_to_string(&val("form 42")), "\"42\"");
    }

    // --- mold (native) — M111 ---

    #[test]
    fn mold_native_basic() {
        // `print mold x` should produce the same text `probe x` would (minus
        // the `== ` prefix). `mold 5` → string `"5"`; printing it → `5`.
        assert_eq!(s(&run_capture("print mold 5")), "5\n");
        assert_eq!(s(&run_capture("print mold \"hi\"")), "\"hi\"\n");
        assert_eq!(s(&run_capture("print mold [1 2]")), "[1 2]\n");
        assert_eq!(s(&run_capture("print mold 'word")), "'word\n");
        assert_eq!(s(&run_capture("print mold none")), "none\n");
    }

    #[test]
    fn mold_native_only() {
        // `mold/only` strips outer brackets on block!; no-op otherwise.
        assert_eq!(s(&run_capture("print mold/only [1 2 3]")), "1 2 3\n");
        assert_eq!(s(&run_capture("print mold/only []")), "\n");
        assert_eq!(s(&run_capture("print mold/only 5")), "5\n");
        assert_eq!(s(&run_capture("print mold/only \"hi\"")), "\"hi\"\n");
    }

    #[test]
    fn mold_native_object_matches_printer() {
        // `mold make object! [x: 1]` must equal the direct Rust
        // `mold_to_string` call (the native just wraps it). Both produce a
        // string! whose contents are the object's source form; print it.
        let obj_val = val("make object! [x: 1]");
        let direct = mold_to_string(&obj_val);
        let via_native = s(&run_capture("print mold make object! [x: 1]"));
        // `print` adds a trailing newline; trim for the content comparison.
        assert_eq!(via_native.trim_end(), direct);
    }

    // --- to-binary / make binary! (M41) ---

    #[test]
    fn binary_literal_molds_to_hex_uppercase() {
        // Source round-trip: lex → parse → mold.
        assert_eq!(mold_to_string(&val("#{48656C6C6F}")), "#{48656C6C6F}");
        assert_eq!(mold_to_string(&val("#{00FF}")), "#{00FF}");
    }

    #[test]
    fn to_binary_from_string() {
        // `"hi"` UTF-8 bytes = 0x68 0x69.
        assert_eq!(mold_to_string(&val("to-binary \"hi\"")), "#{6869}");
    }

    #[test]
    fn to_binary_from_binary_is_identity() {
        assert_eq!(mold_to_string(&val("to-binary #{0102}")), "#{0102}");
    }

    #[test]
    fn to_binary_from_integer_is_big_endian_8() {
        // `1` → big-endian i64 = eight bytes, only the last non-zero.
        let v = val("to-binary 1");
        match v {
            Value::String8 { bytes, .. } => {
                assert_eq!(bytes.len(), 8);
                assert_eq!(bytes[7], 1);
            }
            other => panic!("expected String8, got {other:?}"),
        }
    }

    #[test]
    fn make_binary_from_block_of_ints() {
        // Each int mod 256.
        assert_eq!(mold_to_string(&val("make binary! [65 66 67]")), "#{414243}");
        assert_eq!(
            mold_to_string(&val("make binary! [0 255 256]")),
            "#{00FF00}"
        );
    }

    #[test]
    fn make_binary_from_block_with_chars_and_strings() {
        // `#"A"` → 0x41; `"xy"` → 0x78 0x79.
        assert_eq!(
            mold_to_string(&val(r#"make binary! [#"A" "xy"]"#)),
            "#{417879}"
        );
    }

    #[test]
    fn to_string_from_binary_decodes_utf8() {
        // `#{6869}` decodes to `"hi"`.
        assert_eq!(mold_to_string(&val("to-string #{6869}")), "\"hi\"");
    }

    #[test]
    fn to_string_from_binary_invalid_utf8_errors() {
        // Lone 0xFF is not valid UTF-8.
        let r = run_capture_val("to-string #{FF}");
        assert!(r.is_err(), "expected UTF-8 decode error");
    }

    #[test]
    fn to_binary_via_to_native() {
        assert_eq!(mold_to_string(&val("to binary! \"hi\"")), "#{6869}");
    }

    #[test]
    fn make_binary_via_make_native() {
        assert_eq!(mold_to_string(&val("make binary! \"hi\"")), "#{6869}");
    }

    // --- round-trip ---
    // Each `to-*` round-trips through `to-string` for in-range inputs (and
    // back through the matching constructor where defined). `to-logic` is
    // intentionally lossy (many values map to `true`) so it's excluded.
    #[test]
    fn to_integer_round_trip() {
        assert_eq!(mold_to_string(&val("to-integer to-string 42")), "42");
        assert_eq!(mold_to_string(&val("to-integer to-string -7")), "-7");
    }

    #[test]
    fn to_float_round_trip() {
        assert_eq!(mold_to_string(&val("to-float to-string 3.14")), "3.14");
        assert_eq!(mold_to_string(&val("to-float to-string 5")), "5.0");
    }

    #[test]
    fn to_string_round_trip() {
        // integer -> string -> integer
        assert_eq!(mold_to_string(&val("to-integer to-string 99")), "99");
        // string -> string is identity
        assert_eq!(mold_to_string(&val("to-string to-string \"hi\"")), "\"hi\"");
    }

    #[test]
    fn to_word_round_trip() {
        // word -> string -> word
        assert_eq!(mold_to_string(&val("to-word to-string 'abc")), "abc");
        // string -> set-word -> string (bare name)
        assert_eq!(mold_to_string(&val("to-set-word to-string 'x")), "x:");
        assert_eq!(mold_to_string(&val("to-get-word to-string 'y")), ":y");
        assert_eq!(mold_to_string(&val("to-lit-word to-string 'z")), "'z");
    }

    #[test]
    fn to_block_round_trip() {
        // block -> string -> block
        assert_eq!(
            mold_to_string(&val("to-block to-string [1 2 3]")),
            "[1 2 3]"
        );
    }

    // --- end-to-end via print ---
    // Note: the M6 `print` native molds every argument uniformly (including
    // strings, which appear quoted — a documented POC divergence from Red's
    // `form`-based printing). So `print form [...]` yields the molded string,
    // i.e. quoted. These tests assert that quoted form to stay consistent
    // with the rest of the test suite.

    #[test]
    fn print_form_block() {
        let out = run_capture("print form [1 2 3]");
        assert_eq!(s(&out), "1 2 3\n");
    }

    #[test]
    fn print_to_string_block() {
        let out = run_capture("print to-string [a b c]");
        assert_eq!(s(&out), "a b c\n");
    }

    // -------------------------------------------------------------------------
    // M135: coverage-focused tests for to-char / make_char / make_money /
    // make_pair / make_tuple / error-* accessors / to-native exotic-type arms.
    // The existing suite exercised only the happy paths; these drive the error
    // branches and the arms that delegate to other crates.
    // -------------------------------------------------------------------------

    // --- to-char (native): arity + error paths via the interpreter ---

    #[test]
    fn to_char_from_integer() {
        let v = val("to-char 65");
        match v {
            Value::Char { c, .. } => assert_eq!(c, 'A'),
            other => panic!("expected char!, got {:?}", other),
        }
    }

    #[test]
    fn to_char_from_float() {
        let v = val("to-char 66.0");
        match v {
            Value::Char { c, .. } => assert_eq!(c, 'B'),
            other => panic!("expected char!, got {:?}", other),
        }
    }

    #[test]
    fn to_char_from_logic() {
        match val("to-char true") {
            Value::Char { c, .. } => assert_eq!(c, '\u{1}'),
            other => panic!("expected char!, got {:?}", other),
        }
        match val("to-char false") {
            Value::Char { c, .. } => assert_eq!(c, '\u{0}'),
            other => panic!("expected char!, got {:?}", other),
        }
    }

    #[test]
    fn to_char_identity() {
        match val("to-char #\"X\"") {
            Value::Char { c, .. } => assert_eq!(c, 'X'),
            other => panic!("expected char!, got {:?}", other),
        }
    }

    #[test]
    fn to_char_invalid_integer_codepoint_errors() {
        let err = run_capture_val("to-char 1114112").unwrap_err();
        assert!(err.contains("not a valid codepoint"), "got: {err}");
    }

    #[test]
    fn to_char_empty_string_errors() {
        let err = run_capture_val("to-char \"\"").unwrap_err();
        assert!(err.contains("empty string"), "got: {err}");
    }

    #[test]
    fn to_char_multi_char_string_errors() {
        let err = run_capture_val("to-char \"ab\"").unwrap_err();
        assert!(err.contains("more than one char"), "got: {err}");
    }

    #[test]
    fn to_char_wrong_type_errors() {
        let err = run_capture_val("to-char [1 2]").unwrap_err();
        assert!(err.contains("block!"), "got: {err}");
    }

    // --- make_char (internal helper): direct calls for each input-type arm ---

    #[test]
    fn make_char_identity() {
        let v = make_char(&Value::char('Z')).unwrap();
        match v {
            Value::Char { c, .. } => assert_eq!(c, 'Z'),
            other => panic!("expected char!, got {:?}", other),
        }
    }

    #[test]
    fn make_char_from_integer() {
        let v = make_char(&Value::integer(97)).unwrap();
        match v {
            Value::Char { c, .. } => assert_eq!(c, 'a'),
            other => panic!("expected char!, got {:?}", other),
        }
    }

    #[test]
    fn make_char_invalid_codepoint_errors() {
        assert!(make_char(&Value::integer(0x110000)).is_err());
    }

    #[test]
    fn make_char_from_single_char_string() {
        let v = make_char(&Value::string("Q")).unwrap();
        match v {
            Value::Char { c, .. } => assert_eq!(c, 'Q'),
            other => panic!("expected char!, got {:?}", other),
        }
    }

    #[test]
    fn make_char_empty_string_errors() {
        assert!(make_char(&Value::string("")).is_err());
    }

    #[test]
    fn make_char_multi_char_string_errors() {
        assert!(make_char(&Value::string("xy")).is_err());
    }

    #[test]
    fn make_char_wrong_type_errors() {
        assert!(make_char(&Value::None).is_err());
    }

    // --- make_money (block form) ---

    #[test]
    fn make_money_from_block_cents_only() {
        let v = make_money(&Value::block(Series::new(vec![Value::integer(500)]))).unwrap();
        match v {
            Value::Money { amount, .. } => {
                assert_eq!(amount.cents, 500);
                assert_eq!(amount.currency.as_ref(), "USD");
            }
            other => panic!("expected money!, got {:?}", other),
        }
    }

    #[test]
    fn make_money_from_block_cents_and_currency_word() {
        let v = make_money(&Value::block(Series::new(vec![
            Value::integer(1000),
            Value::lit_word("EUR"),
        ])))
        .unwrap();
        match v {
            Value::Money { amount, .. } => {
                assert_eq!(amount.cents, 1000);
                assert_eq!(amount.currency.as_ref(), "EUR");
            }
            other => panic!("expected money!, got {:?}", other),
        }
    }

    #[test]
    fn make_money_from_block_cents_and_currency_string() {
        let v = make_money(&Value::block(Series::new(vec![
            Value::integer(750),
            Value::string("JPY"),
        ])))
        .unwrap();
        match v {
            Value::Money { amount, .. } => assert_eq!(amount.currency.as_ref(), "JPY"),
            other => panic!("expected money!, got {:?}", other),
        }
    }

    #[test]
    fn make_money_from_block_wrong_cents_type_errors() {
        let err = make_money(&Value::block(Series::new(vec![Value::string("not-cents")]))).unwrap_err();
        match err {
            EvalError::TypeError { expected, .. } => {
                assert!(expected.contains("integer!"), "got: {expected}");
            }
            other => panic!("expected TypeError, got {:?}", other),
        }
    }

    #[test]
    fn make_money_from_block_wrong_currency_type_errors() {
        let err = make_money(&Value::block(Series::new(vec![
            Value::integer(100),
            Value::integer(999),
        ])))
        .unwrap_err();
        match err {
            EvalError::TypeError { expected, .. } => {
                assert!(expected.contains("string! or word!"), "got: {expected}");
            }
            other => panic!("expected TypeError, got {:?}", other),
        }
    }

    #[test]
    fn make_money_from_empty_block_errors() {
        let err = make_money(&Value::block(Series::empty())).unwrap_err();
        match err {
            EvalError::TypeError { expected, .. } => {
                assert!(expected.contains("integer!"), "got: {expected}");
            }
            other => panic!("expected TypeError, got {:?}", other),
        }
    }

    #[test]
    fn make_money_wrong_spec_type_errors() {
        let err = make_money(&Value::None).unwrap_err();
        match err {
            EvalError::TypeError { expected, .. } => assert!(expected.contains("integer!")),
            other => panic!("expected TypeError, got {:?}", other),
        }
    }

    // --- make_pair (block form error paths) ---

    #[test]
    fn make_pair_from_wrong_length_block_errors() {
        let err = make_pair(&Value::block(Series::new(vec![Value::integer(1)]))).unwrap_err();
        match err {
            EvalError::Native { message, .. } => {
                assert!(message.contains("exactly 2 elements"), "got: {message}");
            }
            other => panic!("expected Native error, got {:?}", other),
        }
    }

    #[test]
    fn make_pair_wrong_spec_type_errors() {
        let err = make_pair(&Value::None).unwrap_err();
        match err {
            EvalError::TypeError { expected, .. } => assert!(expected.contains("pair!")),
            other => panic!("expected TypeError, got {:?}", other),
        }
    }

    // --- make_tuple (error paths) ---

    #[test]
    fn make_tuple_integer_wrong_count_errors() {
        let err = make_tuple(&Value::integer(5)).unwrap_err();
        match err {
            EvalError::Native { message, .. } => {
                assert!(message.contains("3 or 4"), "got: {message}");
            }
            other => panic!("expected Native error, got {:?}", other),
        }
    }

    #[test]
    fn make_tuple_block_wrong_length_errors() {
        let err = make_tuple(&Value::block(Series::new(vec![
            Value::integer(1),
            Value::integer(2),
        ])))
        .unwrap_err();
        match err {
            EvalError::Native { message, .. } => {
                assert!(message.contains("3 or 4 elements"), "got: {message}");
            }
            other => panic!("expected Native error, got {:?}", other),
        }
    }

    #[test]
    fn make_tuple_block_wrong_element_type_errors() {
        let err = make_tuple(&Value::block(Series::new(vec![
            Value::integer(1),
            Value::integer(2),
            Value::string("not-int"),
        ])))
        .unwrap_err();
        match err {
            EvalError::TypeError { expected, .. } => assert!(expected.contains("integer!")),
            other => panic!("expected TypeError, got {:?}", other),
        }
    }

    #[test]
    fn make_tuple_component_out_of_range_errors() {
        let err = make_tuple(&Value::block(Series::new(vec![
            Value::integer(1),
            Value::integer(2),
            Value::integer(300),
        ])))
        .unwrap_err();
        match err {
            EvalError::Native { message, .. } => {
                assert!(message.contains("out of range"), "got: {message}");
            }
            other => panic!("expected Native error, got {:?}", other),
        }
    }

    #[test]
    fn make_tuple_wrong_spec_type_errors() {
        let err = make_tuple(&Value::None).unwrap_err();
        match err {
            EvalError::TypeError { expected, .. } => assert!(expected.contains("tuple!")),
            other => panic!("expected TypeError, got {:?}", other),
        }
    }

    // --- error-* accessors: happy paths + None-field + TypeError arms ---

    #[test]
    fn error_type_returns_litword_when_present() {
        let v = val("error-type make error! [type: 'math message: \"boom\"]");
        match v {
            Value::LitWord { sym, .. } => assert_eq!(sym.as_str(), "math"),
            other => panic!("expected lit-word!, got {:?}", other),
        }
    }

    #[test]
    fn error_type_returns_none_when_absent() {
        let v = val("error-type make error! \"msg\"");
        assert!(matches!(v, Value::None), "expected none, got {v:?}");
    }

    #[test]
    fn error_type_wrong_arg_type_errors() {
        let err = run_capture_val("error-type 42").unwrap_err();
        assert!(err.contains("error!"), "got: {err}");
    }

    #[test]
    fn error_code_returns_integer_when_present() {
        let v = val("error-code make error! [code: 42 message: \"x\"]");
        match v {
            Value::Integer { n, .. } => assert_eq!(n, 42),
            other => panic!("expected integer!, got {:?}", other),
        }
    }

    #[test]
    fn error_code_returns_none_when_absent() {
        let v = val("error-code make error! \"msg\"");
        assert!(matches!(v, Value::None), "expected none, got {v:?}");
    }

    #[test]
    fn error_code_wrong_arg_type_errors() {
        let err = run_capture_val("error-code 42").unwrap_err();
        assert!(err.contains("error!"), "got: {err}");
    }

    #[test]
    fn error_args_returns_block_when_present() {
        let v = val("error-args make error! [args: [1 2 3] message: \"x\"]");
        match v {
            Value::Block { series, .. } => {
                assert_eq!(series.data.borrow().len(), 3);
            }
            other => panic!("expected block!, got {:?}", other),
        }
    }

    #[test]
    fn error_args_returns_empty_block_when_absent() {
        let v = val("error-args make error! \"msg\"");
        match v {
            Value::Block { series, .. } => {
                assert!(series.data.borrow().is_empty());
            }
            other => panic!("expected block!, got {:?}", other),
        }
    }

    #[test]
    fn error_args_wrong_arg_type_errors() {
        let err = run_capture_val("error-args 42").unwrap_err();
        assert!(err.contains("error!"), "got: {err}");
    }

    #[test]
    fn error_near_returns_value_when_present() {
        let v = val("error-near make error! [near: [1 2] message: \"x\"]");
        assert!(matches!(v, Value::Block { .. }), "got: {v:?}");
    }

    #[test]
    fn error_near_returns_none_when_absent() {
        let v = val("error-near make error! \"msg\"");
        assert!(matches!(v, Value::None), "expected none, got {v:?}");
    }

    #[test]
    fn error_near_wrong_arg_type_errors() {
        let err = run_capture_val("error-near 42").unwrap_err();
        assert!(err.contains("error!"), "got: {err}");
    }

    // --- to-native dispatcher: exotic-type arms that delegate to other crates ---

    #[test]
    fn to_map_from_block() {
        let v = val("to map! [a 1 b 2]");
        assert!(matches!(v, Value::Map { .. }), "got: {v:?}");
    }

    #[test]
    fn to_hash_from_block() {
        let v = val("to hash! [a 1 b 2]");
        assert!(matches!(v, Value::Hash { .. }), "got: {v:?}");
    }

    #[test]
    fn to_vector_from_block() {
        let v = val("to vector! [1 2 3]");
        assert!(matches!(v, Value::Vector { .. }), "got: {v:?}");
    }

    #[test]
    fn to_image_from_block() {
        let v = val("to image! [1 1 [0 0 0 0]]");
        assert!(matches!(v, Value::Image { .. }), "got: {v:?}");
    }

    #[test]
    fn to_bitset_from_string() {
        let v = val("to bitset! \"ABC\"");
        assert!(matches!(v, Value::Bitset { .. }), "got: {v:?}");
    }

    #[test]
    fn to_typeset_from_block() {
        let v = val("to typeset! [integer!]");
        assert!(matches!(v, Value::Typeset { .. }), "got: {v:?}");
    }

    #[test]
    fn to_native_unknown_type_errors() {
        let err = run_capture_val("to bogus! 5").unwrap_err();
        assert!(err.contains("not supported"), "got: {err}");
    }
}
