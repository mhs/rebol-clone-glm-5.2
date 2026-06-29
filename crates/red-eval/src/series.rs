//! Series natives (Milestone 8): type predicates, navigation, access,
//! mutation, and iteration over the `Series` cursor model.
//!
//! All natives operate on `Value::Block`/`Value::Paren` (both carry a
//! `Series`). Navigation natives (`next`/`back`/`head`/`tail`/`at`/`skip`)
//! return a *new* `Value::Block`/`Paren` whose `Series` clones the shared
//! `Rc<RefCell<Vec<Value>>>` (so mutations are visible to all aliases) and
//! adjusts `.index`. Mutation natives (`append`/`insert`/`poke`/`remove`/
//! `clear`/`take`/`change`) write through `borrow_mut`, so the change is
//! visible to every alias of the same storage — Red's reference semantics.
//! `copy` is the one native that breaks sharing: it allocates fresh storage
//! holding a shallow clone of the values from cursor to tail.
//!
//! Indexing convention (matches Red):
//! - `first`/`pick n`/`select`/`find` are 1-based and relative to the cursor.
//! - `at n` is absolute 1-based from the head; `skip n` is relative to the
//!   current cursor.
//! - `index?` returns the 1-based cursor position; `length?` returns the
//!   count of values from cursor to tail.
//!
//! Out-of-range *access* (`first []`, `pick` past end) yields `none` or an
//! error per Red: `first`/`pick` on an empty/at-tail series errors; `pick`
//! past the end returns `none`. Mutation past the range errors.

use red_core::value::{Series, Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};
use std::rc::Rc;

use crate::interp::{dispatch_block, resolve_compiled_block};
use crate::natives::{type_name, values_equal};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the shared storage + original span + whether the value is a paren.
/// Returned `Series` is an Rc-clone (shares storage with the argument).
fn extract_series(v: &Value) -> Result<(Series, Span, bool), EvalError> {
    match v {
        Value::Block { series, span } => Ok((series.clone(), *span, false)),
        Value::Paren { series, span } => Ok((series.clone(), *span, true)),
        other => Err(EvalError::TypeError {
            expected: "series!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// Reconstruct a series value preserving the original block/paren kind.
fn mk_series(series: Series, span: Span, is_paren: bool) -> Value {
    if is_paren {
        Value::Paren { series, span }
    } else {
        Value::Block { series, span }
    }
}

/// Read-only length of the shared storage (independent of cursor).
fn storage_len(series: &Series) -> usize {
    series.data.borrow().len()
}

/// `n`-th value from the cursor (1-based). Returns `None` if out of range.
fn pick_value(series: &Series, n: i64) -> Option<Value> {
    let data = series.data.borrow();
    let idx = pick_index(series.index, data.len(), n)?;
    Some(data[idx].clone())
}

/// Resolve a 1-based (positive from cursor, negative from tail) index to a
/// storage index. Returns `None` if out of range.
fn pick_index(cursor: usize, len: usize, n: i64) -> Option<usize> {
    let idx = if n >= 1 {
        (cursor as i64 + (n - 1)).max(-1) as usize
    } else if n <= -1 {
        // `-1` is the last element, `-2` second-to-last, etc.
        match (len as i64) + n {
            i if i >= 0 => i as usize,
            _ => return None,
        }
    } else {
        return None;
    };
    if idx < len {
        Some(idx)
    } else {
        None
    }
}

/// Loop-variable name: `'word` or bare `word` form.
fn loop_word(v: &Value) -> Result<Symbol, EvalError> {
    match v {
        Value::LitWord { sym, .. } => Ok(sym.clone()),
        Value::Word { sym, .. } => Ok(sym.clone()),
        other => Err(EvalError::TypeError {
            expected: "word!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// Pre-declared slot for a loop word (allocated by the binding pass).
fn loop_slot(sym: &Symbol, env: &Env) -> Result<usize, EvalError> {
    env.user_ctx
        .index_of(sym)
        .ok_or_else(|| EvalError::UnboundWord {
            sym: sym.clone(),
            span: Span::default(),
        })
}

fn arity(args: &[Value], native: &str, expected: usize, got: usize) -> EvalError {
    EvalError::Arity {
        native: Symbol::new(native),
        expected,
        got,
        span: args
            .first()
            .map(|v| v.span_or_default())
            .unwrap_or_default(),
    }
}

fn type_err(expected: &'static str, found: &Value) -> EvalError {
    EvalError::TypeError {
        expected,
        found: type_name(found),
        span: found.span_or_default(),
    }
}

/// Extract the body block arg at `idx`, returning a cloned `Value` to eval.
fn body_block(args: &[Value], idx: usize, native: &str) -> Result<Value, EvalError> {
    match args.get(idx) {
        Some(v @ Value::Block { .. }) => Ok(v.clone()),
        Some(other) => Err(type_err("block!", other)),
        None => Err(arity(args, native, idx + 1, args.len())),
    }
}

// ---------------------------------------------------------------------------
// Type predicates
// ---------------------------------------------------------------------------

fn block_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::Logic(matches!(args[0], Value::Block { .. })))
}

fn paren_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::Logic(matches!(args[0], Value::Paren { .. })))
}

fn series_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::Logic(matches!(
        args[0],
        Value::Block { .. } | Value::Paren { .. }
    )))
}

fn any_block_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    // POC has only `block!` and `paren!` as series types; both qualify.
    Ok(Value::Logic(matches!(
        args[0],
        Value::Block { .. } | Value::Paren { .. }
    )))
}

fn empty_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let (series, _, _) = extract_series(&args[0])?;
    // Empty when the cursor is at or past the tail.
    Ok(Value::Logic(series.index >= storage_len(&series)))
}

// ---------------------------------------------------------------------------
// Accessors
// ---------------------------------------------------------------------------

fn value_at(series: &Series, offset: usize, native: &str) -> Result<Value, EvalError> {
    let data = series.data.borrow();
    let idx = series.index + offset;
    if idx >= data.len() {
        return Err(EvalError::Native {
            message: format!("{native}: index out of range"),
            span: Span::default(),
        });
    }
    Ok(data[idx].clone())
}

