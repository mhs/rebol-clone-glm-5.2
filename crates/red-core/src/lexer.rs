//! Lexer: source string → `Vec<Token>`. Whitespace-delimited, `;` comments,
//! `"..."` and `{...}` strings, integers/floats, word family, `[ ] ( )`.
//!
//! Single-character lookahead, no backtracking. Every token carries a byte-
//! offset `Span` so the parser/CLI can point at the offending bytes.

use std::rc::Rc;

use chrono::{NaiveDate, NaiveTime};

use crate::value::{DateValue, MoneyValue, Span, Symbol};

/// One lexical token.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Integer(i64),
    Float(f64),
    /// `3.14dec` — a decimal! literal (M150). Backed by
    /// `rust_decimal::Decimal` (28-digit precision, 96-bit mantissa, no
    /// NaN/Inf). Produced by `scan_number` when a digit run is immediately
    /// followed by the `dec` suffix (collision-free — duration unit
    /// suffixes are 1-2 chars `s`/`m`/`h`/`d`/`ms`/`us`/`ns`, never
    /// `dec`). The suffix must be followed by a delimiter/EOF (not a
    /// word-extending char) to commit — `3.14decal` lexes as float `3.14`
    /// + word `decal`.
    Decimal(rust_decimal::Decimal),
    /// `50%` — a percent! literal (M80). Stored as the fractional float
    /// (`50%` ⇒ 0.5). Produced by `scan_number` when a digit run is
    /// immediately followed by `%` (the `%` does not collide with file!
    /// literals because `%`-files don't follow digits).
    Percent(f64),
    /// `$10.00` / `$1,234.56:EUR` — a money! literal (M80). Stored as a
    /// fully-parsed `MoneyValue` (integer cents + currency code). The lexer
    /// validates the structure (digits, optional fractional, optional `:CCC`
    /// suffix, optional inter-digit commas which are stripped on lex).
    Money(MoneyValue),
    /// `#1234` / `#ABC` — an issue! literal (M80). Stored as the raw body
    /// `Rc<str>` (without the leading `#`). Produced by `scan_issue` when a
    /// `#` is followed by anything other than `"` (char!) or `{` (binary!).
    Issue(Rc<str>),
    /// `foo@bar.com` — an email! literal (M80). Stored as the raw address
    /// `Rc<str>`. Produced when a word run contains a single `@` with at
    /// least one dot in the host portion. Bare `user@localhost` (no dot in
    /// host) is NOT an email! — it lexes as a plain Word.
    Email(Rc<str>),
    /// `<b>` / `</p>` / `<img src="x">` — a tag! literal (M81). Stored as the
    /// raw body `Rc<str>` (the text between `<` and `>`, with `\<`/`\>`/`\\`
    /// escapes decoded). Produced by `scan_tag` when `<` is followed by a
    /// non-delimiter, non-operator char. `<` followed by EOF/delimiter/
    /// `=`/`<`/`>` falls through to `scan_word` (preserves `<=`/`<>`/`<`/`<<`
    /// as comparison operators).
    Tag(Rc<str>),
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
    /// `100x200` — a pair! literal. Stored as the two raw component substrings
    /// (each parses as int or float); the parser converts to `Value::Pair`.
    /// The lexer routes here via `detect_pair_tuple` (an `x` separator
    /// between digit-runs disambiguates from floats/words).
    Pair(Rc<str>, Rc<str>),
    /// `255.0.0` / `128.64.32.128` — a tuple! literal (RGB or RGBA bytes).
    /// 3 or 4 byte components, each 0–255, dot-joined. The lexer routes here
    /// via `detect_pair_tuple` (2 or 3 dots between digit-runs).
    Tuple(Rc<[u8]>),
    /// `29-Jun-2024` / `2024-06-29T12:30:00Z` / `12:30:00-04:00` — a `date!`
    /// literal (M45). Stored as a fully-parsed `DateValue` (the lexer
    /// validates the structure + values). A single variant covers date-only,
    /// date+time, and date+time+zone; the parser wraps it as `Value::Date`.
    Date(DateValue),
    /// `30s` / `1.5h` / `250ms` / `1d1h` — a `duration!` literal (M140).
    /// Stored as a fully-accumulated `chrono::Duration` (signed i64
    /// nanoseconds). The lexer accepts both single-unit (`30s`) and compound
    /// (`1d1h`/`1h30m45s`) forms; compound rules: strict descending unit
    /// order (`d`>`h`>`m`>`s`>`ms`>`us`>`ns`), no repeats, sub-component
    /// overflow rejected (`1h70m` is an error). Leading sign negates the
    /// whole literal (`-1d1h`); per-component negatives are not allowed.
    Duration(chrono::Duration),
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
    /// A `NN%` percent! literal where the value overflowed `f64` (e.g. a
    /// huge exponent run producing infinity). The digits parsed but the
    /// resulting `f64 / 100.0` is not finite.
    InvalidPercent { span: Span, chars: String },
    /// A `$...` money! literal with a malformed body (non-digit where a digit
    /// was expected, an unterminated currency suffix, fractional cents with
    /// more than 2 decimal places, etc.).
    InvalidMoney { span: Span, chars: String },
    /// A `#...` issue! literal with an empty body (e.g. `#` followed by
    /// whitespace or a delimiter). The `#`-then-word-char form is the only
    /// valid issue! shape; `#"` and `#{` are handled by the char!/binary!
    /// scanners before issue! is considered.
    InvalidIssue { span: Span, chars: String },
    /// A `@`-containing word run that looks like an email! but has an empty
    /// local part, empty host, or no dot in the host portion (e.g. `@bar.com`,
    /// `foo@`, `foo@bar`). Bare `user@localhost` (no dot) lexes as a Word
    /// instead — this error is reserved for runs that match the email shape
    /// structurally but fail validation.
    InvalidEmail { span: Span, chars: String },
    /// A `<...` tag! literal (M81) that hit EOF before the closing `>`.
    /// `chars` is the unterminated body (for the diagnostic).
    UnterminatedTag { span: Span, chars: String },
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
    /// `1x`-style pair! literal where the `x` is not followed by a valid
    /// number component, or one side is empty.
    InvalidPair { span: Span, chars: String },
    /// `255.0.0`-style tuple! literal with a component > 255, too many
    /// components (> 4), or too few (< 3).
    InvalidTuple { span: Span, chars: String },
    /// `31-Feb-2024`-style date! literal with an invalid date (bad day/month
    /// combination, out-of-range values, etc.). The run structurally matches
    /// a date/time form but the values don't validate.
    InvalidDate { span: Span, chars: String },
    /// `+15:00`-style zone offset suffix with |minutes| > 14*60 or a malformed
    /// suffix shape.
    InvalidZone { span: Span, chars: String },
    /// A `NNs`/`1d1h`-style duration! literal (M140) with a malformed body:
    /// non-descending or repeated unit (`1h1d`/`1h1h`), sub-component
    /// overflow (`1h70m`/`1d25h`/`1m60s`), or a non-numeric magnitude. The
    /// run structurally matches a duration form (digit run + unit suffix)
    /// but the values don't validate.
    InvalidDuration { span: Span, chars: String },
}

