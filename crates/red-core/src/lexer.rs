//! Lexer: source string → `Vec<Token>`. Whitespace-delimited, `;` comments,
//! `"..."` and `{...}` strings, integers/floats, word family, `[ ] ( )`.
//!
//! Single-character lookahead, no backtracking. Every token carries a byte-
//! offset `Span` so the parser/CLI can point at the offending bytes.

use std::rc::Rc;

use crate::value::{Span, Symbol};

/// One lexical token.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Integer(i64),
    Float(f64),
    String(Rc<str>),
    Word(Symbol),
    SetWord(Symbol),
    GetWord(Symbol),
    LitWord(Symbol),
    LBracket,
    RBracket,
    LParen,
    RParen,
}

/// A tagged span of source text.
#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Lexical failure. Every variant carries the span where the error was
/// detected so downstream layers can render `file:line:col:` diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub enum LexError {
    /// `"...` hit EOF before the closing quote.
    UnterminatedString { span: Span },
    /// A numeric run didn't parse as a valid integer/float (e.g. `1.2.3`).
    InvalidNumber { span: Span, chars: String },
    /// A word-shaped token had an empty body (e.g. `::`, `''`).
    InvalidWord { span: Span },
    /// `{...` hit EOF with braces still open. `depth` is the number of
    /// unclosed `{` at EOF.
    UnbalancedBrace { span: Span, depth: i32 },
}

/// Tokenize `src`. Whitespace and `;` comments are skipped; every emitted
/// token has a correct byte-offset span.
pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let start = i;
        let c = bytes[i];

        // Whitespace: space, tab, CR, LF. In Red, `,` is also whitespace
        // (so `1,2,3` reads as three values, like `1 2 3`).
        if c == b' ' || c == b'\t' || c == b'\r' || c == b'\n' || c == b',' {
            i += 1;
            continue;
        }

        // `;` comment to EOL.
        if c == b';' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Single-char delimiters.
        let single = match c {
            b'[' => Some(TokenKind::LBracket),
            b']' => Some(TokenKind::RBracket),
            b'(' => Some(TokenKind::LParen),
            b')' => Some(TokenKind::RParen),
            _ => None,
        };
        if let Some(kind) = single {
            i += 1;
            out.push(Token {
                kind,
                span: Span::new(start, i),
            });
            continue;
        }

        // String literals.
        if c == b'"' {
            let (end, s) = scan_quoted(src, &mut i)?;
            out.push(Token {
                kind: TokenKind::String(Rc::from(s.as_str())),
                span: Span::new(start, end),
            });
            continue;
        }
        if c == b'{' {
            let (end, s) = scan_braced(src, &mut i)?;
            out.push(Token {
                kind: TokenKind::String(Rc::from(s.as_str())),
                span: Span::new(start, end),
            });
            continue;
        }

        // Numbers: digit, or `-` followed by digit.
        if c.is_ascii_digit() || (c == b'-' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit())
        {
            let (end, kind) = scan_number(src, &mut i)?;
            out.push(Token {
                kind,
                span: Span::new(start, end),
            });
            continue;
        }

        // Everything else is a word (incl. `:foo`, `'foo`, `foo:`).
        let (end, kind) = scan_word(src, &mut i)?;
        out.push(Token {
            kind,
            span: Span::new(start, end),
        });
    }

    Ok(out)
}