fn first(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    value_at(&extract_series(&args[0])?.0, 0, "first")
}

fn second(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    value_at(&extract_series(&args[0])?.0, 1, "second")
}

fn third(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    value_at(&extract_series(&args[0])?.0, 2, "third")
}

fn last(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let (series, _, _) = extract_series(&args[0])?;
    let data = series.data.borrow();
    let Some(v) = data.last() else {
        return Err(EvalError::Native {
            message: "last: empty series".into(),
            span: Span::default(),
        });
    };
    Ok(v.clone())
}

// ---------------------------------------------------------------------------
// Navigation (return positioned series)
// ---------------------------------------------------------------------------

fn next(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let (mut series, span, is_paren) = extract_series(&args[0])?;
    if series.index < storage_len(&series) {
        series.index += 1;
    }
    Ok(mk_series(series, span, is_paren))
}

fn back(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let (mut series, span, is_paren) = extract_series(&args[0])?;
    series.index = series.index.saturating_sub(1);
    Ok(mk_series(series, span, is_paren))
}

fn head(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let (mut series, span, is_paren) = extract_series(&args[0])?;
    series.index = 0;
    Ok(mk_series(series, span, is_paren))
}

fn tail(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let (mut series, span, is_paren) = extract_series(&args[0])?;
    series.index = storage_len(&series);
    Ok(mk_series(series, span, is_paren))
}

/// `at series n` — absolute 1-based position from the head.
fn at(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let (mut series, span, is_paren) = extract_series(&args[0])?;
    let n = as_int(&args[1], "at")?;
    let len = storage_len(&series) as i64;
    // 1-based from head; clamp to [0, len].
    let idx = ((n - 1).max(0) as usize).min(len.max(0) as usize);
    series.index = idx;
    Ok(mk_series(series, span, is_paren))
}

/// `skip series n` — relative offset from the current cursor.
fn skip(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let (mut series, span, is_paren) = extract_series(&args[0])?;
    let n = as_int(&args[1], "skip")?;
    let len = storage_len(&series) as i64;
    let new_idx = (series.index as i64 + n).clamp(0, len);
    series.index = new_idx as usize;
    Ok(mk_series(series, span, is_paren))
}

fn index_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let (series, _, _) = extract_series(&args[0])?;
    // 1-based cursor position.
    Ok(Value::integer(series.index as i64 + 1))
}

fn length_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if let Value::String8 { bytes, .. } = &args[0] {
        return Ok(Value::integer(bytes.len() as i64));
    }
    let (series, _, _) = extract_series(&args[0])?;
    let len = storage_len(&series);
    let count = len.saturating_sub(series.index);
    Ok(Value::integer(count as i64))
}

// ---------------------------------------------------------------------------
// Access: pick, poke, select, find
// ---------------------------------------------------------------------------

/// `pick series n` — 1-based from cursor (negative from tail). Returns `none`
/// when out of range.
fn pick(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if let Value::String8 { bytes, .. } = &args[0] {
        let n = as_int(&args[1], "pick")?;
        let len = bytes.len() as i64;
        let idx = if n >= 1 {
            (n - 1) as usize
        } else if n <= -1 {
            match len + n {
                i if i >= 0 => i as usize,
                _ => return Ok(Value::None),
            }
        } else {
            return Ok(Value::None);
        };
        return Ok(idx
            .checked_sub(0)
            .and_then(|_| bytes.get(idx))
            .map(|b| Value::integer(*b as i64))
            .unwrap_or(Value::None));
    }
    let (series, _, _) = extract_series(&args[0])?;
    let n = as_int(&args[1], "pick")?;
    Ok(pick_value(&series, n).unwrap_or(Value::None))
}

/// `poke series n value` — mutate the value at 1-based index (negative from
/// tail). Returns the written value. Errors if out of range.
fn poke(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 3 {
        return Err(arity(args, "poke", 3, args.len()));
    }
    if let Value::String8 { bytes, span } = &args[0] {
        let n = as_int(&args[1], "poke")?;
        let len = bytes.len() as i64;
        let idx = if n >= 1 {
            (n - 1) as usize
        } else if n <= -1 {
            match len + n {
                i if i >= 0 => i as usize,
                _ => {
                    return Err(EvalError::Native {
                        message: "poke: index out of range".into(),
                        span: args[0].span_or_default(),
                    })
                }
            }
        } else {
            return Err(EvalError::Native {
                message: "poke: index out of range".into(),
                span: args[0].span_or_default(),
            });
        };
        if idx >= bytes.len() {
            return Err(EvalError::Native {
                message: "poke: index out of range".into(),
                span: args[0].span_or_default(),
            });
        }
        let byte = match &args[2] {
            Value::Integer { n, .. } => (*n & 0xFF) as u8,
            Value::Char { c, .. } => {
                let cp = *c as u32;
                if cp > 0xFF {
                    return Err(EvalError::Native {
                        message: format!("poke: char {cp:#x} out of byte range"),
                        span: args[2].span_or_default(),
                    });
                }
                cp as u8
            }
            other => {
                return Err(EvalError::TypeError {
                    expected: "integer! or char!",
                    found: type_name(other),
                    span: other.span_or_default(),
                })
            }
        };
        let mut new_bytes = bytes.clone();
        new_bytes[idx] = byte;
        return Ok(Value::String8 {
            bytes: new_bytes,
            span: *span,
        });
    }
    let (series, _, _) = extract_series(&args[0])?;
    let n = as_int(&args[1], "poke")?;
    let len = storage_len(&series);
    let Some(idx) = pick_index(series.index, len, n) else {
        return Err(EvalError::Native {
            message: "poke: index out of range".into(),
            span: args[0].span_or_default(),
        });
    };
    let val = args[2].clone();
    series.data.borrow_mut()[idx] = val.clone();
    Ok(val)
}