impl LexError {
    /// Byte-offset span where this error was detected.
    pub fn span(&self) -> Span {
        match self {
            LexError::UnterminatedString { span }
            | LexError::InvalidNumber { span, .. }
            | LexError::InvalidPercent { span, .. }
            | LexError::InvalidMoney { span, .. }
            | LexError::InvalidIssue { span, .. }
            | LexError::InvalidEmail { span, .. }
            | LexError::UnterminatedTag { span, .. }
            | LexError::InvalidWord { span }
            | LexError::UnbalancedBrace { span, .. }
            | LexError::InvalidChar { span, .. }
            | LexError::InvalidBinary { span, .. }
            | LexError::InvalidPair { span, .. }
            | LexError::InvalidTuple { span, .. }
            | LexError::InvalidDate { span, .. }
            | LexError::InvalidZone { span, .. }
            | LexError::InvalidDuration { span, .. } => *span,
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
            LexError::InvalidPercent { chars, .. } => {
                write!(f, "invalid percent literal: {chars:?}")
            }
            LexError::InvalidMoney { chars, .. } => {
                write!(f, "invalid money literal: {chars:?}")
            }
            LexError::InvalidIssue { chars, .. } => {
                write!(f, "invalid issue literal: {chars:?}")
            }
            LexError::InvalidEmail { chars, .. } => {
                write!(f, "invalid email literal: {chars:?}")
            }
            LexError::UnterminatedTag { chars, .. } => {
                write!(f, "unterminated tag literal: {chars:?}")
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
            LexError::InvalidPair { chars, .. } => {
                write!(f, "invalid pair literal: {chars:?}")
            }
            LexError::InvalidTuple { chars, .. } => {
                write!(f, "invalid tuple literal: {chars:?}")
            }
            LexError::InvalidDate { chars, .. } => {
                write!(f, "invalid date literal: {chars:?}")
            }
            LexError::InvalidZone { chars, .. } => {
                write!(f, "invalid zone offset: {chars:?}")
            }
            LexError::InvalidDuration { chars, .. } => {
                write!(f, "invalid duration literal: {chars:?}")
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

        // M80: Issue literal: `#` followed by a run of non-delimiter word chars
        // (letters, digits, `-`, `_`, `.`, `?`, `!`). This is the fall-through
        // after `#"` (char!) and `#{` (binary!). A bare `#` followed by a
        // delimiter (whitespace, `[`, etc.) is an error (InvalidIssue). A `#`
        // followed by other word chars produces `Issue("body")`.
        //
        // **Behavior change:** previously a bare `#foo` fell through to
        // `scan_word` and produced `Word("#foo")`. Now it produces
        // `Issue("foo")`. No existing fixture relied on `#word`-as-`Word`
        // (audited before this change).
        if c == b'#' {
            let (end, kind) = scan_issue(src, &mut i)?;
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

        // M80: Email literal: a word run containing `@` with at least one dot
        // in the host portion (`foo@bar.com`). `@` is a word character today
        // (not a delimiter), so `foo@bar.com` currently scans as a single
        // Word. Detect the email shape here (after URL, before the
        // number/date checks) and route to `scan_email`. A bare
        // `user@localhost` (no dot after `@`) is NOT an email! — it falls
        // through to `scan_word`.
        if c.is_ascii_alphabetic() {
            if let Some(end) = detect_email(src, i) {
                let addr: Rc<str> = Rc::from(&src[i..end]);
                i = end;
                out.push(Token {
                    kind: TokenKind::Email(addr),
                    span: Span::new(start, end),
                });
                continue;
            }
        }

        // Numbers: digit, or `-` followed by digit. M44: pre-detect pair!
        // (`NxM`) and tuple! (`R.G.B[.A]`) forms here so `scan_number` only
        // sees plain integers/floats (preserving its existing 2nd-dot error).
        // M45: pre-detect date!/time! forms first — `29-Jun-2024` starts with
        // a digit but has alpha chars that `scan_number` can't handle.
        if c.is_ascii_digit() || (c == b'-' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit())
        {
            // M45: date!/time! detection (before pair/tuple/number). Must run
            // first because `29-Jun-2024` starts with a digit but isn't a
            // valid number/pair/tuple.
            match detect_date_time(src, i) {
                DateDetect::NotADate => {}
                DateDetect::Valid { end, value } => {
                    out.push(Token {
                        kind: TokenKind::Date(value),
                        span: Span::new(start, end),
                    });
                    i = end;
                    continue;
                }
                DateDetect::Invalid { end } => {
                    return Err(LexError::InvalidDate {
                        span: Span::new(start, end),
                        chars: src[start..end].to_string(),
                    });
                }
            }
            let (end, kind) = match detect_pair_tuple(src, i) {
                Some(true) => scan_pair(src, &mut i)?,
                Some(false) => scan_tuple(src, &mut i)?,
                None => scan_number(src, &mut i)?,
            };
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

        // Money literal: `$`-prefixed run (`$10.00`, `$1,234.56:EUR`). `$`
        // is not a delimiter today (it's a word character), so a bare `$foo`
        // would otherwise scan as a Word; this arm intercepts `$` followed by
        // a digit and routes to `scan_money`. `$` not followed by a digit
        // falls through to `scan_word` (preserving `$foo`-as-word). A leading
        // `-` (`-$10.00`) is also handled here.
        if c == b'$' && bytes.get(i + 1).is_some_and(|b| b.is_ascii_digit()) {
            let (end, kind) = scan_money(src, &mut i, false)?;
            out.push(Token {
                kind,
                span: Span::new(start, end),
            });
            continue;
        }
        if c == b'-'
            && bytes.get(i + 1) == Some(&b'$')
            && bytes.get(i + 2).is_some_and(|b| b.is_ascii_digit())
        {
            i += 1; // consume the `-`
            let (end, kind) = scan_money(src, &mut i, true)?;
            out.push(Token {
                kind,
                span: Span::new(start, end),
            });
            continue;
        }

        // M81: Tag literal: `<...>`. A `<` starts a tag ONLY when followed by
        // a non-delimiter, non-operator char. `<` followed by EOF, a delimiter
        // (`[](){}/;"`+whitespace), or an operator char (`=`/`<`/`>`) falls
        // through to `scan_word` so the comparison operators (`<`/`<=`/`<>`/
        // `<<`) lex as Words (today's behavior). Escapes `\<`/`\>`/`\\` are
        // honored inside the tag body; EOF before `>` → `UnterminatedTag`.
        if c == b'<' && starts_tag_body(bytes.get(i + 1).copied()) {
            let (end, kind) = scan_tag(src, &mut i)?;
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

    // M80: a digit run immediately followed by `%` is a percent! literal
    // (`50%` ⇒ 0.5). The `%`-file dispatch arm in the main scan loop never
    // sees this case because the digit branch routes here first; a bare `%`
    // (not preceded by a digit) still starts a file! literal.
    if *i < bytes.len() && bytes[*i] == b'%' {
        let pct_end = *i + 1;
        // Parse the digit run as f64 (treat as float regardless of `is_float`,
        // since percent is always fractional). An integer-shaped run like
        // `50` parses as 50.0 then divides by 100.0 → 0.5.
        let raw = match text.parse::<f64>() {
            Ok(f) => f / 100.0,
            Err(_) => {
                return Err(LexError::InvalidNumber {
                    span: Span::new(start, pct_end),
                    chars: src[start..pct_end].to_string(),
                });
            }
        };
        if !raw.is_finite() {
            return Err(LexError::InvalidPercent {
                span: Span::new(start, pct_end),
                chars: src[start..pct_end].to_string(),
            });
        }
        *i = pct_end;
        return Ok((pct_end, TokenKind::Percent(raw)));
    }

    // M140: a digit run immediately followed by a duration unit suffix is a
    // duration! literal (`30s`/`1.5h`/`250ms`/`1d1h`). Both single-unit and
    // compound forms are accepted. If no suffix matches or the collision
    // guard rejects (the char after the suffix is a word-extending char),
    // fall through to the Integer/Float assembly below.
    if let Some((dur_end, duration)) = try_scan_duration(src, i, start, end, bytes)? {
        return Ok((dur_end, TokenKind::Duration(duration)));
    }

    // M150: a digit run immediately followed by `dec` is a decimal! literal
    // (`3.14dec`/`100dec`/`1e9dec`). Collision-free — duration unit suffixes
    // are 1-2 chars (`s`/`m`/`h`/`d`/`ms`/`us`/`ns`), never `dec`, and `%`
    // (percent) is checked above. The suffix must be followed by a delimiter
    // or EOF (not a word-extending char) to commit — `3.14decal` lexes as
    // float `3.14` + word `decal`. A following digit (e.g. `3dec1`) is also
    // rejected — that would be a word boundary (digit-runs in words are
    // allowed mid-word, but the digit immediately after `dec` would make it
    // `dec1`, not a clean suffix). Matching the duration guard's policy:
    // delimiter/EOF commits, anything else falls through.
    if end + 3 <= bytes.len() && &bytes[end..end + 3] == b"dec" {
        let after = end + 3;
        let committed = after >= bytes.len() || is_delimiter(bytes[after]);
        if committed {
            let text = &src[start..end];
            match text.parse::<rust_decimal::Decimal>() {
                Ok(d) => {
                    *i = after;
                    return Ok((after, TokenKind::Decimal(d)));
                }
                Err(_) => {
                    return Err(LexError::InvalidNumber {
                        span: Span::new(start, after),
                        chars: src[start..after].to_string(),
                    });
                }
            }
        }
    }

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

/// M140: unit info for a duration suffix.
struct DurationUnit {
    factor: i64,
    rank: u8,
}

/// M140: match the longest duration unit suffix at `pos`. Returns
/// `(unit, suffix_len, committed)`. `committed` is false when the collision
/// guard rejects (the char after the suffix is a word-extending char — i.e.
/// not a delimiter, not EOF, and not a digit). 2-char suffixes (`ms`/`us`/
/// `ns`) are tried before 1-char (`s`/`m`/`h`/`d`).
fn match_unit_suffix(bytes: &[u8], pos: usize) -> Option<(&DurationUnit, usize, bool)> {
    const D: DurationUnit = DurationUnit {
        factor: 86_400_000_000_000,
        rank: 6,
    };
    const H: DurationUnit = DurationUnit {
        factor: 3_600_000_000_000,
        rank: 5,
    };
    const M: DurationUnit = DurationUnit {
        factor: 60_000_000_000,
        rank: 4,
    };
    const S: DurationUnit = DurationUnit {
        factor: 1_000_000_000,
        rank: 3,
    };
    const MS: DurationUnit = DurationUnit {
        factor: 1_000_000,
        rank: 2,
    };
    const US: DurationUnit = DurationUnit {
        factor: 1_000,
        rank: 1,
    };
    const NS: DurationUnit = DurationUnit { factor: 1, rank: 0 };
    // 2-char suffixes first (longest match).
    if pos + 2 <= bytes.len() {
        let u = match &bytes[pos..pos + 2] {
            b"ms" => Some(&MS),
            b"us" => Some(&US),
            b"ns" => Some(&NS),
            _ => None,
        };
        if let Some(u) = u {
            let after = pos + 2;
            let committed =
                after >= bytes.len() || is_delimiter(bytes[after]) || bytes[after].is_ascii_digit();
            return Some((u, 2, committed));
        }
    }
    // 1-char suffixes.
    if pos < bytes.len() {
        let u = match bytes[pos] {
            b'd' => Some(&D),
            b'h' => Some(&H),
            b'm' => Some(&M),
            b's' => Some(&S),
            _ => None,
        };
        if let Some(u) = u {
            let after = pos + 1;
            let committed =
                after >= bytes.len() || is_delimiter(bytes[after]) || bytes[after].is_ascii_digit();
            return Some((u, 1, committed));
        }
    }
    None
}

/// M140: try to scan a duration! literal from a digit run that `scan_number`
/// already consumed. `start..end` is the first digit run (possibly including
/// a leading `-`); `*i == end` (cursor just past the run). Returns
/// `Some((token_end, duration))` if a unit suffix commits at `*i`; returns
/// `None` if no suffix matches or the collision guard rejects (fall back to
/// Integer/Float). Errors on structural validation failures (non-descending
/// or repeated unit, sub-component overflow).
///
/// Compound rules: strict descending unit order (`d`>`h`>`m`>`s`>`ms`>`us`>
/// `ns`), no repeats, sub-component overflow rejected (`1h70m` errors because
/// 70m ≥ 1h). Leading sign negates the whole literal; per-component negatives
/// are not allowed (the leading `-` was consumed by `scan_number`).
fn try_scan_duration(
    src: &str,
    i: &mut usize,
    start: usize,
    end: usize,
    bytes: &[u8],
) -> Result<Option<(usize, chrono::Duration)>, LexError> {
    // Try to match the first unit suffix at `end` (= `*i`).
    let (first_unit, suffix_len, committed) = match match_unit_suffix(bytes, end) {
        Some((u, sl, true)) => (u, sl, true),
        _ => return Ok(None), // no suffix or collision guard rejected
    };
    let _ = committed; // already checked true

    let negative = bytes.get(start) == Some(&b'-');
    let mag_start = if negative { start + 1 } else { start };
    let first_text = &src[mag_start..end];

    let first_mag = match first_text.parse::<f64>() {
        Ok(f) => f,
        Err(_) => {
            let err_end = end + suffix_len;
            return Err(LexError::InvalidDuration {
                span: Span::new(start, err_end),
                chars: src[start..err_end].to_string(),
            });
        }
    };

    let mut total_nanos: i128 = (first_mag * first_unit.factor as f64) as i128;
    let mut prev_rank = first_unit.rank;
    let mut cursor = end + suffix_len;

    loop {
        // Peek the next char to decide: compound continuation, done, or break.
        if cursor >= bytes.len() {
            break; // EOF — done
        }
        let next = bytes[cursor];
        if next.is_ascii_digit() {
            // Compound continuation — scan the next number run + suffix.
        } else if is_delimiter(next) {
            break; // delimiter — done
        } else {
            // Word-extending char (operator, letter, etc.) — shouldn't happen
            // for the first suffix (collision guard already rejected). For
            // subsequent suffixes, the guard also checked before committing,
            // so this branch is unreachable in normal flow.
            break;
        }

        // Scan the next number run (digits + optional fractional + exponent).
        let comp_start = cursor;
        cursor += consume_digits(bytes, cursor);
        // Fractional part.
        if cursor < bytes.len()
            && bytes[cursor] == b'.'
            && cursor + 1 < bytes.len()
            && bytes[cursor + 1].is_ascii_digit()
        {
            cursor += 1;
            cursor += consume_digits(bytes, cursor);
        }
        // Exponent part.
        if cursor < bytes.len() && (bytes[cursor] == b'e' || bytes[cursor] == b'E') {
            let saved = cursor;
            cursor += 1;
            if cursor < bytes.len() && (bytes[cursor] == b'+' || bytes[cursor] == b'-') {
                cursor += 1;
            }
            if cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                cursor += consume_digits(bytes, cursor);
            } else {
                cursor = saved + 1;
            }
        }
        let comp_text = &src[comp_start..cursor];

        // Match the unit suffix at `cursor`.
        let (unit, sl, comm) = match match_unit_suffix(bytes, cursor) {
            Some((u, sl, true)) => (u, sl, true),
            _ => {
                // No committed suffix — the digit run is standalone. Rewind
                // to `comp_start` so the main loop emits it as Integer/Float.
                // This handles `1d1hx` → Duration(1d) + Integer(1) + Word("hx").
                cursor = comp_start;
                break;
            }
        };
        let _ = comm;

        let comp_mag = match comp_text.parse::<f64>() {
            Ok(f) => f,
            Err(_) => {
                let err_end = cursor + sl;
                return Err(LexError::InvalidDuration {
                    span: Span::new(start, err_end),
                    chars: src[start..err_end].to_string(),
                });
            }
        };

        // Descending check: current rank must be strictly less than prev.
        if unit.rank >= prev_rank {
            let err_end = cursor + sl;
            return Err(LexError::InvalidDuration {
                span: Span::new(start, err_end),
                chars: src[start..err_end].to_string(),
            });
        }

        // Sub-component overflow check: current contribution must be strictly
        // less than the prev (next-larger) unit's factor.
        let prev_factor = unit_factor_by_rank(prev_rank);
        let comp_contrib = comp_mag * unit.factor as f64;
        if comp_contrib >= prev_factor as f64 {
            let err_end = cursor + sl;
            return Err(LexError::InvalidDuration {
                span: Span::new(start, err_end),
                chars: src[start..err_end].to_string(),
            });
        }

        total_nanos += (comp_mag * unit.factor as f64) as i128;
        prev_rank = unit.rank;
        cursor += sl;
    }

    if negative {
        total_nanos = -total_nanos;
    }

    // Saturate to i64 (~292-year range).
    let ns = if total_nanos > i64::MAX as i128 {
        i64::MAX
    } else if total_nanos < i64::MIN as i128 {
        i64::MIN
    } else {
        total_nanos as i64
    };
    let duration = chrono::Duration::nanoseconds(ns);

    *i = cursor;
    Ok(Some((cursor, duration)))
}

/// M140: look up the nanosecond factor for a unit rank.
fn unit_factor_by_rank(rank: u8) -> i64 {
    match rank {
        6 => 86_400_000_000_000, // d
        5 => 3_600_000_000_000,  // h
        4 => 60_000_000_000,     // m
        3 => 1_000_000_000,      // s
        2 => 1_000_000,          // ms
        1 => 1_000,              // us
        _ => 1,                  // ns
    }
}

/// M80: scan a `$`-led money! literal. Forms accepted:
/// - `$<digits>` → integer cents (`$10` ⇒ 1000 cents).
/// - `$<digits>.<digits>` → dollars.cents (`$10.00` ⇒ 1000 cents). The
///   fractional part must be exactly 2 digits (Red parity: no `$10.5`).
/// - `$<digits>,<digits>[,...].<digits>` → comma-grouped whole part
///   (`$1,234.56` ⇒ 123456 cents). Commas are stripped on lex; they may
///   appear only between digit groups (not leading/trailing/double).
/// - Optional `:CCC` currency suffix (3 ASCII letters; default `USD`).
///
/// Negative amounts use a leading `-` (e.g. `-$10.00`) — the `-` is
/// consumed by the main loop's number branch, so `-$10.00` reaches here
/// without the `-`. This scanner handles only the `$`-onward portion; the
/// sign must be applied by the caller (here we just parse the magnitude and
/// let the constructor take a signed cents). For the common `$10.00` form the
/// sign is always positive.
///
/// Errors: `InvalidMoney` on malformed shape, fractional cents with ≠ 2
/// decimal digits, a bad currency suffix, or i64 overflow.
fn scan_money(src: &str, i: &mut usize, negative: bool) -> Result<(usize, TokenKind), LexError> {
    let start = if negative { *i - 1 } else { *i };
    let bytes = src.as_bytes();
    *i += 1; // consume the `$`

    // Whole part: digits with optional inter-digit commas. Commas must be
    // followed by a digit (no leading/trailing/double commas).
    let mut whole_digits = String::new();
    while *i < bytes.len() {
        let b = bytes[*i];
        if b.is_ascii_digit() {
            whole_digits.push(b as char);
            *i += 1;
        } else if b == b',' {
            // Comma must be followed by a digit.
            if *i + 1 >= bytes.len() || !bytes[*i + 1].is_ascii_digit() {
                let end = *i + 1;
                return Err(LexError::InvalidMoney {
                    span: Span::new(start, end),
                    chars: src[start..end].to_string(),
                });
            }
            *i += 1; // consume the comma (digit consumed next iteration)
        } else {
            break;
        }
    }
    if whole_digits.is_empty() {
        let end = *i;
        return Err(LexError::InvalidMoney {
            span: Span::new(start, end),
            chars: src[start..end].to_string(),
        });
    }

    // Fractional part: optional `.DD` (exactly 2 digits).
    let mut frac_digits = String::new();
    if *i < bytes.len() && bytes[*i] == b'.' {
        *i += 1; // consume `.`
                 // Read exactly 2 fractional digits.
        for _ in 0..2 {
            if *i < bytes.len() && bytes[*i].is_ascii_digit() {
                frac_digits.push(bytes[*i] as char);
                *i += 1;
            } else {
                let end = *i;
                return Err(LexError::InvalidMoney {
                    span: Span::new(start, end),
                    chars: src[start..end].to_string(),
                });
            }
        }
        // A third digit after the decimal is an error (Red requires exactly 2).
        if *i < bytes.len() && bytes[*i].is_ascii_digit() {
            let end = *i + 1;
            return Err(LexError::InvalidMoney {
                span: Span::new(start, end),
                chars: src[start..end].to_string(),
            });
        }
    }

    // Optional currency suffix: `:CCC` (3 ASCII letters).
    let mut currency: Rc<str> = Rc::from("USD");
    if *i < bytes.len() && bytes[*i] == b':' {
        let suffix_start = *i;
        *i += 1; // consume `:`
        let mut code = String::new();
        for _ in 0..3 {
            if *i < bytes.len() && bytes[*i].is_ascii_alphabetic() {
                code.push(bytes[*i] as char);
                *i += 1;
            } else {
                let end = *i;
                return Err(LexError::InvalidMoney {
                    span: Span::new(start, end.max(suffix_start + 1)),
                    chars: src[start..end].to_string(),
                });
            }
        }
        // The suffix must be exactly 3 letters (no 4th letter).
        if *i < bytes.len() && bytes[*i].is_ascii_alphabetic() {
            let end = *i + 1;
            return Err(LexError::InvalidMoney {
                span: Span::new(start, end),
                chars: src[start..end].to_string(),
            });
        }
        currency = Rc::from(code.to_uppercase().as_str());
    }

    // Parse the whole-part digits as i64 (cents = whole * 100 + frac).
    let whole: i64 = whole_digits.parse().map_err(|_| LexError::InvalidMoney {
        span: Span::new(start, *i),
        chars: src[start..*i].to_string(),
    })?;
    let whole_cents = whole
        .checked_mul(100)
        .ok_or_else(|| LexError::InvalidMoney {
            span: Span::new(start, *i),
            chars: src[start..*i].to_string(),
        })?;
    let frac_cents: i64 = if frac_digits.is_empty() {
        0
    } else {
        frac_digits.parse().unwrap_or(0)
    };
    let cents = whole_cents
        .checked_add(frac_cents)
        .ok_or_else(|| LexError::InvalidMoney {
            span: Span::new(start, *i),
            chars: src[start..*i].to_string(),
        })?;
    let cents = if negative { -cents } else { cents };

    Ok((*i, TokenKind::Money(MoneyValue { cents, currency })))
}

/// M80: scan an issue! literal (`#body`). The caller has already confirmed
/// the `#` is not followed by `"` (char!) or `{` (binary!). This scanner
/// consumes the `#` and a run of issue-word chars (letters, digits, `-`,
/// `_`, `.`, `?`, `!`). An empty body (e.g. `#` followed by whitespace or
/// a delimiter) is an error.
fn scan_issue(src: &str, i: &mut usize) -> Result<(usize, TokenKind), LexError> {
    let start = *i;
    let bytes = src.as_bytes();
    *i += 1; // consume the `#`

    // Read a run of issue-word chars. The issue body accepts the same chars
    // as a word run (minus `:` which is a SetWord lead and `/` which is a
    // refinement delimiter — both delimiters).
    let body_start = *i;
    while *i < bytes.len() && is_issue_char(bytes[*i]) {
        *i += 1;
    }
    let body_end = *i;
    if body_end == body_start {
        let end = (*i).min(bytes.len()).max(start + 1);
        return Err(LexError::InvalidIssue {
            span: Span::new(start, end),
            chars: src[start..end].to_string(),
        });
    }
    let body: Rc<str> = Rc::from(&src[body_start..body_end]);
    Ok((body_end, TokenKind::Issue(body)))
}

/// Issue body character predicate: letters, digits, and `-`, `_`, `.`, `?`,
/// `!`. Matches the word-char set minus delimiters (`:`/`/` are excluded so
/// `#foo:` lexes as `Issue("foo")` + `SetWord("foo")`, and `#foo/bar` lexes
/// as `Issue("foo")` + `Refinement("bar")` for path folding).
fn is_issue_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'?' | b'!')
}

/// M81: Does the char after `<` begin a tag body? A tag starts iff the next
/// char is NOT a structural delimiter (`[](){}`/whitespace/`;`/`"`/`{`) and
/// NOT one of the operator chars (`=`/`<`/`>`). Note `/` IS allowed (closing
/// tags like `</p>`), even though `/` is normally a delimiter — `<` + `/`
/// unambiguously starts a closing tag, not the `<` comparison operator (Red
/// has no `</` token). This preserves `<`/`<=`/`<>`/`<<` and `<` + delimiter
/// (e.g. `<`/`<[`) as the comparison-operator form (`scan_word`).
fn starts_tag_body(next: Option<u8>) -> bool {
    match next {
        None => false,
        Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') | Some(b',') => false,
        Some(b'[') | Some(b']') | Some(b'(') | Some(b')') | Some(b'{') | Some(b'}')
        | Some(b';') | Some(b'"') => false,
        Some(b'=') | Some(b'<') | Some(b'>') => false,
        // Everything else (letters, digits, `/`, symbols) starts a tag.
        _ => true,
    }
}

/// `<...>` tag! literal (M81). Consume from `<` to the next `>`, honoring
/// `\<`/`\>`/`\\` escapes (the body stores the decoded forms). EOF before `>`
/// → `UnterminatedTag`. The stored body is the text between `<` and `>`
/// (escapes decoded); the span covers the whole `<...>` run including the
/// brackets.
fn scan_tag(src: &str, i: &mut usize) -> Result<(usize, TokenKind), LexError> {
    let start = *i;
    *i += 1; // consume the `<`
    let rest = &src[*i..];
    let mut body = String::new();
    let mut chars = rest.char_indices();
    while let Some((offset, ch)) = chars.next() {
        if ch == '>' {
            *i += offset + ch.len_utf8();
            return Ok((*i, TokenKind::Tag(Rc::from(body.as_str()))));
        }
        if ch == '\\' {
            match chars.next() {
                Some((_, '<')) => body.push('<'),
                Some((_, '>')) => body.push('>'),
                Some((_, '\\')) => body.push('\\'),
                // Unknown escape: keep backslash + char verbatim (mirrors the
                // quoted-string escape policy).
                Some((_, other)) => {
                    body.push('\\');
                    body.push(other);
                }
                None => break, // `\` at EOF → unterminated
            }
            continue;
        }
        body.push(ch);
    }
    // EOF before `>`.
    *i = src.len();
    Err(LexError::UnterminatedTag {
        span: Span::new(start, src.len()),
        chars: body,
    })
}

fn consume_digits(bytes: &[u8], mut i: usize) -> usize {
    let start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    i - start
}

/// M44: peek a non-delimiter run starting at `start` and classify it as a
/// pair! (`NxM`, an `x` between digit-led runs), a tuple! (`R.G.B[.A]`,
/// 2+ dots between digit-only runs), or neither (`None` → plain int/
/// float, handled by `scan_number`). Does not advance `i`.
///
/// Disambiguation rules (per docs/plans/plan5.md M44):
/// - `x` separator between two digit-led runs → pair (both sides may be int
///   or float, e.g. `1x2`, `1.5x2.5`, `1.0e2x3`).
/// - 2+ dots between digit-only runs → tuple (scan_tuple enforces the 3–4
///   component limit; 5+ components route here and error cleanly).
/// - 1 dot → float (scan_number handles).
/// - 0 dots → integer (scan_number handles).
///
/// Returns `Some(true)` for pair, `Some(false)` for tuple, `None` otherwise.
fn detect_pair_tuple(src: &str, start: usize) -> Option<bool> {
    let bytes = src.as_bytes();
    let mut j = start;
    while j < bytes.len() && !is_delimiter(bytes[j]) {
        j += 1;
    }
    let run = &src[start..j];

    // Pair: `x` between two digit-led runs. Each side may contain digits,
    // `.`, `-`, `e`/`E` (exponent), `+` (exponent sign) — the chars valid in
    // a number body. The right side may have a leading `-`.
    if let Some(xpos) = run.find('x') {
        let (left, right) = (&run[..xpos], &run[xpos + 1..]);
        let num_byte = |c: u8| {
            c.is_ascii_digit() || c == b'.' || c == b'-' || c == b'e' || c == b'E' || c == b'+'
        };
        let left_ok = !left.is_empty()
            && left.bytes().all(num_byte)
            && left
                .bytes()
                .next()
                .is_some_and(|c| c.is_ascii_digit() || c == b'-');
        let right_ok = !right.is_empty()
            && right.bytes().all(num_byte)
            && right
                .bytes()
                .next()
                .is_some_and(|c| c.is_ascii_digit() || c == b'-');
        if left_ok && right_ok {
            return Some(true);
        }
    }

    // Tuple: 2+ dots, all bytes digits or dots, leading/trailing digit.
    // (Leading `-` rejected: tuples are unsigned bytes.) scan_tuple enforces
    // the 3–4 component ceiling; 5+ dots route here and error InvalidTuple.
    let dot_count = run.bytes().filter(|&c| c == b'.').count();
    if dot_count >= 2
        && run.bytes().all(|c| c.is_ascii_digit() || c == b'.')
        && run.bytes().next().is_some_and(|c| c.is_ascii_digit())
        && run.bytes().last().is_some_and(|c| c.is_ascii_digit())
    {
        return Some(false);
    }

    None
}

/// `NxM` — scan a pair! literal. Both sides parse as int or float; the raw
/// component substrings are returned in `TokenKind::Pair` and the parser
/// converts to `Value::Pair`. Does NOT enforce value ranges (integers and
/// floats are both valid pair components).
fn scan_pair(src: &str, i: &mut usize) -> Result<(usize, TokenKind), LexError> {
    let start = *i;
    let bytes = src.as_bytes();

    // First component: optional `-`, digits, optional `.digits`, optional
    // exponent. Mirrors `scan_number` minus the int/float classification
    // (the parser does that from the raw substring).
    let _x_start = scan_pair_component(src, i)?;

    // Expect `x` separator.
    if *i >= bytes.len() || bytes[*i] != b'x' {
        return Err(LexError::InvalidPair {
            span: Span::new(start, *i),
            chars: src[start..*i].to_string(),
        });
    }
    *i += 1; // consume `x`

    let y_start = scan_pair_component(src, i)?;

    if y_start == *i {
        // Empty second component (e.g. `1x` followed by delimiter/EOF).
        return Err(LexError::InvalidPair {
            span: Span::new(start, *i),
            chars: src[start..*i].to_string(),
        });
    }

    let end = *i;
    let split = start + src[start..end].find('x').unwrap_or(end - start);
    let x_text = Rc::from(&src[start..split]);
    let y_text = Rc::from(&src[split + 1..end]);
    Ok((end, TokenKind::Pair(x_text, y_text)))
}

/// Scan one component of a pair! (int or float body, no `x`/tuple recursion).
/// Returns the start offset of the component (== the caller's `*i` on entry).
fn scan_pair_component(src: &str, i: &mut usize) -> Result<usize, LexError> {
    let bytes = src.as_bytes();
    let start = *i;

    if *i < bytes.len() && bytes[*i] == b'-' {
        *i += 1;
    }
    if *i >= bytes.len() || !bytes[*i].is_ascii_digit() {
        return Err(LexError::InvalidPair {
            span: Span::new(start, *i),
            chars: src[start..*i].to_string(),
        });
    }
    *i += consume_digits(bytes, *i);

    // Fractional part.
    if *i + 1 < bytes.len() && bytes[*i] == b'.' && bytes[*i + 1].is_ascii_digit() {
        *i += 1; // consume `.`
        *i += consume_digits(bytes, *i);
    }

    // Exponent part.
    if *i < bytes.len() && (bytes[*i] == b'e' || bytes[*i] == b'E') {
        let saved = *i;
        *i += 1;
        if *i < bytes.len() && (bytes[*i] == b'+' || bytes[*i] == b'-') {
            *i += 1;
        }
        if *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += consume_digits(bytes, *i);
        } else {
            *i = saved + 1; // not an exponent; let `e` start the next token
        }
    }

    Ok(start)
}

/// `R.G.B[.A]` — scan a tuple! literal. Each component is a 0–255 integer
/// (no floats, no negatives). 3 or 4 components; more or fewer is an error.
fn scan_tuple(src: &str, i: &mut usize) -> Result<(usize, TokenKind), LexError> {
    let start = *i;
    let bytes = src.as_bytes();
    let mut comps: Vec<u8> = Vec::with_capacity(4);

    // First component: digits only (leading `-` rejected — tuples are unsigned).
    let comp_start = *i;
    *i += consume_digits(bytes, *i);
    let n = parse_tuple_component(src, comp_start, *i, start)?;
    comps.push(n);

    // Remaining components: `.digits`.
    while *i + 1 < bytes.len() && bytes[*i] == b'.' && bytes[*i + 1].is_ascii_digit() {
        *i += 1; // consume `.`
        let comp_start = *i;
        *i += consume_digits(bytes, *i);
        let n = parse_tuple_component(src, comp_start, *i, start)?;
        comps.push(n);
        if comps.len() > 4 {
            return Err(LexError::InvalidTuple {
                span: Span::new(start, *i),
                chars: src[start..*i].to_string(),
            });
        }
    }

    if comps.len() < 3 {
        // detect_pair_tuple only routes 2-or-3-dot runs here, so this is a
        // defensive guard for malformed input that slipped through.
        return Err(LexError::InvalidTuple {
            span: Span::new(start, *i),
            chars: src[start..*i].to_string(),
        });
    }

    Ok((*i, TokenKind::Tuple(Rc::from(&comps[..]))))
}

/// Parse one tuple component (a 0–255 integer) from `src[comp_start..comp_end]`.
/// `start` is the tuple's start (for error spans).
fn parse_tuple_component(
    src: &str,
    comp_start: usize,
    comp_end: usize,
    start: usize,
) -> Result<u8, LexError> {
    let text = &src[comp_start..comp_end];
    let n: i64 = text.parse().map_err(|_| LexError::InvalidTuple {
        span: Span::new(start, comp_end),
        chars: src[start..comp_end].to_string(),
    })?;
    if !(0..=255).contains(&n) {
        return Err(LexError::InvalidTuple {
            span: Span::new(start, comp_end),
            chars: src[start..comp_end].to_string(),
        });
    }
    Ok(n as u8)
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

/// M80: peek ahead from `start` to determine if the word run is an email!
/// literal. Returns `Some(end)` if the run matches the email shape
/// (`<word-chars>@<word-chars>.<word-chars>`); `None` otherwise (so the
/// caller falls through to `scan_word`). `@` is a word character today
/// (not a delimiter), so the whole `foo@bar.com` run scans as one token.
///
/// Rules (Red parity):
/// - exactly one `@` in the run.
/// - non-empty local part (before `@`).
/// - non-empty host part (after `@`) with at least one `.` and a non-empty
///   TLD (the segment after the last `.`).
/// - the run ends at a delimiter or EOF.
///
/// `user@localhost` (no dot after `@`) is NOT an email! — returns `None`.
fn detect_email(src: &str, start: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    // Scan the full word run (same chars as scan_word accepts: non-delimiter).
    let mut j = start;
    while j < bytes.len() && !is_delimiter(bytes[j]) {
        j += 1;
    }
    let run = &src[start..j];
    // Find exactly one `@`.
    let at_pos = run.find('@')?;
    // Reject multiple `@`.
    if run[at_pos + 1..].contains('@') {
        return None;
    }
    let local = &run[..at_pos];
    let host = &run[at_pos + 1..];
    // Non-empty local, non-empty host.
    if local.is_empty() || host.is_empty() {
        return None;
    }
    // Host must contain at least one `.` with a non-empty TLD after it.
    let last_dot = host.rfind('.')?;
    let tld = &host[last_dot + 1..];
    if tld.is_empty() {
        return None;
    }
    // Local and host must be word-char runs (alphanumeric + common word chars).
    // Reject if any char is not a valid email word char (allowing `.`/`-`/`_`).
    let is_email_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+');
    if !local.chars().all(is_email_char) || !host.chars().all(is_email_char) {
        return None;
    }
    Some(j)
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

// ---------------------------------------------------------------------------
// M45: date! / time! / zone scanning
// ---------------------------------------------------------------------------

/// Result of `detect_date_time`: peek the non-delimiter run starting at `i`
/// and classify whether it's a date/time/zone literal.
#[derive(Clone, Debug)]
enum DateDetect {
    /// The run doesn't structurally match any date/time form. Fall through to
    /// pair/tuple/number scanning.
    NotADate,
    /// A valid date!/time! literal. `end` is the byte offset just past the
    /// consumed run; `value` is the fully-parsed `DateValue`.
    Valid { end: usize, value: DateValue },
    /// The run structurally matches a date/time form but the values are
    /// invalid (e.g. `31-Feb-2024`, or a zone offset > 14h). `end` is the run
    /// end for the error span.
    Invalid { end: usize },
}

/// Peek the non-delimiter run starting at `start` and decide if it's a
/// date/time literal. Does not advance any cursor — the caller consumes the
/// run based on the returned `end`.
///
/// Special handling: `DD-Mon-YYYY/HH:MM:SS[zone]` — the `/` is a lexer
/// delimiter, so the initial run stops at the `/`. If the first run is a
/// valid date-only and the next char is `/` followed by a time-shaped run,
/// the detection extends past the `/` to include the time + zone.
/// (`DD/MM/YYYY` is not supported — its internal `/`s split it into separate
/// tokens; use `DD-Mon-YYYY` or `YYYY-MM-DD` instead.)
fn detect_date_time(src: &str, start: usize) -> DateDetect {
    let bytes = src.as_bytes();
    let mut j = start;
    while j < bytes.len() && !is_delimiter(bytes[j]) {
        j += 1;
    }
    let run = &src[start..j];

    if !looks_like_date_time(run) {
        return DateDetect::NotADate;
    }

    match parse_date_run(run) {
        Ok(dv) => {
            // Date-only: check for `/` + time extension
            // (`DD-Mon-YYYY/HH:MM:SS[zone]`).
            if !dv.has_time() && dv.zone.is_none() && j < bytes.len() && bytes[j] == b'/' {
                let mut k = j + 1;
                while k < bytes.len() && !is_delimiter(bytes[k]) {
                    k += 1;
                }
                let ext_run = &src[j + 1..k];
                if looks_like_time(ext_run) {
                    let full_run = &src[start..k];
                    return match parse_date_run(full_run) {
                        Ok(dv2) => DateDetect::Valid { end: k, value: dv2 },
                        Err(()) => DateDetect::Invalid { end: k },
                    };
                }
            }
            DateDetect::Valid { end: j, value: dv }
        }
        Err(()) => DateDetect::Invalid { end: j },
    }
}

/// Quick check: does `run` look like a time form (`HH:MM:SS[.mmm][zone]`)?
/// Structural only — value validation happens in `parse_time`.
fn looks_like_time(run: &str) -> bool {
    let bytes = run.as_bytes();
    if bytes.len() < 8 {
        return false;
    }
    // Must start with 2 digits + `:`.
    if bytes[0].is_ascii_digit() && bytes[1].is_ascii_digit() && bytes[2] == b':' {
        return true;
    }
    false
}

/// Quick structural check: does `run` look like a date/time form? This is a
/// fast pre-filter — `parse_date_run` does the full validation. Returns false
/// for plain integers/floats/pairs/tuples so they fall through to their
/// scanners.
///
/// Note: `DD/MM/YYYY` is NOT detected here because `/` is a lexer delimiter
/// (the run splits before reaching this function). Only `/`-free date forms
/// (`DD-Mon-YYYY`, `YYYY-MM-DD`) and time forms (`HH:MM:SS`) are detected;
/// the `DD-Mon-YYYY/HH:MM:SS` combined form is handled by `detect_date_time`'s
/// `/`-extension logic.
fn looks_like_date_time(run: &str) -> bool {
    let bytes = run.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_digit() {
        return false;
    }
    // DD-Mon-YYYY: 1-2 digits + `-` + alpha (e.g. `29-Jun-2024`, `1-Jan-2024`).
    if bytes.len() >= 3 && bytes[0].is_ascii_digit() {
        // Find the first `-` within the first 3 chars.
        if let Some(dash) = bytes[..3usize.min(bytes.len())]
            .iter()
            .position(|&b| b == b'-')
        {
            if (1..=2).contains(&dash)
                && bytes.get(dash + 1).is_some_and(|b| b.is_ascii_alphabetic())
            {
                return true;
            }
        }
    }
    // ISO YYYY-MM-DD: 4 digits + `-`.
    if bytes.len() >= 5 && bytes[0..4].iter().all(|b| b.is_ascii_digit()) && bytes[4] == b'-' {
        return true;
    }
    // Time-only: HH:MM:SS — starts with 2 digits then `:`.
    if looks_like_time(run) {
        return true;
    }
    false
}

/// Parse a full date/time/zone run into a `DateValue`. Returns `Err(())` if
/// the run structurally matches a date/time form but the values are invalid
/// (e.g. `31-Feb-2024`); returns `Ok(DateValue)` on success.
///
/// Supported forms:
/// - `DD-Mon-YYYY` (date-only)
/// - `DD-Mon-YYYY/HH:MM:SS[.mmm][zone]`
/// - `YYYY-MM-DD` (ISO date-only)
/// - `YYYY-MM-DDTHH:MM:SS[.mmm][zone]` (ISO datetime)
/// - `DD/MM/YYYY` (date-only)
/// - `DD/MM/YYYY/HH:MM:SS[.mmm][zone]`
/// - `HH:MM:SS[.mmm][zone]` (time-only; epoch date 1970-01-01)
/// - Zone: `Z`, `+HH:MM`, `+H:MM`, `-HH:MM`, `+HHMM`, `-HHMM`, `+HH`, `-HH`
fn parse_date_run(run: &str) -> Result<DateValue, ()> {
    // Split off the trailing zone suffix (if any).
    let (body, zone) = split_zone_suffix(run)?;
    let zone = match zone {
        Ok(z) => z,
        Err(()) => return Err(()), // malformed zone
    };

    // Try ISO datetime: YYYY-MM-DDTHH:MM:SS[.mmm]
    // (Only uppercase `T` — lowercase `t` appears in month abbreviations like
    // `Oct`.)
    if let Some(t_pos) = body.find('T') {
        let date_str = &body[..t_pos];
        let time_str = &body[t_pos + 1..];
        if let Some(date) = parse_iso_date(date_str)? {
            if let Some(time) = parse_time(time_str)? {
                return Ok(DateValue::from_local(date.and_time(time), zone));
            }
        }
        return Err(());
    }

    // Try date + `/` + time. Split on the LAST `/` so DD/MM/YYYY's internal
    // slashes stay with the date part.
    if let Some(slash_pos) = body.rfind('/') {
        let date_str = &body[..slash_pos];
        let time_str = &body[slash_pos + 1..];
        // Try each date form on date_str. If a form structurally matches but
        // values are invalid (Err), propagate the error.
        let date_opt: Option<NaiveDate> = {
            match parse_iso_date(date_str) {
                Ok(o) => o,
                Err(()) => return Err(()),
            }
        };
        let date_opt = match date_opt {
            Some(d) => Some(d),
            None => match parse_dmonyyyy(date_str) {
                Ok(o) => o,
                Err(()) => return Err(()),
            },
        };
        let date_opt = match date_opt {
            Some(d) => Some(d),
            None => match parse_dslashyyyy(date_str) {
                Ok(o) => o,
                Err(()) => return Err(()),
            },
        };
        if let Some(date) = date_opt {
            if let Some(time) = parse_time(time_str)? {
                return Ok(DateValue::from_local(date.and_time(time), zone));
            }
        }
        return Err(());
    }

    // No `/` or `T` separator. Try date-only, then time-only.
    if let Some(date) = parse_iso_date(body)? {
        // Date-only can't have a zone (zone only valid on date+time forms).
        if zone.is_some() {
            return Err(());
        }
        return Ok(DateValue::date_only(date));
    }
    if let Some(date) = parse_dmonyyyy(body)? {
        if zone.is_some() {
            return Err(());
        }
        return Ok(DateValue::date_only(date));
    }
    if let Some(date) = parse_dslashyyyy(body)? {
        if zone.is_some() {
            return Err(());
        }
        return Ok(DateValue::date_only(date));
    }
    // Time-only: HH:MM:SS[.mmm] with optional zone. Epoch date 1970-01-01.
    if let Some(time) = parse_time(body)? {
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
        return Ok(DateValue::from_local(epoch.and_time(time), zone));
    }
    Err(())
}

/// Split a trailing zone suffix off `s`. Returns `(body, Ok(zone_opt))` on
/// success (zone_opt is `Some(minutes)` or `None` if no suffix), or
/// `(body, Err(()))` if a suffix is present but malformed.
///
/// A zone is only valid on a time form (the body before the sign must contain
/// `:`), so a `-` inside a date-only body like `2024-06-29` is not mistaken
/// for a zone.
fn split_zone_suffix(s: &str) -> Result<(&str, Result<Option<i32>, ()>), ()> {
    let bytes = s.as_bytes();
    // `Z` suffix (UTC).
    if let Some(body) = s.strip_suffix('Z') {
        if body.contains(':') {
            return Ok((body, Ok(Some(0))));
        }
        // `Z` on a date-only form is malformed.
        return Ok((body, Err(())));
    }
    // Scan from the end for the last `+` or `-`. The zone must be at the end
    // and the body before it must contain `:` (indicating a time form).
    for i in (1..bytes.len()).rev() {
        let c = bytes[i];
        if c != b'+' && c != b'-' {
            continue;
        }
        let body = &s[..i];
        let suffix = &s[i..];
        // A zone is only valid on a time form.
        if !body.contains(':') {
            // This sign is part of the date body (e.g. `2024-06-29`), not a
            // zone. There's no zone in this run.
            return Ok((s, Ok(None)));
        }
        // Try to parse the suffix as a zone.
        match parse_zone_suffix(suffix) {
            Some(zone) => return Ok((body, Ok(Some(zone)))),
            None => {
                // Malformed zone — error only if the suffix looks zone-shaped
                // (sign + digit). Otherwise treat as no zone (the sign is
                // part of some other construct).
                if suffix.len() >= 2 && bytes[i + 1].is_ascii_digit() {
                    return Ok((body, Err(())));
                }
                return Ok((s, Ok(None)));
            }
        }
    }
    Ok((s, Ok(None)))
}

/// Parse a zone offset suffix (`+HH:MM`, `-HH:MM`, `+H:MM`, `+HHMM`,
/// `-HHMM`, `+HH`, `-HH`, `Z`). Returns `Some(minutes)` or `None` if the
/// string doesn't match any valid zone form. `|minutes| > 14*60` is rejected.
fn parse_zone_suffix(s: &str) -> Option<i32> {
    if s == "Z" {
        return Some(0);
    }
    let bytes = s.as_bytes();
    if bytes.is_empty() || (bytes[0] != b'+' && bytes[0] != b'-') {
        return None;
    }
    let sign = if bytes[0] == b'+' { 1 } else { -1 };
    let rest = &s[1..];
    let rest_bytes = rest.as_bytes();
    if rest.is_empty() {
        return None;
    }
    // Validate: only digits and at most one `:`.
    if !rest_bytes.iter().all(|b| b.is_ascii_digit() || *b == b':') {
        return None;
    }
    let colon_count = rest_bytes.iter().filter(|&&b| b == b':').count();
    if colon_count > 1 {
        return None;
    }
    let (h, m) = if colon_count == 1 {
        // `H:MM` or `HH:MM` — minutes must be exactly 2 digits.
        let colon_pos = rest.find(':').unwrap();
        let h_str = &rest[..colon_pos];
        let m_str = &rest[colon_pos + 1..];
        if h_str.is_empty() || h_str.len() > 2 {
            return None;
        }
        if m_str.len() != 2 {
            return None;
        }
        (h_str.parse::<i32>().ok()?, m_str.parse::<i32>().ok()?)
    } else {
        // No colon: `HH` (2 digits) or `HHMM` (4 digits) only.
        match rest.len() {
            2 => (rest.parse::<i32>().ok()?, 0),
            4 => (
                rest[..2].parse::<i32>().ok()?,
                rest[2..].parse::<i32>().ok()?,
            ),
            _ => return None,
        }
    };
    if h > 14 || m > 59 {
        return None;
    }
    Some(sign * (h * 60 + m))
}

/// Parse `YYYY-MM-DD` (exactly 10 chars). Returns:
/// - `Ok(Some(date))` — valid date.
/// - `Ok(None)` — not structurally an ISO date (wrong length/shape).
/// - `Err(())` — structurally an ISO date but values invalid (e.g. Feb 31).
fn parse_iso_date(s: &str) -> Result<Option<NaiveDate>, ()> {
    if s.len() != 10 {
        return Ok(None);
    }
    let bytes = s.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return Ok(None);
    }
    if !bytes[0..4].iter().all(|b| b.is_ascii_digit())
        || !bytes[5..7].iter().all(|b| b.is_ascii_digit())
        || !bytes[8..10].iter().all(|b| b.is_ascii_digit())
    {
        return Ok(None);
    }
    let y: i32 = s[0..4].parse().map_err(|_| ())?;
    let m: u32 = s[5..7].parse().map_err(|_| ())?;
    let d: u32 = s[8..10].parse().map_err(|_| ())?;
    match NaiveDate::from_ymd_opt(y, m, d) {
        Some(d) => Ok(Some(d)),
        None => Err(()),
    }
}

/// Parse `DD-Mon-YYYY` or `D-Mon-YYYY` (10 or 11 chars: `1-Jan-2024` or
/// `29-Jun-2024`). Month is a 3-letter English abbreviation, case-insensitive.
/// Same return convention as [`parse_iso_date`].
fn parse_dmonyyyy(s: &str) -> Result<Option<NaiveDate>, ()> {
    // Find the first `-` (day/month separator).
    let first_dash = match s.find('-') {
        Some(p) if p == 1 || p == 2 => p,
        _ => return Ok(None),
    };
    // Find the second `-` (month/year separator) at first_dash+4.
    let second_dash = first_dash + 4;
    let bytes = s.as_bytes();
    if bytes.len() < second_dash + 5 {
        return Ok(None);
    }
    if bytes[second_dash] != b'-' {
        return Ok(None);
    }
    // Validate day part (1-2 digits).
    if !bytes[..first_dash].iter().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    // Validate month part (3 alpha chars).
    if !bytes[first_dash + 1..second_dash]
        .iter()
        .all(|b| b.is_ascii_alphabetic())
    {
        return Ok(None);
    }
    // Validate year part (4 digits after second dash).
    if bytes.len() != second_dash + 5
        || !bytes[second_dash + 1..].iter().all(|b| b.is_ascii_digit())
    {
        return Ok(None);
    }
    let d: u32 = s[..first_dash].parse().map_err(|_| ())?;
    let m = month_from_abbr(&s[first_dash + 1..second_dash]).ok_or(())?;
    let y: i32 = s[second_dash + 1..].parse().map_err(|_| ())?;
    match NaiveDate::from_ymd_opt(y, m, d) {
        Some(d) => Ok(Some(d)),
        None => Err(()),
    }
}

/// Parse `DD/MM/YYYY` (exactly 10 chars). Same return convention as
/// [`parse_iso_date`].
fn parse_dslashyyyy(s: &str) -> Result<Option<NaiveDate>, ()> {
    if s.len() != 10 {
        return Ok(None);
    }
    let bytes = s.as_bytes();
    if bytes[2] != b'/' || bytes[5] != b'/' {
        return Ok(None);
    }
    if !bytes[0..2].iter().all(|b| b.is_ascii_digit())
        || !bytes[3..5].iter().all(|b| b.is_ascii_digit())
        || !bytes[6..10].iter().all(|b| b.is_ascii_digit())
    {
        return Ok(None);
    }
    let d: u32 = s[0..2].parse().map_err(|_| ())?;
    let m: u32 = s[3..5].parse().map_err(|_| ())?;
    let y: i32 = s[6..10].parse().map_err(|_| ())?;
    match NaiveDate::from_ymd_opt(y, m, d) {
        Some(d) => Ok(Some(d)),
        None => Err(()),
    }
}

/// Parse `HH:MM:SS` or `HH:MM:SS.mmm`. Same return convention as
/// [`parse_iso_date`] but for `NaiveTime`.
fn parse_time(s: &str) -> Result<Option<NaiveTime>, ()> {
    let bytes = s.as_bytes();
    if bytes.len() < 8 {
        return Ok(None);
    }
    if bytes[2] != b':' || bytes[5] != b':' {
        return Ok(None);
    }
    if !bytes[0..2].iter().all(|b| b.is_ascii_digit())
        || !bytes[3..5].iter().all(|b| b.is_ascii_digit())
        || !bytes[6..8].iter().all(|b| b.is_ascii_digit())
    {
        return Ok(None);
    }
    let h: u32 = s[0..2].parse().map_err(|_| ())?;
    let m: u32 = s[3..5].parse().map_err(|_| ())?;
    let sec: u32 = s[6..8].parse().map_err(|_| ())?;
    // Optional `.mmm` (exactly 3 fractional digits).
    let mut millis = 0;
    if bytes.len() == 12 && bytes[8] == b'.' {
        if !bytes[9..12].iter().all(|b| b.is_ascii_digit()) {
            return Ok(None);
        }
        millis = s[9..12].parse().map_err(|_| ())?;
    } else if bytes.len() != 8 {
        return Ok(None);
    }
    if h > 23 || m > 59 || sec > 60 {
        return Err(());
    }
    match NaiveTime::from_hms_milli_opt(h, m, sec, millis) {
        Some(t) => Ok(Some(t)),
        None => Err(()),
    }
}

/// 3-letter English month abbreviation → month number (1–12). Case-insensitive.
fn month_from_abbr(s: &str) -> Option<u32> {
    let up: String = s.chars().map(|c| c.to_ascii_uppercase()).collect();
    Some(match up.as_str() {
        "JAN" => 1,
        "FEB" => 2,
        "MAR" => 3,
        "APR" => 4,
        "MAY" => 5,
        "JUN" => 6,
        "JUL" => 7,
        "AUG" => 8,
        "SEP" => 9,
        "OCT" => 10,
        "NOV" => 11,
        "DEC" => 12,
        _ => return None,
    })
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
    fn percent_literal() {
        // M80: digit-run-then-`%` lexes as Percent (stored fractional).
        assert_eq!(one("0%"), TokenKind::Percent(0.0));
        assert_eq!(one("50%"), TokenKind::Percent(0.5));
        assert_eq!(one("100%"), TokenKind::Percent(1.0));
        // Negative and fractional.
        assert_eq!(one("-50%"), TokenKind::Percent(-0.5));
        assert_eq!(one("0.5%"), TokenKind::Percent(0.005));
        assert_eq!(one("1.5%"), TokenKind::Percent(0.015));
        // Exponent run works (the `%` is consumed after the full number).
        assert_eq!(one("1e2%"), TokenKind::Percent(1.0));
    }

    #[test]
    fn percent_followed_by_more() {
        // `50% 25%` lexes as two percent tokens.
        let toks = lex("50% 25%").expect("lex");
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].kind, TokenKind::Percent(0.5));
        assert_eq!(toks[1].kind, TokenKind::Percent(0.25));
        // `5%foo` — `%` consumed as part of percent; `foo` is a word.
        let toks = lex("5%foo").expect("lex");
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].kind, TokenKind::Percent(0.05));
        assert!(matches!(toks[1].kind, TokenKind::Word(_)));
    }

    #[test]
    fn percent_does_not_collide_with_file_literal() {
        // A bare `%` (not preceded by a digit) still starts a file! literal.
        let toks = lex("%foo/bar.txt").expect("lex");
        assert_eq!(toks.len(), 1);
        assert!(matches!(toks[0].kind, TokenKind::File(_)));
        // A digit followed by a `%`-file is a percent + a file.
        let toks = lex("50% %foo").expect("lex");
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].kind, TokenKind::Percent(0.5));
        assert!(matches!(toks[1].kind, TokenKind::File(_)));
    }

    #[test]
    fn money_literal() {
        // M80: `$`-led money! literals (stored as cents + currency).
        let mv = |t: &TokenKind| match t {
            TokenKind::Money(m) => (m.cents, m.currency.as_ref().to_string()),
            _ => panic!("not Money: {t:?}"),
        };
        assert_eq!(mv(&one("$0")), (0, "USD".to_string()));
        assert_eq!(mv(&one("$10")), (1000, "USD".to_string()));
        assert_eq!(mv(&one("$10.00")), (1000, "USD".to_string()));
        assert_eq!(mv(&one("$1,234.56")), (123456, "USD".to_string()));
        // Currency suffix.
        assert_eq!(mv(&one("$10.00:EUR")), (1000, "EUR".to_string()));
        assert_eq!(mv(&one("$10.00:eur")), (1000, "EUR".to_string())); // uppercased
    }

    #[test]
    fn money_bad_forms() {
        // M80: malformed money! literals error. (`$` alone, not followed by a
        // digit, falls through to `scan_word` as `Word("$")` — not an error.)
        assert!(matches!(lex("$10."), Err(LexError::InvalidMoney { .. })));
        assert!(matches!(lex("$10.5"), Err(LexError::InvalidMoney { .. }))); // 1 fractional digit
        assert!(matches!(lex("$10.555"), Err(LexError::InvalidMoney { .. }))); // 3 fractional digits
        assert!(matches!(
            lex("$10.00:US"),
            Err(LexError::InvalidMoney { .. })
        )); // 2-letter currency
        assert!(matches!(
            lex("$10.00:USDD"),
            Err(LexError::InvalidMoney { .. })
        )); // 4-letter currency
            // `$,100` and `$` alone don't trigger the money arm (`$` not followed
            // by a digit falls through to scan_word as a Word) — documented
            // behavior, not an error.
    }

    #[test]
    fn money_negative() {
        // M80: `-$10.00` lexes as a single negative Money token.
        let mv = |t: &TokenKind| match t {
            TokenKind::Money(m) => (m.cents, m.currency.as_ref().to_string()),
            _ => panic!("not Money: {t:?}"),
        };
        assert_eq!(mv(&one("-$10.00")), (-1000, "USD".to_string()));
        assert_eq!(mv(&one("-$0.01")), (-1, "USD".to_string()));
    }

    #[test]
    fn money_does_not_collide_with_word() {
        // `$foo` (not followed by a digit) still scans as a Word.
        let toks = lex("$foo").expect("lex");
        assert_eq!(toks.len(), 1);
        assert!(matches!(toks[0].kind, TokenKind::Word(_)));
    }

    #[test]
    fn duration_literal_single() {
        // M140: single-unit duration literals.
        let d = |t: &TokenKind| match t {
            TokenKind::Duration(d) => *d,
            _ => panic!("not Duration: {t:?}"),
        };
        assert_eq!(d(&one("30s")), chrono::Duration::seconds(30));
        assert_eq!(d(&one("1.5h")), chrono::Duration::seconds(5400));
        assert_eq!(d(&one("250ms")), chrono::Duration::milliseconds(250));
        assert_eq!(d(&one("100ns")), chrono::Duration::nanoseconds(100));
        assert_eq!(d(&one("5m")), chrono::Duration::minutes(5));
        assert_eq!(d(&one("2h")), chrono::Duration::hours(2));
        assert_eq!(d(&one("1d")), chrono::Duration::days(1));
        assert_eq!(d(&one("500us")), chrono::Duration::microseconds(500));
        assert_eq!(d(&one("-30s")), chrono::Duration::seconds(-30));
    }

    #[test]
    fn duration_literal_compound() {
        // M140: compound duration literals (strict descending unit order).
        let d = |t: &TokenKind| match t {
            TokenKind::Duration(d) => *d,
            _ => panic!("not Duration: {t:?}"),
        };
        assert_eq!(d(&one("1d1h")), chrono::Duration::seconds(86400 + 3600));
        assert_eq!(d(&one("1d2s")), chrono::Duration::seconds(86400 + 2));
        assert_eq!(
            d(&one("1h30m45s")),
            chrono::Duration::seconds(3600 + 1800 + 45)
        );
        assert_eq!(d(&one("1.5h30m")), chrono::Duration::seconds(5400 + 1800));
        assert_eq!(d(&one("1h30.5m")), chrono::Duration::seconds(3600 + 1830));
        assert_eq!(d(&one("-1d1h")), chrono::Duration::seconds(-(86400 + 3600)));
        assert_eq!(d(&one("0d0h")), chrono::Duration::zero());
    }

    #[test]
    fn duration_compound_errors() {
        // M140: non-descending, repeated, sub-component overflow.
        assert!(matches!(lex("1h1h"), Err(LexError::InvalidDuration { .. }))); // repeated
        assert!(matches!(lex("1h1d"), Err(LexError::InvalidDuration { .. }))); // non-descending
        assert!(matches!(
            lex("1h70m"),
            Err(LexError::InvalidDuration { .. })
        )); // overflow: 70m >= 1h
        assert!(matches!(
            lex("1d25h"),
            Err(LexError::InvalidDuration { .. })
        )); // overflow: 25h >= 1d
        assert!(matches!(
            lex("1m60s"),
            Err(LexError::InvalidDuration { .. })
        )); // overflow: 60s >= 1m
        assert!(matches!(
            lex("1h60.5m"),
            Err(LexError::InvalidDuration { .. })
        )); // fractional overflow
    }

    #[test]
    fn duration_collision_guard() {
        // M140: `30stuff` — `s` suffix not committed (next char `t` is word).
        let toks = lex("30stuff").expect("lex");
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].kind, TokenKind::Integer(30));
        assert!(matches!(toks[1].kind, TokenKind::Word(_)));
        // `30s x` — `s` committed (next char is delimiter).
        let toks = lex("30s x").expect("lex");
        assert_eq!(toks.len(), 2);
        assert!(matches!(toks[0].kind, TokenKind::Duration(_)));
        assert!(matches!(toks[1].kind, TokenKind::Word(_)));
        // `1d1hx` — first `1d` commits; second `1h` not committed (`x` follows).
        let toks = lex("1d1hx").expect("lex");
        assert_eq!(toks.len(), 3);
        assert!(matches!(toks[0].kind, TokenKind::Duration(_)));
        assert_eq!(toks[1].kind, TokenKind::Integer(1));
        assert!(matches!(toks[2].kind, TokenKind::Word(_)));
        // `5s+foo` — `s` not committed (`+` is a word-extending char).
        let toks = lex("5s+foo").expect("lex");
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].kind, TokenKind::Integer(5));
        assert!(matches!(toks[1].kind, TokenKind::Word(_)));
    }

    #[test]
    fn duration_mold_round_trips() {
        // M140: mold then reparse yields same value (value-equal).
        let cases = [
            chrono::Duration::seconds(30),
            chrono::Duration::minutes(90),
            chrono::Duration::milliseconds(250),
            chrono::Duration::nanoseconds(100),
            chrono::Duration::seconds(-30),
            chrono::Duration::hours(30), // 1d1h molds as 30h
        ];
        for dur in cases {
            let v = crate::value::Value::duration(dur);
            let molded = crate::printer::mold_to_string(&v);
            let toks = lex(&molded).expect("reparse");
            assert_eq!(toks.len(), 1, "molded: {molded}");
            match &toks[0].kind {
                TokenKind::Duration(d) => assert_eq!(*d, dur, "molded: {molded}"),
                other => panic!("expected Duration, got {other:?} for molded: {molded}"),
            }
        }
    }

    #[test]
    fn issue_literal() {
        // M80: `#` followed by word chars scans as Issue (body without `#`).
        assert_eq!(one("#1234"), TokenKind::Issue(std::rc::Rc::from("1234")));
        assert_eq!(one("#ABC"), TokenKind::Issue(std::rc::Rc::from("ABC")));
        assert_eq!(one("#FF00"), TokenKind::Issue(std::rc::Rc::from("FF00")));
        // Issue with hyphen/underscore/dot.
        assert_eq!(
            one("#foo-bar"),
            TokenKind::Issue(std::rc::Rc::from("foo-bar"))
        );
        assert_eq!(
            one("#foo_bar"),
            TokenKind::Issue(std::rc::Rc::from("foo_bar"))
        );
    }

    #[test]
    fn issue_regression_char_and_binary_still_work() {
        // M80: the issue fall-through must not break `#"x"` (char) or
        // `#{hex}` (binary).
        assert!(matches!(one("#\"a\""), TokenKind::Char('a')));
        assert!(matches!(one("#{00FF}"), TokenKind::Binary(_)));
    }

    #[test]
    fn issue_bad_form() {
        // `#` followed by a delimiter (space, bracket) is an InvalidIssue.
        assert!(matches!(lex("# "), Err(LexError::InvalidIssue { .. })));
        assert!(matches!(lex("#["), Err(LexError::InvalidIssue { .. })));
    }

    #[test]
    fn email_literal() {
        // M80: a word run containing `@` with a dot in the host lexes as Email.
        assert_eq!(
            one("foo@bar.com"),
            TokenKind::Email(std::rc::Rc::from("foo@bar.com"))
        );
        assert_eq!(
            one("user@host.example.org"),
            TokenKind::Email(std::rc::Rc::from("user@host.example.org"))
        );
    }

    #[test]
    fn email_bare_host_not_email() {
        // M80: `user@localhost` (no dot in host) is NOT an email! — it lexes
        // as a Word (regression guard).
        let toks = lex("user@localhost").expect("lex");
        assert_eq!(toks.len(), 1);
        assert!(matches!(toks[0].kind, TokenKind::Word(_)));
    }

    #[test]
    fn tag_literal() {
        // M81: `<...>` lexes as a tag! literal. Body is the text between the
        // angle brackets (escapes decoded).
        assert_eq!(one("<b>"), TokenKind::Tag(std::rc::Rc::from("b")));
        assert_eq!(one("</p>"), TokenKind::Tag(std::rc::Rc::from("/p")));
        assert_eq!(one("<br/>"), TokenKind::Tag(std::rc::Rc::from("br/")));
        // Spaces, quotes, and `=` inside the body are verbatim.
        assert_eq!(
            one("<img src=\"x\">"),
            TokenKind::Tag(std::rc::Rc::from("img src=\"x\""))
        );
        assert_eq!(one("<a=b>"), TokenKind::Tag(std::rc::Rc::from("a=b")));
    }

    #[test]
    fn tag_escape_decoding() {
        // M81: `\<`/`\>`/`\\` decode to literal `<`/`>`/`\` in the body.
        assert_eq!(one("<a\\>b>"), TokenKind::Tag(std::rc::Rc::from("a>b")));
        assert_eq!(one("<a\\<b>"), TokenKind::Tag(std::rc::Rc::from("a<b")));
        assert_eq!(one("<a\\\\b>"), TokenKind::Tag(std::rc::Rc::from("a\\b")));
    }

    #[test]
    fn tag_operator_disambiguation() {
        // M81: `<` followed by EOF/delimiter/operator char is the comparison
        // operator, not a tag (regression guard).
        let toks = lex("< 5").expect("lex");
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].kind, TokenKind::Word(Symbol::new("<")));
        assert_eq!(toks[1].kind, TokenKind::Integer(5));
        assert_eq!(one("<="), TokenKind::Word(Symbol::new("<=")));
        assert_eq!(one("<>"), TokenKind::Word(Symbol::new("<>")));
        assert_eq!(one("<"), TokenKind::Word(Symbol::new("<")));
        // `<` + delimiter → operator (single Word).
        let toks = lex("<[").expect("lex");
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].kind, TokenKind::Word(Symbol::new("<")));
        assert!(matches!(toks[1].kind, TokenKind::LBracket));
    }

    #[test]
    fn tag_regression_char_and_binary_still_work() {
        // M81: the `<`-dispatch does not interfere with `#`-dispatch (char/
        // binary). Regression guards.
        assert_eq!(one("#\"a\""), TokenKind::Char('a'));
        assert_eq!(
            one("#{00FF}"),
            TokenKind::Binary(Rc::from(&[0x00, 0xFF][..]))
        );
    }

    #[test]
    fn tag_unterminated() {
        // M81: EOF before `>` → UnterminatedTag.
        assert!(matches!(lex("<b"), Err(LexError::UnterminatedTag { .. })));
        assert!(matches!(
            lex("<img src=\"x\""),
            Err(LexError::UnterminatedTag { .. })
        ));
        // A backslash at EOF is also unterminated.
        assert!(matches!(lex("<a\\"), Err(LexError::UnterminatedTag { .. })));
    }

    #[test]
    fn tag_local_marker_lexes_as_tag() {
        // M81: `<local>` (the `function` spec marker) now lexes as a Tag, not
        // a Word. `function`'s spec parser accepts both forms.
        assert_eq!(one("<local>"), TokenKind::Tag(std::rc::Rc::from("local")));
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
        // M44: `1.2.3` is now a tuple! literal (3-byte RGB), not an error.
        assert_eq!(one("1.2.3"), TokenKind::Tuple(Rc::from(&[1u8, 2, 3][..])));
        // 5+ components are still rejected (4-byte RGBA is the max).
        let err = lex("1.2.3.4.5").unwrap_err();
        assert!(matches!(err, LexError::InvalidTuple { .. }));
    }

    #[test]
    fn pair_literal_basic() {
        assert_eq!(
            one("100x200"),
            TokenKind::Pair(Rc::from("100"), Rc::from("200"))
        );
        assert_eq!(one("0x0"), TokenKind::Pair(Rc::from("0"), Rc::from("0")));
        assert_eq!(one("-1x2"), TokenKind::Pair(Rc::from("-1"), Rc::from("2")));
    }

    #[test]
    fn pair_literal_float_components() {
        assert_eq!(
            one("1.5x2.5"),
            TokenKind::Pair(Rc::from("1.5"), Rc::from("2.5"))
        );
        assert_eq!(
            one("1.0e2x3"),
            TokenKind::Pair(Rc::from("1.0e2"), Rc::from("3"))
        );
    }

    #[test]
    fn pair_literal_bad_form() {
        // `1x` followed by a digit is a pair; `1x-` (no digit after `-`)
        // routes to scan_pair but fails in the second component.
        let err = lex("1x-").unwrap_err();
        assert!(matches!(err, LexError::InvalidPair { .. }));
        // `1x` followed by EOF/delimiter is Integer(1) + Word("x") — detect
        // returns None for an empty right side, so this is NOT a pair error.
        let toks = lex("1x ").expect("lex");
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].kind, TokenKind::Integer(1));
        assert_eq!(toks[1].kind, TokenKind::Word(Symbol::new("x")));
    }

    #[test]
    fn tuple_literal_basic() {
        assert_eq!(
            one("255.0.0"),
            TokenKind::Tuple(Rc::from(&[255u8, 0, 0][..]))
        );
        assert_eq!(one("0.0.0"), TokenKind::Tuple(Rc::from(&[0u8, 0, 0][..])));
        assert_eq!(
            one("128.64.32.128"),
            TokenKind::Tuple(Rc::from(&[128u8, 64, 32, 128][..]))
        );
    }

    #[test]
    fn tuple_literal_out_of_range() {
        let err = lex("300.0.0").unwrap_err();
        assert!(matches!(err, LexError::InvalidTuple { .. }));
        let err = lex("255.256.0").unwrap_err();
        assert!(matches!(err, LexError::InvalidTuple { .. }));
    }

    #[test]
    fn tuple_literal_too_many_components() {
        let err = lex("1.2.3.4.5").unwrap_err();
        assert!(matches!(err, LexError::InvalidTuple { .. }));
    }

    #[test]
    fn pair_tuple_do_not_break_floats() {
        // `1.5` stays a float (1 dot, no `x`).
        assert_eq!(one("1.5"), TokenKind::Float(1.5));
        // `1e3` stays a float exponent.
        assert_eq!(one("1e3"), TokenKind::Float(1000.0));
        // `5` stays an integer.
        assert_eq!(one("5"), TokenKind::Integer(5));
    }

    // --- M45 date!/time! lexer tests ---

    #[test]
    fn date_dmonyyyy_single_digit_day() {
        // Day with leading zero (01-Oct-1900) should parse correctly.
        let dv = match one("01-Oct-1900") {
            TokenKind::Date(dv) => dv,
            other => panic!("expected Date, got {other:?}"),
        };
        assert_eq!(
            dv.dt,
            NaiveDate::from_ymd_opt(1900, 10, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
        );
        assert_eq!(dv.zone, None);
    }

    #[test]
    fn date_dmonyyyy_basic() {
        let dv = match one("29-Jun-2024") {
            TokenKind::Date(dv) => dv,
            other => panic!("expected Date, got {other:?}"),
        };
        assert_eq!(
            dv.dt,
            NaiveDate::from_ymd_opt(2024, 6, 29)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
        );
        assert_eq!(dv.zone, None);
    }

    #[test]
    fn date_iso_basic() {
        let dv = match one("2024-06-29") {
            TokenKind::Date(dv) => dv,
            other => panic!("expected Date, got {other:?}"),
        };
        assert_eq!(
            dv.dt,
            NaiveDate::from_ymd_opt(2024, 6, 29)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
        );
        assert_eq!(dv.zone, None);
    }

    #[test]
    fn date_dslashyyyy_not_supported() {
        // DD/MM/YYYY is not supported because `/` is a lexer delimiter —
        // the run splits before reaching the date scanner. The lexer produces
        // separate Integer/Word tokens instead. This is a documented
        // limitation; use `DD-Mon-YYYY` or `YYYY-MM-DD` instead.
        let toks = kinds("29/06/2024");
        assert_eq!(toks[0], TokenKind::Integer(29));
    }

    #[test]
    fn date_datetime_with_zone() {
        // `29-Jun-2024/12:30:00+5:30` → zone = Some(330)
        let dv = match one("29-Jun-2024/12:30:00+5:30") {
            TokenKind::Date(dv) => dv,
            other => panic!("expected Date, got {other:?}"),
        };
        assert_eq!(
            dv.dt,
            NaiveDate::from_ymd_opt(2024, 6, 29)
                .unwrap()
                .and_hms_opt(12, 30, 0)
                .unwrap()
        );
        assert_eq!(dv.zone, Some(330));
    }

    #[test]
    fn date_iso_datetime_utc() {
        // `2024-06-29T12:30:00Z` → zone = Some(0)
        let dv = match one("2024-06-29T12:30:00Z") {
            TokenKind::Date(dv) => dv,
            other => panic!("expected Date, got {other:?}"),
        };
        assert_eq!(
            dv.dt,
            NaiveDate::from_ymd_opt(2024, 6, 29)
                .unwrap()
                .and_hms_opt(12, 30, 0)
                .unwrap()
        );
        assert_eq!(dv.zone, Some(0));
    }

    #[test]
    fn date_time_only_with_zone() {
        // `12:30:00-04:00` → epoch date, zone = Some(-240)
        let dv = match one("12:30:00-04:00") {
            TokenKind::Date(dv) => dv,
            other => panic!("expected Date, got {other:?}"),
        };
        assert_eq!(
            dv.dt,
            NaiveDate::from_ymd_opt(1970, 1, 1)
                .unwrap()
                .and_hms_opt(12, 30, 0)
                .unwrap()
        );
        assert_eq!(dv.zone, Some(-240));
    }

    #[test]
    fn date_time_only_no_zone() {
        let dv = match one("12:30:00") {
            TokenKind::Date(dv) => dv,
            other => panic!("expected Date, got {other:?}"),
        };
        assert_eq!(
            dv.dt,
            NaiveDate::from_ymd_opt(1970, 1, 1)
                .unwrap()
                .and_hms_opt(12, 30, 0)
                .unwrap()
        );
        assert_eq!(dv.zone, None);
    }

    #[test]
    fn date_zone_variants() {
        // `+HH:MM`, `-HH:MM`, `+HHMM`, `-HHMM`, `+HH`, `Z`
        let z = |s: &str| match one(s) {
            TokenKind::Date(dv) => dv.zone,
            other => panic!("expected Date for {s:?}, got {other:?}"),
        };
        // Use time-only forms so the zone attaches.
        assert_eq!(z("12:00:00+05:30"), Some(330));
        assert_eq!(z("12:00:00-04:00"), Some(-240));
        assert_eq!(z("12:00:00+0530"), Some(330));
        assert_eq!(z("12:00:00-0400"), Some(-240));
        assert_eq!(z("12:00:00+05"), Some(300));
        assert_eq!(z("12:00:00-04"), Some(-240));
        assert_eq!(z("12:00:00Z"), Some(0));
    }

    #[test]
    fn date_invalid_date() {
        // Feb 31 — structurally a date but values don't validate.
        let err = lex("31-Feb-2024").unwrap_err();
        assert!(matches!(err, LexError::InvalidDate { .. }));
    }

    #[test]
    fn date_invalid_zone() {
        // |zone| > 14*60 → invalid.
        let err = lex("29-Jun-2024/12:30:00+15:00").unwrap_err();
        assert!(matches!(err, LexError::InvalidDate { .. }));
    }

    #[test]
    fn date_does_not_break_plain_integers() {
        // `42` stays an integer.
        assert_eq!(one("42"), TokenKind::Integer(42));
        // `2024` stays an integer (4 digits, no `-` after).
        assert_eq!(one("2024"), TokenKind::Integer(2024));
    }

    #[test]
    fn date_mold_round_trips() {
        // Verify that the printer's mold form reparses to the same value.
        use crate::printer::mold_to_string;
        use crate::value::DateValue;
        let dv = DateValue::from_local(
            NaiveDate::from_ymd_opt(2024, 6, 29)
                .unwrap()
                .and_hms_opt(12, 30, 0)
                .unwrap(),
            Some(330),
        );
        let v = crate::value::Value::date(dv.clone());
        let molded = mold_to_string(&v);
        assert_eq!(molded, "29-Jun-2024/12:30:00+05:30");
        // Reparse.
        let reparsed = match one(&molded) {
            TokenKind::Date(d) => d,
            other => panic!("expected Date, got {other:?}"),
        };
        assert_eq!(reparsed, dv);
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
    fn char_and_binary_do_not_affect_bare_hash_issue() {
        // M80 behavior change: a bare `#` not followed by `"` or `{` now
        // scans as an issue! literal (was: `Word("#foo")`). Confirm the two
        // `#`-led forms that DO scan as char!/binary! still work.
        assert!(matches!(one("#\"a\""), TokenKind::Char('a')));
        assert!(matches!(one("#{48656C6C6F}"), TokenKind::Binary(_)));
        // And the bare `#` form is now an Issue.
        assert_eq!(one("#foo"), TokenKind::Issue(std::rc::Rc::from("foo")));
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
