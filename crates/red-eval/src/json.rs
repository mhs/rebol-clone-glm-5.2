//! M155–M156: JSON codec dialect — `to-json` (encoder) and `load-json`
//! (decoder).
//!
//! `to-json` walks a `Value` tree and produces a JSON string. `load-json`
//! parses a JSON string into Red values (`map!` for objects, `block!` for
//! arrays, scalars for leaves).
//!
//! Both natives live in this module (parallel to `codec.rs`/`strings.rs`),
//! registered via `register_json_natives` from `natives/registry.rs`.

use std::rc::Rc;

use red_core::value::{MapDef, MapKey, Series, Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::natives::{reg_refined, type_name};
use crate::NativeFn;

// ===========================================================================
// M155 — JSON encoder (`to-json`)
// ===========================================================================

/// `to-json value` / `to-json/pretty value` / `to-json/pretty value N`.
///
/// `/pretty` takes 0 or 1 args: when 0, defaults to 2-space indentation; when
/// 1 (an integer), uses that many spaces.
pub fn to_json_native(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("to-json"),
            expected: 1,
            got: 0,
            span: Span::default(),
        });
    }
    let value = &args[0];
    let span = value.span_or_default();

    // `/pretty` refinement: flag-only (arity 0), always 2-space indent.
    let (pretty, indent_width) = if refs.has(&Symbol::new("pretty")) {
        (true, 2)
    } else {
        (false, 0)
    };

    let mut out = String::new();
    encode(value, &mut out, 0, pretty, indent_width, span)?;
    Ok(Value::string(Rc::from(out.as_str())))
}