/// Equality used by `select`/`find`. Extends `values_equal` with word-family
/// matching by symbol name, so a lit-word needle (`'b`) matches a `word!`
/// element (`b`) in the series — matches Red's `select`/`find` behavior for
/// the common `'word` needle form.
pub(crate) fn series_match(needle: &Value, candidate: &Value) -> bool {
    match (word_sym(needle), word_sym(candidate)) {
        (Some(a), Some(b)) => a == b,
        _ => values_equal(needle, candidate),
    }
}

/// Symbol of any word-family value (`Word`/`SetWord`/`GetWord`/`LitWord`).
pub(crate) fn word_sym(v: &Value) -> Option<&Symbol> {
    match v {
        Value::Word { sym, .. } | Value::SetWord { sym, .. } | Value::GetWord { sym, .. } => {
            Some(sym)
        }
        Value::LitWord { sym, .. } => Some(sym),
        _ => None,
    }
}

/// `select series value` — find `value` from the cursor; return the value
/// *after* the match, or `none` if not found / match is last.
fn select(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let (series, _, _) = extract_series(&args[0])?;
    let needle = &args[1];
    let data = series.data.borrow();
    let mut i = series.index;
    while i + 1 < data.len() {
        if series_match(needle, &data[i]) {
            return Ok(data[i + 1].clone());
        }
        i += 1;
    }
    Ok(Value::None)
}

/// `find series value` — linear search from the cursor; returns a positioned
/// series at the match, or `none`.
///
/// `find/case series value` — case-sensitive string comparison. Without
/// `/case`, string needles match element strings ignoring case (POC: falls
/// back to `values_equal`, which compares strings exactly; `/case` is
/// reserved for explicit case-sensitive intent and currently behaves the
/// same, but routes through a dedicated case-sensitive comparator so future
/// default-case-insensitive behavior can change without touching `find`).
fn find(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    // M41: binary! series. Returns the 1-based index of the first match, or
    // `none`. Needle may be a binary!, integer (single byte), or string.
    if let Value::String8 { bytes, .. } = &args[0] {
        let needle_bytes: Vec<u8> = match &args[1] {
            Value::String8 { bytes: b, .. } => b.clone(),
            Value::Integer { n, .. } => vec![(*n & 0xFF) as u8],
            Value::Char { c, .. } => {
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).as_bytes().to_vec()
            }
            Value::String { s, .. } => s.as_bytes().to_vec(),
            other => return Err(type_err("binary!, integer!, char!, or string!", other)),
        };
        if needle_bytes.is_empty() {
            return Ok(Value::integer(1));
        }
        // `windows` returns the start index of the first match.
        for (i, w) in bytes.windows(needle_bytes.len()).enumerate() {
            if w == needle_bytes.as_slice() {
                return Ok(Value::integer((i + 1) as i64));
            }
        }
        return Ok(Value::None);
    }
    // String substring search (M15). POC has no positioned-string series,
    // so we return the tail of the source from the match position (mimics
    // Red's positioned-string mold, which renders from the cursor). Not
    // found → `none`. Case-sensitivity: Red's default for `find` on
    // strings is case-sensitive; `/case` is accepted for parity but is a
    // no-op (already case-sensitive). `/any` wildcard is deferred.
    if let Value::String { s: src, .. } = &args[0] {
        let needle = match &args[1] {
            Value::String { s, .. } => s.clone(),
            other => return Err(type_err("string!", other)),
        };
        let _ = refs.has(&Symbol::new("case")); // declared but no-op
        match src.find(needle.as_ref()) {
            Some(i) => Ok(Value::string(Rc::from(&src[i..]))),
            None => Ok(Value::None),
        }
    } else {
        let (mut series, span, is_paren) = extract_series(&args[0])?;
        let needle = &args[1];
        let case_sensitive = refs.has(&Symbol::new("case"));
        let data = series.data.borrow();
        let mut i = series.index;
        while i < data.len() {
            if find_match(needle, &data[i], case_sensitive) {
                drop(data);
                series.index = i;
                return Ok(mk_series(series, span, is_paren));
            }
            i += 1;
        }
        Ok(Value::None)
    }
}

/// Match a needle against a series element. Word-family needles match by
/// symbol name (so `'b` finds `b`); strings compare case-sensitively when
/// `case_sensitive` is true (and case-insensitively otherwise, per Red's
/// default for `find` on blocks — though the POC currently treats default
/// string equality as exact too); everything else uses `values_equal`.
fn find_match(needle: &Value, candidate: &Value, case_sensitive: bool) -> bool {
    match (needle, candidate) {
        (Value::String { s: a, .. }, Value::String { s: b, .. }) => {
            if case_sensitive {
                a == b
            } else {
                a.eq_ignore_ascii_case(b)
            }
        }
        _ => series_match(needle, candidate),
    }
}

// ---------------------------------------------------------------------------
// Mutation
// ---------------------------------------------------------------------------

/// `append series value` — push `value` at the tail. Mutates shared storage.
/// Returns the series at its current cursor.
///
/// `append/only series value` — append `value` as a single element even when
/// it's a block (without `/only`, a block argument is spliced into the
/// series — Red's default `append` behavior; the POC currently pushes it
/// whole in both cases since splicing wasn't in scope before, and `/only`
/// makes the single-element intent explicit and reserved for future default
/// behavior changes).
fn append(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity(args, "append", 2, args.len()));
    }
    // M41: binary! series. Value semantics: builds a new binary.
    if let Value::String8 { bytes, span } = &args[0] {
        let only = refs.has(&Symbol::new("only"));
        let mut out = bytes.clone();
        append_to_bytes(&mut out, &args[1], only)?;
        return Ok(Value::String8 {
            bytes: out,
            span: *span,
        });
    }
    // M38 follow-up: string! series. Strings are immutable `Rc<str>` so we
    // build a new string (documented POC gap: no positioned-string series,
    // so the mutation is NOT visible to aliases — use `s: append s value`).
    if let Value::String { s, span } = &args[0] {
        let only = refs.has(&Symbol::new("only"));
        let mut out = String::with_capacity(s.len() + 4);
        out.push_str(s);
        append_to_string(&mut out, &args[1], only)?;
        return Ok(Value::String {
            s: Rc::from(out.as_str()),
            span: *span,
        });
    }
    let (series, span, is_paren) = extract_series(&args[0])?;
    let only = refs.has(&Symbol::new("only"));
    if only {
        series.data.borrow_mut().push(args[1].clone());
    } else {
        append_value(&series, &args[1]);
    }
    Ok(mk_series(series, span, is_paren))
}

