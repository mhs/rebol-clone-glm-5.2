//! red-core: value model, context, env, printer, lexer, parser.

pub mod context;
pub mod env;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod printer;
pub mod source;
pub mod value;
pub mod vm_ir;

pub use context::Context;
pub use env::{CallFrame, Env, EvalError, NativeFn, RefineArgs};
pub use error::{render_error, Error};
pub use lexer::{lex, LexError, Token, TokenKind};
pub use parser::{load, load_source, parse_program, ParseError, Parser};
pub use printer::{form, form_to_string, mold, mold_to_string};
pub use source::LineMap;
pub use value::{Binding, ErrorValue, FuncDef, ObjectDef, Series, Span, Symbol, Value};
pub use vm_ir::{CompiledBlock, Frame, Instr};