/// Recursive JSON encoder. `indent` is the current nesting depth (0 = top
/// level). `span` is the span of the originating value, used for errors.
fn encode(
    value: &Value,
    out: &mut String,
    indent: usize,
    pretty: bool,
    indent_width: usize,
    span: Span,
) -> Result<(), EvalError> {
    match value {
        Value::None => {
            out.push_str("null");
            Ok(())
        }
        Value::Unset => {
            out.push_str("null");
            Ok(())
        }
        Value::Logic(b) => {
            out.push_str(if *b { "true" } else { "false" });
            Ok(())
        }
        Value::Integer { n, .. } => {
            use std::fmt::Write;
            let _ = write!(out, "{n}");
            Ok(())
        }
        Value::Float { f, .. } => {
            encode_float(*f, out, span)
        }
        Value::Decimal { d, .. } => {
            // Decimal: Display, no `dec` suffix. Always has a `.` per
            // `mold_decimal`'s convention.
            let s = d.to_string();
            out.push_str(&s);
            if !s.contains('.') && !s.contains('e') {
                out.push_str(".0");
            }
            Ok(())
        }
        Value::Percent { value, .. } => {
            // Emit the raw fractional value (`50%` → `0.5`).
            encode_float(*value, out, span)
        }
        Value::String { s, .. } => {
            encode_string(s, out);
            Ok(())
        }
        Value::Char { c, .. } => {
            // Single-char JSON string.
            let mut tmp = String::new();
            tmp.push(*c);
            encode_string(&tmp, out);
            Ok(())
        }
        Value::Block { series, .. } => {
            encode_array(series, out, indent, pretty, indent_width, span)
        }
        Value::Paren { series, .. } => {
            // Parens evaluate to their last value; for JSON we treat them as
            // arrays (consistent with `mold` rendering them as `(...)`)
            // — but since a paren carries the same Series shape, encode as
            // an array for round-trip symmetry with `Block`.
            encode_array(series, out, indent, pretty, indent_width, span)
        }
        Value::Map(m) => {
            let m = m.borrow();
            encode_object_from_map(&m, out, indent, pretty, indent_width, span)
        }
        Value::Hash(h) => {
            let h = h.borrow();
            // Use `key_order` for deterministic insertion order.
            let keys: Vec<MapKey> = h.key_order.borrow().clone();
            let entries = h.entries.borrow();
            encode_object_from_kv(
                &keys
                    .iter()
                    .filter_map(|k| entries.get(k).map(|v| (k, v)))
                    .collect::<Vec<_>>(),
                out,
                indent,
                pretty,
                indent_width,
                span,
            )
        }
        Value::Object(obj) => {
            let obj = obj.borrow();
            let words = obj.ctx.words();
            let slots = obj.ctx.slots.borrow();
            // Collect (sym, value) pairs by cloning the values so we don't
            // hold a borrow across the recursive `encode` call.
            let pairs: Vec<(Symbol, Value)> = words
                .iter()
                .filter(|s| s.as_str() != "self")
                .filter_map(|s| {
                    obj.ctx.index_of(s).map(|idx| (s.clone(), slots[idx].borrow().clone()))
                })
                .collect();
            let field_refs: Vec<(&Symbol, &Value)> = pairs.iter().map(|(s, v)| (s, v)).collect();
            encode_object_from_fields(&field_refs, out, indent, pretty, indent_width, span)
        }
        Value::Tuple { bytes, .. } => {
            // Array of byte integers.
            encode_array_from_slice(
                &bytes.iter().map(|b| Value::integer(*b as i64)).collect::<Vec<_>>(),
                out,
                indent,
                pretty,
                indent_width,
                span,
            )
        }
        Value::Pair { x, y, .. } => {
            encode_array_from_slice(
                &[x.as_ref().clone(), y.as_ref().clone()],
                out,
                indent,
                pretty,
                indent_width,
                span,
            )
        }
        Value::Tag { text, .. } => {
            // Render as `"<...>"`.
            let mut s = String::from("<");
            s.push_str(text);
            s.push('>');
            encode_string(&s, out);
            Ok(())
        }
        Value::Issue { s, .. } => {
            encode_string(s, out);
            Ok(())
        }
        Value::Email { addr, .. } => {
            encode_string(addr, out);
            Ok(())
        }
        Value::File { path, .. } => {
            encode_string(path, out);
            Ok(())
        }
        Value::Url { url, .. } => {
            encode_string(url, out);
            Ok(())
        }
        Value::Money { amount, .. } => {
            // Money has no JSON-native type — emit as a string "$DD.CC[:CUR]".
            let mut s = String::new();
            encode_money_string(amount, &mut s);
            encode_string(&s, out);
            Ok(())
        }
        Value::Date { dt, .. } => {
            // ISO 8601 string.
            let s = encode_date_string(dt);
            encode_string(&s, out);
            Ok(())
        }
        Value::Duration { d, .. } => {
            // Seconds as a float. `chrono::Duration::num_nanoseconds`
            // returns `Option<i64>`; fall back through coarser units.
            let secs = if let Some(n) = d.num_nanoseconds() {
                n as f64 / 1_000_000_000.0
            } else if let Some(n) = d.num_microseconds() {
                n as f64 / 1_000_000.0
            } else {
                d.num_milliseconds() as f64 / 1_000.0
            };
            encode_float(secs, out, span)
        }
        Value::String8 { bytes, .. } => {
            // Binary → base64 string. Uses the `base64` crate already in
            // red-eval's deps (codec.rs pulls it). Encode with standard
            // alphabet (no URL-safe).
            use base64::Engine;
            let s = base64::engine::general_purpose::STANDARD.encode(bytes);
            encode_string(&s, out);
            Ok(())
        }
        // Unencodable types.
        Value::Func(_)
        | Value::Closure(_)
        | Value::Module(_)
        | Value::Port(_)
        | Value::Typeset(_)
        | Value::Vector(_)
        | Value::Image(_)
        | Value::Bitset(_)
        | Value::Error(_) => Err(EvalError::Native {
            message: format!(
                "to-json: cannot encode {} (no JSON representation)",
                type_name(value)
            ),
            span,
        }),
        // Word-family: render as JSON string of the word name.
        Value::Word { sym, .. }
        | Value::SetWord { sym, .. }
        | Value::GetWord { sym, .. }
        | Value::LitWord { sym, .. } => {
            encode_string(sym.as_str(), out);
            Ok(())
        }
        // Paths/Refinement: render as a string of the molded path.
        Value::Path { .. }
        | Value::GetPath { .. }
        | Value::LitPath { .. }
        | Value::SetPath { .. } => {
            let s = red_core::printer::mold_to_string(value);
            encode_string(&s, out);
            Ok(())
        }
        Value::Refinement { sym, .. } => {
            let mut s = String::from("/");
            s.push_str(sym.as_str());
            encode_string(&s, out);
            Ok(())
        }
    }
}

