//! Core value model: `Value`, `Symbol`, `Span`, `Series`, `Binding`, `FuncDef`.
//!
//! Milestone 2 scope: types exist with stubbed binding/function fields so the
//! printer can be built and tested. Real binding/function machinery lands in
//! Milestones 5 and 9.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;

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
/// ones not exercised until later milestones (`Path`, `Func`).
///
/// Every variant that originates from source (`Integer`, `Float`, `String`,
/// the word family, `Block`, `Paren`, `String8`) carries the byte-offset
/// `Span` of its originating token so eval-time errors can render
/// `file:line:col:`. The synthetic variants (`None`, `Logic`, `Func`) are
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
    /// `#"a"` — a char! literal. Source-origin (the lexer scans the `#"-led
    /// form); carries the byte-offset span of the whole token.
    Char { c: char, span: Span },
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
    /// `binary!` literal (`#{hex}` form). Source-origin (the lexer scans the
    /// `#{...}` run); carries the byte-offset span of the whole token.
    String8 { bytes: Vec<u8>, span: Span },
    /// A caught error value (M16). Produced by `try` when an error is raised
    /// inside its block; carries the error message body. Synthetic — no source
    /// span of its own (the originating error's span is not preserved across
    /// the catch boundary in the POC).
    Error(Rc<ErrorValue>),
    /// An object (M18): a word→value context with optional prototype parent.
    /// Synthetic — produced by `make object!`/`object`/`context`; carries no
    /// source span of its own.
    Object(Rc<RefCell<ObjectDef>>),
    /// A map (M43): an insertion-ordered heterogeneous key→value table. Keys
    /// are the hashable subset of `Value` (`MapKey`); values are arbitrary.
    /// Synthetic — produced by `make map!`/`to-map`; carries no source span.
    Map(Rc<RefCell<MapDef>>),
}

/// Payload of a `Value::Error`. M42 extends the prior message-only stub to
/// the full Red field set: `code`/`type`/`args`/`near`/`where`/`by`. The
/// `message` field is kept (derived from the template when `code` is set,
/// or the user-supplied string otherwise) so `form` of an error stays just
/// the message text.
///
/// `PartialEq` is intentionally NOT derived: `args`/`near` carry `Value`s,
/// which have no `PartialEq` impl (equality lives in `red-eval::compare`).
/// Structural equality for `Value::Error` is hand-rolled in `compare.rs`.
#[derive(Clone, Debug)]
pub struct ErrorValue {
    pub message: String,
    /// Numeric error code; `None` for user-thrown errors with no code.
    pub code: Option<i64>,
    /// Category word: `'math`/`'syntax`/`'script`/`'user`/`'access`/
    /// `'reference`/`'io`. `None` for message-only errors.
    pub kind: Option<Symbol>,
    /// Values referenced by the message template (e.g. the offending
    /// operands). May be empty.
    pub args: Vec<Value>,
    /// Block/expression nearest the error — typically the call site. Stored
    /// as a `Value` (usually a `Block` or `None` when not captured).
    pub near: Option<Value>,
    /// Function/frame name where the error was raised. `None` when not
    /// captured.
    pub cause: Option<Symbol>,
    /// Actor — the calling function name. `None` when not captured.
    pub by: Option<Symbol>,
}

impl ErrorValue {
    /// Build a message-only error value (all structured fields `None`/empty).
    /// Back-compat with the M16 `Value::error(msg)` shape.
    pub fn new_message(message: impl Into<String>) -> Self {
        ErrorValue {
            message: message.into(),
            code: None,
            kind: None,
            args: Vec::new(),
            near: None,
            cause: None,
            by: None,
        }
    }

    /// Build a structured error value with all fields. `message` is the
    /// user-visible body; the other fields populate the structured slots.
    #[allow(clippy::too_many_arguments)]
    pub fn new_structed(
        message: impl Into<String>,
        code: Option<i64>,
        kind: Option<Symbol>,
        args: Vec<Value>,
        near: Option<Value>,
        cause: Option<Symbol>,
        by: Option<Symbol>,
    ) -> Self {
        ErrorValue {
            message: message.into(),
            code,
            kind,
            args,
            near,
            cause,
            by,
        }
    }