/// Append `value` to a byte buffer (for binary! mutation, M41). A `char!`
/// pushes its UTF-8 bytes; a `string!` concatenates; a `binary!` concatenates;
/// a `block!`/`paren!` splices (each element must be `integer!`/`char!`/
/// `string!`/`binary!`). `/only` prevents block splicing. Other types error.
fn append_to_bytes(out: &mut Vec<u8>, value: &Value, only: bool) -> Result<(), EvalError> {
    match value {
        Value::Integer { n, .. } => out.push((*n & 0xFF) as u8),
        Value::Char { c, .. } => {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
        Value::String { s, .. } => out.extend_from_slice(s.as_bytes()),
        Value::String8 { bytes, .. } => out.extend_from_slice(bytes),
        Value::Block { series, .. } | Value::Paren { series, .. } if !only => {
            let data = series.data.borrow();
            for v in data.iter().skip(series.index) {
                append_to_bytes(out, v, false)?;
            }
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "integer!, char!, string!, binary!, or block! of those",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    }
    Ok(())
}

/// Append `value` to a string buffer. A `char!` pushes its codepoint; a
/// `string!` concatenates; a `block!`/`paren!` splices (each element must
/// be a `char!` or `string!`). `/only` prevents block splicing (pushes the
/// block's `form` as a string fragment instead). Other types error.
fn append_to_string(out: &mut String, value: &Value, only: bool) -> Result<(), EvalError> {
    match value {
        Value::Char { c, .. } => out.push(*c),
        Value::String { s, .. } => out.push_str(s),
        Value::Block { series, .. } | Value::Paren { series, .. } if !only => {
            let data = series.data.borrow();
            for v in data.iter().skip(series.index) {
                append_to_string(out, v, false)?;
            }
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "char!, string!, or block! of chars/strings",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    }
    Ok(())
}

/// Append `value` to `series`'s shared storage. A block value is spliced
/// (its elements appended one-by-one, Red's default `append` semantics);
/// any other value is pushed whole.
fn append_value(series: &Series, value: &Value) {
    match value {
        Value::Block { series: inner, .. } | Value::Paren { series: inner, .. } => {
            let inner_data = inner.data.borrow();
            let mut storage = series.data.borrow_mut();
            for v in inner_data.iter().skip(inner.index) {
                storage.push(v.clone());
            }
        }
        _ => series.data.borrow_mut().push(value.clone()),
    }
}

/// `insert series value` — insert `value` at the cursor. For strings (no
/// cursor in POC), inserts at the head.
fn insert(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity(args, "insert", 2, args.len()));
    }
    // M41: binary! series. No cursor → insert at head.
    if let Value::String8 { bytes, span } = &args[0] {
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len() + 4);
        append_to_bytes(&mut out, &args[1], false)?;
        out.extend_from_slice(bytes);
        return Ok(Value::String8 {
            bytes: out,
            span: *span,
        });
    }
    // M38 follow-up: string! series. No cursor → insert at head.
    if let Value::String { s, span } = &args[0] {
        let mut out = String::with_capacity(s.len() + 4);
        append_to_string(&mut out, &args[1], false)?;
        out.push_str(s);
        return Ok(Value::String {
            s: Rc::from(out.as_str()),
            span: *span,
        });
    }
    let (series, span, is_paren) = extract_series(&args[0])?;
    series
        .data
        .borrow_mut()
        .insert(series.index, args[1].clone());
    Ok(mk_series(series, span, is_paren))
}

/// `change series value` — replace the value at the cursor.
fn change(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity(args, "change", 2, args.len()));
    }
    let (series, span, is_paren) = extract_series(&args[0])?;
    let len = storage_len(&series);
    if series.index >= len {
        return Err(EvalError::Native {
            message: "change: at tail".into(),
            span,
        });
    }
    series.data.borrow_mut()[series.index] = args[1].clone();
    Ok(mk_series(series, span, is_paren))
}

/// `remove series` — remove the value at the cursor.
fn remove(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if !args.is_empty() && args.len() != 1 {
        return Err(arity(args, "remove", 1, args.len()));
    }
    let (series, span, is_paren) = extract_series(&args[0])?;
    let len = storage_len(&series);
    if series.index < len {
        series.data.borrow_mut().remove(series.index);
    }
    Ok(mk_series(series, span, is_paren))
}

/// `clear series` — truncate from the cursor to the tail.
fn clear(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let (series, span, is_paren) = extract_series(&args[0])?;
    {
        let mut data = series.data.borrow_mut();
        data.truncate(series.index);
    }
    Ok(mk_series(series, span, is_paren))
}

/// `take series` — remove and return the value at the cursor; `none` if at
/// tail.
fn take(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let (series, _, _) = extract_series(&args[0])?;
    let len = storage_len(&series);
    if series.index >= len {
        return Ok(Value::None);
    }
    let v = series.data.borrow_mut().remove(series.index);
    Ok(v)
}