/// `"..."` with escapes `\"`, `\\`, `\n`, `\t`, `\r`. EOF before closing
/// quote → `UnterminatedString`.
fn scan_quoted(src: &str, i: &mut usize) -> Result<(usize, String), LexError> {
    let start = *i;
    let bytes = src.as_bytes();
    *i += 1; // consume opening `"`
    let mut out = String::new();

    while *i < bytes.len() {
        let c = bytes[*i];
        if c == b'"' {
            *i += 1;
            return Ok((*i, out));
        }
        if c == b'\\' {
            *i += 1;
            if *i >= bytes.len() {
                return Err(LexError::UnterminatedString {
                    span: Span::new(start, bytes.len()),
                });
            }
            let esc = bytes[*i];
            let decoded = match esc {
                b'"' => '"',
                b'\\' => '\\',
                b'n' => '\n',
                b't' => '\t',
                b'r' => '\r',
                _ => {
                    // Unknown escape: keep the backslash and the char verbatim
                    // so the round-trip preserves user input.
                    out.push('\\');
                    esc as char
                }
            };
            out.push(decoded);
            *i += 1;
            continue;
        }
        // Ordinary byte — push as UTF-8. We advance by the char's byte length.
        let ch_len = utf8_len(c);
        if let Some(s) = src.get(*i..*i + ch_len) {
            out.push_str(s);
            *i += ch_len;
        } else {
            *i += 1;
        }
    }

    Err(LexError::UnterminatedString {
        span: Span::new(start, bytes.len()),
    })
}

/// `{...}` — nested braces, multi-line. Depth counter starts at 1; EOF with
/// depth > 0 → `UnbalancedBrace`.
fn scan_braced(src: &str, i: &mut usize) -> Result<(usize, String), LexError> {
    let start = *i;
    let bytes = src.as_bytes();
    *i += 1; // consume opening `{`
    let mut depth: i32 = 1;
    let mut out = String::new();

    while *i < bytes.len() {
        let c = bytes[*i];
        if c == b'{' {
            depth += 1;
            out.push('{');
            *i += 1;
            continue;
        }
        if c == b'}' {
            depth -= 1;
            *i += 1;
            if depth == 0 {
                return Ok((*i, out));
            }
            out.push('}');
            continue;
        }
        let ch_len = utf8_len(c);
        if let Some(s) = src.get(*i..*i + ch_len) {
            out.push_str(s);
            *i += ch_len;
        } else {
            *i += 1;
        }
    }

    Err(LexError::UnbalancedBrace {
        span: Span::new(start, bytes.len()),
        depth,
    })
}

/// `[0-9]+` optionally followed by `.[0-9]+` and/or `[eE][+-]?[0-9]+`.
/// Rejects a second `.` (e.g. `1.2.3`) with `InvalidNumber`.
fn scan_number(src: &str, i: &mut usize) -> Result<(usize, TokenKind), LexError> {
    let start = *i;
    let bytes = src.as_bytes();

    // Optional leading `-` (caller guarantees digit follows, but handle anyway).
    if bytes[*i] == b'-' {
        *i += 1;
    }

    // Integer part.
    *i += consume_digits(bytes, *i);

    let mut is_float = false;

    // Fractional part: `.` followed by digits. A `.` NOT followed by a digit
    // ends the number (so `5.foo` lexes as Integer `5` then Word `foo`).
    if *i < bytes.len() && bytes[*i] == b'.' {
        let after_dot = *i + 1;
        if after_dot < bytes.len() && bytes[after_dot].is_ascii_digit() {
            is_float = true;
            *i += 1; // consume `.`
            *i += consume_digits(bytes, *i);
        }
    }

    // Exponent part.
    if *i < bytes.len() && (bytes[*i] == b'e' || bytes[*i] == b'E') {
        let saved = *i;
        *i += 1;
        // Optional sign.
        if *i < bytes.len() && (bytes[*i] == b'+' || bytes[*i] == b'-') {
            *i += 1;
        }
        if *i < bytes.len() && bytes[*i].is_ascii_digit() {
            is_float = true;
            *i += consume_digits(bytes, *i);
        } else {
            // No digits after `e`/`E` — not an exponent; rewind and treat
            // the `e` as the start of the next token (a word).
            *i = saved + 1;
        }
    }

    // A `.` immediately following a complete number is only an error when it
    // itself begins another fractional run (e.g. `1.2.3`). A `.` followed by
    // a non-digit (e.g. `5.foo`) ends the number and lets the `.` start the
    // next token.
    if *i < bytes.len()
        && bytes[*i] == b'.'
        && *i + 1 < bytes.len()
        && bytes[*i + 1].is_ascii_digit()
    {
        let end = *i + 1;
        return Err(LexError::InvalidNumber {
            span: Span::new(start, end),
            chars: src[start..end].to_string(),
        });
    }

    let end = *i;
    let text = &src[start..end];

    let kind = if is_float {
        match text.parse::<f64>() {
            Ok(f) => TokenKind::Float(f),
            Err(_) => {
                return Err(LexError::InvalidNumber {
                    span: Span::new(start, end),
                    chars: text.to_string(),
                });
            }
        }
    } else {
        match text.parse::<i64>() {
            Ok(n) => TokenKind::Integer(n),
            Err(_) => {
                return Err(LexError::InvalidNumber {
                    span: Span::new(start, end),
                    chars: text.to_string(),
                });
            }
        }
    };

    Ok((end, kind))
}