    /// True if every structured field is `None`/empty — i.e. this is a
    /// message-only error that molds as `make error! "msg"`.
    pub fn is_message_only(&self) -> bool {
        self.code.is_none()
            && self.kind.is_none()
            && self.args.is_empty()
            && self.near.is_none()
            && self.cause.is_none()
            && self.by.is_none()
    }
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

/// Hashable key for a `MapDef`. The subset of `Value` that is hashable and
/// non-container: word-family (as `Sym`), integers, strings, chars, logic,
/// and `none`. Container values (blocks, parens, objects, maps, funcs,
/// paths, errors, files, urls, refinements) are not hashable.
///
/// `PartialEq`/`Eq`/`Hash` are derived; every variant is hashable. `Sym`
/// compares by `Symbol` (interned `Rc<str>`), so two equal symbols hash
/// equal even if interned separately.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MapKey {
    Sym(Symbol),
    Int(i64),
    Str(Rc<str>),
    Char(char),
    Bool(bool),
    None,
}

impl MapKey {
    /// Convert a `Value` to a `MapKey`. Returns `None` for unhashable types
    /// (blocks, objects, funcs, paths, files, urls, refinements, errors,
    /// other maps).
    pub fn from_value(v: &Value) -> Option<Self> {
        Some(match v {
            Value::None => MapKey::None,
            Value::Logic(b) => MapKey::Bool(*b),
            Value::Integer { n, .. } => MapKey::Int(*n),
            Value::Char { c, .. } => MapKey::Char(*c),
            Value::String { s, .. } => MapKey::Str(s.clone()),
            Value::Word { sym, .. }
            | Value::SetWord { sym, .. }
            | Value::GetWord { sym, .. }
            | Value::LitWord { sym, .. } => MapKey::Sym(sym.clone()),
            // Unhashable: container values, paths, funcs, errors, objects,
            // maps, files, urls, refinements. (Refinements are word-shaped
            // but path-dispatched; excluding them keeps map keys
            // unambiguous.)
            _ => return None,
        })
    }

    /// Reconstruct the `Value` form of this key. Sym keys return a bare
    /// `Word` (unbound) — the natural source form for a map key.
    pub fn to_value(&self) -> Value {
        match self {
            MapKey::None => Value::None,
            MapKey::Bool(b) => Value::Logic(*b),
            MapKey::Int(n) => Value::integer(*n),
            MapKey::Char(c) => Value::char(*c),
            MapKey::Str(s) => Value::string(s.clone()),
            MapKey::Sym(sym) => Value::Word {
                sym: sym.clone(),
                binding: Binding::Unbound,
                span: Span::default(),
            },
        }
    }
}

/// A map! (M43): an insertion-ordered heterogeneous key→value table backed
/// by `indexmap::IndexMap<MapKey, Value>`. Keys are the hashable subset of
/// `Value` (see `MapKey`); values are arbitrary `Value`s. Insertion order
/// is preserved (matching Red's `map!` semantics and mirroring `Context`'s
/// ordered-word behavior) so `keys-of`/`values-of`/iteration are stable.
///
/// Mutation is interior (`RefCell`), so a `Map` value is shared by aliases
/// the same way `Object`/`Series` are: `m: make map! [a 1] n: m m/b: 2 n/b`
/// reads `2` from both. The `IndexMap` is wrapped in `RefCell` because the
/// enum stores `Rc<RefCell<MapDef>>` (shared ownership + mutation).
#[derive(Clone, Debug, Default)]
pub struct MapDef {
    pub entries: RefCell<IndexMap<MapKey, Value>>,
}

impl MapDef {
    pub fn new() -> Self {
        Self {
            entries: RefCell::new(IndexMap::new()),
        }
    }

    pub fn get(&self, key: &MapKey) -> Option<Value> {
        self.entries.borrow().get(key).cloned()
    }

