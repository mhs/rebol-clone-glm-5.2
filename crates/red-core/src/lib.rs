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
pub use env::{CallFrame, CompileErrorKind, Env, EvalError, EvalMode, NativeFn, RefineArgs};
pub use error::{render_error, Error};
pub use lexer::{lex, LexError, Token, TokenKind};
pub use parser::{load, load_source, parse_program, ParseError, Parser};
pub use printer::{form, form_to_string, mold, mold_to_string};
pub use source::LineMap;
pub use value::{
    to_components, Binding, BitsetDef, ClosureDef, DateValue, ErrorValue, FuncDef, HashDef,
    ImageDef, MapDef, MapKey, ModuleDef, MoneyValue, ObjectDef, PortDef, PortScheme, PortState,
    SemanticShape, SemanticTypeDef, Series, Span, Symbol, TypesetDef, Value, VectorDef,
};
// M45: re-export the chrono types used in `DateValue` so downstream crates
// (red-eval) can construct/inspect dates without a direct chrono dependency.
// M140: also re-export `chrono::Duration` for the `duration!` value type.
pub use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
pub use vm_ir::{disasm, disasm_with_spans, CompiledBlock, Frame, Instr};
