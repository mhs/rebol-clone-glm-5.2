//! `mold`: value → Red source text. Inverse of the parser; round-trip
//! property is `mold(parse(s)) == normalize(s)`.

use crate::value::Value;

/// Append the Red source form of `value` to `out`.
pub fn mold(value: &Value, out: &mut String) {
    match value {
        Value::None => out.push_str("none"),
        Value::Logic(true) => out.push_str("true"),
        Value::Logic(false) => out.push_str("false"),
        Value::Integer(n) => {
            use std::fmt::Write;
            let _ = write!(out, "{}", n);
        }
        Value::Float(f) => mold_float(*f, out),
        Value::String(s) => mold_string(s, out),
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
        Value::LitWord(sym) => {
            out.push('\'');
            out.push_str(sym.as_str());
        }
        Value::Block { series, .. } => {
            out.push('[');
            let data = series.data.borrow();
            for (i, v) in data.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                mold(v, out);
            }
            out.push(']');
        }
        Value::Paren { series, .. } => {
            out.push('(');
            let data = series.data.borrow();
            for (i, v) in data.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                mold(v, out);
            }
            out.push(')');
        }
        Value::Func(_) => out.push_str("#[function]"),
        Value::Path(parts) => {
            for (i, p) in parts.iter().enumerate() {
                if i > 0 {
                    out.push('/');
                }
                mold(p, out);
            }
        }
    }
}

/// Convenience: return the mold as an owned `String`.
pub fn mold_to_string(value: &Value) -> String {
    let mut out = String::new();
    mold(value, &mut out);
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

fn mold_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{Series, Symbol};
    use std::rc::Rc;

    fn s(literal: &str) -> Value {
        Value::String(Rc::from(literal))
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
        assert_eq!(mold_to_string(&Value::Integer(0)), "0");
        assert_eq!(mold_to_string(&Value::Integer(42)), "42");
        assert_eq!(mold_to_string(&Value::Integer(-7)), "-7");
    }

    #[test]
    fn mold_float() {
        assert_eq!(mold_to_string(&Value::Float(5.0)), "5.0");
        assert_eq!(mold_to_string(&Value::Float(1.5)), "1.5");
        assert_eq!(mold_to_string(&Value::Float(-2.25)), "-2.25");
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
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
        ]));
        assert_eq!(mold_to_string(&v), "[1 2 3]");
    }

    #[test]
    fn mold_block_mixed() {
        let v = Value::block(Series::new(vec![
            Value::word("print"),
            s("hi"),
            Value::Integer(7),
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
        let v = Value::paren(Series::new(vec![Value::Integer(1), Value::Integer(2)]));
        assert_eq!(mold_to_string(&v), "(1 2)");
    }

    #[test]
    fn mold_nested_block_in_paren() {
        let inner = Value::block(Series::new(vec![Value::Integer(1), Value::Integer(2)]));
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
        let p = Value::Path(vec![Value::word("foo"), Value::word("bar")]);
        assert_eq!(mold_to_string(&p), "foo/bar");
    }

    #[test]
    fn mold_path_three_parts() {
        let p = Value::Path(vec![Value::word("a"), Value::word("b"), Value::word("c")]);
        assert_eq!(mold_to_string(&p), "a/b/c");
    }

    #[test]
    fn symbol_intern_share() {
        // Sanity: two Symbols over the same `Rc<str>` share via Rc::clone.
        let a = Symbol::new("foo");
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(a.as_str(), "foo");
    }
}
