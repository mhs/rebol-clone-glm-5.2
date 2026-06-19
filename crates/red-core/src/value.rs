//! Core value model: `Value`, `Symbol`, `Span`, `Series`, `Binding`, `FuncDef`.
//!
//! Milestone 2 scope: types exist with stubbed binding/function fields so the
//! printer can be built and tested. Real binding/function machinery lands in
//! Milestones 5 and 9.

use std::cell::RefCell;
use std::rc::Rc;

use crate::context::Context;
use crate::env::NativeFn;

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

    /// True for the placeholder span used by synthetic/test values that have
    /// no real source position. Error rendering skips `line:col:` for these.
    pub fn is_default(self) -> bool {
        self.start == 0 && self.end == 0
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

/// How a word is attached to a context.
/// - `Unbound`: no binding attached yet; resolved at eval time via the
///   user context or native registry.
/// - `Local(Rc<Context>, usize)`: bound to a slot in the given context.
///   Attached by `bind_pass` (M5) for script-level words and for function-body
///   references to outer (user-context) words (M9, supports recursion).
/// - `Func(usize)`: bound to a function-local slot. Resolved at eval time via
///   `env.call_stack.last().ctx` — the current call frame's per-call context
///   clone. Attached by `bind_function_body` (M9) for params and body-local
///   set-words. The index refers to a slot in the active call frame's `ctx`.
#[derive(Clone, Debug, Default)]
pub enum Binding {
    #[default]
    Unbound,
    Local(Rc<Context>, usize),
    Func(usize),
}

/// Function definition shared by `Value::Func`. Fields stubbed for Milestone 2:
/// `params` empty, `body`/`ctx` default-constructed, `native` `None`. Real
/// population happens in Milestones 6 (natives) and 9 (`func`/`does`).
///
/// `variadic` marks natives (like `print`/`prin`/`probe`) that collect all
/// remaining args in the enclosing block up to the next native word. Fixed
/// natives ignore it and use `params.len()` for arity.
///
/// `infix` marks natives that participate in left-to-right infix chaining
/// (Milestone 7): an infix native consumes the already-evaluated left value
/// as its first operand plus one trailing prefix value as its second. Used
/// for `+`/`-`/`*`/`/`/`=`/`<>`/`<`/`>`/`<=`/`>=`/`and`/`or`.
#[derive(Clone, Debug, Default)]
pub struct FuncDef {
    pub params: Vec<Symbol>,
    /// Refinements declared in the spec block, in declaration order. Each
    /// entry is `(refinement_name, arg_word_names)`. For user functions the
    /// arg words are the words following `/ref` in the spec (e.g.
    /// `func [x /with y]` → `("with", ["y"])`); for natives the names are
    /// synthetic placeholders whose count gives the refinement's arity.
    /// `dispatch_call` walks params then this list in order, collecting
    /// caller-supplied refinements into a `RefineArgs`.
    pub refinements: Vec<(Symbol, Vec<Symbol>)>,
    pub body: Series,
    pub ctx: Context,
    pub native: Option<NativeFn>,
    pub variadic: bool,
    pub infix: bool,
}

/// The single runtime value type. Covers every variant from the brief, even
/// ones not exercised until later milestones (`Path`, `String8`, `Func`).
///
/// Every variant that originates from source (`Integer`, `Float`, `String`,
/// the word family, `Block`, `Paren`) carries the byte-offset `Span` of its
/// originating token so eval-time errors can render `file:line:col:`. The
/// synthetic variants (`None`, `Logic`, `Func`, `Path`, `String8`) are
/// produced at runtime by natives and have no source position; their
/// `span()` returns `None`.
#[derive(Clone, Debug)]
pub enum Value {
    /// Runtime-only sentinel (the result of evaluating the word `none`, or of
    /// a native like `if false [...]`). The source token `none` is parsed as
    /// a `Word`; eval resolves it to this variant.
    None,
    /// Runtime-only boolean (result of `true`/`false` words, comparison
    /// natives, etc.).
    Logic(bool),
    /// `42`, `-7` — integer literal.
    Integer { n: i64, span: Span },
    /// `3.14`, `1e3` — float literal.
    Float { f: f64, span: Span },
    /// `"..."` / `{...}` string literal.
    String { s: Rc<str>, span: Span },
    /// `foo`
    Word {
        sym: Symbol,
        binding: Binding,
        span: Span,
    },
    /// `foo:`
    SetWord {
        sym: Symbol,
        binding: Binding,
        span: Span,
    },
    /// `:foo`
    GetWord {
        sym: Symbol,
        binding: Binding,
        span: Span,
    },
    /// `'foo`
    LitWord { sym: Symbol, span: Span },
    /// `[...]` — code is data; only walked when a native like `do` enters it.
    /// `span` is the byte range of the `[ ... ]` delimiters in the source.
    Block { series: Series, span: Span },
    /// `(...)` — evaluated eagerly in place by the surrounding eval loop.
    /// `span` is the byte range of the `( ... )` delimiters in the source.
    Paren { series: Series, span: Span },
    /// A function value (native or user-defined via `func`/`does`). Synthetic —
    /// produced by `func`/`does`/`make function!` and by native lookup; has
    /// no source span of its own.
    Func(Rc<FuncDef>),
    /// `foo/bar` — a path. Source-origin (the parser assembles these from a
    /// word immediately followed by one or more refinement tokens); carries
    /// the span of the whole `foo/bar` run. Used both for data selection
    /// (`block/2`, `obj/field` — M19) and for refined function calls
    /// (`copy/part`, `find/case` — M13). Path parts are stored as `Word`
    /// values so molding yields `foo/bar`.
    Path { parts: Vec<Value>, span: Span },
    /// `/foo` — a refinement word. Source-origin. Appears standalone in
    /// function spec blocks (`func [x /only y]`) and as a refinement-flag
    /// token in call sites (`copy /part x` — the spaced form). When adjacent
    /// to a preceding word (`copy/part`) the parser folds it into a `Path`.
    Refinement { sym: Symbol, span: Span },
    /// `binary!` (optional in brief; included for completeness). Synthetic.
    String8(Vec<u8>),
}

impl Value {
    /// Span of this value in the original source. Every source-origin variant
    /// (`Integer`/`Float`/`String`/word-family/`Block`/`Paren`/`Path`/
    /// `Refinement`) carries its token span; synthetic variants
    /// (`None`/`Logic`/`Func`/`String8`) return `None`.
    pub fn span(&self) -> Option<Span> {
        match self {
            Value::Integer { span, .. }
            | Value::Float { span, .. }
            | Value::String { span, .. }
            | Value::Word { span, .. }
            | Value::SetWord { span, .. }
            | Value::GetWord { span, .. }
            | Value::LitWord { span, .. }
            | Value::Block { span, .. }
            | Value::Paren { span, .. }
            | Value::Path { span, .. }
            | Value::Refinement { span, .. } => Some(*span),
            Value::None | Value::Logic(_) | Value::Func(_) | Value::String8(_) => None,
        }
    }

    /// Like `span()` but returns the zero span when the value is synthetic
    /// (has no source position). Used by natives that need *some* span for an
    /// error and fall back to the offending argument's position when
    /// available, else to `Span::new(0,0)`.
    pub fn span_or_default(self: &Value) -> Span {
        self.span().unwrap_or_default()
    }

    /// Constructor shorthand for an unbound word (zero span — test/REPL use).
    pub fn word(s: &str) -> Self {
        Value::Word {
            sym: Symbol::new(s),
            binding: Binding::Unbound,
            span: Span::new(0, 0),
        }
    }

    /// Constructor shorthand for an unbound set-word (zero span).
    pub fn set_word(s: &str) -> Self {
        Value::SetWord {
            sym: Symbol::new(s),
            binding: Binding::Unbound,
            span: Span::new(0, 0),
        }
    }

    /// Constructor shorthand for an unbound get-word (zero span).
    pub fn get_word(s: &str) -> Self {
        Value::GetWord {
            sym: Symbol::new(s),
            binding: Binding::Unbound,
            span: Span::new(0, 0),
        }
    }

    /// Constructor shorthand for a lit-word (zero span).
    pub fn lit_word(s: &str) -> Self {
        Value::LitWord {
            sym: Symbol::new(s),
            span: Span::new(0, 0),
        }
    }

    /// Constructor shorthand for an integer literal (zero span).
    pub fn integer(n: i64) -> Self {
        Value::Integer {
            n,
            span: Span::new(0, 0),
        }
    }

    /// Constructor shorthand for a float literal (zero span).
    pub fn float(f: f64) -> Self {
        Value::Float {
            f,
            span: Span::new(0, 0),
        }
    }

    /// Constructor shorthand for a string literal (zero span).
    pub fn string(s: impl Into<Rc<str>>) -> Self {
        Value::String {
            s: s.into(),
            span: Span::new(0, 0),
        }
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

    /// Constructor shorthand for a path with a zero span (test/REPL use).
    /// Parts are typically `Value::Word` values.
    pub fn path(parts: Vec<Value>) -> Self {
        Value::Path {
            parts,
            span: Span::new(0, 0),
        }
    }

    /// Constructor shorthand for a refinement word with a zero span.
    pub fn refinement(s: &str) -> Self {
        Value::Refinement {
            sym: Symbol::new(s),
            span: Span::new(0, 0),
        }
    }
}

impl Default for Span {
    fn default() -> Self {
        Span::new(0, 0)
    }
}