    /// Insert `val` at `key`, replacing any existing entry. Returns the
    /// previous value (if any). Preserves insertion order on first insert;
    /// updates in place on replace.
    pub fn set(&self, key: MapKey, val: Value) -> Option<Value> {
        self.entries.borrow_mut().insert(key, val)
    }

    /// Remove the entry at `key`. Returns the removed value (if any).
    /// Subsequent keys keep their relative order.
    pub fn remove(&self, key: &MapKey) -> Option<Value> {
        self.entries.borrow_mut().shift_remove(key)
    }

    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.borrow().is_empty()
    }

    pub fn clear(&self) {
        self.entries.borrow_mut().clear();
    }

    /// Keys in insertion order, as `Value`s.
    pub fn keys(&self) -> Vec<Value> {
        self.entries.borrow().keys().map(MapKey::to_value).collect()
    }

    /// Values in insertion order.
    pub fn values(&self) -> Vec<Value> {
        self.entries.borrow().values().cloned().collect()
    }
}

impl Value {
    /// Span of this value in the original source. Every source-origin variant
    /// (`Integer`/`Float`/`String`/word-family/`Block`/`Paren`/`Path`/
    /// `Refinement`/`String8`) carries its token span; synthetic variants
    /// (`None`/`Logic`/`Func`) return `None`.
    pub fn span(&self) -> Option<Span> {
        match self {
            Value::Integer { span, .. }
            | Value::Float { span, .. }
            | Value::String { span, .. }
            | Value::Char { span, .. }
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
            | Value::Url { span, .. }
            | Value::String8 { span, .. } => Some(*span),
            Value::None
            | Value::Logic(_)
            | Value::Func(_)
            | Value::Error(_)
            | Value::Object(_)
            | Value::Map(_) => None,
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
            span: Span::default(),
        }
    }

    /// Constructor shorthand for an unbound set-word (zero span).
    pub fn set_word(s: &str) -> Self {
        Value::SetWord {
            sym: Symbol::new(s),
            binding: Binding::Unbound,
            span: Span::default(),
        }
    }