/// Encode an `f64` as a JSON number. Rejects `NaN`/`Inf` (JSON has no
/// representation for them).
fn encode_float(f: f64, out: &mut String, span: Span) -> Result<(), EvalError> {
    use std::fmt::Write;
    if f.is_nan() || f.is_infinite() {
        return Err(EvalError::Native {
            message: format!("to-json: cannot encode {} (NaN/Inf has no JSON representation)", f),
            span,
        });
    }
    let _ = write!(out, "{f:?}");
    let s = out.split('/').last().unwrap_or("");
    // Ensure there's a `.` so the result parses as a JSON number (not an
    // integer). `{:?}` on `5.0` produces `5.0`; on `1e20` produces
    // `1e20`. If the formatted string has no `.` and no `e`, append `.0`.
    if !s.contains('.') && !s.contains('e') && !s.contains("inf") {
        out.push_str(".0");
    }
    Ok(())
}

/// JSON-escape a string and write it to `out` (wrapped in `"`). Handles the
/// standard JSON escapes plus `\u00XX` for control chars < 0x20. Non-ASCII
/// characters (> 0x7E) are emitted raw (JSON allows UTF-8).
fn encode_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Render a `MoneyValue` as a `$DD.CC[:CUR]` string (no JSON quoting — caller
/// does that). Mirrors `mold_money` but without the printer's Rc handling.
fn encode_money_string(m: &red_core::value::MoneyValue, out: &mut String) {
    use std::fmt::Write;
    let negative = m.cents < 0;
    let abs = m.cents.unsigned_abs();
    let dollars = abs / 100;
    let cents = abs % 100;
    if negative {
        out.push('-');
    }
    out.push('$');
    let _ = write!(out, "{}.{:02}", dollars, cents);
    if m.currency.as_ref() != "USD" {
        out.push(':');
        out.push_str(&m.currency);
    }
}

/// Render a `DateValue` as an ISO 8601 string. Date-only produces
/// `"YYYY-MM-DD"`; date+time produces `"YYYY-MM-DDTHH:MM:SS"` with an
/// optional zone suffix `+HH:MM`/`-HH:MM`/`Z`.
fn encode_date_string(d: &red_core::value::DateValue) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = write!(s, "{}", d.dt.date().format("%Y-%m-%d"));
    if d.has_time() {
        let _ = write!(s, "T{}", d.dt.time().format("%H:%M:%S"));
    }
    if let Some(zone) = d.zone {
        if zone == 0 {
            s.push('Z');
        } else {
            let sign = if zone < 0 { '-' } else { '+' };
            let m = zone.unsigned_abs();
            let _ = write!(s, "{sign}{:02}:{:02}", m / 60, m % 60);
        }
    }
    s
}

/// Encode a `Vec<(k, v)>` of (MapKey, Value) pairs as a JSON object.
fn encode_object_from_kv(
    pairs: &[(&MapKey, &Value)],
    out: &mut String,
    indent: usize,
    pretty: bool,
    indent_width: usize,
    span: Span,
) -> Result<(), EvalError> {
    if pairs.is_empty() {
        out.push_str("{}");
        return Ok(());
    }
    out.push('{');
    let next_indent = indent + 1;
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if pretty {
            out.push('\n');
            push_indent(out, next_indent, indent_width);
        }
        let key_str = map_key_to_json_string(k);
        encode_string(&key_str, out);
        out.push(':');
        if pretty {
            out.push(' ');
        }
        encode(v, out, next_indent, pretty, indent_width, span)?;
    }
    if pretty {
        out.push('\n');
        push_indent(out, indent, indent_width);
    }
    out.push('}');
    Ok(())
}

