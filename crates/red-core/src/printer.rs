//! `mold`: value → Red source text. Inverse of the parser; round-trip
//! property is `mold(parse(s)) == normalize(s)`.

use crate::value::{ObjectDef, Value};

/// Append the Red source form of `value` to `out`.
pub fn mold(value: &Value, out: &mut String) {
    match value {
        Value::None => out.push_str("none"),
        Value::Logic(true) => out.push_str("true"),
        Value::Logic(false) => out.push_str("false"),
        Value::Integer { n, .. } => {
            use std::fmt::Write;
            let _ = write!(out, "{}", n);
        }
        Value::Float { f, .. } => mold_float(*f, out),
        Value::String { s, .. } => mold_string(s, out),
        Value::String8(bytes) => {
            // POC: mold as `#{hex}` so it round-trips as a distinct literal.
            out.push_str("#{");
            for b in bytes {
                use std::fmt::Write;
                let _ = write!(out, "{:02X}", b);
            }
            out.push('}');
        }
        Value::Word { sym, .. } => out.push_str(sym.as_str()),
        Value::SetWord { sym, .. } => {
            out.push_str(sym.as_str());
            out.push(':');
        }
        Value::GetWord { sym, .. } => {
            out.push(':');
            out.push_str(sym.as_str());
        }
        Value::LitWord { sym, .. } => {
            out.push('\'');
            out.push_str(sym.as_str());
        }
        Value::Block { series, .. } => {
            out.push('[');
            let data = series.data.borrow();
            // Red molds a positioned series from its cursor to the tail, so
            // `mold next [1 2 3]` renders `[2 3]`. Parsed blocks always start
            // at index 0, so this only affects series produced by navigation
            // natives (`next`/`skip`/`find`/etc.).
            for (n, v) in data.iter().enumerate().skip(series.index) {
                if n > series.index {
                    out.push(' ');
                }
                mold(v, out);
            }
            out.push(']');
        }
        Value::Paren { series, .. } => {
            out.push('(');
            let data = series.data.borrow();
            for (n, v) in data.iter().enumerate().skip(series.index) {
                if n > series.index {
                    out.push(' ');
                }
                mold(v, out);
            }
            out.push(')');
        }
        Value::Func(_) => out.push_str("#[function]"),
        Value::Error(err) => {
            // Mold as `make error! "..."` — reparseable shape (the `make`
            // native constructs an error value from a string). The message
            // is quoted/escaped via the standard string mold.
            out.push_str("make error! ");
            mold_string(&err.message, out);
        }
        Value::Path { parts, .. } => mold_path_parts(parts, None, None, out),
        Value::GetPath { parts, .. } => mold_path_parts(parts, Some(':'), None, out),
        Value::LitPath { parts, .. } => mold_path_parts(parts, Some('\''), None, out),
        Value::SetPath { parts, .. } => mold_path_parts(parts, None, Some(':'), out),
        Value::Refinement { sym, .. } => {
            out.push('/');
            out.push_str(sym.as_str());
        }
        Value::File { path, .. } => mold_file(path, out),
        Value::Url { url, .. } => out.push_str(url),
        Value::Object(obj) => mold_object(&obj.borrow(), out),
    }
}

/// Convenience: return the mold as an owned `String`.
pub fn mold_to_string(value: &Value) -> String {
    let mut out = String::new();
    mold(value, &mut out);
    out
}

