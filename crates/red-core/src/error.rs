//! Combined error type for the lex → parse → eval pipeline. Keeps
//! `red-core`'s public surface small: callers match on `Error` instead of
//! stitching `LexError`/`ParseError`/`EvalError` together themselves.

use crate::env::EvalError;
use crate::lexer::LexError;
use crate::parser::ParseError;

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

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Lex(e) => write!(f, "lex error: {e:?}"),
            Error::Parse(e) => write!(f, "parse error: {e:?}"),
            Error::Eval(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}
