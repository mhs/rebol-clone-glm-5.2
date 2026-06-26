//! Core value model: `Value`, `Symbol`, `Span`, `Series`, `Binding`, `FuncDef`.
//!
//! Milestone 2 scope: types exist with stubbed binding/function fields so the
//! printer can be built and tested. Real binding/function machinery lands in
//! Milestones 5 and 9.

use std::cell::RefCell;
use std::rc::Rc;

use crate::context::Context;
use crate::env::NativeFn;
use crate::vm_ir::CompiledBlock;

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
/// - `Lexical(usize, usize)`: statically-resolved `(depth, slot)` pair for the
///   v0.3 bytecode VM (M22+). `depth` is the lexical distance from the current
///   frame to the defining frame; `slot` is the slot index in that frame's
///   `locals`. Attached by the compile-time scope analyzer (M23); the
///   tree-walker never produces or reads this variant.
#[derive(Clone, Debug, Default)]
pub enum Binding {
    #[default]
    Unbound,
    Local(Rc<Context>, usize),
    Func(usize),
    Lexical(usize, usize),
}

impl Binding {
    /// True iff this is a `Lexical(_, _)` binding (set by the v0.3 compiler).
    pub fn is_lexical(&self) -> bool {
        matches!(self, Binding::Lexical(_, _))
    }

    /// If this is a `Lexical(depth, slot)`, return `Some((depth, slot))`;
    /// otherwise `None`.
    pub fn as_lexical(&self) -> Option<(usize, usize)> {
        if let Binding::Lexical(d, s) = self {
            Some((*d, *s))
        } else {
            None
        }
    }
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
    /// Explicit function-local words declared via `<local>` in a `function`
    /// spec (M16). Empty for `func`/`does`. These get slots after params +
    /// refinements but before body-local SetWords, so they're usable even if
    /// the body never assigns them (they default to `none`).
    pub locals: Vec<Symbol>,
    /// Lexical free-variable capture list (v0.3, M23). Words referenced inside
    /// this function's body that resolve to an ancestor scope are listed here
    /// so the VM can capture them at `MakeFunc` time (shallow capture; full
    /// closures remain deferred). Populated by the compile-time scope analyzer;
    /// empty for natives and for funcs created before M23 runs.
    pub freevars: Vec<Symbol>,
    /// Lazily-filled compiled-form cache (v0.3, M22+). `None` until the
    /// bytecode compiler runs (M24), then `Some(Rc<CompiledBlock>)`. Retained
    /// as `Rc` so pointer identity (`Rc::ptr_eq`) can drive cache invalidation
    /// in M27 when `bind` mutates the body. Absent from the data model — a
    /// `Block` passed as data is never compiled.
    pub compiled: Option<Rc<CompiledBlock>>,
    pub body: Series,
    pub ctx: Context,
    pub native: Option<NativeFn>,
    pub variadic: bool,
    pub infix: bool,
}

impl FuncDef {
    /// Clear the construction-time `compiled` hint. Defensive — called by
    /// `bind_function_body` after it mutates the body's word bindings, and
    /// by any rebind path that touches a `Value::Func`'s body. The
    /// *authoritative* VM cache lives on `Env::func_cache` (M27); callers
    /// with `&mut Env` should also call `Env::invalidate_func_cache` to clear
    /// that entry. This field stays `None` for funcs created in `Walk` mode
    /// (the slot for recursive `CallUser` emission isn't known until
    /// runtime), so clearing it is a no-op in the common case — the method
    /// exists for correctness against future eager-compile paths.
    pub fn invalidate_compiled(&mut self) {
        self.compiled = None;
    }
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
    /// values so molding yields `foo/bar`. Parens (`foo/(a+b)/bar`) may also
    /// appear as parts; the evaluator evaluates them in place when walking.
    Path { parts: Vec<Value>, span: Span },
    /// `:foo/bar` — a get-path. Source-origin. Like `GetWord`, resolves its
    /// head and walks the field chain, returning the value at the path
    /// *without* invoking it (so `:obj/method` yields the function value,
    /// not the result of calling it).
    GetPath { parts: Vec<Value>, span: Span },
    /// `'foo/bar` — a lit-path. Source-origin. Returns the path itself as
    /// data (mirrors `LitWord`).
    LitPath { parts: Vec<Value>, span: Span },
    /// `obj/field:` — a set-path. Source-origin. Evaluates the following
    /// expression and writes it into the final field/slot identified by
    /// walking the path. Mirrors `SetWord`.
    SetPath { parts: Vec<Value>, span: Span },
    /// `/foo` — a refinement word. Source-origin. Appears standalone in
    /// function spec blocks (`func [x /only y]`) and as a refinement-flag
    /// token in call sites (`copy /part x` — the spaced form). When adjacent
    /// to a preceding word (`copy/part`) the parser folds it into a `Path`.
    Refinement { sym: Symbol, span: Span },
    /// `%foo/bar.txt` — a file! literal. Source-origin (the lexer scans the
    /// `%`-prefixed run); carries the byte-offset span of the whole token.
    /// `path` is the raw path text without the leading `%` (and without any
    /// mold-time quoting).
    File { path: Rc<str>, span: Span },
    /// `http://example.com/x` — a url! literal. Source-origin (the lexer
    /// detects `scheme://...` inside a word run); carries the byte-offset span
    /// of the whole token. `url` is the raw url text including the scheme.
    Url { url: Rc<str>, span: Span },
    /// `binary!` (optional in brief; included for completeness). Synthetic.
    String8(Vec<u8>),
    /// A caught error value (M16). Produced by `try` when an error is raised
    /// inside its block; carries the error message body. Synthetic — no source
    /// span of its own (the originating error's span is not preserved across
    /// the catch boundary in the POC).
    Error(Rc<ErrorValue>),
    /// An object (M18): a word→value context with optional prototype parent.
    /// Synthetic — produced by `make object!`/`object`/`context`; carries no
    /// source span of its own.
    Object(Rc<RefCell<ObjectDef>>),
}

