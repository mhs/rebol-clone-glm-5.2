//! Core value model: `Value`, `Symbol`, `Span`, `Series`, `Binding`, `FuncDef`.
//!
//! Milestone 2 scope: types exist with stubbed binding/function fields so the
//! printer can be built and tested. Real binding/function machinery lands in
//! Milestones 5 and 9.

use std::cell::RefCell;
use std::rc::Rc;

use crate::context::Context;

/// Byte-offset span into the original source. Carried through lex → parse →
/// eval so errors can point at the offending bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// Interned word. POC uses a simple `Rc<str>` newtype; `string_cache` is
/// deferred until profiling shows a need.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Symbol(pub Rc<str>);

impl Symbol {
    pub fn new(s: &str) -> Self {
        Symbol(Rc::from(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A positioned view over shared storage. Red's `series!` semantics: multiple
/// `Series` values can alias the same `Rc<RefCell<Vec<Value>>>` at different
/// cursors; mutation via `append`/`insert`/`poke` is visible to all aliases.
#[derive(Clone, Debug)]
pub struct Series {
    pub data: Rc<RefCell<Vec<Value>>>,
    pub index: usize,
}

impl Series {
    pub fn new(values: Vec<Value>) -> Self {
        Self {
            data: Rc::new(RefCell::new(values)),
            index: 0,
        }
    }

    /// Convenience: an empty series at index 0.
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }
}

impl Default for Series {
    fn default() -> Self {
        Self::empty()
    }
}

/// How a word is attached to a context. Milestone 2 stubs `Local`/`Func` as
/// unit variants; Milestone 5 fills in `Local(Rc<Context>, usize)` and
/// Milestone 9 fills in `Func(Rc<FuncDef>, usize)`.
#[derive(Clone, Debug, Default)]
pub enum Binding {
    #[default]
    Unbound,
    Local,
    Func,
}

/// Placeholder native signature. Replaced in Milestone 5 with the real
/// `fn(&[Value], &mut Env) -> Result<Value, EvalError>` once `Env`/`EvalError`
/// exist in `red-eval`. Kept here so `FuncDef` has its full field set up front.
pub type NativeFn = fn(&[Value]) -> Result<Value, ()>;

/// Function definition shared by `Value::Func`. Fields stubbed for Milestone 2:
/// `params` empty, `body`/`ctx` default-constructed, `native` `None`. Real
/// population happens in Milestones 6 (natives) and 9 (`func`/`does`).
#[derive(Clone, Debug, Default)]
pub struct FuncDef {
    pub params: Vec<Symbol>,
    pub body: Series,
    pub ctx: Context,
    pub native: Option<NativeFn>,
}

/// The single runtime value type. Covers every variant from the brief, even
/// ones not exercised until later milestones (`Path`, `String8`, `Func`).
#[derive(Clone, Debug)]
pub enum Value {
    None,
    Logic(bool),
    Integer(i64),
    Float(f64),
    String(Rc<str>),
    /// `foo`
    Word {
        sym: Symbol,
        binding: Binding,
    },
    /// `foo:`
    SetWord {
        sym: Symbol,
        binding: Binding,
    },
    /// `:foo`
    GetWord {
        sym: Symbol,
        binding: Binding,
    },
    /// `'foo`
    LitWord(Symbol),
    /// `[...]` — code is data; only walked when a native like `do` enters it.
    /// `span` is the byte range of the `[ ... ]` delimiters in the source.
    Block {
        series: Series,
        span: Span,
    },
    /// `(...)` — evaluated eagerly in place by the surrounding eval loop.
    /// `span` is the byte range of the `( ... )` delimiters in the source.
    Paren {
        series: Series,
        span: Span,
    },
    /// A function value (native or user-defined via `func`/`does`).
    Func(Rc<FuncDef>),
    /// `foo/bar` — simple select-on-block in POC.
    Path(Vec<Value>),
    /// `binary!` (optional in brief; included for completeness).
    String8(Vec<u8>),
}

impl Value {
    /// Span of this value in the original source, if attached. `Block`/`Paren`
    /// carry their delimiter span; literals and words return `None` for now
    /// (their spans get wired on during a later milestone's error-polish pass).
    pub fn span(&self) -> Option<Span> {
        match self {
            Value::Block { span, .. } | Value::Paren { span, .. } => Some(*span),
            _ => None,
        }
    }

    /// Constructor shorthand for an unbound word.
    pub fn word(s: &str) -> Self {
        Value::Word {
            sym: Symbol::new(s),
            binding: Binding::Unbound,
        }
    }

    /// Constructor shorthand for an unbound set-word.
    pub fn set_word(s: &str) -> Self {
        Value::SetWord {
            sym: Symbol::new(s),
            binding: Binding::Unbound,
        }
    }

    /// Constructor shorthand for an unbound get-word.
    pub fn get_word(s: &str) -> Self {
        Value::GetWord {
            sym: Symbol::new(s),
            binding: Binding::Unbound,
        }
    }

    /// Constructor shorthand for a lit-word.
    pub fn lit_word(s: &str) -> Self {
        Value::LitWord(Symbol::new(s))
    }

    /// Constructor shorthand for a block with a zero span (test/REPL use).
    pub fn block(series: Series) -> Self {
        Value::Block {
            series,
            span: Span::new(0, 0),
        }
    }

    /// Constructor shorthand for a paren with a zero span (test/REPL use).
    pub fn paren(series: Series) -> Self {
        Value::Paren {
            series,
            span: Span::new(0, 0),
        }
    }
}