    /// Constructor shorthand for an unbound get-word (zero span).
    pub fn get_word(s: &str) -> Self {
        Value::GetWord {
            sym: Symbol::new(s),
            binding: Binding::Unbound,
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a lit-word (zero span).
    pub fn lit_word(s: &str) -> Self {
        Value::LitWord {
            sym: Symbol::new(s),
            span: Span::default(),
        }
    }

    /// Constructor shorthand for an integer literal (zero span).
    pub fn integer(n: i64) -> Self {
        Value::Integer {
            n,
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a float literal (zero span).
    pub fn float(f: f64) -> Self {
        Value::Float {
            f,
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a string literal (zero span).
    pub fn string(s: impl Into<Rc<str>>) -> Self {
        Value::String {
            s: s.into(),
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a char literal (zero span).
    pub fn char(c: char) -> Self {
        Value::Char {
            c,
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a block with a zero span (test/REPL use).
    pub fn block(series: Series) -> Self {
        Value::Block {
            series,
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a paren with a zero span (test/REPL use).
    pub fn paren(series: Series) -> Self {
        Value::Paren {
            series,
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a path with a zero span (test/REPL use).
    /// Parts are typically `Value::Word` values.
    pub fn path(parts: Vec<Value>) -> Self {
        Value::Path {
            parts,
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a get-path with a zero span (test/REPL use).
    pub fn get_path(parts: Vec<Value>) -> Self {
        Value::GetPath {
            parts,
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a lit-path with a zero span (test/REPL use).
    pub fn lit_path(parts: Vec<Value>) -> Self {
        Value::LitPath {
            parts,
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a set-path with a zero span (test/REPL use).
    pub fn set_path(parts: Vec<Value>) -> Self {
        Value::SetPath {
            parts,
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a refinement word with a zero span.
    pub fn refinement(s: &str) -> Self {
        Value::Refinement {
            sym: Symbol::new(s),
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a file! literal with a zero span (test/REPL use).
    pub fn file(s: impl Into<Rc<str>>) -> Self {
        Value::File {
            path: s.into(),
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a url! literal with a zero span (test/REPL use).
    pub fn url(s: impl Into<Rc<str>>) -> Self {
        Value::Url {
            url: s.into(),
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a message-only error value (zero span,
    /// synthetic). Back-compat with the M16 shape; M42 adds the structured
    /// fields via [`Value::error_structed`].
    pub fn error(message: impl Into<String>) -> Self {
        Value::Error(Rc::new(ErrorValue::new_message(message)))
    }

    /// Constructor shorthand for a structured error value. Fills the
    /// M42 field set (`code`/`type`/`args`/`near`/`where`/`by`).
    #[allow(clippy::too_many_arguments)]
    pub fn error_structed(
        message: impl Into<String>,
        code: Option<i64>,
        kind: Option<Symbol>,
        args: Vec<Value>,
        near: Option<Value>,
        cause: Option<Symbol>,
        by: Option<Symbol>,
    ) -> Self {
        Value::Error(Rc::new(ErrorValue::new_structed(
            message, code, kind, args, near, cause, by,
        )))
    }

    /// Constructor shorthand for a binary! literal with a zero span
    /// (test/REPL use).
    pub fn binary(bytes: Vec<u8>) -> Self {
        Value::String8 {
            bytes,
            span: Span::default(),
        }
    }

    /// Back-compat alias for `Value::binary`.
    pub fn string8(bytes: Vec<u8>) -> Self {
        Self::binary(bytes)
    }

    /// Constructor shorthand for an object wrapping `obj_def`.
    pub fn object(obj_def: ObjectDef) -> Self {
        Value::Object(Rc::new(RefCell::new(obj_def)))
    }

    /// Constructor shorthand for a map wrapping `map_def`.
    pub fn map(map_def: MapDef) -> Self {
        Value::Map(Rc::new(RefCell::new(map_def)))
    }
}

impl Default for Span {
    fn default() -> Self {
        Span::new(0, 0)
    }
}

#[cfg(test)]
mod tests {
    //! Unit coverage for `Value`/`Span`/`Binding`/`FuncDef` core types. M34.
    //!
    //! Pins the constructor shorthand invariants (zero span, unbound binding),
    //! the `span()` per-variant contract, and `FuncDef::invalidate_compiled`.

    use super::*;
    use crate::vm_ir::CompiledBlock;

    #[test]
    fn span_new_stores_offsets() {
        let s = Span::new(5, 11);
        assert_eq!(s.start, 5);
        assert_eq!(s.end, 11);
    }

    #[test]
    fn span_default_is_zero_zero() {
        assert_eq!(Span::default().start, 0);
        assert_eq!(Span::default().end, 0);
        assert!(Span::default().is_default());
    }

    #[test]
    fn span_is_default_true_only_for_zero_zero() {
        assert!(Span::new(0, 0).is_default());
        assert!(!Span::new(0, 1).is_default());
        assert!(!Span::new(1, 0).is_default());
        assert!(!Span::new(3, 7).is_default());
    }

    #[test]
    fn binding_default_is_unbound() {
        let b: Binding = Binding::default();
        assert!(matches!(b, Binding::Unbound));
        assert!(!b.is_lexical());
        assert!(b.as_lexical().is_none());
    }

    #[test]
    fn binding_is_lexical_true_only_for_lexical() {
        let lex = Binding::Lexical(2, 4);
        assert!(lex.is_lexical());
        assert_eq!(lex.as_lexical(), Some((2, 4)));

        assert!(!Binding::Unbound.is_lexical());
        assert!(Binding::Unbound.as_lexical().is_none());

        let ctx = Rc::new(Context::new());
        let local = Binding::Local(ctx, 3);
        assert!(!local.is_lexical());
        assert!(local.as_lexical().is_none());

        let func = Binding::Func(1);
        assert!(!func.is_lexical());
        assert!(func.as_lexical().is_none());
    }

    #[test]
    fn binding_as_lexical_round_trips() {
        for (d, s) in [(0, 0), (1, 2), (5, 9), (usize::MAX, usize::MAX)] {
            let b = Binding::Lexical(d, s);
            assert_eq!(b.as_lexical(), Some((d, s)));
        }
    }

    #[test]
    fn funcdef_invalidate_compiled_clears_compiled_field() {
        // The `compiled` field starts `None` for a default `FuncDef`, gets
        // populated by the bytecode compiler, and is cleared by
        // `invalidate_compiled` (called by rebind paths that mutate the
        // body's word bindings). This test pins that contract.
        //
        // Note (M34 plan text correction): `needs_rebind` lives on
        // `CompiledBlock` (`vm_ir.rs:131`), not on `FuncDef`.
        // `invalidate_compiled` only clears `FuncDef::compiled`.
        let block: Rc<CompiledBlock> = Rc::new(empty_compiled_block());
        let mut fd = FuncDef {
            compiled: Some(block),
            ..FuncDef::default()
        };
        assert!(fd.compiled.is_some());
        fd.invalidate_compiled();
        assert!(fd.compiled.is_none());
    }

    #[test]
    fn funcdef_default_has_no_compiled() {
        let fd = FuncDef::default();
        assert!(fd.compiled.is_none());
        assert!(fd.params.is_empty());
        assert!(fd.refinements.is_empty());
        assert!(fd.locals.is_empty());
        assert!(fd.freevars.is_empty());
        assert!(!fd.variadic);
        assert!(!fd.infix);
        assert!(fd.native.is_none());
    }

    #[test]
    fn value_word_constructor() {
        match Value::word("foo") {
            Value::Word { sym, binding, span } => {
                assert_eq!(sym.as_str(), "foo");
                assert!(matches!(binding, Binding::Unbound));
                assert!(span.is_default());
            }
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn value_set_word_constructor() {
        match Value::set_word("bar") {
            Value::SetWord { sym, binding, span } => {
                assert_eq!(sym.as_str(), "bar");
                assert!(matches!(binding, Binding::Unbound));
                assert!(span.is_default());
            }
            other => panic!("expected SetWord, got {other:?}"),
        }
    }

    #[test]
    fn value_get_word_constructor() {
        match Value::get_word("baz") {
            Value::GetWord { sym, binding, span } => {
                assert_eq!(sym.as_str(), "baz");
                assert!(matches!(binding, Binding::Unbound));
                assert!(span.is_default());
            }
            other => panic!("expected GetWord, got {other:?}"),
        }
    }

    #[test]
    fn value_lit_word_constructor() {
        match Value::lit_word("qux") {
            Value::LitWord { sym, span } => {
                assert_eq!(sym.as_str(), "qux");
                assert!(span.is_default());
            }
            other => panic!("expected LitWord, got {other:?}"),
        }
    }

    #[test]
    fn value_integer_constructor() {
        match Value::integer(-7) {
            Value::Integer { n, span } => {
                assert_eq!(n, -7);
                assert!(span.is_default());
            }
            other => panic!("expected Integer, got {other:?}"),
        }
    }

    #[test]
    fn value_float_constructor() {
        match Value::float(2.5) {
            Value::Float { f, span } => {
                assert_eq!(f, 2.5);
                assert!(span.is_default());
            }
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn value_string_constructor() {
        match Value::string("hi") {
            Value::String { s, span } => {
                assert_eq!(&*s, "hi");
                assert!(span.is_default());
            }
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn value_block_constructor() {
        match Value::block(Series::empty()) {
            Value::Block { series, span } => {
                assert!(series.data.borrow().is_empty());
                assert_eq!(series.index, 0);
                assert!(span.is_default());
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn value_paren_constructor() {
        match Value::paren(Series::empty()) {
            Value::Paren { series, span } => {
                assert!(series.data.borrow().is_empty());
                assert!(span.is_default());
            }
            other => panic!("expected Paren, got {other:?}"),
        }
    }

    #[test]
    fn value_path_constructor() {
        let p = Value::path(vec![Value::word("a"), Value::word("b")]);
        match p {
            Value::Path { parts, span } => {
                assert_eq!(parts.len(), 2);
                assert!(span.is_default());
            }
            other => panic!("expected Path, got {other:?}"),
        }
    }

    #[test]
    fn value_get_path_constructor() {
        match Value::get_path(vec![Value::word("a")]) {
            Value::GetPath { parts, span } => {
                assert_eq!(parts.len(), 1);
                assert!(span.is_default());
            }
            other => panic!("expected GetPath, got {other:?}"),
        }
    }

    #[test]
    fn value_lit_path_constructor() {
        match Value::lit_path(vec![Value::word("a")]) {
            Value::LitPath { parts, span } => {
                assert_eq!(parts.len(), 1);
                assert!(span.is_default());
            }
            other => panic!("expected LitPath, got {other:?}"),
        }
    }

    #[test]
    fn value_set_path_constructor() {
        match Value::set_path(vec![Value::word("a")]) {
            Value::SetPath { parts, span } => {
                assert_eq!(parts.len(), 1);
                assert!(span.is_default());
            }
            other => panic!("expected SetPath, got {other:?}"),
        }
    }

    #[test]
    fn value_refinement_constructor() {
        match Value::refinement("only") {
            Value::Refinement { sym, span } => {
                assert_eq!(sym.as_str(), "only");
                assert!(span.is_default());
            }
            other => panic!("expected Refinement, got {other:?}"),
        }
    }

    #[test]
    fn value_file_constructor() {
        match Value::file("foo/bar.txt") {
            Value::File { path, span } => {
                assert_eq!(&*path, "foo/bar.txt");
                assert!(span.is_default());
            }
            other => panic!("expected File, got {other:?}"),
        }
    }

    #[test]
    fn value_url_constructor() {
        match Value::url("http://example.com") {
            Value::Url { url, span } => {
                assert_eq!(&*url, "http://example.com");
                assert!(span.is_default());
            }
            other => panic!("expected Url, got {other:?}"),
        }
    }

    #[test]
    fn value_error_constructor() {
        match Value::error("boom") {
            Value::Error(ev) => assert_eq!(ev.message, "boom"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn value_object_constructor() {
        let obj = ObjectDef::new();
        match Value::object(obj) {
            Value::Object(_) => {}
            other => panic!("expected Object, got {other:?}"),
        }
    }

    #[test]
    fn value_binary_constructor() {
        match Value::binary(vec![0xDE, 0xAD]) {
            Value::String8 { bytes, .. } => assert_eq!(bytes, vec![0xDE, 0xAD]),
            other => panic!("expected String8, got {other:?}"),
        }
        // alias
        match Value::string8(vec![0xBE, 0xEF]) {
            Value::String8 { bytes, .. } => assert_eq!(bytes, vec![0xBE, 0xEF]),
            other => panic!("expected String8, got {other:?}"),
        }
    }

    #[test]
    fn span_returns_some_for_source_origin_variants() {
        let s = Span::new(10, 20);
        macro_rules! check {
            ($v:expr) => {{
                let v: Value = $v;
                // Re-construct with a non-zero span where possible to confirm
                // `span()` echoes the stored span, not the zero placeholder.
                let with_span = set_span(v, s);
                assert_eq!(with_span.span(), Some(s), "span not propagated");
            }};
        }
        check!(Value::Integer { n: 1, span: s });
        check!(Value::Float { f: 1.0, span: s });
        check!(Value::String {
            s: Rc::from("x"),
            span: s
        });
        check!(Value::Char { c: 'a', span: s });
        check!(Value::Word {
            sym: Symbol::new("w"),
            binding: Binding::Unbound,
            span: s
        });
        check!(Value::SetWord {
            sym: Symbol::new("w"),
            binding: Binding::Unbound,
            span: s
        });
        check!(Value::GetWord {
            sym: Symbol::new("w"),
            binding: Binding::Unbound,
            span: s
        });
        check!(Value::LitWord {
            sym: Symbol::new("w"),
            span: s
        });
        check!(Value::Block {
            series: Series::empty(),
            span: s
        });
        check!(Value::Paren {
            series: Series::empty(),
            span: s
        });
        check!(Value::Path {
            parts: vec![],
            span: s
        });
        check!(Value::GetPath {
            parts: vec![],
            span: s
        });
        check!(Value::LitPath {
            parts: vec![],
            span: s
        });
        check!(Value::SetPath {
            parts: vec![],
            span: s
        });
        check!(Value::Refinement {
            sym: Symbol::new("r"),
            span: s
        });
        check!(Value::File {
            path: Rc::from("p"),
            span: s
        });
        check!(Value::Url {
            url: Rc::from("u"),
            span: s
        });
        check!(Value::String8 {
            bytes: vec![1, 2, 3],
            span: s
        });
    }

    #[test]
    fn span_returns_none_for_synthetic_variants() {
        assert!(Value::None.span().is_none());
        assert!(Value::Logic(true).span().is_none());
        assert!(Value::Func(Rc::new(FuncDef::default())).span().is_none());
        assert!(Value::error("x").span().is_none());
        assert!(Value::object(ObjectDef::new()).span().is_none());
    }

    #[test]
    fn span_or_default_returns_zero_for_synthetic() {
        assert!(Value::None.span_or_default().is_default());
        assert!(Value::Logic(true).span_or_default().is_default());
        assert!(Value::Func(Rc::new(FuncDef::default()))
            .span_or_default()
            .is_default());
    }

    #[test]
    fn span_or_default_returns_real_for_source_origin() {
        let s = Span::new(7, 13);
        let v = Value::Integer { n: 0, span: s };
        assert_eq!(v.span_or_default(), s);
    }

    /// Helper: overwrite the span of a source-origin `Value` with `s`. Used
    /// by `span_returns_some_for_source_origin_variants` to confirm `span()`
    /// echoes the stored span rather than the zero placeholder the shorthand
    /// constructors set.
    fn set_span(v: Value, s: Span) -> Value {
        match v {
            Value::Integer { n, .. } => Value::Integer { n, span: s },
            Value::Float { f, .. } => Value::Float { f, span: s },
            Value::String { s: ss, .. } => Value::String { s: ss, span: s },
            Value::Char { c, .. } => Value::Char { c, span: s },
            Value::Word { sym, binding, .. } => Value::Word {
                sym,
                binding,
                span: s,
            },
            Value::SetWord { sym, binding, .. } => Value::SetWord {
                sym,
                binding,
                span: s,
            },
            Value::GetWord { sym, binding, .. } => Value::GetWord {
                sym,
                binding,
                span: s,
            },
            Value::LitWord { sym, .. } => Value::LitWord { sym, span: s },
            Value::Block { series, .. } => Value::Block { series, span: s },
            Value::Paren { series, .. } => Value::Paren { series, span: s },
            Value::Path { parts, .. } => Value::Path { parts, span: s },
            Value::GetPath { parts, .. } => Value::GetPath { parts, span: s },
            Value::LitPath { parts, .. } => Value::LitPath { parts, span: s },
            Value::SetPath { parts, .. } => Value::SetPath { parts, span: s },
            Value::Refinement { sym, .. } => Value::Refinement { sym, span: s },
            Value::File { path, .. } => Value::File { path, span: s },
            Value::Url { url, .. } => Value::Url { url, span: s },
            Value::String8 { bytes, .. } => Value::String8 { bytes, span: s },
            other => other,
        }
    }

    fn empty_compiled_block() -> CompiledBlock {
        CompiledBlock {
            instrs: Rc::from(&[][..]),
            pool: Rc::from(&[][..]),
            symbols: Rc::from(&Vec::<Symbol>::new()[..]),
            freevars_table: Rc::from(&Vec::<Vec<Symbol>>::new()[..]),
            n_locals: 0,
            freevars: Vec::new(),
            source_span: Span::default(),
            spans: Rc::from(&[][..]),
            needs_rebind: false,
            arity: 0,
        }
    }
}
