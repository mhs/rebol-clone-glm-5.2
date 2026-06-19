//! Combined error type for the lex → parse → eval pipeline. Keeps
//! `red-core`'s public surface small: callers match on `Error` instead of
//! stitching `LexError`/`ParseError`/`EvalError` together themselves.
//!
//! `Display` renders just the message body (no `*** Error:` prefix, no
//! `file:line:col:` location). Use [`render_error`] to produce the full
//! Red-style `*** Error: [file:line:col: ]<msg>` line for CLI/REPL output.

use crate::env::EvalError;
use crate::lexer::LexError;
use crate::parser::ParseError;
use crate::source::LineMap;
use crate::value::Span;

/// Any error raised while turning source text into a result value.
#[derive(Debug)]
pub enum Error {
    Lex(LexError),
    Parse(ParseError),
    Eval(EvalError),
}

impl From<LexError> for Error {
    fn from(e: LexError) -> Self {
        Error::Lex(e)
    }
}

impl From<ParseError> for Error {
    fn from(e: ParseError) -> Self {
        Error::Parse(e)
    }
}

impl From<EvalError> for Error {
    fn from(e: EvalError) -> Self {
        Error::Eval(e)
    }
}

impl Error {
    /// Byte-offset span where this error originated, if any. Delegates to the
    /// inner error type. Used by [`render_error`] to build the
    /// `file:line:col:` prefix.
    pub fn span(&self) -> Option<Span> {
        match self {
            Error::Lex(e) => Some(e.span()),
            Error::Parse(e) => e.span(),
            Error::Eval(e) => e.span(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Just the message body — no `*** Error:` prefix and no
        // `lex error:`/`parse error:` wrapper. `render_error` produces the
        // full diagnostic line; this bare `Display` is used by test helpers
        // that only care about the message body.
        match self {
            Error::Lex(e) => write!(f, "{e}"),
            Error::Parse(e) => write!(f, "{e}"),
            Error::Eval(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Render an `Error` as a full Red-style diagnostic line:
/// `*** Error: [file:line:col: ]<msg>`.
///
/// - `file`: the source file path (`Some("examples/hello.red")`) or `None`
///   for the REPL / stdin. When `Some`, the path is prepended.
/// - `src`: the source text the error refers to. Used to build a `LineMap`
///   so the error's byte-offset span can be translated to `line:col`. The
///   `src` passed here must be the same text the lexer saw.
/// - `err`: the error to render.
///
/// The `file:line:col:` location is included only when the error carries a
/// non-default span (i.e. a real source position). Synthetic errors with a
/// zero span (or `EmptyInput`) omit the location and render just
/// `*** Error: <msg>`.
pub fn render_error(file: Option<&str>, src: &str, err: &Error) -> String {
    let body = err.to_string();
    let Some(span) = err.span() else {
        return format!("*** Error: {body}");
    };
    if span.is_default() {
        return format!("*** Error: {body}");
    }
    let map = LineMap::new(src);
    let (line, col) = map.line_col(span.start);
    match file {
        Some(path) => format!("*** Error: {path}:{line}:{col}: {body}"),
        None => format!("*** Error: {line}:{col}: {body}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_eval_error_with_location() {
        // `foo` at offset 0 in "foo" → line 1, col 1.
        let err = Error::Eval(EvalError::UnboundWord {
            sym: crate::value::Symbol::new("foo"),
            span: Span::new(0, 3),
        });
        let rendered = render_error(Some("test.red"), "foo", &err);
        assert_eq!(rendered, "*** Error: test.red:1:1: \"foo\" has no value");
    }

    #[test]
    fn render_eval_error_no_file() {
        let err = Error::Eval(EvalError::UnboundWord {
            sym: crate::value::Symbol::new("bar"),
            span: Span::new(5, 8),
        });
        // "x: 5 bar" — bar at offset 5, line 1, col 6.
        let rendered = render_error(None, "x: 5 bar", &err);
        assert_eq!(rendered, "*** Error: 1:6: \"bar\" has no value");
    }

    #[test]
    fn render_eval_error_multiline() {
        // "print 1\nfoo" — foo at offset 8, line 2, col 1.
        let err = Error::Eval(EvalError::UnboundWord {
            sym: crate::value::Symbol::new("foo"),
            span: Span::new(8, 11),
        });
        let rendered = render_error(Some("f.red"), "print 1\nfoo", &err);
        assert_eq!(rendered, "*** Error: f.red:2:1: \"foo\" has no value");
    }

    #[test]
    fn render_eval_error_zero_span_omits_location() {
        let err = Error::Eval(EvalError::Native {
            message: "something went wrong".into(),
            span: Span::new(0, 0),
        });
        let rendered = render_error(Some("f.red"), "src", &err);
        assert_eq!(rendered, "*** Error: something went wrong");
    }

    #[test]
    fn render_lex_error_with_location() {
        // `"abc` — unterminated string starting at offset 0.
        let err = Error::Lex(LexError::UnterminatedString {
            span: Span::new(0, 4),
        });
        let rendered = render_error(Some("f.red"), "\"abc", &err);
        assert_eq!(rendered, "*** Error: f.red:1:1: unterminated string");
    }

    #[test]
    fn render_parse_error_with_location() {
        // `1 ] 2` — stray `]` at offset 2.
        let toks = crate::lexer::lex("1 ] 2").unwrap();
        let parse_err = crate::parser::load(&toks).unwrap_err();
        let err = Error::Parse(parse_err);
        let rendered = render_error(Some("f.red"), "1 ] 2", &err);
        assert_eq!(
            rendered,
            "*** Error: f.red:1:3: expected value, found RBracket"
        );
    }

    #[test]
    fn render_empty_input_no_location() {
        let err = Error::Parse(ParseError::EmptyInput);
        let rendered = render_error(Some("f.red"), "", &err);
        assert_eq!(rendered, "*** Error: empty input");
    }

    #[test]
    fn display_eval_error_is_just_message_body() {
        let err = Error::Eval(EvalError::UnboundWord {
            sym: crate::value::Symbol::new("foo"),
            span: Span::new(0, 3),
        });
        // No `*** Error:` prefix, no location — just the body.
        assert_eq!(err.to_string(), "\"foo\" has no value");
    }
}