/// Encode an object's fields (symbol → value) as a JSON object. The fields
/// are `(Symbol, Value)` references.
fn encode_object_from_fields(
    fields: &[(&Symbol, &Value)],
    out: &mut String,
    indent: usize,
    pretty: bool,
    indent_width: usize,
    span: Span,
) -> Result<(), EvalError> {
    if fields.is_empty() {
        out.push_str("{}");
        return Ok(());
    }
    out.push('{');
    let next_indent = indent + 1;
    for (i, (sym, v)) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if pretty {
            out.push('\n');
            push_indent(out, next_indent, indent_width);
        }
        encode_string(sym.as_str(), out);
        out.push(':');
        if pretty {
            out.push(' ');
        }
        encode(v, out, next_indent, pretty, indent_width, span)?;
    }
    if pretty {
        out.push('\n');
        push_indent(out, indent, indent_width);
    }
    out.push('}');
    Ok(())
}

/// Encode a `MapDef` as a JSON object by walking its `IndexMap` in insertion
/// order.
fn encode_object_from_map(
    m: &MapDef,
    out: &mut String,
    indent: usize,
    pretty: bool,
    indent_width: usize,
    span: Span,
) -> Result<(), EvalError> {
    let entries = m.entries.borrow();
    let pairs: Vec<(&MapKey, &Value)> = entries.iter().map(|(k, v)| (k, v)).collect();
    encode_object_from_kv(&pairs, out, indent, pretty, indent_width, span)
}

/// Encode a slice of `Value`s as a JSON array.
fn encode_array_from_slice(
    elems: &[Value],
    out: &mut String,
    indent: usize,
    pretty: bool,
    indent_width: usize,
    span: Span,
) -> Result<(), EvalError> {
    if elems.is_empty() {
        out.push_str("[]");
        return Ok(());
    }
    out.push('[');
    let next_indent = indent + 1;
    for (i, v) in elems.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if pretty {
            out.push('\n');
            push_indent(out, next_indent, indent_width);
        }
        encode(v, out, next_indent, pretty, indent_width, span)?;
    }
    if pretty {
        out.push('\n');
        push_indent(out, indent, indent_width);
    }
    out.push(']');
    Ok(())
}

/// Encode a `Series` (block) as a JSON array by walking from cursor to tail.
fn encode_array(
    series: &Series,
    out: &mut String,
    indent: usize,
    pretty: bool,
    indent_width: usize,
    span: Span,
) -> Result<(), EvalError> {
    let data = series.data.borrow();
    let elems: Vec<Value> = data.iter().skip(series.index).cloned().collect();
    drop(data);
    encode_array_from_slice(&elems, out, indent, pretty, indent_width, span)
}

/// Write `n` levels of indentation, each `indent_width` spaces.
fn push_indent(out: &mut String, n: usize, indent_width: usize) {
    for _ in 0..(n * indent_width) {
        out.push(' ');
    }
}

/// Convert a `MapKey` to a JSON string key (the string that goes inside the
/// `"..."` of a JSON object key).
fn map_key_to_json_string(k: &MapKey) -> String {
    match k {
        MapKey::Sym(s) => s.as_str().to_string(),
        MapKey::Int(n) => n.to_string(),
        MapKey::Str(s) => (*s).to_string(),
        MapKey::Char(c) => c.to_string(),
        MapKey::Bool(b) => if *b { "true".into() } else { "false".into() },
        MapKey::None => "null".into(),
    }
}

// ===========================================================================
// M156 — JSON decoder (`load-json`)
// ===========================================================================

const MAX_JSON_DEPTH: usize = 256;

