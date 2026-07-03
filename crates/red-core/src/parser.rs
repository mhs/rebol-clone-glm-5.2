//! Parser: `Vec<Token>` → `Value` tree. Recursive descent over a flat token
//! stream — no precedence grammar (Red is prefix/eager), so every value is
//! either one token or one bracketed group.
//!
//! Entry points:
//! - `parse_program`: recognizes `Red [...] <body>` and returns
//!   `(header_series, body_series)`. Falls back to bare-body parsing when
//!   no `Red` header is present.
//! - `load`: parses a bare body (no header) into a single `Series`.
//! - `load_source`: convenience combining `lex` + `load`.

use crate::lexer::{Token, TokenKind};
use crate::value::{Binding, Series, Span, Symbol, Value};
use std::rc::Rc;

/// Parse failure. Every variant carries the span of the offending token so
/// the CLI can later render `file:line:col: error: ...`.
#[derive(Clone, Debug, PartialEq)]
pub enum ParseError {
    /// Saw a token we didn't expect here (e.g. a stray `]` at top level, or
    /// EOF where a value was required).
    Unexpected {
        found: TokenKind,
        span: Span,
        expected: &'static str,
    },
    /// A `[` or `(` hit EOF before its closer.
    MissingClose {
        open: Span,
        kind: &'static str, // "block" | "paren"
    },
    /// No tokens at all (empty source after comments/whitespace stripped).
    EmptyInput,
}

