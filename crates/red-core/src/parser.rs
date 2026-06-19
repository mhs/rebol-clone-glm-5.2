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
use crate::value::{Binding, Series, Span, Value};

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
            TokenKind::String(s) => {
                self.advance()?;
                Ok(Value::String { s, span: tok.span })
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

    /// Fold any run of *adjacent* refinement tokens (`/foo`, no whitespace)
    /// following `head` into a `Value::Path`. A refinement separated by
    /// whitespace from its predecessor is left as a standalone `Refinement`
    /// value — the evaluator handles spaced refinement flags at call sites.
    /// Path parts are stored as `Word` values so molding yields `foo/bar`.
    fn assemble_path(&mut self, head: Value, head_span: Span) -> Result<Value, ParseError> {
        let mut parts = vec![head];
        let mut end = head_span.end;
        loop {
            // Peek + clone the needed fields so we can release the immutable
            // borrow before `advance` (which needs `&mut self`).
            let next = self.peek_opt().and_then(|tok| match &tok.kind {
                TokenKind::Refinement(sym) => {
                    // Adjacency: refinement must start where the prior part
                    // ended (no whitespace between).
                    if tok.span.start != end {
                        return None;
                    }
                    Some((sym.clone(), tok.span))
                }
                _ => None,
            });
            match next {
                Some((sym, span)) => {
                    end = span.end;
                    self.advance()?;
                    parts.push(Value::Word {
                        sym,
                        binding: Binding::Unbound,
                        span,
                    });
                }
                None => break,
            }
        }
        if parts.len() == 1 {
            Ok(parts.pop().unwrap())
        } else {
            Ok(Value::Path {
                parts,
                span: Span::new(head_span.start, end),
            })
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
}
