//! red-core: value model, context, env, printer, lexer, parser.

pub mod context;
pub mod env;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod printer;
pub mod value;

pub use context::Context;
pub use env::{CallFrame, Env, EvalError, NativeFn};
pub use error::Error;
pub use lexer::{lex, LexError, Token, TokenKind};
pub use parser::{load, load_source, parse_program, ParseError, Parser};
pub use printer::{mold, mold_to_string};
pub use value::{Binding, FuncDef, Series, Span, Symbol, Value};