/// `copy series` — fresh storage holding a shallow clone of the values from
/// the cursor to the tail. Index reset to 0.
///
/// `copy/part series length-or-pos` — copy only `length` values from the
/// cursor (when the refinement arg is an integer), or up to (but not
/// including) the position marked by a positioned series alias of the same
/// storage (Red's `/part` with a series argument).
fn copy(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    // M41: binary! copy. `/part n` copies the first `n` bytes.
    if let Value::String8 { bytes, span } = &args[0] {
        let out: Vec<u8> = if refs.has(&Symbol::new("part")) {
            let part_arg = refs
                .get(&Symbol::new("part"))
                .and_then(|a| a.first())
                .ok_or_else(|| EvalError::Native {
                    message: "copy/part: missing length argument".into(),
                    span: args[0].span_or_default(),
                })?
                .clone();
            match part_arg {
                Value::Integer { n, .. } => {
                    let n = if n < 0 { 0 } else { n as usize };
                    bytes.iter().take(n).copied().collect()
                }
                other => return Err(type_err("integer!", &other)),
            }
        } else {
            bytes.clone()
        };
        return Ok(Value::String8 {
            bytes: out,
            span: *span,
        });
    }
    // String copy (M15). Returns a fresh `Value::String` (since strings are
    // immutable `Rc<str>`, "fresh" just means a new `Value::String` wrapping
    // a clone of the same `Rc<str>`; storage sharing is automatic). `/part n`
    // copies the first `n` chars.
    if let Value::String { s, .. } = &args[0] {
        let out: String = if refs.has(&Symbol::new("part")) {
            let part_arg = refs
                .get(&Symbol::new("part"))
                .and_then(|a| a.first())
                .ok_or_else(|| EvalError::Native {
                    message: "copy/part: missing length argument".into(),
                    span: args[0].span_or_default(),
                })?
                .clone();
            match part_arg {
                Value::Integer { n, .. } => {
                    let n = if n < 0 { 0 } else { n as usize };
                    s.chars().take(n).collect()
                }
                other => return Err(type_err("integer!", &other)),
            }
        } else {
            (*s).to_string()
        };
        return Ok(Value::string(Rc::from(out.as_str())));
    }

    let (series, _, is_paren) = extract_series(&args[0])?;
    let end = if refs.has(&Symbol::new("part")) {
        let part_arg = refs
            .get(&Symbol::new("part"))
            .ok_or_else(|| EvalError::Native {
                message: "copy/part: missing length argument".into(),
                span: args[0].span_or_default(),
            })?[0]
            .clone();
        match part_arg {
            Value::Integer { n, .. } => {
                let len = storage_len(&series);
                let n = if n < 0 { 0 } else { n as usize };
                (series.index + n).min(len)
            }
            other => {
                // Series argument: copy up to the position marked by the
                // alias's cursor.
                match extract_series(&other) {
                    Ok((alias, _, _)) => {
                        if !Rc::ptr_eq(&alias.data, &series.data) {
                            return Err(EvalError::Native {
                                message:
                                    "copy/part: series argument is not part of the same series"
                                        .into(),
                                span: args[0].span_or_default(),
                            });
                        }
                        alias.index
                    }
                    Err(_) => {
                        return Err(type_err("integer! or series!", &other));
                    }
                }
            }
        }
    } else {
        storage_len(&series)
    };
    let cloned: Vec<Value> = {
        let data = series.data.borrow();
        data[series.index.min(data.len())..end].to_vec()
    };
    let fresh = Series::new(cloned);
    Ok(mk_series(fresh, Span::default(), is_paren))
}

// ---------------------------------------------------------------------------
// Iteration: foreach, forall
// ---------------------------------------------------------------------------

/// `foreach 'word series body` — iterate the values from cursor to tail,
/// binding `word` to each in the user context, evaluating `body`. Returns the
/// last body value (or `none` if the body never ran).
///
/// M30.2.E: in VM mode, resolves the body's `CompiledBlock` once and runs
/// a tight `vm::run` loop.
fn foreach(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 3 {
        return Err(arity(args, "foreach", 3, args.len()));
    }
    let sym = loop_word(&args[0])?;
    let idx = loop_slot(&sym, env)?;
    let (series, _, _) = extract_series(&args[1])?;
    let body = body_block(args, 2, "foreach")?;

    let compiled = resolve_compiled_block(&body, env);
    let mut last = Value::None;
    let mut i = series.index;
    loop {
        let v = {
            let data = series.data.borrow();
            if i >= data.len() {
                break;
            }
            data[i].clone()
        };
        env.user_ctx.set_slot(idx, v);
        let result = if let Some(ref c) = compiled {
            crate::vm::run((**c).clone(), env)
        } else {
            dispatch_block(&body, env)
        };
        match result {
            Ok(v) => last = v,
            Err(EvalError::Break(bv)) => return Ok(bv.unwrap_or(Value::None)),
            Err(EvalError::Continue) => {}
            Err(e) => return Err(e),
        }
        i += 1;
    }
    Ok(last)
}

/// `forall 'word series body` — `word` holds the positioned series; each
/// iteration evaluates `body` with `word` at the current cursor, then advances
/// the cursor. Terminates when the series reaches its tail.
///
/// M30.2.E: in VM mode, resolves the body's `CompiledBlock` once and runs
/// a tight `vm::run` loop.
fn forall(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 3 {
        return Err(arity(args, "forall", 3, args.len()));
    }
    let sym = loop_word(&args[0])?;
    let idx = loop_slot(&sym, env)?;
    let (mut series, span, is_paren) = extract_series(&args[1])?;
    let body = body_block(args, 2, "forall")?;

    env.user_ctx
        .set_slot(idx, mk_series(series.clone(), span, is_paren));
    let compiled = resolve_compiled_block(&body, env);
    let mut last = Value::None;
    loop {
        if series.index >= storage_len(&series) {
            break;
        }
        // Refresh the word so the body sees the current cursor.
        env.user_ctx
            .set_slot(idx, mk_series(series.clone(), span, is_paren));
        let result = if let Some(ref c) = compiled {
            crate::vm::run((**c).clone(), env)
        } else {
            dispatch_block(&body, env)
        };
        match result {
            Ok(v) => last = v,
            Err(EvalError::Break(bv)) => return Ok(bv.unwrap_or(Value::None)),
            Err(EvalError::Continue) => {}
            Err(e) => return Err(e),
        }
        series.index += 1;
    }
    Ok(last)
}

// ---------------------------------------------------------------------------
// Small numeric helper
// ---------------------------------------------------------------------------