/// `form`: human-readable rendering, distinct from `mold` (which is
/// reparseable). Differences from `mold`:
/// - `String` renders its raw contents (no surrounding quotes, no escapes).
/// - Word-family values render their bare name (no `:`/`'`/`/` prefix/suffix).
/// - `Block`/`Paren` render their elements space-joined from the cursor to
///   the tail, with no surrounding `[]`/`()` delimiters.
/// - `Path` renders parts (each `form`ed) slash-joined.
///
/// All other variants render the same as `mold` (integers, floats, logic,
/// none, func placeholder, binary hex).
pub fn form(value: &Value, out: &mut String) {
    match value {
        Value::None => out.push_str("none"),
        Value::Logic(true) => out.push_str("true"),
        Value::Logic(false) => out.push_str("false"),
        Value::Integer { n, .. } => {
            use std::fmt::Write;
            let _ = write!(out, "{}", n);
        }
        Value::Float { f, .. } => mold_float(*f, out),
        Value::String { s, .. } => out.push_str(s),
        Value::String8(bytes) => {
            out.push_str("#{");
            for b in bytes {
                use std::fmt::Write;
                let _ = write!(out, "{:02X}", b);
            }
            out.push('}');
        }
        Value::Word { sym, .. }
        | Value::SetWord { sym, .. }
        | Value::GetWord { sym, .. }
        | Value::LitWord { sym, .. } => out.push_str(sym.as_str()),
        Value::Refinement { sym, .. } => out.push_str(sym.as_str()),
        Value::File { path, .. } => out.push_str(path),
        Value::Url { url, .. } => out.push_str(url),
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            let data = series.data.borrow();
            for (n, v) in data.iter().enumerate().skip(series.index) {
                if n > series.index {
                    out.push(' ');
                }
                form(v, out);
            }
        }
        Value::Func(_) => out.push_str("#[function]"),
        Value::Error(err) => out.push_str(&err.message),
        Value::Path { parts, .. } => form_path_parts(parts, None, None, out),
        Value::GetPath { parts, .. } => form_path_parts(parts, Some(':'), None, out),
        Value::LitPath { parts, .. } => form_path_parts(parts, Some('\''), None, out),
        Value::SetPath { parts, .. } => form_path_parts(parts, None, Some(':'), out),
        Value::Object(obj) => {
            // form renders just the inner body (no `make object!` wrapper),
            // space-joined, matching `form` of a block.
            let o = obj.borrow();
            form_object_body(&o, out);
        }
    }
}

/// Convenience: return the form as an owned `String`.
pub fn form_to_string(value: &Value) -> String {
    let mut out = String::new();
    form(value, &mut out);
    out
}

fn mold_float(f: f64, out: &mut String) {
    // `{:?}` prints `5.0` rather than `5`, and scientific notation only when
    // Rust thinks it's appropriate. We post-process to guarantee a `.` so the
    // result always parses back as a Float, not an Integer.
    let s = format!("{:?}", f);
    out.push_str(&s);
    if !s.contains('.') && !s.contains('e') && !s.contains("inf") && !s.contains("NaN") {
        out.push_str(".0");
    }
}

fn mold_object(obj: &ObjectDef, out: &mut String) {
    out.push_str("make object! [");
    let words = obj.ctx.words();
    let slots = obj.ctx.slots.borrow();
    let mut first = true;
    for sym in words.iter() {
        if sym.as_str() == "self" {
            continue; // skip self-reference (would infinite-loop)
        }
        let idx = obj.ctx.index_of(sym).unwrap();
        let val = slots[idx].borrow();
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str(sym.as_str());
        out.push_str(": ");
        mold(&val, out);
    }
    out.push(']');
}

fn form_object_body(obj: &ObjectDef, out: &mut String) {
    let words = obj.ctx.words();
    let slots = obj.ctx.slots.borrow();
    let mut first = true;
    for sym in words.iter() {
        if sym.as_str() == "self" {
            continue;
        }
        let idx = obj.ctx.index_of(sym).unwrap();
        let val = slots[idx].borrow();
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str(sym.as_str());
        out.push_str(": ");
        form(&val, out);
    }
}

/// Mold a path's parts joined by `/`, with optional prefix (`:` for get-path,
/// `'` for lit-path) and optional suffix (`:` for set-path). Each part is
/// molded via [`mold`]; paren parts mold as `(...)` so `foo/(a+b)/bar`
/// round-trips.
fn mold_path_parts(parts: &[Value], prefix: Option<char>, suffix: Option<char>, out: &mut String) {
    if let Some(p) = prefix {
        out.push(p);
    }
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        mold(p, out);
    }
    if let Some(s) = suffix {
        out.push(s);
    }
}

