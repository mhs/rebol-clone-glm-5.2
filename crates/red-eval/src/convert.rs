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

use red_core::form_to_string;
use red_core::lexer;
use red_core::parser::load;
use red_core::value::{Series, Span, Symbol, Value};
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
        Value::Logic(b) => Value::integer(if *b { 1 } else { 0 }),
        Value::None => Value::integer(0),
        Value::String { s, .. } => Value::integer(parse_i64(s, v)?),
        Value::Char { c, .. } => Value::integer(*c as i64),
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

/// `to-string value` — Red's `to-string` is `form`: human-readable, no
/// quoting/escaping. Returns a `string!`.
fn to_string(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-string", 1, args.len()));
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
        "string!" | "string" => make_string(spec)?,
        "block!" | "block" => make_block(spec)?,
        "file!" | "file" => make_file(spec)?,
        "url!" | "url" => make_url(spec)?,
        "char!" | "char" => make_char(spec)?,
        "object!" | "object" => return crate::object::make_object(spec, env),
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

    // make / to (arity 2)
    reg(env, "make", make_native as NF, 2);
    reg(env, "to", to_native as NF, 2);

    // form (arity 1)
    reg(env, "form", form_native as NF, 1);
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
        assert!(run_capture_val("make vector! 5").is_err());
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
        assert_eq!(s(&out), "\"1 2 3\"\n");
    }

    #[test]
    fn print_to_string_block() {
        let out = run_capture("print to-string [a b c]");
        assert_eq!(s(&out), "\"a b c\"\n");
    }
}