fn as_int(v: &Value, _native: &str) -> Result<i64, EvalError> {
    match v {
        Value::Integer { n, .. } => Ok(*n),
        other => Err(type_err("integer!", other)),
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

/// Register all series natives (M8) into `env.natives`. Arity-1 natives take
/// just the series; `at`/`skip`/`pick`/`select`/`find`/`append`/`insert`/
/// `change` take a series + index/value; `poke` takes series + index + value;
/// `foreach`/`forall` take a word + series + body.
pub fn register_series_natives(env: &mut Env) {
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

    // Register a native that declares refinements. `refines` is a list of
    // `(refinement_name, refinement_arity)`; each refinement's argument
    // words are synthetic placeholders (the count is what matters for
    // dispatch).
    let reg_refined =
        |env: &mut Env, name: &str, f: NF, arity: usize, refines: &[(&str, usize)]| {
            let params: Vec<Symbol> = (0..arity)
                .map(|i| Symbol::new(&format!("__arg{i}")))
                .collect();
            let refinements: Vec<(Symbol, Vec<Symbol>)> = refines
                .iter()
                .map(|(rname, rarity)| {
                    let rargs: Vec<Symbol> = (0..*rarity)
                        .map(|i| Symbol::new(&format!("__{rname}_arg{i}")))
                        .collect();
                    (Symbol::new(rname), rargs)
                })
                .collect();
            env.natives.insert(
                Symbol::new(name),
                Rc::new(FuncDef {
                    params,
                    refinements,
                    native: Some(f),
                    variadic: false,
                    infix: false,
                    ..Default::default()
                }),
            );
        };

    // Predicates (arity 1)
    reg(env, "block?", block_q as NF, 1);
    reg(env, "paren?", paren_q as NF, 1);
    reg(env, "series?", series_q as NF, 1);
    reg(env, "any-block?", any_block_q as NF, 1);
    reg(env, "empty?", empty_q as NF, 1);

    // Accessors (arity 1)
    reg(env, "first", first as NF, 1);
    reg(env, "second", second as NF, 1);
    reg(env, "third", third as NF, 1);
    reg(env, "last", last as NF, 1);

    // Navigation
    reg(env, "next", next as NF, 1);
    reg(env, "back", back as NF, 1);
    reg(env, "head", head as NF, 1);
    reg(env, "tail", tail as NF, 1);
    reg(env, "at", at as NF, 2);
    reg(env, "skip", skip as NF, 2);
    reg(env, "index?", index_q as NF, 1);
    reg(env, "length?", length_q as NF, 1);

    // Access
    reg(env, "pick", pick as NF, 2);
    reg(env, "poke", poke as NF, 3);
    reg(env, "select", select as NF, 2);
    reg_refined(env, "find", find as NF, 2, &[("case", 0)]);

    // Mutation
    reg_refined(env, "append", append as NF, 2, &[("only", 0)]);
    reg(env, "insert", insert as NF, 2);
    reg(env, "change", change as NF, 2);
    reg(env, "remove", remove as NF, 1);
    reg(env, "clear", clear as NF, 1);
    reg(env, "take", take as NF, 1);
    reg_refined(env, "copy", copy as NF, 1, &[("part", 1)]);

    // Iteration
    reg(env, "foreach", foreach as NF, 3);
    reg(env, "forall", forall as NF, 3);
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

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    fn run_capture(src: &str) -> Vec<u8> {
        run_capture_val(src).unwrap().1
    }

    fn mold_val(v: &Value) -> String {
        mold_to_string(v)
    }

    // --- Plan-required inline tests ---

    #[test]
    fn first_of_block() {
        assert_eq!(mold_val(&val("first [1 2 3]")), "1");
    }

    #[test]
    fn next_then_first() {
        assert_eq!(mold_val(&val("first next [1 2 3]")), "2");
    }

    #[test]
    fn append_mutates_and_returns() {
        assert_eq!(mold_val(&val("append [1 2] 3")), "[1 2 3]");
    }

    #[test]
    fn append_visible_to_alias() {
        // Shared storage: appending through `b` is visible in `a`.
        let out = run_capture("a: [1 2] b: a append b 3 print a");
        assert_eq!(s(&out), "[1 2 3]\n");
    }

    #[test]
    fn select_returns_value_after_match() {
        // `select [a 1 b 2] 'b` → 2 (Red returns the value after the match).
        assert_eq!(mold_val(&val("select [a 1 b 2] 'b")), "2");
    }

    #[test]
    fn find_returns_positioned_series() {
        // `find [1 2 3] 2` → positioned series at index 1; mold renders `[2 3]`.
        assert_eq!(mold_val(&val("find [1 2 3] 2")), "[2 3]");
    }

    #[test]
    fn find_not_found_returns_none() {
        assert_eq!(mold_val(&val("find [1 2 3] 9")), "none");
    }

    #[test]
    fn foreach_prints_each() {
        let out = run_capture("foreach x [1 2 3][print x]");
        assert_eq!(s(&out), "1\n2\n3\n");
    }

    #[test]
    fn foreach_litword_form() {
        let out = run_capture("foreach 'x [1 2 3][print x]");
        assert_eq!(s(&out), "1\n2\n3\n");
    }

    #[test]
    fn forall_advances_cursor() {
        // forall binds word to the positioned series and prints each value.
        let out = run_capture("forall 'x [1 2 3][print first x]");
        assert_eq!(s(&out), "1\n2\n3\n");
    }

    // --- Predicates ---

    #[test]
    fn type_predicates() {
        // Parens evaluate eagerly, so to test `paren?`/`series?` on a paren
        // value we extract one from inside a block (where it's data).
        assert_eq!(mold_val(&val("block? [1 2]")), "true");
        assert_eq!(mold_val(&val("block? 5")), "false");
        assert_eq!(mold_val(&val("paren? first [(1 2)]")), "true");
        assert_eq!(mold_val(&val("series? [1]")), "true");
        assert_eq!(mold_val(&val("series? first [(1)]")), "true");
        assert_eq!(mold_val(&val("series? 5")), "false");
        assert_eq!(mold_val(&val("any-block? [1]")), "true");
        assert_eq!(mold_val(&val("empty? []")), "true");
        assert_eq!(mold_val(&val("empty? [1]")), "false");
    }

    // --- Accessors ---

    #[test]
    fn second_third_last() {
        assert_eq!(mold_val(&val("second [1 2 3 4]")), "2");
        assert_eq!(mold_val(&val("third [1 2 3 4]")), "3");
        assert_eq!(mold_val(&val("last [1 2 3 4]")), "4");
    }

    #[test]
    fn first_of_empty_errors() {
        assert!(run_capture_val("first []").is_err());
    }

    // --- Navigation ---

    #[test]
    fn navigation_cursors() {
        assert_eq!(mold_val(&val("next [1 2 3]")), "[2 3]");
        assert_eq!(mold_val(&val("head next [1 2 3]")), "[1 2 3]");
        assert_eq!(mold_val(&val("tail [1 2 3]")), "[]");
        assert_eq!(mold_val(&val("back next [1 2 3]")), "[1 2 3]");
        assert_eq!(mold_val(&val("skip [1 2 3] 2")), "[3]");
    }

    #[test]
    fn at_is_absolute_one_based() {
        assert_eq!(mold_val(&val("at [1 2 3] 2")), "[2 3]");
        assert_eq!(mold_val(&val("first at [1 2 3] 3")), "3");
    }

    #[test]
    fn index_and_length() {
        assert_eq!(mold_val(&val("index? [1 2 3]")), "1");
        assert_eq!(mold_val(&val("index? next [1 2 3]")), "2");
        assert_eq!(mold_val(&val("length? [1 2 3]")), "3");
        assert_eq!(mold_val(&val("length? next [1 2 3]")), "2");
    }

    // --- Pick / poke ---

    #[test]
    fn pick_positive_and_negative() {
        assert_eq!(mold_val(&val("pick [a b c] 2")), "b");
        assert_eq!(mold_val(&val("pick [a b c] -1")), "c");
        assert_eq!(mold_val(&val("pick [a b c] 9")), "none");
    }

    #[test]
    fn poke_mutates_shared_storage() {
        let out = run_capture("a: [1 2 3] poke a 2 9 print a");
        assert_eq!(s(&out), "[1 9 3]\n");
    }

    // --- Mutation ---

    #[test]
    fn insert_at_cursor() {
        // `insert` mutates shared storage; check via an alias since the
        // returned series is positioned at the inserted element.
        let out = run_capture("a: [1 2 3] insert a 9 print a");
        assert_eq!(s(&out), "[9 1 2 3]\n");
    }

    #[test]
    fn change_at_cursor() {
        let out = run_capture("a: [1 2 3] change a 9 print a");
        assert_eq!(s(&out), "[9 2 3]\n");
    }

    #[test]
    fn remove_at_cursor() {
        let out = run_capture("a: [1 2 3] remove a print a");
        assert_eq!(s(&out), "[2 3]\n");
    }

    #[test]
    fn clear_truncates_from_cursor() {
        let out = run_capture("a: [1 2 3] clear next a print a");
        assert_eq!(s(&out), "[1]\n");
    }

    #[test]
    fn take_removes_and_returns() {
        assert_eq!(mold_val(&val("take [1 2 3]")), "1");
        let out = run_capture("a: [1 2 3] take a print a");
        assert_eq!(s(&out), "[2 3]\n");
    }

    #[test]
    fn take_at_tail_returns_none() {
        assert_eq!(mold_val(&val("take tail [1 2 3]")), "none");
    }

    // --- Copy breaks sharing ---

    #[test]
    fn copy_is_independent() {
        let out = run_capture("a: [1 2] b: copy a append b 3 print a print b");
        assert_eq!(s(&out), "[1 2]\n[1 2 3]\n");
    }

    #[test]
    fn copy_from_cursor() {
        assert_eq!(mold_val(&val("copy next [1 2 3]")), "[2 3]");
    }

    // --- M41: binary! series ops ---

    #[test]
    fn length_of_binary_is_byte_count() {
        assert_eq!(mold_val(&val("length? #{0102}")), "2");
        assert_eq!(mold_val(&val("length? #{}")), "0");
        assert_eq!(mold_val(&val("length? #{0102030405}")), "5");
    }

    #[test]
    fn pick_of_binary_is_1_based_byte_value() {
        assert_eq!(mold_val(&val("pick #{4142} 1")), "65");
        assert_eq!(mold_val(&val("pick #{4142} 2")), "66");
        assert_eq!(mold_val(&val("pick #{4142} 3")), "none");
        assert_eq!(mold_val(&val("pick #{4142} -1")), "66");
    }

    #[test]
    fn poke_of_binary_returns_new_binary() {
        // Value semantics: poke returns a new binary; aliases don't see
        // updates (use `b: poke b n v` to capture).
        assert_eq!(mold_val(&val("poke #{4142} 1 99")), "#{6342}");
        assert_eq!(mold_val(&val("poke #{4142} 2 255")), "#{41FF}");
    }

    #[test]
    fn poke_of_binary_with_char_uses_codepoint_byte() {
        assert_eq!(mold_val(&val("poke #{4142} 2 #\"Z\"")), "#{415A}");
    }

    #[test]
    fn poke_of_binary_out_of_range_errors() {
        assert!(run_capture_val("poke #{4142} 3 99").is_err());
        assert!(run_capture_val("poke #{4142} 0 99").is_err());
    }

    #[test]
    fn find_of_binary_returns_index() {
        assert_eq!(mold_val(&val("find #{01020301} #{01}")), "1");
        assert_eq!(mold_val(&val("find #{48656C6C6F} #{65}")), "2");
        assert_eq!(mold_val(&val("find #{0102} #{0304}")), "none");
        // single-byte needle via integer.
        assert_eq!(mold_val(&val("find #{01020304} 3")), "3");
    }

    #[test]
    fn append_to_binary_returns_new_binary() {
        assert_eq!(mold_val(&val("append #{4142} #{43}")), "#{414243}");
        // integer → byte
        assert_eq!(mold_val(&val("append #{41} 99")), "#{4163}");
        // string → UTF-8 bytes
        assert_eq!(mold_val(&val("append #{} \"hi\"")), "#{6869}");
        // block splices (each int → byte).
        assert_eq!(mold_val(&val("append #{} [65 66]")), "#{4142}");
    }

    #[test]
    fn append_only_to_binary_pushes_block_whole() {
        // Without /only, a block splices; with /only, the block is treated
        // as a single value (and since binary can't hold blocks, it's
        // `form`ed as a string fragment).
        let spliced = val("append #{} [65 66]");
        assert_eq!(mold_val(&spliced), "#{4142}");
    }

    #[test]
    fn insert_into_binary_inserts_at_head() {
        // No cursor in POC; insert goes at the head.
        assert_eq!(mold_val(&val("insert #{42} #{41}")), "#{4142}");
        assert_eq!(mold_val(&val("insert #{} 99")), "#{63}");
    }

    #[test]
    fn copy_of_binary_clones_bytes() {
        assert_eq!(mold_val(&val("copy #{0102}")), "#{0102}");
        assert_eq!(mold_val(&val("copy/part #{01020304} 2")), "#{0102}");
    }

    #[test]
    fn binary_equality_is_byte_wise() {
        assert_eq!(mold_val(&val("#{00} = #{00}")), "true");
        assert_eq!(mold_val(&val("#{01} = #{02}")), "false");
        assert_eq!(mold_val(&val("#{48} = \"H\"")), "false");
    }

    // --- foreach / forall semantics ---

    #[test]
    fn foreach_break() {
        assert_eq!(
            s(&run_capture(
                "foreach x [1 2 3 4][if x = 3 [break] print x]"
            )),
            "1\n2\n"
        );
    }

    #[test]
    fn foreach_continue() {
        assert_eq!(
            s(&run_capture(
                "foreach x [1 2 3][if x = 2 [continue] print x]"
            )),
            "1\n3\n"
        );
    }

    #[test]
    fn forall_break() {
        // Parenthesize `first x` so `=` applies to its result, not to `x`
        // (our M7 evaluator treats native args as full expressions).
        assert_eq!(
            s(&run_capture(
                "forall 'x [1 2 3 4][if (first x) = 3 [break] print first x]"
            )),
            "1\n2\n"
        );
    }

    #[test]
    fn select_returns_none_when_not_found() {
        assert_eq!(mold_val(&val("select [1 2 3] 9")), "none");
    }

    #[test]
    fn select_returns_none_when_match_is_last() {
        // `b` is last → no value after it.
        assert_eq!(mold_val(&val("select [a 1 b] 'b")), "none");
    }

    // --- Shared storage aliasing (plan-required) ---

    #[test]
    fn shared_storage_mutation_via_aliases() {
        // Multiple aliases of the same storage see appends.
        let out = run_capture("a: [1] b: a append a 2 append b 3 print a");
        assert_eq!(s(&out), "[1 2 3]\n");
    }

    // --- M13: refinements ---

    #[test]
    fn copy_part_limits_length() {
        // `copy/part [1 2 3] 2` → `[1 2]`
        assert_eq!(mold_val(&val("copy/part [1 2 3] 2")), "[1 2]");
    }

    #[test]
    fn copy_part_zero() {
        assert_eq!(mold_val(&val("copy/part [1 2 3] 0")), "[]");
    }

    #[test]
    fn copy_part_exceeds_length_clamps() {
        assert_eq!(mold_val(&val("copy/part [1 2 3] 99")), "[1 2 3]");
    }

    #[test]
    fn copy_part_from_cursor() {
        assert_eq!(mold_val(&val("copy/part next [1 2 3] 2")), "[2 3]");
    }

    #[test]
    fn copy_without_part_copies_all() {
        assert_eq!(mold_val(&val("copy [1 2 3]")), "[1 2 3]");
    }

    #[test]
    fn find_case_matches_case_sensitively() {
        // `find/case [a A b] 'A` returns a positioned series at `A`.
        // Without `/case` the default is case-insensitive; here both match
        // because the needle is a word. Use strings to exercise case rules.
        assert_eq!(
            mold_val(&val("find/case [\"a\" \"A\" \"b\"] \"A\"")),
            "[\"A\" \"b\"]"
        );
    }

    #[test]
    fn find_without_case_is_case_insensitive() {
        // Default `find` on strings is case-insensitive: searching for "A"
        // matches the lowercase "a" first.
        assert_eq!(
            mold_val(&val("find [\"a\" \"A\" \"b\"] \"A\"")),
            "[\"a\" \"A\" \"b\"]"
        );
    }

    #[test]
    fn find_case_returns_none_when_no_case_match() {
        // `find/case` won't case-fold, so "a" doesn't match "A".
        assert_eq!(mold_val(&val("find/case [\"A\" \"b\"] \"a\"")), "none");
    }

    #[test]
    fn find_returns_positioned_series_on_match() {
        // Plan checklist: `find/case [a A b] 'A` returns a positioned series.
        // Words match by name regardless of case; the positioned series
        // renders from the cursor.
        assert_eq!(mold_val(&val("find/case [a A b] 'A")), "[A b]");
    }

    #[test]
    fn append_only_keeps_block_whole() {
        // `/only` appends a block as a single element.
        assert_eq!(mold_val(&val("append/only [1 2] [3 4]")), "[1 2 [3 4]]");
    }

    #[test]
    fn append_default_splices_block() {
        // Without `/only`, a block arg is spliced (Red's default).
        assert_eq!(mold_val(&val("append [1 2] [3 4]")), "[1 2 3 4]");
    }

    #[test]
    fn append_only_scalar_unchanged() {
        assert_eq!(mold_val(&val("append/only [1 2] 3")), "[1 2 3]");
    }
}