/// Like [`mold_path_parts`] but each part is `form`ed (so a paren part renders
/// its evaluated-look, and word parts render their bare name). The prefix/
/// suffix are still emitted in mold-style so the variant is recognizable.
fn form_path_parts(parts: &[Value], prefix: Option<char>, suffix: Option<char>, out: &mut String) {
    if let Some(p) = prefix {
        out.push(p);
    }
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        form(p, out);
    }
    if let Some(s) = suffix {
        out.push(s);
    }
}

fn mold_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

/// Mold a file! path. Uses the bare `%path` form when the path contains no
/// file-delimiter characters (so `%foo/bar.txt` round-trips compactly), and
/// the quoted `%"..."` form (with string-style escapes) when it does (e.g.
/// paths with spaces). Either form re-parses to the same value.
fn mold_file(path: &str, out: &mut String) {
    let needs_quotes = path.is_empty()
        || path.as_bytes().iter().any(|c| {
            matches!(
                c,
                b' ' | b'\t'
                    | b'\r'
                    | b'\n'
                    | b'['
                    | b']'
                    | b'('
                    | b')'
                    | b'{'
                    | b'}'
                    | b';'
                    | b'"'
            )
        });
    out.push('%');
    if needs_quotes {
        mold_string(path, out);
    } else {
        out.push_str(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{Series, Symbol};
    use std::rc::Rc;

    fn s(literal: &str) -> Value {
        Value::string(Rc::<str>::from(literal))
    }

    #[test]
    fn mold_none() {
        assert_eq!(mold_to_string(&Value::None), "none");
    }

    #[test]
    fn mold_logic() {
        assert_eq!(mold_to_string(&Value::Logic(true)), "true");
        assert_eq!(mold_to_string(&Value::Logic(false)), "false");
    }

    #[test]
    fn mold_integer() {
        assert_eq!(mold_to_string(&Value::integer(0)), "0");
        assert_eq!(mold_to_string(&Value::integer(42)), "42");
        assert_eq!(mold_to_string(&Value::integer(-7)), "-7");
    }

    #[test]
    fn mold_float() {
        assert_eq!(mold_to_string(&Value::float(5.0)), "5.0");
        assert_eq!(mold_to_string(&Value::float(1.5)), "1.5");
        assert_eq!(mold_to_string(&Value::float(-2.25)), "-2.25");
    }

    #[test]
    fn mold_float_always_has_dot() {
        // Every finite float must mold with a `.` so it re-parses as Float
        // (not Integer). `{:?}` on f64 already does this for whole numbers.
        for n in [0.0, 1.0, -1.0, 100.0, 1_000_000.0] {
            let molded = mold_to_string(&Value::float(n));
            assert!(molded.contains('.'), "{n} molded to {molded:?} (no dot)");
        }
    }

    #[test]
    fn mold_float_scientific_notation_round_trips() {
        // Large/small magnitudes use scientific notation via `{:?}`; the
        // lexer accepts `e`/`E` exponents, so these re-parse.
        for f in [1e20, 1e-10, 1.5e30] {
            let molded = mold_to_string(&Value::float(f));
            let toks = crate::lexer::lex(&molded).expect("lex float");
            assert_eq!(toks.len(), 1);
            match &toks[0].kind {
                crate::lexer::TokenKind::Float(parsed) => {
                    assert_eq!(*parsed, f, "round-trip mismatch: {f} molded to {molded}");
                }
                other => panic!("expected Float token for {molded}, got {other:?}"),
            }
        }
    }

    #[test]
    fn mold_float_nan_inf_documented_gap() {
        // NaN/inf are NOT reparseable (the lexer has no literal for them) —
        // a documented POC limitation. We just confirm they mold to *some*
        // string without panicking; the property test excludes them.
        let _ = mold_to_string(&Value::float(f64::NAN));
        let _ = mold_to_string(&Value::float(f64::INFINITY));
        let _ = mold_to_string(&Value::float(f64::NEG_INFINITY));
    }

    #[test]
    fn mold_deeply_nested_block() {
        // Deep nesting must not overflow; recursion handles arbitrary depth.
        let mut v = Value::integer(1);
        for _ in 0..50 {
            v = Value::block(Series::new(vec![v]));
        }
        let molded = mold_to_string(&v);
        // 50 opening brackets, the integer, 50 closing brackets.
        assert_eq!(molded.chars().filter(|&c| c == '[').count(), 50);
        assert_eq!(molded.chars().filter(|&c| c == ']').count(), 50);
        assert!(molded.contains('1'));
    }

    #[test]
    fn mold_string_plain() {
        assert_eq!(mold_to_string(&s("hello")), "\"hello\"");
    }

    #[test]
    fn mold_string_escapes() {
        assert_eq!(mold_to_string(&s("a\"b")), "\"a\\\"b\"");
        assert_eq!(mold_to_string(&s("a\\b")), "\"a\\\\b\"");
        assert_eq!(mold_to_string(&s("a\nb")), "\"a\\nb\"");
        assert_eq!(mold_to_string(&s("a\tb")), "\"a\\tb\"");
        assert_eq!(mold_to_string(&s("a\rb")), "\"a\\rb\"");
    }

    #[test]
    fn mold_string_carriage_return_round_trips() {
        // A string containing a raw CR must mold to an escaped form so it
        // re-parses to the same value (the lexer's `\r` escape decodes to CR).
        let raw = s("line1\rline2");
        let molded = mold_to_string(&raw);
        assert_eq!(molded, "\"line1\\rline2\"");
        // No raw CR inside the quotes.
        assert!(!molded[1..molded.len() - 1].contains('\r'));
    }

    #[test]
    fn mold_string_control_chars_preserved() {
        // The four lexer-supported escapes round-trip.
        for raw in ["a\"b", "a\\b", "a\nb", "a\tb", "a\rb"] {
            let molded = mold_to_string(&s(raw));
            // Re-parse the molded form and compare.
            let toks = crate::lexer::lex(&molded).expect("lex molded string");
            assert_eq!(toks.len(), 1);
            match &toks[0].kind {
                crate::lexer::TokenKind::String(parsed) => {
                    assert_eq!(parsed.as_ref(), raw, "round-trip mismatch for {raw:?}");
                }
                other => panic!("expected String token, got {other:?}"),
            }
        }
    }

    #[test]
    fn string_escape_round_trip() {
        for raw in [
            "hello",
            "a\"b",
            "back\\slash",
            "tab\there",
            "new\nline",
            "mix\"\n\\t",
        ] {
            let molded = mold_to_string(&s(raw));
            // Molded form always starts/ends with a quote and contains no
            // raw control characters.
            assert!(molded.starts_with('"') && molded.ends_with('"'));
            assert!(!molded[1..molded.len() - 1].contains('\n'));
            assert!(!molded[1..molded.len() - 1].contains('\t'));
            // Manually unescape and compare to the original.
            let inner = &molded[1..molded.len() - 1];
            let mut decoded = String::new();
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next().unwrap() {
                        '"' => decoded.push('"'),
                        '\\' => decoded.push('\\'),
                        'n' => decoded.push('\n'),
                        't' => decoded.push('\t'),
                        other => panic!("unexpected escape \\{}", other),
                    }
                } else {
                    decoded.push(c);
                }
            }
            assert_eq!(decoded, raw);
        }
    }

    #[test]
    fn mold_word_kinds() {
        assert_eq!(mold_to_string(&Value::word("foo")), "foo");
        assert_eq!(mold_to_string(&Value::set_word("foo")), "foo:");
        assert_eq!(mold_to_string(&Value::get_word("foo")), ":foo");
        assert_eq!(mold_to_string(&Value::lit_word("foo")), "'foo");
    }

    #[test]
    fn mold_empty_block() {
        assert_eq!(mold_to_string(&Value::block(Series::empty())), "[]");
    }

    #[test]
    fn mold_simple_block() {
        let v = Value::block(Series::new(vec![
            Value::integer(1),
            Value::integer(2),
            Value::integer(3),
        ]));
        assert_eq!(mold_to_string(&v), "[1 2 3]");
    }

    #[test]
    fn mold_block_mixed() {
        let v = Value::block(Series::new(vec![
            Value::word("print"),
            s("hi"),
            Value::integer(7),
        ]));
        assert_eq!(mold_to_string(&v), "[print \"hi\" 7]");
    }

    #[test]
    fn mold_nested_block() {
        let inner = Value::block(Series::new(vec![Value::word("b"), Value::word("c")]));
        let outer = Value::block(Series::new(vec![Value::word("a"), inner, Value::word("d")]));
        assert_eq!(mold_to_string(&outer), "[a [b c] d]");
    }

    #[test]
    fn mold_empty_paren() {
        assert_eq!(mold_to_string(&Value::paren(Series::empty())), "()");
    }

    #[test]
    fn mold_paren() {
        let v = Value::paren(Series::new(vec![Value::integer(1), Value::integer(2)]));
        assert_eq!(mold_to_string(&v), "(1 2)");
    }

    #[test]
    fn mold_nested_block_in_paren() {
        let inner = Value::block(Series::new(vec![Value::integer(1), Value::integer(2)]));
        let v = Value::paren(Series::new(vec![inner, Value::word("x")]));
        assert_eq!(mold_to_string(&v), "([1 2] x)");
    }

    #[test]
    fn mold_func_placeholder() {
        let fd = std::rc::Rc::new(crate::value::FuncDef::default());
        assert_eq!(mold_to_string(&Value::Func(fd)), "#[function]");
    }

    #[test]
    fn mold_path() {
        let p = Value::path(vec![Value::word("foo"), Value::word("bar")]);
        assert_eq!(mold_to_string(&p), "foo/bar");
    }

    #[test]
    fn mold_path_three_parts() {
        let p = Value::path(vec![Value::word("a"), Value::word("b"), Value::word("c")]);
        assert_eq!(mold_to_string(&p), "a/b/c");
    }

    #[test]
    fn mold_refinement() {
        assert_eq!(mold_to_string(&Value::refinement("part")), "/part");
        assert_eq!(mold_to_string(&Value::refinement("only")), "/only");
    }

    #[test]
    fn symbol_intern_share() {
        // Sanity: two Symbols over the same `Rc<str>` share via Rc::clone.
        let a = Symbol::new("foo");
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(a.as_str(), "foo");
    }

    // --- form (M14) ---

    #[test]
    fn form_scalar_matches_mold() {
        assert_eq!(form_to_string(&Value::None), "none");
        assert_eq!(form_to_string(&Value::Logic(true)), "true");
        assert_eq!(form_to_string(&Value::Logic(false)), "false");
        assert_eq!(form_to_string(&Value::integer(42)), "42");
        assert_eq!(form_to_string(&Value::float(3.5)), "3.5");
    }

    #[test]
    fn form_string_is_raw() {
        // form strips quotes/escapes — the key difference from mold.
        assert_eq!(form_to_string(&s("hello")), "hello");
        assert_eq!(form_to_string(&s("a\nb")), "a\nb");
        assert_eq!(mold_to_string(&s("a\nb")), "\"a\\nb\"");
    }

    #[test]
    fn form_word_family_strips_markers() {
        assert_eq!(form_to_string(&Value::word("foo")), "foo");
        assert_eq!(form_to_string(&Value::set_word("foo")), "foo");
        assert_eq!(form_to_string(&Value::get_word("foo")), "foo");
        assert_eq!(form_to_string(&Value::lit_word("foo")), "foo");
        assert_eq!(form_to_string(&Value::refinement("part")), "part");
    }

    #[test]
    fn form_block_is_space_joined_no_brackets() {
        let v = Value::block(Series::new(vec![
            Value::integer(1),
            Value::integer(2),
            Value::integer(3),
        ]));
        assert_eq!(form_to_string(&v), "1 2 3");
        // mold still produces bracketed form.
        assert_eq!(mold_to_string(&v), "[1 2 3]");
    }

    #[test]
    fn form_block_with_strings_no_inner_quotes() {
        let v = Value::block(Series::new(vec![s("a"), s("b"), s("c")]));
        assert_eq!(form_to_string(&v), "a b c");
    }

    #[test]
    fn form_path_is_slash_joined() {
        let p = Value::path(vec![Value::word("foo"), Value::word("bar")]);
        assert_eq!(form_to_string(&p), "foo/bar");
    }
}