fn consume_digits(bytes: &[u8], mut i: usize) -> usize {
    let start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    i - start
}

/// Read a run of non-delimiter chars (delimiters = whitespace, `[](){};",`)
/// and classify into Word/SetWord/GetWord/LitWord. Rejects an empty body.
fn scan_word(src: &str, i: &mut usize) -> Result<(usize, TokenKind), LexError> {
    let start = *i;
    let bytes = src.as_bytes();

    // Consume a run of non-delimiter bytes. Track UTF-8 boundaries.
    while *i < bytes.len() && !is_delimiter(bytes[*i]) {
        *i += 1;
    }

    let end = *i;
    let raw = &src[start..end];

    let kind = classify_word(raw).ok_or(LexError::InvalidWord {
        span: Span::new(start, end),
    })?;
    Ok((end, kind))
}

/// Delimiter set per architecture.md: whitespace, `[](){};"`. (`,` is
/// whitespace, not a delimiter — handled in the main scan loop.)
fn is_delimiter(c: u8) -> bool {
    matches!(
        c,
        b' ' | b'\t' | b'\r' | b'\n' | b'[' | b']' | b'(' | b')' | b'{' | b'}' | b';' | b'"'
    )
}

/// Classify a word run into its token kind. Returns `None` for an empty body
/// (e.g. `::`, `''`).
fn classify_word(raw: &str) -> Option<TokenKind> {
    let bytes = raw.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    // Leading `:` → GetWord; leading `'` → LitWord.
    let (body, leading) = if bytes[0] == b':' {
        (&raw[1..], Some(':'))
    } else if bytes[0] == b'\'' {
        (&raw[1..], Some('\''))
    } else {
        (raw, None)
    };

    // Single trailing `:` → SetWord (only when no leading marker).
    let body_bytes = body.as_bytes();
    let (core, trailing_colon) =
        if leading.is_none() && body_bytes.len() >= 2 && body_bytes[body_bytes.len() - 1] == b':' {
            (&body[..body.len() - 1], true)
        } else {
            (body, false)
        };

    if core.is_empty() {
        return None;
    }
    // Reject bodies that are only colons/quotes (e.g. `::`, `''`, `:`).
    if core.chars().all(|c| c == ':' || c == '\'') {
        return None;
    }

    match leading {
        Some(':') => Some(TokenKind::GetWord(Symbol::new(core))),
        Some('\'') => Some(TokenKind::LitWord(Symbol::new(core))),
        None if trailing_colon => Some(TokenKind::SetWord(Symbol::new(core))),
        None => Some(TokenKind::Word(Symbol::new(core))),
        _ => None,
    }
}

