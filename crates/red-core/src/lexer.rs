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
    /// `/foo` — a refinement word. Produced by a `/` followed by a run of
    /// word chars. A bare `/` (slash not followed by word chars) is emitted
    /// as `Word("/")` instead, since `/` is also the division operator.
    Refinement(Symbol),
    /// `%foo/bar.txt` — a file! literal. Produced by a `%` followed by a run
    /// of non-delimiter bytes (where `/` is *not* a delimiter inside a file
    /// run, so paths stay together).
    File(Rc<str>),
    /// `http://example.com/x` — a url! literal. Produced when a word run
    /// matches `scheme://...`. The full text (including scheme) is stored.
    Url(Rc<str>),
    /// `#"a"` — a char! literal. Produced by `#"` followed by either a single
    /// char, a `^`-escape (`^-` tab, `^/` newline, `^@` null, `^M-C` meta,
    /// `^^` literal caret, `^"` literal quote), or `^(NN)` codepoint hex.
    Char(char),
    /// `#{hex}` — a binary! literal. Produced by `#{` followed by a run of
    /// hex digits (whitespace-skipping per Red is not implemented — digits
    /// must be contiguous) and a closing `}`. An odd digit count is
    /// zero-padded on the high nibble (Red behavior).
    Binary(Rc<[u8]>),
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
    /// `#"...` hit EOF before the closing `"`, or contained a malformed
    /// `^`-escape or `^(NN)` codepoint.
    InvalidChar { span: Span, chars: String },
    /// `#{...}` hit EOF before the closing `}`, or contained a non-hex digit.
    InvalidBinary { span: Span, chars: String },
}

impl LexError {
    /// Byte-offset span where this error was detected.
    pub fn span(&self) -> Span {
        match self {
            LexError::UnterminatedString { span }
            | LexError::InvalidNumber { span, .. }
            | LexError::InvalidWord { span }
            | LexError::UnbalancedBrace { span, .. }
            | LexError::InvalidChar { span, .. }
            | LexError::InvalidBinary { span, .. } => *span,
        }
    }
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Message body only — `render_error` adds the `*** Error:` prefix
        // and `file:line:col:` location.
        match self {
            LexError::UnterminatedString { .. } => write!(f, "unterminated string"),
            LexError::InvalidNumber { chars, .. } => {
                write!(f, "invalid number: {chars:?}")
            }
            LexError::InvalidWord { .. } => write!(f, "invalid word (empty body)"),
            LexError::UnbalancedBrace { depth, .. } => {
                write!(f, "unbalanced brace — {depth} unclosed `{{` at EOF")
            }
            LexError::InvalidChar { chars, .. } => {
                write!(f, "invalid char literal: {chars:?}")
            }
            LexError::InvalidBinary { chars, .. } => {
                write!(f, "invalid binary literal: {chars:?}")
            }
        }
    }
}