/// `load-json "..."` / `load-json "..." /only`.
///
/// Decodes a JSON string into Red values. Objects → `map!`, arrays → `block!`,
/// `null` → `none`, booleans → `logic!`, numbers → `integer!` or `float!`,
/// strings → `string!`. The `/only` refinement returns the value as-is
/// (without wrapping a scalar in a block); without `/only`, a top-level
/// scalar is still returned directly (this refinement is reserved for
/// future consistency and currently behaves identically).
pub fn load_json_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("load-json"),
            expected: 1,
            got: 0,
            span: Span::default(),
        });
    }
    let src: String = match &args[0] {
        Value::String { s, .. } => (**s).to_string(),
        Value::String8 { bytes, .. } => String::from_utf8_lossy(bytes).into_owned(),
        other => {
            return Err(EvalError::TypeError {
                expected: "string! or binary!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    let span = args[0].span_or_default();
    let mut parser = JsonParser::new(&src, span);
    parser.skip_ws();
    let v = parser.parse_value(0)?;
    parser.skip_ws();
    if !parser.at_end() {
        return Err(parser.err("trailing content after JSON value"));
    }
    Ok(v)
}

struct JsonParser<'a> {
    src: &'a [u8],
    pos: usize,
    span: Span,
}

impl<'a> JsonParser<'a> {
    fn new(src: &'a str, span: Span) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
            span,
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn err(&self, msg: &str) -> EvalError {
        EvalError::Native {
            message: format!("load-json: {msg} at byte {}", self.pos),
            span: self.span,
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, EvalError> {
        if depth > MAX_JSON_DEPTH {
            return Err(self.err("JSON nesting depth exceeded"));
        }
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(depth),
            Some(b'[') => self.parse_array(depth),
            Some(b'"') => self.parse_string().map(Value::string),
            Some(b't') => self.parse_literal("true", Value::Logic(true)),
            Some(b'f') => self.parse_literal("false", Value::Logic(false)),
            Some(b'n') => self.parse_literal("null", Value::None),
            Some(c) if c == b'-' || c == b'+' || c.is_ascii_digit() => self.parse_number(),
            Some(c) => Err(self.err(&format!("unexpected character '{}'", c as char))),
            None => Err(self.err("unexpected end of input")),
        }
    }

    fn parse_literal(&mut self, lit: &str, val: Value) -> Result<Value, EvalError> {
        let bytes = lit.as_bytes();
        if self.src.len() >= self.pos + bytes.len()
            && &self.src[self.pos..self.pos + bytes.len()] == bytes
        {
            self.pos += bytes.len();
            Ok(val)
        } else {
            Err(self.err(&format!("expected '{lit}'")))
        }
    }

    fn parse_string(&mut self) -> Result<String, EvalError> {
        // Assumes current byte is `"`.
        self.bump(); // consume opening quote
        let mut out = String::new();
        loop {
            match self.bump() {
                Some(b'"') => return Ok(out),
                Some(b'\\') => {
                    let esc = self.bump().ok_or_else(|| self.err("unterminated escape"))?;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0C}'),
                        b'u' => {
                            let c = self.parse_unicode_escape()?;
                            out.push(c);
                        }
                        _ => return Err(self.err("invalid escape sequence")),
                    }
                }
                Some(c) => {
                    // Handle UTF-8: if this is a lead byte of a multi-byte
                    // sequence, consume the right number of bytes.
                    let len = utf8_len(c);
                    if len == 1 {
                        out.push(c as char);
                    } else {
                        let start = self.pos - 1;
                        let needed = len - 1;
                        if self.src.len() < self.pos + needed {
                            return Err(self.err("unterminated UTF-8 sequence in string"));
                        }
                        let bytes = &self.src[start..start + len];
                        match std::str::from_utf8(bytes) {
                            Ok(s) => out.push_str(s),
                            Err(_) => return Err(self.err("invalid UTF-8 in string")),
                        }
                        self.pos += needed;
                    }
                }
                None => return Err(self.err("unterminated string")),
            }
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, EvalError> {
        let hi = self.parse_hex4()?;
        // Handle surrogate pairs.
        if (0xD800..=0xDBFF).contains(&hi) {
            // High surrogate — expect `\uDCXX-DEXX`.
            if self.peek() != Some(b'\\') {
                return Err(self.err("expected low surrogate after high surrogate"));
            }
            self.bump();
            if self.bump() != Some(b'u') {
                return Err(self.err("expected 'u' after backslash in surrogate pair"));
            }
            let lo = self.parse_hex4()?;
            if !(0xDC00..=0xDFFF).contains(&lo) {
                return Err(self.err("invalid low surrogate"));
            }
            let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
            char::from_u32(cp).ok_or_else(|| self.err("invalid surrogate pair"))
        } else if (0xDC00..=0xDFFF).contains(&hi) {
            Err(self.err("unexpected low surrogate without high surrogate"))
        } else {
            char::from_u32(hi).ok_or_else(|| self.err("invalid unicode codepoint"))
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, EvalError> {
        let mut v = 0u32;
        for _ in 0..4 {
            let c = self.bump().ok_or_else(|| self.err("unterminated \\u escape"))?;
            let d = match c {
                b'0'..=b'9' => (c - b'0') as u32,
                b'a'..=b'f' => (c - b'a' + 10) as u32,
                b'A'..=b'F' => (c - b'A' + 10) as u32,
                _ => return Err(self.err("non-hex digit in \\u escape")),
            };
            v = (v << 4) | d;
        }
        Ok(v)
    }

    fn parse_number(&mut self) -> Result<Value, EvalError> {
        let start = self.pos;
        // Optional sign.
        if matches!(self.peek(), Some(b'-') | Some(b'+')) {
            self.bump();
        }
        let mut is_float = false;
        // Integer part.
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.bump();
            } else {
                break;
            }
        }
        // Fractional part.
        if self.peek() == Some(b'.') {
            is_float = true;
            self.bump();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        // Exponent.
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_float = true;
            self.bump();
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.bump();
            }
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|_| self.err("invalid UTF-8 in number"))?;
        if is_float {
            text.parse::<f64>()
                .map(Value::float)
                .map_err(|_| self.err("invalid float literal"))
        } else {
            // Try i64 first; overflow → f64 (matches Red's int→float promotion).
            match text.parse::<i64>() {
                Ok(n) => Ok(Value::integer(n)),
                Err(_) => text
                    .parse::<f64>()
                    .map(Value::float)
                    .map_err(|_| self.err("invalid integer literal")),
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<Value, EvalError> {
        self.bump(); // consume `[`
        let mut elems: Vec<Value> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.bump();
            return Ok(Value::block(Series::new(elems)));
        }
        loop {
            let v = self.parse_value(depth + 1)?;
            elems.push(v);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.bump();
                    self.skip_ws();
                }
                Some(b']') => {
                    self.bump();
                    return Ok(Value::block(Series::new(elems)));
                }
                _ => return Err(self.err("expected ',' or ']' in array")),
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, EvalError> {
        self.bump(); // consume `{`
        let map = MapDef::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.bump();
            return Ok(Value::map(map));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected string key in object"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected ':' after object key"));
            }
            self.bump();
            self.skip_ws();
            let val = self.parse_value(depth + 1)?;
            map.set(MapKey::Str(Rc::from(key.as_str())), val);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.bump();
                    self.skip_ws();
                }
                Some(b'}') => {
                    self.bump();
                    return Ok(Value::map(map));
                }
                _ => return Err(self.err("expected ',' or '}' in object")),
            }
        }
    }
}