/// Payload of a `Value::Error`. POC keeps just the message body; a fuller
/// error model (code/type/args) is deferred to v0.3 per `plan2.md`.
#[derive(Clone, Debug)]
pub struct ErrorValue {
    pub message: String,
}

/// An object: an ordered word→value context plus an optional prototype
/// (parent) object. The context (`Rc<Context>`) is the same shape as the
/// user context, so it can be temporarily installed as `env.user_ctx` during
/// spec evaluation and method calls (mirroring the `use` native). Words
/// inside method bodies bind to `Binding::Local(obj_ctx, idx)` via the
/// standard binding pass — no special binding variant is needed (M18).
///
/// Inheritance is **copy-based**: `make object! parent [spec]` pre-seeds the
/// child context with copies of the parent's words+values, then evaluates the
/// spec block (which can override). The `parent` pointer is retained for
/// reference/identity purposes.
#[derive(Clone, Debug)]
pub struct ObjectDef {
    pub ctx: Rc<Context>,
    pub parent: Option<Rc<RefCell<ObjectDef>>>,
    pub self_word: Symbol,
}

impl Default for ObjectDef {
    fn default() -> Self {
        Self::new()
    }
}

impl ObjectDef {
    pub fn new() -> Self {
        Self {
            ctx: Rc::new(Context::new()),
            parent: None,
            self_word: Symbol::new("self"),
        }
    }
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
            | Value::GetPath { span, .. }
            | Value::LitPath { span, .. }
            | Value::SetPath { span, .. }
            | Value::Refinement { span, .. }
            | Value::File { span, .. }
            | Value::Url { span, .. } => Some(*span),
            Value::None
            | Value::Logic(_)
            | Value::Func(_)
            | Value::String8(_)
            | Value::Error(_)
            | Value::Object(_) => None,
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

    /// Constructor shorthand for a get-path with a zero span (test/REPL use).
    pub fn get_path(parts: Vec<Value>) -> Self {
        Value::GetPath {
            parts,
            span: Span::new(0, 0),
        }
    }

    /// Constructor shorthand for a lit-path with a zero span (test/REPL use).
    pub fn lit_path(parts: Vec<Value>) -> Self {
        Value::LitPath {
            parts,
            span: Span::new(0, 0),
        }
    }

    /// Constructor shorthand for a set-path with a zero span (test/REPL use).
    pub fn set_path(parts: Vec<Value>) -> Self {
        Value::SetPath {
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

    /// Constructor shorthand for a file! literal with a zero span (test/REPL use).
    pub fn file(s: impl Into<Rc<str>>) -> Self {
        Value::File {
            path: s.into(),
            span: Span::new(0, 0),
        }
    }

    /// Constructor shorthand for a url! literal with a zero span (test/REPL use).
    pub fn url(s: impl Into<Rc<str>>) -> Self {
        Value::Url {
            url: s.into(),
            span: Span::new(0, 0),
        }
    }

    /// Constructor shorthand for an error value carrying `message` (zero span,
    /// synthetic).
    pub fn error(message: impl Into<String>) -> Self {
        Value::Error(Rc::new(ErrorValue {
            message: message.into(),
        }))
    }

    /// Constructor shorthand for an object wrapping `obj_def`.
    pub fn object(obj_def: ObjectDef) -> Self {
        Value::Object(Rc::new(RefCell::new(obj_def)))
    }
}

impl Default for Span {
    fn default() -> Self {
        Span::new(0, 0)
    }
}