/// Length in bytes of the UTF-8 char starting at `first_byte`. Falls back to 1.
fn utf8_len(first_byte: u8) -> usize {
    if first_byte < 0x80 {
        1
    } else if first_byte >> 5 == 0b110 {
        2
    } else if first_byte >> 4 == 0b1110 {
        3
    } else if first_byte >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Symbol;

    /// Helper: lex and assert the single token produced matches `kind`.
    fn one(src: &str) -> TokenKind {
        let toks = lex(src).expect("lex failed");
        assert_eq!(
            toks.len(),
            1,
            "expected one token, got {toks:?} for {src:?}"
        );
        toks[0].kind.clone()
    }

    /// Helper: lex and return the token kinds (ignoring spans).
    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src)
            .expect("lex failed")
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn integer() {
        assert_eq!(one("0"), TokenKind::Integer(0));
        assert_eq!(one("42"), TokenKind::Integer(42));
        assert_eq!(one("007"), TokenKind::Integer(7));
    }

    #[test]
    fn negative_integer() {
        assert_eq!(one("-7"), TokenKind::Integer(-7));
        assert_eq!(one("-0"), TokenKind::Integer(0));
    }

    #[test]
    fn float() {
        assert_eq!(one("1.5"), TokenKind::Float(1.5));
        assert_eq!(one("-2.25"), TokenKind::Float(-2.25));
        assert_eq!(one("0.0"), TokenKind::Float(0.0));
    }

    #[test]
    fn float_with_exponent() {
        assert_eq!(one("1e3"), TokenKind::Float(1000.0));
        assert_eq!(one("1.5e2"), TokenKind::Float(150.0));
        assert_eq!(one("1E-2"), TokenKind::Float(0.01));
        assert_eq!(one("2.0e+3"), TokenKind::Float(2000.0));
    }

    #[test]
    fn number_then_word_no_dot() {
        // `5.foo` — the `.` not followed by a digit ends the number.
        let toks = lex("5.foo").expect("lex");
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].kind, TokenKind::Integer(5));
        assert_eq!(toks[1].kind, TokenKind::Word(Symbol::new(".foo")));
    }

    #[test]
    fn invalid_number_double_dot() {
        let err = lex("1.2.3").unwrap_err();
        assert!(matches!(err, LexError::InvalidNumber { .. }));
    }

    #[test]
    fn quoted_string_plain() {
        assert_eq!(one("\"hello\""), TokenKind::String(Rc::from("hello")));
    }

    #[test]
    fn quoted_string_each_escape() {
        assert_eq!(one("\"a\\\"b\""), TokenKind::String(Rc::from("a\"b")));
        assert_eq!(one("\"a\\\\b\""), TokenKind::String(Rc::from("a\\b")));
        assert_eq!(one("\"a\\nb\""), TokenKind::String(Rc::from("a\nb")));
        assert_eq!(one("\"a\\tb\""), TokenKind::String(Rc::from("a\tb")));
        assert_eq!(one("\"a\\rb\""), TokenKind::String(Rc::from("a\rb")));
    }

    #[test]
    fn quoted_string_empty() {
        assert_eq!(one("\"\""), TokenKind::String(Rc::from("")));
    }

    #[test]
    fn unterminated_string() {
        let err = lex("\"abc").unwrap_err();
        assert!(matches!(err, LexError::UnterminatedString { .. }));
    }

    #[test]
    fn braced_string_single_line() {
        assert_eq!(one("{abc}"), TokenKind::String(Rc::from("abc")));
        assert_eq!(one("{}"), TokenKind::String(Rc::from("")));
    }

    #[test]
    fn braced_string_multi_line() {
        let src = "{line1\nline2}";
        assert_eq!(one(src), TokenKind::String(Rc::from("line1\nline2")));
    }

    #[test]
    fn braced_string_nested() {
        assert_eq!(one("{{a}}"), TokenKind::String(Rc::from("{a}")));
        assert_eq!(one("{a{b}c}"), TokenKind::String(Rc::from("a{b}c")));
    }

    #[test]
    fn unbalanced_brace() {
        let err = lex("{abc").unwrap_err();
        assert!(matches!(err, LexError::UnbalancedBrace { depth: 1, .. }));
        let err = lex("{{a").unwrap_err();
        assert!(matches!(err, LexError::UnbalancedBrace { depth: 2, .. }));
    }

    #[test]
    fn word() {
        assert_eq!(one("foo"), TokenKind::Word(Symbol::new("foo")));
        assert_eq!(one("print"), TokenKind::Word(Symbol::new("print")));
    }

    #[test]
    fn set_word() {
        assert_eq!(one("foo:"), TokenKind::SetWord(Symbol::new("foo")));
    }

    #[test]
    fn get_word() {
        assert_eq!(one(":foo"), TokenKind::GetWord(Symbol::new("foo")));
    }

    #[test]
    fn lit_word() {
        assert_eq!(one("'foo"), TokenKind::LitWord(Symbol::new("foo")));
    }

    #[test]
    fn invalid_word_empty() {
        // `::` — leading `:` consumes one, then trailing `:` on a 1-char body
        // would leave an empty core.
        let err = lex("::").unwrap_err();
        assert!(matches!(err, LexError::InvalidWord { .. }));
        let err = lex("''").unwrap_err();
        assert!(matches!(err, LexError::InvalidWord { .. }));
    }

    #[test]
    fn block_delimiters() {
        assert_eq!(one("["), TokenKind::LBracket);
        assert_eq!(one("]"), TokenKind::RBracket);
        assert_eq!(one("("), TokenKind::LParen);
        assert_eq!(one(")"), TokenKind::RParen);
    }

    #[test]
    fn block_and_paren_intermixed() {
        let toks = kinds("[1 (2 3) 4]");
        assert_eq!(
            toks,
            vec![
                TokenKind::LBracket,
                TokenKind::Integer(1),
                TokenKind::LParen,
                TokenKind::Integer(2),
                TokenKind::Integer(3),
                TokenKind::RParen,
                TokenKind::Integer(4),
                TokenKind::RBracket,
            ]
        );
    }

    #[test]
    fn comment_to_eol_skipped() {
        // Leading comment, then a token on the next line.
        let toks = kinds("; this is a comment\n42");
        assert_eq!(toks, vec![TokenKind::Integer(42)]);
    }

    #[test]
    fn comment_at_eof() {
        let toks = kinds("42 ; trailing comment");
        assert_eq!(toks, vec![TokenKind::Integer(42)]);
    }

    #[test]
    fn whitespace_skipped() {
        let toks = kinds("  1\t2\n3\r4  ");
        assert_eq!(
            toks,
            vec![
                TokenKind::Integer(1),
                TokenKind::Integer(2),
                TokenKind::Integer(3),
                TokenKind::Integer(4),
            ]
        );
    }

    #[test]
    fn span_offsets_correct() {
        let toks = lex("  [42]  ").expect("lex");
        assert_eq!(toks.len(), 3);
        // `[` at byte 2
        assert_eq!(toks[0].span, Span::new(2, 3));
        // `42` at bytes 3..5
        assert_eq!(toks[1].span, Span::new(3, 5));
        // `]` at byte 5
        assert_eq!(toks[2].span, Span::new(5, 6));
    }

    #[test]
    fn span_for_quoted_string() {
        let toks = lex("\"ab\"").expect("lex");
        assert_eq!(toks[0].span, Span::new(0, 4));
    }

    #[test]
    fn span_for_braced_string_multiline() {
        let toks = lex("{a\nb}").expect("lex");
        assert_eq!(toks[0].span, Span::new(0, 5));
    }

    #[test]
    fn mixed_program() {
        let toks = kinds("Red [title: \"Hi\"] print \"Hello\"");
        assert_eq!(
            toks,
            vec![
                TokenKind::Word(Symbol::new("Red")),
                TokenKind::LBracket,
                TokenKind::SetWord(Symbol::new("title")),
                TokenKind::String(Rc::from("Hi")),
                TokenKind::RBracket,
                TokenKind::Word(Symbol::new("print")),
                TokenKind::String(Rc::from("Hello")),
            ]
        );
    }
}