/// Return the byte-length of the UTF-8 sequence starting with lead byte `c`.
fn utf8_len(c: u8) -> usize {
    if c < 0x80 {
        1
    } else if c >> 5 == 0b110 {
        2
    } else if c >> 4 == 0b1110 {
        3
    } else if c >> 3 == 0b11110 {
        4
    } else {
        // Invalid lead byte — treat as single byte so the error surfaces.
        1
    }
}

// ===========================================================================
// Registration
// ===========================================================================

pub fn register_json_natives(env: &mut Env) {
    reg_refined(
        env,
        "to-json",
        to_json_native as NativeFn,
        1,
        &[("pretty", 0)],
    );
    reg_refined(
        env,
        "load-json",
        load_json_native as NativeFn,
        1,
        &[],
    );
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::register_json_natives;
    use crate::binding::bind_pass;
    use crate::natives::{install_constants, register_natives, type_name};
    use crate::eval;
    use red_core::context::Context;
    use red_core::parser::load_source;
    use red_core::value::{MapKey, Value};
    use red_core::{Env, EvalError};
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

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
        register_natives(&mut env);
        register_json_natives(&mut env);
        let block = Value::block(body);
        let val = match eval(&block, &mut env) {
            Ok(v) => v,
            Err(EvalError::Quit(_)) => Value::None,
            Err(e) => return Err(e.to_string()),
        };
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    fn out(src: &str) -> String {
        s(&run_capture_val(src).unwrap().1)
    }

    // --- to-json tests ---

    #[test]
    fn to_json_scalars() {
        assert_eq!(out("print to-json 42").trim_end(), "42");
        assert_eq!(out("print to-json true").trim_end(), "true");
        assert_eq!(out("print to-json false").trim_end(), "false");
        assert_eq!(out("print to-json none").trim_end(), "null");
        assert_eq!(out("print to-json 3.14").trim_end(), "3.14");
    }

    #[test]
    fn to_json_string_escapes() {
        let v = val("to-json {He said \"hi\"}");
        // The raw string contents should be the JSON: "He said \"hi\""
        match &v {
            Value::String { s, .. } => assert_eq!(s.as_ref(), "\"He said \\\"hi\\\"\""),
            other => panic!("expected string!, got {}", type_name(other)),
        }
    }

    #[test]
    fn to_json_string_newline() {
        let v = val("to-json {line1\nline2}");
        match &v {
            Value::String { s, .. } => assert_eq!(s.as_ref(), "\"line1\\nline2\""),
            other => panic!("expected string!, got {}", type_name(other)),
        }
    }

    #[test]
    fn to_json_array_compact() {
        assert_eq!(out("print to-json [1 2 3]").trim_end(), "[1,2,3]");
    }

    #[test]
    fn to_json_array_pretty() {
        let result = out("print to-json/pretty [1 2 3]");
        assert!(result.contains("[\n"), "pretty array should have newline: {result}");
        assert!(result.contains("  1"), "pretty array should indent: {result}");
    }

    #[test]
    fn to_json_empty_array() {
        assert_eq!(out("print to-json []").trim_end(), "[]");
    }

    #[test]
    fn to_json_map() {
        let result = out("print to-json make map! [name \"Ada\" age 36]");
        // Insertion-order preserving.
        assert!(result.contains("\"name\":\"Ada\"") || result.contains("\"name\": \"Ada\""),
            "map should have name field: {result}");
        assert!(result.contains("\"age\":36") || result.contains("\"age\": 36"),
            "map should have age field: {result}");
    }

    #[test]
    fn to_json_empty_map() {
        assert_eq!(out("print to-json make map! []").trim_end(), "{}");
    }

    #[test]
    fn to_json_object() {
        let result = out("print to-json make object! [x: 1 y: 2]");
        assert!(result.contains("\"x\":1") || result.contains("\"x\": 1"),
            "object should have x field: {result}");
        assert!(result.contains("\"y\":2") || result.contains("\"y\": 2"),
            "object should have y field: {result}");
    }

    #[test]
    fn to_json_nested() {
        let result = out("print to-json make object! [user: make object! [name: \"Bob\"] age: 30]");
        assert!(result.contains("\"user\":"), "nested object: {result}");
        assert!(result.contains("\"name\":\"Bob\""), "nested field: {result}");
    }

    #[test]
    fn to_json_tuple() {
        assert_eq!(out("print to-json 255.0.0").trim_end(), "[255,0,0]");
    }

    #[test]
    fn to_json_pair() {
        assert_eq!(out("print to-json 100x200").trim_end(), "[100,200]");
    }

    #[test]
    fn to_json_unencodable_func() {
        let result = run_capture_val("to-json func [x] [x]");
        assert!(result.is_err(), "func should not be encodable");
        assert!(result.unwrap_err().contains("cannot encode"));
    }

    #[test]
    fn to_json_word_as_string() {
        assert_eq!(out("print to-json 'foo").trim_end(), "\"foo\"");
    }

    // --- load-json tests ---

    #[test]
    fn load_json_scalar_int() {
        let v = val("load-json \"42\"");
        assert_eq!(mold_to_string(&v), "42");
    }

    #[test]
    fn load_json_scalar_bool() {
        let v = val("load-json \"true\"");
        assert_eq!(mold_to_string(&v), "true");
    }

    #[test]
    fn load_json_null() {
        let v = val("load-json \"null\"");
        assert_eq!(mold_to_string(&v), "none");
    }

    #[test]
    fn load_json_string_escape() {
        // Use a quoted string with Red escapes: "\"hi\\n\"" → the JSON
        // source is "hi\n" → Red string `hi` + newline.
        let v = val("load-json {\"hi\n\"}");
        match &v {
            Value::String { s, .. } => assert_eq!(s.as_ref(), "hi\n"),
            other => panic!("expected string!, got {}", type_name(other)),
        }
    }

    #[test]
    fn load_json_unicode_escape() {
        // The JSON source: "\u0041" → 'A'
        let v = val("load-json {\"\\u0041\"}");
        match &v {
            Value::String { s, .. } => assert_eq!(s.as_ref(), "A"),
            other => panic!("expected string!, got {}", type_name(other)),
        }
    }

    #[test]
    fn load_json_array() {
        let v = val("load-json \"[1,2,3]\"");
        assert_eq!(mold_to_string(&v), "[1 2 3]");
    }

    #[test]
    fn load_json_empty_array() {
        let v = val("load-json \"[]\"");
        assert_eq!(mold_to_string(&v), "[]");
    }

    #[test]
    fn load_json_object() {
        let v = val("load-json {{\"name\":\"Ada\",\"age\":36}}");
        // Should be a map! with name → "Ada", age → 36.
        match &v {
            Value::Map(m) => {
                let m = m.borrow();
                assert_eq!(m.len(), 2);
                assert_eq!(mold_to_string(&m.get(&MapKey::Str(Rc::from("name"))).unwrap()), "\"Ada\"");
                assert_eq!(mold_to_string(&m.get(&MapKey::Str(Rc::from("age"))).unwrap()), "36");
            }
            other => panic!("expected map!, got {}", type_name(other)),
        }
    }

    #[test]
    fn load_json_empty_object() {
        let v = val("load-json {{}}");
        match &v {
            Value::Map(m) => assert!(m.borrow().is_empty()),
            other => panic!("expected map!, got {}", type_name(other)),
        }
    }

    #[test]
    fn load_json_nested() {
        let v = val("load-json {{\"a\":[1,{\"b\":2}]}}");
        match &v {
            Value::Map(m) => {
                let inner = m.borrow().get(&MapKey::Str(Rc::from("a"))).unwrap();
                match &inner {
                    Value::Block { series, .. } => {
                        let data = series.data.borrow();
                        assert_eq!(mold_to_string(&data[0]), "1");
                        match &data[1] {
                            Value::Map(inner_m) => {
                                let im = inner_m.borrow();
                                assert_eq!(
                                    mold_to_string(&im.get(&MapKey::Str(Rc::from("b"))).unwrap()),
                                    "2"
                                );
                            }
                            o => panic!("expected inner map, got {}", type_name(o)),
                        }
                    }
                    o => panic!("expected block, got {}", type_name(o)),
                }
            }
            o => panic!("expected outer map, got {}", type_name(o)),
        }
    }

    #[test]
    fn load_json_float() {
        let v = val("load-json \"3.14\"");
        assert_eq!(mold_to_string(&v), "3.14");
    }

    #[test]
    fn load_json_exponent() {
        let v = val("load-json \"1e2\"");
        assert_eq!(mold_to_string(&v), "100.0");
    }

    #[test]
    fn load_json_negative_int() {
        let v = val("load-json \"-42\"");
        assert_eq!(mold_to_string(&v), "-42");
    }

    #[test]
    fn load_json_round_trip_int() {
        let v = val("load-json to-json 42");
        assert_eq!(mold_to_string(&v), "42");
    }

    #[test]
    fn load_json_round_trip_array() {
        let v = val("load-json to-json [1 2 3]");
        assert_eq!(mold_to_string(&v), "[1 2 3]");
    }

    #[test]
    fn load_json_round_trip_map() {
        let v = val("load-json to-json make map! [a 1 b 2]");
        match &v {
            Value::Map(m) => {
                let m = m.borrow();
                assert_eq!(m.len(), 2);
                assert_eq!(mold_to_string(&m.get(&MapKey::Str(Rc::from("a"))).unwrap()), "1");
                assert_eq!(mold_to_string(&m.get(&MapKey::Str(Rc::from("b"))).unwrap()), "2");
            }
            o => panic!("expected map!, got {}", type_name(o)),
        }
    }

    #[test]
    fn load_json_error_unterminated() {
        let result = run_capture_val("load-json {\"unterminated}");
        assert!(result.is_err(), "unterminated string should error");
    }

    #[test]
    fn load_json_error_trailing() {
        let result = run_capture_val("load-json {42 garbage}");
        assert!(result.is_err(), "trailing content should error");
    }

    fn mold_to_string(v: &Value) -> String {
        red_core::printer::mold_to_string(v)
    }
}