impl std::error::Error for LexError {}

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

        // Refinement word `/foo`, or bare `/` (the division operator) when
        // not followed by word chars. `/` is a delimiter, so `foo/bar` lexes
        // as `Word(foo) Refinement(bar)`; the parser assembles adjacent
        // `Word`+`Refinement` runs into a `Path`.
        if c == b'/' {
            let (end, kind) = scan_refinement(src, &mut i)?;
            out.push(Token {
                kind,
                span: Span::new(start, end),
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

        // File literal: `%`-prefixed run. `/` is allowed inside (file paths),
        // so this scans its own run rather than reusing `scan_word`.
        if c == b'%' {
            let (end, kind) = scan_file(src, &mut i)?;
            out.push(Token {
                kind,
                span: Span::new(start, end),
            });
            continue;
        }

        // Char literal: `#"..."` form. A bare `#` not followed by `"` or `{`
        // falls through to `scan_word` (preserving back-compat with `#foo` words).
        if c == b'#' && bytes.get(i + 1) == Some(&b'"') {
            let (end, kind) = scan_char(src, &mut i)?;
            out.push(Token {
                kind,
                span: Span::new(start, end),
            });
            continue;
        }

        // Binary literal: `#{hex}` form. A bare `#` not followed by `{`
        // falls through to `scan_word`.
        if c == b'#' && bytes.get(i + 1) == Some(&b'{') {
            let (end, kind) = scan_binary(src, &mut i)?;
            out.push(Token {
                kind,
                span: Span::new(start, end),
            });
            continue;
        }

        // URL literal: `scheme://...` where scheme is alpha-led. Detected
        // here (before the `/` refinement arm and the word fall-through)
        // because `/` is otherwise a delimiter and would split the URL.
        if c.is_ascii_alphabetic() {
            if let Some(scheme_end) = url_scheme_end(src, i) {
                let (end, kind) = scan_url(src, &mut i, scheme_end);
                out.push(Token {
                    kind,
                    span: Span::new(start, end),
                });
                continue;
            }
        }

        // Numbers: digit, or `-` followed by digit.
        if c.is_ascii_digit() || (c == b'-' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit())
        {
            let (end, kind) = scan_number(src, &mut i)?;
            // M38 follow-up: integer SetPath. `2:` lexes as `Integer(2)` +
            // `SetWord("2")` with overlapping spans (mirrors `obj/field:`
            // which lexes as `Refinement(field)` + `SetWord(field)`). The
            // parser folds the run into a `SetPath` via span-overlap
            // detection. Only a single trailing `:` triggers this (`::`
            // falls through). Floats are excluded (Red has no float set-path).
            if let TokenKind::Integer(n) = kind {
                if i < bytes.len() && bytes[i] == b':' && bytes.get(i + 1) != Some(&b':') {
                    out.push(Token {
                        kind: TokenKind::Integer(n),
                        span: Span::new(start, end),
                    });
                    let setword_span = Span::new(start, end + 1);
                    i += 1; // consume the `:`
                    out.push(Token {
                        kind: TokenKind::SetWord(Symbol::new(&src[start..end])),
                        span: setword_span,
                    });
                    continue;
                }
            }
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

/// `#"..."` — a char! literal. Inside the quotes exactly one "char unit"
/// appears:
/// - a literal character: `#"a"` → `'a'`
/// - a `^`-escape: `#"^-"` → tab, `#"^/"` → newline, `#"^@"` → null,
///   `#"^^"` → literal caret, `#"^""` → literal quote, `#"^M-C"` → meta
///   (`C` with high bit cleared: codepoint `C` XOR `0x80` ... actually Red's
///   `^M-C` form yields `chr & 0x1F` for control, but for POC we map `M-<c>`
///   to `(c as u8).wrapping_sub(0x40)` matching Red's "control" semantics)
/// - a `^(NN)` codepoint hex form (1-6 hex digits): `#"^(41)"` → `'A'`
///
/// EOF before closing `"` or a malformed escape → `InvalidChar`.
fn scan_char(src: &str, i: &mut usize) -> Result<(usize, TokenKind), LexError> {
    let start = *i;
    let bytes = src.as_bytes();
    // Consume `#"`.
    *i += 2;
    if *i >= bytes.len() {
        return Err(LexError::InvalidChar {
            span: Span::new(start, bytes.len()),
            chars: src[start..].to_string(),
        });
    }

    let c = decode_char_unit(src, bytes, i).map_err(|chars| LexError::InvalidChar {
        span: Span::new(start, *i),
        chars,
    })?;

    // Expect closing `"`.
    if *i >= bytes.len() || bytes[*i] != b'"' {
        return Err(LexError::InvalidChar {
            span: Span::new(start, bytes.len()),
            chars: src[start..(*i).min(bytes.len())].to_string(),
        });
    }
    *i += 1; // consume closing `"`
    Ok((*i, TokenKind::Char(c)))
}

/// `#{hex}` — a binary! literal. Reads hex digits (contiguous, no internal
/// whitespace — Red allows whitespace inside the braces but the POC keeps it
/// strict) until the closing `}`. An odd digit count is zero-padded on the
/// high nibble (Red behavior: `#{ABC}` → bytes `[0x0A, 0xBC]`).
///
/// EOF before `}` or a non-hex digit → `InvalidBinary`.
fn scan_binary(src: &str, i: &mut usize) -> Result<(usize, TokenKind), LexError> {
    let start = *i;
    let bytes = src.as_bytes();
    // Consume `#{`.
    *i += 2;
    let mut hex = String::new();
    while *i < bytes.len() {
        let c = bytes[*i];
        if c == b'}' {
            *i += 1; // consume `}`
            let mut bytes_out: Vec<u8> = Vec::with_capacity(hex.len() / 2 + 1);
            let mut padded = hex.clone();
            if padded.len() % 2 == 1 {
                // Odd digit count → prepend a `0` to the high nibble.
                padded.insert(0, '0');
            }
            let mut j = 0;
            while j + 1 < padded.len() {
                let pair = &padded[j..j + 2];
                let b = u8::from_str_radix(pair, 16).map_err(|_| LexError::InvalidBinary {
                    span: Span::new(start, *i),
                    chars: padded[j..j + 2].to_string(),
                })?;
                bytes_out.push(b);
                j += 2;
            }
            return Ok((*i, TokenKind::Binary(Rc::from(bytes_out.as_slice()))));
        }
        if !c.is_ascii_hexdigit() {
            return Err(LexError::InvalidBinary {
                span: Span::new(start, *i),
                chars: format!("{:?}", c as char),
            });
        }
        hex.push(c as char);
        *i += 1;
    }
    // EOF before `}`.
    Err(LexError::InvalidBinary {
        span: Span::new(start, bytes.len()),
        chars: src[start..].to_string(),
    })
}

/// Decode a single "char unit" starting at `*i` inside a `#"..."` literal.
/// On error returns a `String` describing the bad input (caller wraps as
/// `InvalidChar`).
fn decode_char_unit(src: &str, bytes: &[u8], i: &mut usize) -> Result<char, String> {
    let c = bytes[*i];
    if c == b'^' {
        *i += 1;
        if *i >= bytes.len() {
            return Err("caret escape at EOF".into());
        }
        let esc = bytes[*i];
        *i += 1;
        // `^(NN)` — codepoint hex form.
        if esc == b'(' {
            let mut hex = String::new();
            while *i < bytes.len() && bytes[*i] != b')' {
                let h = bytes[*i];
                if !h.is_ascii_hexdigit() {
                    return Err(format!("invalid hex digit in ^(...): {:?}", h as char));
                }
                hex.push(h as char);
                *i += 1;
                if hex.len() > 6 {
                    return Err("^(...) codepoint too long".into());
                }
            }
            if *i >= bytes.len() {
                return Err("unterminated ^(...)".into());
            }
            *i += 1; // consume `)`
            if hex.is_empty() {
                return Err("empty ^()".into());
            }
            let n = u32::from_str_radix(&hex, 16)
                .map_err(|e| format!("bad codepoint ^({hex}): {e}"))?;
            return char::from_u32(n).ok_or_else(|| format!("codepoint ^({hex}) out of range",));
        }
        // Single-char caret escape.
        // Single-char caret escape.
        let decoded = match esc {
            b'-' => '\t',       // tab
            b'/' => '\n',       // newline
            b'@' => '\u{0000}', // null
            b'^' => '^',        // literal caret
            b'"' => '"',        // literal quote
            b')' => ')',
            b'(' => '(',
            // `^M-C` meta form: control/meta syntax. Only triggered when `M`
            // is followed by `-` — otherwise `^M` is Ctrl-M (CR, codepoint 13).
            b'M' if bytes.get(*i) == Some(&b'-') => {
                *i += 1; // consume `-`
                if *i >= bytes.len() {
                    return Err("`^M-` at EOF".into());
                }
                let mc = bytes[*i];
                *i += 1;
                mc.wrapping_sub(0x40) as char
            }
            // Any other letter: `^A` = Ctrl-A (codepoint 1), `^M` = CR (13),
            // `^Z` = Ctrl-Z (26). Maps to `(letter - 0x40)` uppercase.
            _ => {
                if esc.is_ascii_uppercase() || esc.is_ascii_lowercase() {
                    esc.to_ascii_uppercase().wrapping_sub(0x40) as char
                } else {
                    esc as char
                }
            }
        };
        return Ok(decoded);
    }
    // Ordinary byte — decode one UTF-8 char.
    let ch_len = utf8_len(c);
    let s = src
        .get(*i..*i + ch_len)
        .ok_or("bad UTF-8 in char literal")?;
    let ch = s.chars().next().ok_or("empty char literal")?;
    *i += ch_len;
    Ok(ch)
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
/// whitespace, not a delimiter — handled in the main scan loop.) `/` is also
/// a delimiter so refinement words (`/foo`) and paths (`foo/bar`) split into
/// separate tokens; the parser reassembles paths.
fn is_delimiter(c: u8) -> bool {
    matches!(
        c,
        b' ' | b'\t' | b'\r' | b'\n' | b'[' | b']' | b'(' | b')' | b'{' | b'}' | b';' | b'"' | b'/'
    )
}

/// Scan a `/`-led token. If word chars follow the `/`, emit
/// `TokenKind::Refinement(Symbol)` covering `/word`. If nothing followable
/// follows (EOF, whitespace, another delimiter, or a digit/`-`-digit run
/// that should be scanned as a number instead), emit `Word("/")` — the bare
/// slash is Red's division operator and a valid word.
fn scan_refinement(src: &str, i: &mut usize) -> Result<(usize, TokenKind), LexError> {
    let start = *i;
    let bytes = src.as_bytes();
    *i += 1; // consume first `/`
             // `//` → modulo operator (a single `Word("//")` token). This must be
             // checked before the number/refinement classification below so that
             // `7 // 3` lexes as three tokens, not `7`, `/`, `/`, `3`.
    if bytes.get(*i) == Some(&b'/') {
        *i += 1;
        return Ok((*i, TokenKind::Word(Symbol::new("//"))));
    }
    // If the next char would start a number (`digit`, or `-` + digit), the
    // `/` is the division operator, not a refinement. Leave `*i` after the
    // `/` so the main loop scans the number next.
    let next = bytes.get(*i).copied();
    let starts_number = match next {
        Some(c) if c.is_ascii_digit() => true,
        Some(b'-') => bytes
            .get(*i + 1)
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false),
        _ => false,
    };
    if starts_number {
        return Ok((*i, TokenKind::Word(Symbol::new("/"))));
    }
    // Consume a run of non-delimiter bytes as the refinement body.
    while *i < bytes.len() && !is_delimiter(bytes[*i]) {
        *i += 1;
    }
    let end = *i;
    if end == start + 1 {
        // Bare `/` — division operator.
        return Ok((end, TokenKind::Word(Symbol::new("/"))));
    }
    let body = &src[start + 1..end];
    // Refinement bodies use the same char rules as words; reject all-colon
    // / all-quote bodies for consistency with `classify_word`.
    if body.chars().all(|c| c == ':' || c == '\'') {
        return Err(LexError::InvalidWord {
            span: Span::new(start, end),
        });
    }
    // Set-path support (M19): if the refinement body ends with a single
    // trailing `:`, the user wrote `obj/field:` — we want this to lex as
    // `Refinement("field")` followed by `SetWord("field")` so the parser
    // can fold the run into a `SetPath`. To produce both tokens, back the
    // cursor up to the start of the body (so the main loop re-scans
    // `field:` as a SetWord on the next iteration) and return the
    // Refinement with its span ending just before the `:`. The two tokens'
    // spans overlap in source (both cover `field`), which is fine — spans
    // are only for error reporting.
    let body_bytes = body.as_bytes();
    if body_bytes.len() >= 2 && body_bytes[body_bytes.len() - 1] == b':' {
        let body_no_colon = &body[..body.len() - 1];
        // Reject `field::` (double trailing colon) — that would produce a
        // Refinement + a malformed SetWord. Let classify_word catch it by
        // leaving the body as-is and falling through.
        if !body_no_colon.is_empty() && !body_no_colon.ends_with(':') {
            // Span end = position of the trailing `:`.
            let ref_end = end - 1;
            // Back the cursor up to the start of the body so the main loop
            // scans `field:` as a SetWord next.
            *i = start + 1;
            return Ok((ref_end, TokenKind::Refinement(Symbol::new(body_no_colon))));
        }
    }
    Ok((end, TokenKind::Refinement(Symbol::new(body))))
}

/// `%foo/bar.txt` or `%"foo bar.txt"` — a file! literal. Two forms:
/// - Bare: `%` followed by a run of non-delimiter bytes (where `/` is
///   allowed, so paths stay together).
/// - Quoted: `%` followed by a `"..."` string (with standard escapes), used
///   when the path contains delimiters like spaces. The decoded string is
///   stored as the file path (without the `%` or quotes).
fn scan_file(src: &str, i: &mut usize) -> Result<(usize, TokenKind), LexError> {
    let start = *i;
    let bytes = src.as_bytes();
    *i += 1; // consume leading `%`
             // Quoted form: `%"..."`.
    if bytes.get(*i) == Some(&b'"') {
        let (end, body) = scan_quoted(src, i)?;
        return Ok((end, TokenKind::File(Rc::from(body.as_str()))));
    }
    // Bare form: run of non-delimiter bytes (with `/` allowed).
    while *i < bytes.len() && !is_file_delimiter(bytes[*i]) {
        *i += 1;
    }
    let end = *i;
    if end == start + 1 {
        return Err(LexError::InvalidWord {
            span: Span::new(start, end),
        });
    }
    let body = &src[start + 1..end];
    Ok((end, TokenKind::File(Rc::from(body))))
}

/// Delimiter set for file! runs: same as `is_delimiter` but `/` and `%` are
/// *not* delimiters here (paths contain slashes; the leading `%` is the
/// marker).
fn is_file_delimiter(c: u8) -> bool {
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

/// If a URL scheme starts at `i` (alpha-led run followed by `://`), return
/// the byte offset just past the scheme (the position of the `:`). The
/// caller verifies the scheme body chars.
fn url_scheme_end(src: &str, i: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    if i >= bytes.len() || !bytes[i].is_ascii_alphabetic() {
        return None;
    }
    let mut j = i + 1;
    while j < bytes.len() {
        let c = bytes[j];
        if c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.' {
            j += 1;
        } else {
            break;
        }
    }
    // Need `://` at position j, with at least one char after.
    if j + 2 < bytes.len() && bytes[j] == b':' && bytes[j + 1] == b'/' && bytes[j + 2] == b'/' {
        Some(j)
    } else {
        None
    }
}

/// Scan a url! literal starting at `i`. `scheme_end` is the offset of the
/// `:` in `://` (returned by `url_scheme_end`). Consumes the scheme, `://`,
/// and a run of non-delimiter bytes (where `/` is allowed). Returns the end
/// offset and `TokenKind::Url`.
fn scan_url(src: &str, i: &mut usize, _scheme_end: usize) -> (usize, TokenKind) {
    let start = *i;
    let bytes = src.as_bytes();
    // Skip scheme + `://`.
    *i += 1; // first alpha (already validated)
    while *i < bytes.len() {
        let c = bytes[*i];
        if c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.' {
            *i += 1;
        } else {
            break;
        }
    }
    // Consume `://`.
    *i += 3;
    // Consume url body: non-delimiter (with `/` allowed, like files).
    while *i < bytes.len() && !is_file_delimiter(bytes[*i]) {
        *i += 1;
    }
    let end = *i;
    let url = &src[start..end];
    (end, TokenKind::Url(Rc::from(url)))
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
    fn char_literal_basic() {
        assert_eq!(one("#\"a\""), TokenKind::Char('a'));
        assert_eq!(one("#\"Z\""), TokenKind::Char('Z'));
        assert_eq!(one("#\"1\""), TokenKind::Char('1'));
    }

    #[test]
    fn char_literal_caret_escape() {
        // `^-` tab, `^/` newline, `^@` null, `^^` literal caret, `^"` quote.
        assert_eq!(one("#\"^-\""), TokenKind::Char('\t'));
        assert_eq!(one("#\"^/\""), TokenKind::Char('\n'));
        assert_eq!(one("#\"^@\""), TokenKind::Char('\u{0}'));
        assert_eq!(one("#\"^^\""), TokenKind::Char('^'));
        assert_eq!(one("#\"^\"\""), TokenKind::Char('"'));
    }

    #[test]
    fn char_literal_control_letter() {
        // `^A` = Ctrl-A = codepoint 1; `^M` = CR = 13.
        assert_eq!(one("#\"^A\""), TokenKind::Char('\u{1}'));
        assert_eq!(one("#\"^M\""), TokenKind::Char('\r'));
        assert_eq!(one("#\"^Z\""), TokenKind::Char('\u{1A}'));
    }

    #[test]
    fn char_literal_codepoint_hex() {
        assert_eq!(one("#\"^(41)\""), TokenKind::Char('A'));
        assert_eq!(one("#\"^(61)\""), TokenKind::Char('a'));
        assert_eq!(one("#\"^(1F600)\""), TokenKind::Char('\u{1F600}'));
    }

    #[test]
    fn char_literal_unterminated() {
        let err = lex("#\"a").unwrap_err();
        assert!(matches!(err, LexError::InvalidChar { .. }));
        // Bad codepoint form.
        let err = lex("#\"^(ZZ)\"").unwrap_err();
        assert!(matches!(err, LexError::InvalidChar { .. }));
    }

    #[test]
    fn char_literal_does_not_affect_bare_hash_word() {
        // A bare `#` not followed by `"` or `{` should fall through to word scan.
        assert!(matches!(one("#foo"), TokenKind::Word(_)));
    }

    #[test]
    fn binary_literal_even_hex() {
        assert_eq!(
            one("#{48656C6C6F}"),
            TokenKind::Binary(Rc::from(&[0x48, 0x65, 0x6C, 0x6C, 0x6F][..]))
        );
        assert_eq!(one("#{00}"), TokenKind::Binary(Rc::from(&[0x00][..])));
        assert_eq!(one("#{FF}"), TokenKind::Binary(Rc::from(&[0xFF][..])));
    }

    #[test]
    fn binary_literal_odd_hex_zero_pads_high_nibble() {
        // `#{ABC}` (3 hex digits) → `[0x0A, 0xBC]` (high nibble zero-padded).
        assert_eq!(
            one("#{ABC}"),
            TokenKind::Binary(Rc::from(&[0x0A, 0xBC][..]))
        );
        assert_eq!(one("#{1}"), TokenKind::Binary(Rc::from(&[0x01][..])));
        assert_eq!(one("#{F}"), TokenKind::Binary(Rc::from(&[0x0F][..])));
    }

    #[test]
    fn binary_literal_lowercase_hex_accepted() {
        assert_eq!(
            one("#{deadbeef}"),
            TokenKind::Binary(Rc::from(&[0xDE, 0xAD, 0xBE, 0xEF][..]))
        );
    }

    #[test]
    fn binary_literal_empty() {
        assert_eq!(one("#{}"), TokenKind::Binary(Rc::from(&[][..])));
    }

    #[test]
    fn binary_literal_unterminated() {
        let err = lex("#{00").unwrap_err();
        assert!(matches!(err, LexError::InvalidBinary { .. }));
    }

    #[test]
    fn binary_literal_non_hex_char() {
        let err = lex("#{XY}").unwrap_err();
        assert!(matches!(err, LexError::InvalidBinary { .. }));
        let err = lex("#{12 G4}").unwrap_err();
        assert!(matches!(err, LexError::InvalidBinary { .. }));
    }

    #[test]
    fn binary_literal_span_covers_full_token() {
        let toks = lex("#{42}").unwrap();
        assert_eq!(toks.len(), 1);
        // Span covers `#{42}` (5 bytes).
        assert_eq!(toks[0].span, Span::new(0, 5));
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

    #[test]
    fn refinement_word() {
        assert_eq!(one("/part"), TokenKind::Refinement(Symbol::new("part")));
        assert_eq!(one("/only"), TokenKind::Refinement(Symbol::new("only")));
        assert_eq!(one("/case"), TokenKind::Refinement(Symbol::new("case")));
    }

    #[test]
    fn bare_slash_is_division_word() {
        // `/` alone is the division operator (a word), not a refinement.
        assert_eq!(one("/"), TokenKind::Word(Symbol::new("/")));
    }

    #[test]
    fn double_slash_is_modulo_word() {
        // `//` is the modulo operator — a single word token.
        assert_eq!(one("//"), TokenKind::Word(Symbol::new("//")));
    }

    #[test]
    fn modulo_expression_splits() {
        // `7 // 3` lexes as three tokens (integer, modulo word, integer).
        let toks = kinds("7 // 3");
        assert_eq!(
            toks,
            vec![
                TokenKind::Integer(7),
                TokenKind::Word(Symbol::new("//")),
                TokenKind::Integer(3),
            ]
        );
    }

    #[test]
    fn path_splits_into_word_and_refinement() {
        // `foo/bar` — `/` is a delimiter, so this is two tokens.
        let toks = kinds("foo/bar");
        assert_eq!(
            toks,
            vec![
                TokenKind::Word(Symbol::new("foo")),
                TokenKind::Refinement(Symbol::new("bar")),
            ]
        );
    }

    #[test]
    fn path_three_segments() {
        let toks = kinds("a/b/c");
        assert_eq!(
            toks,
            vec![
                TokenKind::Word(Symbol::new("a")),
                TokenKind::Refinement(Symbol::new("b")),
                TokenKind::Refinement(Symbol::new("c")),
            ]
        );
    }

    #[test]
    fn spaced_refinement_stays_separate() {
        // `copy /part` — space separates, so both are distinct tokens and
        // `/part` is a standalone Refinement (not folded into a path by the
        // lexer; the parser's adjacency check decides path assembly).
        let toks = kinds("copy /part");
        assert_eq!(
            toks,
            vec![
                TokenKind::Word(Symbol::new("copy")),
                TokenKind::Refinement(Symbol::new("part")),
            ]
        );
    }

    #[test]
    fn division_expression_splits() {
        // `1/2` now splits into Integer, Word("/"), Integer (was one word in
        // the pre-refinement lexer). `1 / 2` is Red division.
        let toks = kinds("1/2");
        assert_eq!(
            toks,
            vec![
                TokenKind::Integer(1),
                TokenKind::Word(Symbol::new("/")),
                TokenKind::Integer(2),
            ]
        );
    }

    #[test]
    fn refinement_span_covers_leading_slash() {
        let toks = lex("/part").expect("lex");
        assert_eq!(toks[0].span, Span::new(0, 5));
    }

    // --- M19 set-path lexing ---

    #[test]
    fn set_path_splits_into_refinement_and_setword() {
        // `obj/field:` lexes as Word(obj) Refinement(field) SetWord(field).
        // The parser folds this run into a `SetPath` value.
        let toks = kinds("obj/field:");
        assert_eq!(
            toks,
            vec![
                TokenKind::Word(Symbol::new("obj")),
                TokenKind::Refinement(Symbol::new("field")),
                TokenKind::SetWord(Symbol::new("field")),
            ]
        );
    }

    #[test]
    fn set_path_three_segments() {
        // `a/b/c:` → Word(a) Refinement(b) Refinement(c) SetWord(c)
        let toks = kinds("a/b/c:");
        assert_eq!(
            toks,
            vec![
                TokenKind::Word(Symbol::new("a")),
                TokenKind::Refinement(Symbol::new("b")),
                TokenKind::Refinement(Symbol::new("c")),
                TokenKind::SetWord(Symbol::new("c")),
            ]
        );
    }

    #[test]
    fn set_path_refinement_span_excludes_trailing_colon() {
        let toks = lex("obj/field:").expect("lex");
        // Refinement span covers `/field` (bytes 3..9, end exclusive), not
        // the trailing `:`.
        assert_eq!(toks[1].span, Span::new(3, 9));
        // SetWord span covers `field:` (bytes 4..10, end exclusive).
        assert_eq!(toks[2].span, Span::new(4, 10));
    }

    // --- M20 file! / url! literals ---

    #[test]
    fn file_bare() {
        assert_eq!(
            one("%foo/bar.txt"),
            TokenKind::File(Rc::from("foo/bar.txt"))
        );
        assert_eq!(one("%foo"), TokenKind::File(Rc::from("foo")));
    }

    #[test]
    fn file_quoted_with_spaces() {
        assert_eq!(
            one("%\"with space.txt\""),
            TokenKind::File(Rc::from("with space.txt"))
        );
    }

    #[test]
    fn file_quoted_with_escapes() {
        assert_eq!(one("%\"a\\\"b\""), TokenKind::File(Rc::from("a\"b")));
    }

    #[test]
    fn file_bare_stops_at_delimiters() {
        let toks = kinds("%foo/bar.txt]");
        assert_eq!(
            toks,
            vec![
                TokenKind::File(Rc::from("foo/bar.txt")),
                TokenKind::RBracket,
            ]
        );
    }

    #[test]
    fn file_empty_is_error() {
        let err = lex("%").unwrap_err();
        assert!(matches!(err, LexError::InvalidWord { .. }));
    }

    #[test]
    fn file_span_covers_percent() {
        let toks = lex("%foo").expect("lex");
        assert_eq!(toks[0].span, Span::new(0, 4));
    }

    #[test]
    fn url_http() {
        assert_eq!(
            one("http://example.com/x"),
            TokenKind::Url(Rc::from("http://example.com/x"))
        );
    }

    #[test]
    fn url_https() {
        assert_eq!(
            one("https://red-lang.org/"),
            TokenKind::Url(Rc::from("https://red-lang.org/"))
        );
    }

    #[test]
    fn url_file_scheme() {
        assert_eq!(
            one("file://localhost/a/b"),
            TokenKind::Url(Rc::from("file://localhost/a/b"))
        );
    }

    #[test]
    fn url_not_word_without_scheme_separator() {
        // `foo` is just a word — no `://`, so no url.
        assert_eq!(one("foo"), TokenKind::Word(Symbol::new("foo")));
        // `http:foo` (no `//`) is a set-word `http:` then... actually `:` is
        // non-delimiter so this scans as one word `http:foo` → SetWord? No:
        // trailing `:` only triggers SetWord when it's a single trailing
        // colon. `http:foo` has the colon in the middle → Word.
        assert_eq!(one("http:foo"), TokenKind::Word(Symbol::new("http:foo")));
    }

    #[test]
    fn url_scheme_must_start_alpha() {
        // `1http://x` — starts with a digit, so it's scanned as a number
        // (stops at the non-digit `h`) then a word; no url.
        let toks = kinds("1http://x");
        assert_eq!(toks[0], TokenKind::Integer(1));
    }

    #[test]
    fn url_in_block() {
        let toks = kinds("[http://a/b %foo/bar]");
        assert_eq!(
            toks,
            vec![
                TokenKind::LBracket,
                TokenKind::Url(Rc::from("http://a/b")),
                TokenKind::File(Rc::from("foo/bar")),
                TokenKind::RBracket,
            ]
        );
    }
}