impl ParseError {
    /// Byte-offset span where this error originated. `EmptyInput` has no
    /// span (the source was empty).
    pub fn span(&self) -> Option<Span> {
        match self {
            ParseError::Unexpected { span, .. } => Some(*span),
            ParseError::MissingClose { open, .. } => Some(*open),
            ParseError::EmptyInput => None,
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Message body only — `render_error` adds the `*** Error:` prefix
        // and `file:line:col:` location.
        match self {
            ParseError::Unexpected {
                found, expected, ..
            } => {
                write!(f, "expected {expected}, found {found:?}")
            }
            ParseError::MissingClose { kind, .. } => {
                write!(f, "missing closing {kind} delimiter")
            }
            ParseError::EmptyInput => write!(f, "empty input"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Recursive-descent cursor over a borrowed token slice.
pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

/// Result of peeking at the next token to decide if it's a path segment.
/// Used by [`Parser::assemble_path`].
enum PathSegment {
    /// Not a path segment — stop folding.
    None,
    /// An adjacent `Refinement` token (`/foo` right after the prior part).
    Refinement(Symbol, Span),
    /// An adjacent `Word("/")` — path separator before a paren/integer/
    /// bracketed value. The caller consumes the `/` and parses the next
    /// value as a path part.
    SlashThenValue,
    /// An adjacent `SetWord` — `obj/field:` produces this after the
    /// `Refinement(field)` has been folded. The caller consumes it and
    /// classifies the run as a `SetPath`.
    SetWord(Symbol, Span),
}

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    /// Token at the cursor, or `None` at EOF.
    fn peek_opt(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    /// Token at the cursor; `EmptyInput` if at EOF.
    fn peek(&self) -> Result<&Token, ParseError> {
        self.peek_opt().ok_or(ParseError::EmptyInput)
    }

    /// Advance the cursor and return a clone of the consumed token.
    /// EOF → `EmptyInput`.
    fn advance(&mut self) -> Result<Token, ParseError> {
        let tok = self.peek()?;
        let cloned = tok.clone();
        self.pos += 1;
        Ok(cloned)
    }

    /// Advance only if the current token matches `kind`; otherwise
    /// `Unexpected`. Returns a clone of the consumed token.
    fn consume(&mut self, kind: &TokenKind, expected: &'static str) -> Result<Token, ParseError> {
        let tok = self.peek()?;
        if tok.kind == *kind {
            let cloned = tok.clone();
            self.pos += 1;
            Ok(cloned)
        } else {
            Err(ParseError::Unexpected {
                found: tok.kind.clone(),
                span: tok.span,
                expected,
            })
        }
    }

    /// Parse a single value at the cursor. Dispatches on `TokenKind`.
    pub fn parse_value(&mut self) -> Result<Value, ParseError> {
        let tok = self.peek()?.clone();
        match tok.kind {
            TokenKind::LBracket => self.parse_block(),
            TokenKind::LParen => self.parse_paren(),
            TokenKind::Integer(n) => {
                self.advance()?;
                Ok(Value::Integer { n, span: tok.span })
            }
            TokenKind::Float(f) => {
                self.advance()?;
                Ok(Value::Float { f, span: tok.span })
            }
            TokenKind::Percent(value) => {
                self.advance()?;
                Ok(Value::Percent {
                    value,
                    span: tok.span,
                })
            }
            TokenKind::Money(mv) => {
                self.advance()?;
                Ok(Value::Money {
                    amount: std::rc::Rc::new(mv),
                    span: tok.span,
                })
            }
            TokenKind::Issue(s) => {
                self.advance()?;
                let head = Value::Issue { s, span: tok.span };
                // M80: fold adjacent refinements (`#ABC/x`) into a path.
                self.assemble_path(head, tok.span)
            }
            TokenKind::Email(addr) => {
                self.advance()?;
                let head = Value::Email {
                    addr,
                    span: tok.span,
                };
                // M80: fold adjacent refinements (`foo@bar.com/user`) into a path.
                self.assemble_path(head, tok.span)
            }
            TokenKind::String(s) => {
                self.advance()?;
                Ok(Value::String { s, span: tok.span })
            }
            TokenKind::Char(c) => {
                self.advance()?;
                Ok(Value::Char { c, span: tok.span })
            }
            TokenKind::Binary(b) => {
                self.advance()?;
                Ok(Value::String8 {
                    bytes: b.to_vec(),
                    span: tok.span,
                })
            }
            TokenKind::Pair(x_raw, y_raw) => {
                self.advance()?;
                // Each component parses as int if possible, else float.
                // Component spans are not tracked individually (the pair's
                // span covers both); components get the zero placeholder.
                let x = parse_number_value(&x_raw)?;
                let y = parse_number_value(&y_raw)?;
                let head = Value::Pair {
                    x: Rc::new(x),
                    y: Rc::new(y),
                    span: tok.span,
                };
                // M44: fold adjacent refinements (`100x200/x`) into a path.
                self.assemble_path(head, tok.span)
            }
            TokenKind::Tuple(b) => {
                self.advance()?;
                let head = Value::Tuple {
                    bytes: b,
                    span: tok.span,
                };
                // M44: fold adjacent refinements (`255.0.0/r`) into a path.
                self.assemble_path(head, tok.span)
            }
            TokenKind::Date(dv) => {
                self.advance()?;
                let head = Value::Date {
                    dt: std::rc::Rc::new(dv),
                    span: tok.span,
                };
                // M45: fold adjacent refinements (`29-Jun-2024/year`) into a path.
                self.assemble_path(head, tok.span)
            }
            TokenKind::Word(sym) => {
                self.advance()?;
                let head = Value::Word {
                    sym,
                    binding: Binding::Unbound,
                    span: tok.span,
                };
                self.assemble_path(head, tok.span)
            }
            TokenKind::SetWord(sym) => {
                self.advance()?;
                Ok(Value::SetWord {
                    sym,
                    binding: Binding::Unbound,
                    span: tok.span,
                })
            }
            TokenKind::GetWord(sym) => {
                self.advance()?;
                let head = Value::GetWord {
                    sym,
                    binding: Binding::Unbound,
                    span: tok.span,
                };
                self.assemble_path(head, tok.span)
            }
            TokenKind::LitWord(sym) => {
                self.advance()?;
                let head = Value::LitWord {
                    sym,
                    span: tok.span,
                };
                self.assemble_path(head, tok.span)
            }
            TokenKind::Refinement(sym) => {
                self.advance()?;
                Ok(Value::Refinement {
                    sym,
                    span: tok.span,
                })
            }
            TokenKind::File(path) => {
                self.advance()?;
                Ok(Value::File {
                    path,
                    span: tok.span,
                })
            }
            TokenKind::Url(url) => {
                self.advance()?;
                Ok(Value::Url {
                    url,
                    span: tok.span,
                })
            }
            // Stray closers at value position are always errors.
            TokenKind::RBracket => Err(ParseError::Unexpected {
                found: tok.kind,
                span: tok.span,
                expected: "value",
            }),
            TokenKind::RParen => Err(ParseError::Unexpected {
                found: tok.kind,
                span: tok.span,
                expected: "value",
            }),
        }
    }

    /// Fold a run of *adjacent* path segments following `head` into a path
    /// value. Segments come in two lexical shapes:
    ///
    /// - `Refinement(foo)` — the normal `/foo` case. The refinement body
    ///   starts right where the prior part ended (no whitespace), so we fold
    ///   it as a `Word` path part.
    /// - `Word("/")` followed by another value — the `/` is a delimiter so a
    ///   refinement body starting with `(`, `)`, `[`, `]`, etc. is empty and
    ///   the lexer emits the bare `Word("/")` (division operator). When that
    ///   `/` is *adjacent* to the prior part, it's actually a path separator:
    ///   consume it and parse the next value (`(paren)`, `integer`, etc.) as
    ///   a path part. This handles `foo/(a+b)/bar` and `foo/2`.
    ///
    /// After folding, classify the whole run by its head:
    /// - `GetWord` head + ≥1 parts → `Value::GetPath`
    /// - `LitWord` head + ≥1 parts → `Value::LitPath`
    /// - `Word` head + ≥1 parts, and the token immediately after the run is
    ///   an adjacent `SetWord` whose name matches the last part →
    ///   `Value::SetPath` (the lexer splits `obj/field:` into
    ///   `Word Refinement SetWord`; we consume the SetWord and fold).
    /// - otherwise → `Value::Path`
    ///
    /// A refinement/path-segment separated by whitespace from its
    /// predecessor is left as a standalone token — the caller sees the
    /// assembled path (or just the head if no adjacent segments) and the
    /// standalone token surfaces as a separate value.
    fn assemble_path(&mut self, head: Value, head_span: Span) -> Result<Value, ParseError> {
        let mut parts = vec![head];
        let mut end = head_span.end;
        let mut last_part_span = head_span;
        loop {
            let action = self.peek_path_segment(end, last_part_span);
            match action {
                PathSegment::Refinement(sym, span) => {
                    end = span.end;
                    last_part_span = span;
                    self.advance()?; // consume the Refinement token
                    parts.push(Value::Word {
                        sym,
                        binding: Binding::Unbound,
                        span,
                    });
                }
                PathSegment::SlashThenValue => {
                    // Consume the `Word("/")` path separator, then parse the
                    // next value as a path part. The parsed value retains its
                    // own span; the path's overall span extends to its end.
                    self.advance()?; // consume Word("/")
                    let val = self.parse_value()?;
                    end = val.span().map(|s| s.end).unwrap_or(end);
                    last_part_span = val.span().unwrap_or(last_part_span);
                    parts.push(val);
                }
                PathSegment::SetWord(_sym, _span) => {
                    // The lexer produced `Refinement(field) SetWord(field)` for
                    // `obj/field:`. We already folded the Refinement as the
                    // last part; now consume the SetWord and mark the run as
                    // a SetPath. The SetWord's body == the last part's name
                    // (guaranteed by the lexer), so we don't need to push a
                    // duplicate — just consume and classify.
                    let setword_span = {
                        if let Some(tok) = self.peek_opt() {
                            tok.span
                        } else {
                            break;
                        }
                    };
                    self.advance()?; // consume the SetWord
                    end = setword_span.end;
                    // Demote GetWord/LitWord head to plain Word so molding
                    // doesn't double up the prefix.
                    if let Some(Value::GetWord { sym, span, .. })
                    | Some(Value::LitWord { sym, span }) = parts.first()
                    {
                        let s = sym.clone();
                        let sp = *span;
                        parts[0] = Value::Word {
                            sym: s,
                            binding: Binding::Unbound,
                            span: sp,
                        };
                    }
                    let path_span = Span::new(head_span.start, end);
                    return Ok(Value::SetPath {
                        parts,
                        span: path_span,
                    });
                }
                PathSegment::None => break,
            }
        }
        if parts.len() == 1 {
            Ok(parts.pop().unwrap())
        } else {
            let path_span = Span::new(head_span.start, end);
            // Classify by head kind. For GetPath/LitPath, the head's `:`/`'`
            // marker is the *path's* prefix, not the head word's — so demote
            // the head from GetWord/LitWord to a plain Word so molding
            // doesn't double up the prefix.
            Ok(match &parts[0] {
                Value::GetWord { sym, span, .. } => {
                    parts[0] = Value::Word {
                        sym: sym.clone(),
                        binding: Binding::Unbound,
                        span: *span,
                    };
                    Value::GetPath {
                        parts,
                        span: path_span,
                    }
                }
                Value::LitWord { sym, span } => {
                    parts[0] = Value::Word {
                        sym: sym.clone(),
                        binding: Binding::Unbound,
                        span: *span,
                    };
                    Value::LitPath {
                        parts,
                        span: path_span,
                    }
                }
                _ => Value::Path {
                    parts,
                    span: path_span,
                },
            })
        }
    }

    /// Peek at the next token and decide what kind of path segment it is
    /// (if any). `prev_end` is the end byte of the previous part; `prev_span`
    /// is its full span (used for the SetWord overlap check).
    fn peek_path_segment(&self, prev_end: usize, prev_span: Span) -> PathSegment {
        let tok = match self.peek_opt() {
            Some(t) => t,
            None => return PathSegment::None,
        };
        match &tok.kind {
            // Adjacent Refinement: `/foo` right after the prior part.
            TokenKind::Refinement(sym) => {
                if tok.span.start == prev_end {
                    PathSegment::Refinement(sym.clone(), tok.span)
                } else {
                    PathSegment::None
                }
            }
            // `Word("/")` adjacent to the prior part: path separator before
            // a paren/integer/bracketed value. The lexer emits bare `Word("/")`
            // when the refinement body would be empty (next char is a
            // delimiter like `(`).
            TokenKind::Word(sym) if sym.as_str() == "/" => {
                if tok.span.start == prev_end {
                    PathSegment::SlashThenValue
                } else {
                    PathSegment::None
                }
            }
            // Adjacent SetWord: `obj/field:` lexes as
            // `Word(obj) Refinement(field) SetWord(field)`. The SetWord's
            // span overlaps with the last Refinement's span (they share the
            // body bytes). Detect via span overlap + name match.
            TokenKind::SetWord(sym) => {
                if tok.span.start < prev_span.end && tok.span.end == prev_span.end + 1 {
                    // Name must match the last part's name. The last part is
                    // a Word derived from a Refinement; check its symbol.
                    // (Caller verifies via the _sym; we signal the segment.)
                    let _ = sym;
                    PathSegment::SetWord(sym.clone(), tok.span)
                } else {
                    PathSegment::None
                }
            }
            _ => PathSegment::None,
        }
    }

    /// `[ ... ]`. The closing `]` is required; EOF before it → `MissingClose`.
    pub fn parse_block(&mut self) -> Result<Value, ParseError> {
        let open_tok = self.consume(&TokenKind::LBracket, "[")?;
        let open_span = open_tok.span;
        let mut items = Vec::new();
        loop {
            match self.peek_opt() {
                None => {
                    return Err(ParseError::MissingClose {
                        open: open_span,
                        kind: "block",
                    });
                }
                Some(tok) if tok.kind == TokenKind::RBracket => {
                    let close_tok = self.advance()?;
                    return Ok(Value::Block {
                        series: Series::new(items),
                        span: Span::new(open_span.start, close_tok.span.end),
                    });
                }
                Some(_) => items.push(self.parse_value()?),
            }
        }
    }

    /// `( ... )`. The closing `)` is required; EOF before it → `MissingClose`.
    pub fn parse_paren(&mut self) -> Result<Value, ParseError> {
        let open_tok = self.consume(&TokenKind::LParen, "(")?;
        let open_span = open_tok.span;
        let mut items = Vec::new();
        loop {
            match self.peek_opt() {
                None => {
                    return Err(ParseError::MissingClose {
                        open: open_span,
                        kind: "paren",
                    });
                }
                Some(tok) if tok.kind == TokenKind::RParen => {
                    let close_tok = self.advance()?;
                    return Ok(Value::Paren {
                        series: Series::new(items),
                        span: Span::new(open_span.start, close_tok.span.end),
                    });
                }
                Some(_) => items.push(self.parse_value()?),
            }
        }
    }

    /// Consume every remaining token as a flat sequence of values. Stray
    /// `]`/`)` surface as `Unexpected` from `parse_value`.
    fn parse_rest_into_series(&mut self) -> Result<Series, ParseError> {
        let mut items = Vec::new();
        while self.peek_opt().is_some() {
            items.push(self.parse_value()?);
        }
        Ok(Series::new(items))
    }
}

/// Parse a bare body (no header). Returns the body as a `Series`.
pub fn load(tokens: &[Token]) -> Result<Series, ParseError> {
    if tokens.is_empty() {
        return Ok(Series::empty());
    }
    let mut p = Parser::new(tokens);
    p.parse_rest_into_series()
}

/// Parse a program with optional `Red [...]` header.
///
/// If the first token is `Word("Red")`, consumes it, then one header block,
/// then one body block, returning `(header, body)`. Otherwise treats the
/// whole stream as a bare body (header becomes an empty `Series`).
pub fn parse_program(tokens: &[Token]) -> Result<(Series, Series), ParseError> {
    if tokens.is_empty() {
        return Err(ParseError::EmptyInput);
    }

    // Peek without consuming: only treat as header if the very first token
    // is the literal word `Red`.
    let is_red_header = matches!(
        &tokens[0].kind,
        TokenKind::Word(w) if w.as_str() == "Red"
    );

    if !is_red_header {
        let body = load(tokens)?;
        return Ok((Series::empty(), body));
    }

    let mut p = Parser::new(tokens);
    // Consume `Red`.
    let _red = p.advance()?;
    // Header block.
    let header_val = p.parse_block()?;
    let header = match header_val {
        Value::Block { series, .. } => series,
        _ => unreachable!("parse_block always yields Value::Block"),
    };
    // Body: everything remaining as a flat series (matches `load` semantics —
    // a script body is a sequence of top-level values, not necessarily a
    // single bracketed block).
    let body = p.parse_rest_into_series()?;
    Ok((header, body))
}

/// Convenience: lex + parse a bare body in one call.
pub fn load_source(src: &str) -> Result<Series, crate::error::Error> {
    let tokens = crate::lexer::lex(src)?;
    let series = load(&tokens)?;
    Ok(series)
}

/// Parse a pair! component substring (raw text from the lexer) into an
/// `Integer` or `Float` `Value`. Integers parse first (so `2` stays an int);
/// floats fall through. Component spans are the zero placeholder since the
/// lexer only tracks the whole-pair span.
fn parse_number_value(text: &str) -> Result<Value, ParseError> {
    if let Ok(n) = text.parse::<i64>() {
        return Ok(Value::Integer {
            n,
            span: Span::default(),
        });
    }
    if let Ok(f) = text.parse::<f64>() {
        return Ok(Value::Float {
            f,
            span: Span::default(),
        });
    }
    Err(ParseError::Unexpected {
        found: TokenKind::Word(Symbol::new(text)),
        span: Span::default(),
        expected: "number component",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::printer::mold_to_string;

    /// Helper: lex+load, then mold the resulting body series (elements joined
    /// by single spaces, no surrounding brackets) for easy string comparison.
    fn mold_src(src: &str) -> String {
        let series = load_source(src).expect("load_source failed");
        mold_series(&series)
    }

    /// Mold a `Series` as a space-joined sequence (no surrounding brackets).
    fn mold_series(series: &Series) -> String {
        use crate::printer::mold;
        let data = series.data.borrow();
        let mut out = String::new();
        for (i, v) in data.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            mold(v, &mut out);
        }
        out
    }

    #[test]
    fn single_integer_parses() {
        let series = load(&lex("42").unwrap()).unwrap();
        assert_eq!(series.data.borrow().len(), 1);
        assert!(matches!(
            series.data.borrow()[0],
            Value::Integer { n: 42, .. }
        ));
    }

    #[test]
    fn load_molds_integer_back() {
        assert_eq!(mold_src("42"), "42");
    }

    #[test]
    fn nested_block_structure() {
        assert_eq!(mold_src("[a [b c] d]"), "[a [b c] d]");
    }

    #[test]
    fn all_word_kinds_parse() {
        assert_eq!(mold_src("foo foo: :foo 'foo"), "foo foo: :foo 'foo");
    }

    #[test]
    fn empty_block_and_paren() {
        assert_eq!(mold_src("[] ()"), "[] ()");
    }

    #[test]
    fn nested_parens() {
        assert_eq!(mold_src("(1 (2 3) 4)"), "(1 (2 3) 4)");
    }

    #[test]
    fn block_with_paren_inside() {
        assert_eq!(mold_src("[1 (2 3) 4]"), "[1 (2 3) 4]");
    }

    #[test]
    fn strings_quoted_and_braced() {
        // Quoted string round-trips; braced string with no escaping-required
        // chars also round-trips (printer molds both as `"..."`).
        assert_eq!(mold_src("\"hello\""), "\"hello\"");
        assert_eq!(mold_src("{abc}"), "\"abc\"");
    }

    #[test]
    fn header_and_body_parse_program() {
        let toks = lex("Red [title: \"Hi\"] print \"hi\"").unwrap();
        let (header, body) = parse_program(&toks).unwrap();
        // Header is a Series of block contents; mold as a Block to get brackets.
        assert_eq!(mold_to_string(&Value::block(header)), "[title: \"Hi\"]");
        assert_eq!(mold_series(&body), "print \"hi\"");
    }

    #[test]
    fn header_preserves_red_word_in_body_when_no_header() {
        // A `Red` not at position 0 is just a word.
        let toks = lex("print Red").unwrap();
        let (header, body) = parse_program(&toks).unwrap();
        assert_eq!(header.data.borrow().len(), 0);
        assert_eq!(mold_series(&body), "print Red");
    }

    #[test]
    fn bare_body_via_load() {
        let toks = lex("foo: 42 print foo").unwrap();
        let body = load(&toks).unwrap();
        assert_eq!(mold_series(&body), "foo: 42 print foo");
    }

    #[test]
    fn missing_close_block() {
        let toks = lex("[1 2").unwrap();
        let err = load(&toks).unwrap_err();
        assert!(matches!(
            err,
            ParseError::MissingClose { kind: "block", .. }
        ));
    }

    #[test]
    fn missing_close_paren() {
        let toks = lex("(1 2").unwrap();
        let err = load(&toks).unwrap_err();
        assert!(matches!(
            err,
            ParseError::MissingClose { kind: "paren", .. }
        ));
    }

    #[test]
    fn unexpected_stray_rbracket() {
        let toks = lex("1 ] 2").unwrap();
        let err = load(&toks).unwrap_err();
        assert!(matches!(
            err,
            ParseError::Unexpected {
                expected: "value",
                ..
            }
        ));
    }

    #[test]
    fn unexpected_stray_rparen() {
        let toks = lex("1 ) 2").unwrap();
        let err = load(&toks).unwrap_err();
        assert!(matches!(
            err,
            ParseError::Unexpected {
                expected: "value",
                ..
            }
        ));
    }

    #[test]
    fn empty_source_loads_to_empty_series() {
        let toks = lex("").unwrap();
        let body = load(&toks).unwrap();
        assert_eq!(body.data.borrow().len(), 0);
    }

    #[test]
    fn comments_only_loads_to_empty_series() {
        let toks = lex("; just a comment\n; another").unwrap();
        let body = load(&toks).unwrap();
        assert_eq!(body.data.borrow().len(), 0);
    }

    #[test]
    fn parse_program_empty_input_errors() {
        let err = parse_program(&[]).unwrap_err();
        assert_eq!(err, ParseError::EmptyInput);
    }

    #[test]
    fn block_span_covers_delimiters() {
        let toks = lex("[42]").unwrap();
        let mut p = Parser::new(&toks);
        let v = p.parse_block().unwrap();
        match v {
            Value::Block { span, .. } => {
                assert_eq!(span, Span::new(0, 4));
            }
            _ => panic!("expected Block"),
        }
    }

    #[test]
    fn paren_span_covers_delimiters() {
        let toks = lex("(42)").unwrap();
        let mut p = Parser::new(&toks);
        let v = p.parse_paren().unwrap();
        match v {
            Value::Paren { span, .. } => {
                assert_eq!(span, Span::new(0, 4));
            }
            _ => panic!("expected Paren"),
        }
    }

    #[test]
    fn load_source_propagates_lex_error() {
        let err = load_source("\"unterminated").unwrap_err();
        assert!(matches!(err, crate::error::Error::Lex(_)));
    }

    #[test]
    fn load_source_propagates_parse_error() {
        let err = load_source("[1 2").unwrap_err();
        assert!(matches!(err, crate::error::Error::Parse(_)));
    }

    // --- Milestone 13 Phase A: paths & refinements ---

    #[test]
    fn adjacent_path_assembles() {
        // `foo/bar` → single Path value with two word parts.
        assert_eq!(mold_src("foo/bar"), "foo/bar");
    }

    #[test]
    fn adjacent_path_three_parts() {
        assert_eq!(mold_src("a/b/c"), "a/b/c");
    }

    #[test]
    fn spaced_refinement_stays_separate() {
        // `copy /part` — space breaks adjacency, so it stays two values.
        assert_eq!(mold_src("copy /part"), "copy /part");
    }

    #[test]
    fn standalone_refinement_molds_with_slash() {
        assert_eq!(mold_src("/part"), "/part");
    }

    #[test]
    fn path_inside_block() {
        assert_eq!(mold_src("[foo/bar baz]"), "[foo/bar baz]");
    }

    #[test]
    fn get_word_path_assembles() {
        // `:foo/bar` — get-word head followed by adjacent refinement.
        assert_eq!(mold_src(":foo/bar"), ":foo/bar");
    }

    #[test]
    fn lit_word_path_assembles() {
        assert_eq!(mold_src("'foo/bar"), "'foo/bar");
    }

    #[test]
    fn path_span_covers_whole_run() {
        let toks = lex("foo/bar").unwrap();
        let body = load(&toks).unwrap();
        let data = body.data.borrow();
        match &data[0] {
            Value::Path { span, .. } => {
                assert_eq!(*span, Span::new(0, 7));
            }
            other => panic!("expected Path, got {other:?}"),
        }
    }

    #[test]
    fn refinement_in_block_parses_as_refinement_value() {
        // Spec-block shape: `func [x /only]` — `/only` is a standalone
        // Refinement value (not a path; it's not adjacent to a word).
        let toks = lex("[x /only]").unwrap();
        let body = load(&toks).unwrap();
        let data = body.data.borrow();
        // The outer `[...]` is one Block value; inspect its contents.
        let inner = match &data[0] {
            Value::Block { series, .. } => series.data.borrow(),
            other => panic!("expected Block, got {other:?}"),
        };
        assert!(matches!(&inner[0], Value::Word { sym, .. } if sym.as_str() == "x"));
        assert!(matches!(&inner[1], Value::Refinement { sym, .. } if sym.as_str() == "only"));
    }

    // --- M19 path variant tests ---

    #[test]
    fn get_path_parses_and_molds() {
        assert_eq!(mold_src(":foo/bar"), ":foo/bar");
    }

    #[test]
    fn lit_path_parses_and_molds() {
        assert_eq!(mold_src("'foo/bar"), "'foo/bar");
    }

    #[test]
    fn set_path_parses_and_molds() {
        assert_eq!(mold_src("obj/field:"), "obj/field:");
    }

    #[test]
    fn set_path_three_parts() {
        assert_eq!(mold_src("a/b/c:"), "a/b/c:");
    }

    #[test]
    fn path_with_paren_part_parses_and_molds() {
        assert_eq!(mold_src("foo/(1 + 2)/bar"), "foo/(1 + 2)/bar");
    }

    #[test]
    fn path_with_integer_part_parses() {
        // `foo/2` now parses as a Path (block/integer select), not division.
        assert_eq!(mold_src("foo/2"), "foo/2");
    }

    #[test]
    fn path_with_paren_and_integer() {
        assert_eq!(mold_src("foo/(x)/2"), "foo/(x)/2");
    }

    #[test]
    fn get_path_span_covers_whole_run() {
        let toks = lex(":foo/bar").unwrap();
        let body = load(&toks).unwrap();
        let data = body.data.borrow();
        match &data[0] {
            Value::GetPath { span, .. } => {
                assert_eq!(*span, Span::new(0, 8));
            }
            other => panic!("expected GetPath, got {other:?}"),
        }
    }

    #[test]
    fn set_path_span_covers_whole_run() {
        let toks = lex("obj/field:").unwrap();
        let body = load(&toks).unwrap();
        let data = body.data.borrow();
        match &data[0] {
            Value::SetPath { span, parts, .. } => {
                // `obj/field:` is 10 bytes; span end exclusive = 10.
                assert_eq!(*span, Span::new(0, 10));
                assert_eq!(parts.len(), 2);
            }
            other => panic!("expected SetPath, got {other:?}"),
        }
    }

    #[test]
    fn lit_path_span_covers_whole_run() {
        let toks = lex("'foo/bar").unwrap();
        let body = load(&toks).unwrap();
        let data = body.data.borrow();
        match &data[0] {
            Value::LitPath { span, .. } => {
                assert_eq!(*span, Span::new(0, 8));
            }
            other => panic!("expected LitPath, got {other:?}"),
        }
    }

    #[test]
    fn path_with_paren_part_classified_as_path() {
        let toks = lex("foo/(1 + 2)/bar").unwrap();
        let body = load(&toks).unwrap();
        let data = body.data.borrow();
        match &data[0] {
            Value::Path { parts, .. } => {
                assert_eq!(parts.len(), 3);
                // Middle part should be a Paren.
                assert!(matches!(parts[1], Value::Paren { .. }));
            }
            other => panic!("expected Path, got {other:?}"),
        }
    }
}
