//! Core value model: `Value`, `Symbol`, `Span`, `Series`, `Binding`, `FuncDef`.
//!
//! Milestone 2 scope: types exist with stubbed binding/function fields so the
//! printer can be built and tested. Real binding/function machinery lands in
//! Milestones 5 and 9.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, NaiveTime, Utc};
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
/// - `Closure(usize)`: a free-variable slot captured into a `ClosureDef`'s
///   `captures` cell at `closure`-creation time (M60). Indexes into
///   `ClosureDef.captures`; resolved at eval time via the active call frame's
///   `captures` Vec (`Frame.captures` in the VM, `CallFrame.captures` in the
///   walker). Unlike `Lexical`, the captured value is *snapshotted* at
///   creation — outer writes after `MakeClosure` don't propagate inward. The
///   `RefCell<Value>` per capture permits interior mutability across
///   invocations of the same closure (a `count: count + 1` body sees its own
///   prior write on the next call).
#[derive(Clone, Debug, Default)]
pub enum Binding {
    #[default]
    Unbound,
    Local(Rc<Context>, usize),
    Func(usize),
    Lexical(usize, usize),
    /// M60: index into the enclosing closure's `ClosureDef.captures` cell.
    /// The cell is read/written via the active call frame's `captures` Vec
    /// (sized at frame push from the `ClosureDef`). For non-closure frames
    /// the resolver reports a `Native` error (should never happen — the
    /// binding is only attached to freevar words inside a `closure` body).
    Closure(usize),
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

/// A closure value (M60): a `func`-style definition plus an owned cell of
/// captured free-variable values. Snapshotted at `closure`-creation time
/// (the v0.3 escaping-closure bug fix). The `captures: Vec<RefCell<Value>>`
/// is indexed in the same order as the analyzer's freevar list; a body word
/// referencing a freevar carries `Binding::Closure(idx)` so the VM/walker can
/// resolve it to `captures[idx]` of the active closure call frame.
///
/// Deviation from upstream Red: real Red `closure!` shares the cell across
/// multiple closures closing over the same variable (and inner writes
/// propagate outward). v0.5 ships snapshot semantics — two closures closing
/// over the same outer `x` get *independent* cells — but `RefCell` per
/// capture permits interior mutability across *invocations of the same
/// closure* (a `count: count + 1` body persists its prior write). Shared
/// cells are deferred to v0.6.
#[derive(Clone, Debug)]
pub struct ClosureDef {
    /// The underlying function definition: spec/body/ctx/etc. The body's
    /// freevar words have `Binding::Closure(idx)` (attached by
    /// `bind_closure_body`); other words bind normally (`Func`/`Local`/
    /// `Lexical`). `Rc`-backed so the VM/walker can cheaply clone for
    /// `ensure_compiled` + `CallFrame.func`.
    pub func: Rc<FuncDef>,
    /// Free-variable capture cells, indexed by `Binding::Closure(idx)`. Sized
    /// at `MakeClosure` time (VM) or `closure_native` time (walker) from the
    /// analyzer's `(Symbol, depth, slot)` list. Each cell is a `RefCell` so
    /// a closure body's `SetWord` to a captured word mutates the same cell
    /// across invocations of that closure. `Rc`-backed so the `Frame`/
    /// `CallFrame` shares the same cells without cloning the Vec.
    pub captures: Rc<Vec<RefCell<Value>>>,
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
    /// `100x200` — a pair! literal. Source-origin (the lexer scans the
    /// `NxM` form); carries the byte-offset span of the whole token. `x`/`y`
    /// are `Rc<Value>` so a pair can hold int/int, int/float, float/float.
    /// Immutable (value-semantics) — set-path returns a new pair.
    Pair {
        x: Rc<Value>,
        y: Rc<Value>,
        span: Span,
    },
    /// `255.0.0` / `128.64.32.128` — a tuple! literal (RGB or RGBA color
    /// bytes). Source-origin (the lexer scans the `R.G.B[.A]` form); carries
    /// the byte-offset span of the whole token. 3 or 4 bytes, each 0–255.
    /// Immutable (value-semantics) — set-path returns a new tuple.
    Tuple { bytes: Rc<[u8]>, span: Span },
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
    /// A closure value (M60): a `FuncDef` plus a captured free-variable cell.
    /// Synthetic — produced by the `closure` native (walker) or
    /// `Instr::MakeClosure` (VM); carries no source span of its own. A closure
    /// is a function (`function?` returns `true` on it); `closure?` is the
    /// strict predicate.
    Closure(Rc<ClosureDef>),
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
    /// A module (M61): a self-contained namespace (`ctx`) carrying its own
    /// word→value slots, a set of *exported* words (the public surface), an
    /// optional name (for named modules cached on `Env`), an optional source
    /// path (for file-based module caching, M62), and an optional parent
    /// context (the script `user_ctx` or another module, for lexical chaining
    /// — unused in M61 but reserved for v0.6+). Synthetic — produced by the
    /// `module` native / `make module!`; carries no source span of its own.
    Module(Rc<RefCell<ModuleDef>>),
    /// A map (M43): an insertion-ordered heterogeneous key→value table. Keys
    /// are the hashable subset of `Value` (`MapKey`); values are arbitrary.
    /// Synthetic — produced by `make map!`/`to-map`; carries no source span.
    Map(Rc<RefCell<MapDef>>),
    /// `29-Jun-2024` / `2024-06-29T12:30:00Z` — a `date!` literal (M45).
    /// Source-origin (the lexer scans the date/time/zone form); carries the
    /// byte-offset span of the whole token. A single variant covers date-only,
    /// date+time, and date+time+zone (Red parity — there is no separate
    /// `time!` type; `time?` is a predicate on `date!`).
    ///
    /// The `zone` field is `Option<i32>` minutes east of UTC, mirroring Red's
    /// internal `date!/zone` representation: `None` = zone-naive (no offset
    /// emitted on mold); `Some(0)` = UTC (molds as `+00:00`); `Some(330)` =
    /// `+05:30`. `FixedOffset` is used transiently during parse/mold/`now`
    /// only — the struct stores raw minutes.
    Date { dt: Rc<DateValue>, span: Span },
    /// A bitset! (M46): a bit-packed set of byte values (0..255) used by
    /// `parse` dialect charset matching and the `charset`/`make bitset!`
    /// constructors. Synthetic — produced at runtime; carries no source span.
    Bitset(Rc<RefCell<BitsetDef>>),
    /// A port! (M113): a synchronous I/O handle over a `File` or `Http`
    /// scheme. Synthetic — produced by the `open`/`create` natives; carries
    /// no source span of its own. Holds a `PortDef` with scheme, target, and
    /// interior-mutable `PortState` (open/closed + an in-process buffered
    /// cursor for streaming reads). For HTTP ports the `ureq::Response` body
    /// `Read` handle is held across multiple `read port` calls so the body is
    /// not slurped at `open` time; file ports slurp the whole file on `read`
    /// (matching today's `read %file` behavior).
    Port(Rc<RefCell<PortDef>>),
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

/// A module (M61): an isolated namespace (`ctx`) plus the set of words marked
/// exported via the `export` native. The module body is evaluated with
/// `env.user_ctx` temporarily swapped to `ctx` (mirroring `make object!`),
/// so SetWords inside the body allocate slots in the module's context.
///
/// Visibility: **inside the module body** all words in `ctx` are visible
/// (private + public) — `env.user_ctx` is the module's ctx during body
/// evaluation, so bare word resolution finds them. **Outside the module**,
/// `module/word` path resolution and `import` (M62) consult only `exports`.
/// The `export` native adds a word to `exports` as a side-effect; it does
/// not restrict inner access.
///
/// Named modules (`module 'name [...]`) are cached on `Env::modules` keyed
/// by name — a second `module 'name [different body]` returns the cached
/// value (the new body is ignored, matching Red's "module is a singleton by
/// name"). Anonymous modules (`module [body]`) are not cached.
///
/// `parent` is reserved for lexical chaining (v0.6+); M61 sets it to the
/// script `user_ctx` at creation time but does not consult it during word
/// resolution (the `resolve_word` `Unbound` fallback that would consult it
/// lands in M62).
#[derive(Clone, Debug)]
pub struct ModuleDef {
    /// The module's namespace: ordered word→slot map (same `Rc<Context>`
    /// shape as the user context and object contexts). Installed as
    /// `env.user_ctx` during body evaluation so SetWords write here and bare
    /// words resolve here.
    pub ctx: Rc<Context>,
    /// Words marked public via `export` (a side-effect declaration inside
    /// the body). `words-of`/`values-of`/`reflect` on a module value return
    /// only these, in `ctx` insertion order; `module/word` path resolution
    /// from outside the module body succeeds only for words in this set.
    pub exports: RefCell<HashSet<Symbol>>,
    /// Name for named modules (`module 'name [...]`). `None` for anonymous
    /// modules. Named modules are cached on `Env::modules[name]`.
    pub name: Option<Symbol>,
    /// Canonical source path for file-based modules (M62 `import %file`).
    /// `None` for modules created directly via the `module` native. Reserved
    /// in M61; the M62 file-import path populates it.
    pub source: Option<Rc<str>>,
    /// Parent context (the script `user_ctx` or another module) for lexical
    /// chaining. Reserved for v0.6+; M61 sets it but the resolver does not
    /// consult it (the `Unbound` fallback that would walk the parent chain
    /// is M62's behavior change).
    pub parent: Option<Rc<Context>>,
}

impl Default for ModuleDef {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleDef {
    pub fn new() -> Self {
        Self {
            ctx: Rc::new(Context::new()),
            exports: RefCell::new(HashSet::new()),
            name: None,
            source: None,
            parent: None,
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

/// A `bitset!` (M46): a bit-packed set of byte values (0..255). Used as a
/// `parse` dialect matcher (matches any char in the set) and as a standalone
/// value type with set operations (`union`/`intersect`/`difference`/
/// `complement`). Bits are packed into `Vec<u64>`; `len` is the bit count
/// (always a multiple of 64 in storage; the logical size is 256 for charsets
/// but may be larger for general bitsets).
///
/// Mutation is interior (`RefCell`), mirroring `MapDef`/`ObjectDef` — a
/// `Value::Bitset(Rc<RefCell<BitsetDef>>)` is shared by aliases and mutations
/// are visible to all references.
#[derive(Clone, Debug, Default)]
pub struct BitsetDef {
    pub bits: RefCell<Vec<u64>>,
    /// Logical bit count (number of bits the bitset can address). Always
    /// rounded up to a multiple of 64 in storage.
    pub len: usize,
}

impl BitsetDef {
    /// Create a new empty bitset addressing `len` bits.
    pub fn new(len: usize) -> Self {
        let words = len.div_ceil(64);
        BitsetDef {
            bits: RefCell::new(vec![0u64; words]),
            len,
        }
    }

    /// Create a charset bitset addressing 256 byte values (the standard
    /// `charset` form for `parse` matching).
    pub fn new_charset() -> Self {
        Self::new(256)
    }

    /// Set the bit for `byte` (no-op if out of range).
    pub fn set(&self, byte: usize) {
        if byte >= self.len {
            return;
        }
        let word = byte / 64;
        let bit = byte % 64;
        self.bits.borrow_mut()[word] |= 1u64 << bit;
    }

    /// Clear the bit for `byte` (no-op if out of range).
    pub fn clear(&self, byte: usize) {
        if byte >= self.len {
            return;
        }
        let word = byte / 64;
        let bit = byte % 64;
        self.bits.borrow_mut()[word] &= !(1u64 << bit);
    }

    /// Test the bit for `byte` (returns `false` if out of range).
    pub fn test(&self, byte: usize) -> bool {
        if byte >= self.len {
            return false;
        }
        let word = byte / 64;
        let bit = byte % 64;
        self.bits.borrow()[word] & (1u64 << bit) != 0
    }

    /// Union `other` into `self` (in-place). Sizes must match; if `other` is
    /// smaller/larger, only the overlapping range is unioned.
    pub fn union(&self, other: &BitsetDef) {
        let mut mine = self.bits.borrow_mut();
        let theirs = other.bits.borrow();
        for i in 0..mine.len().min(theirs.len()) {
            mine[i] |= theirs[i];
        }
    }

    /// Intersect `self` with `other` (in-place).
    pub fn intersect(&self, other: &BitsetDef) {
        let mut mine = self.bits.borrow_mut();
        let theirs = other.bits.borrow();
        for i in 0..mine.len().min(theirs.len()) {
            mine[i] &= theirs[i];
        }
    }

    /// Subtract `other` from `self` (in-place).
    pub fn difference(&self, other: &BitsetDef) {
        let mut mine = self.bits.borrow_mut();
        let theirs = other.bits.borrow();
        for i in 0..mine.len().min(theirs.len()) {
            mine[i] &= !theirs[i];
        }
    }

    /// Complement all bits in-place (within `len`).
    pub fn complement(&self) {
        let mut mine = self.bits.borrow_mut();
        for w in mine.iter_mut() {
            *w = !*w;
        }
        // Mask off bits beyond `len` in the trailing word.
        let extra = self.len % 64;
        if extra != 0 && !mine.is_empty() {
            let mask = (1u64 << extra) - 1;
            let last = mine.len() - 1;
            mine[last] &= mask;
        }
    }

    /// Build a charset from the chars of `s` (each char's codepoint becomes
    /// a set bit, modulo 256).
    pub fn from_chars(s: &str) -> Self {
        let bs = Self::new_charset();
        for c in s.chars() {
            let b = (c as u32) as usize;
            if b < 256 {
                bs.set(b);
            }
        }
        bs
    }

    /// Build a charset with bits set for `lo..=hi` (inclusive byte range).
    pub fn from_range(lo: u8, hi: u8) -> Self {
        let bs = Self::new_charset();
        let (start, end) = if lo <= hi {
            (lo as usize, hi as usize)
        } else {
            (hi as usize, lo as usize)
        };
        for b in start..=end {
            bs.set(b);
        }
        bs
    }

    /// Count of set bits.
    pub fn count(&self) -> usize {
        self.bits
            .borrow()
            .iter()
            .map(|w| w.count_ones() as usize)
            .sum()
    }

    /// Is the bitset empty (no set bits)?
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// Iterate over the byte values (as `char`s) of set bits, in ascending
    /// order. Used by the printer for the `make bitset! "ABC"` form.
    pub fn iter_set_chars(&self) -> Vec<char> {
        let mut out = Vec::new();
        let mine = self.bits.borrow();
        for (word_idx, &w) in mine.iter().enumerate() {
            let mut bits = w;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                let byte = word_idx * 64 + bit;
                if byte < self.len && byte < 256 {
                    out.push(char::from_u32(byte as u32).unwrap_or('\0'));
                }
                bits &= !(1u64 << bit);
            }
        }
        out
    }

    /// Raw byte view of the bit packs (byte i = bits 8i..8i+7, little-endian
    /// within each u64 word). Used by the printer's `#{hex}` fallback form.
    pub fn raw_bytes(&self) -> Vec<u8> {
        let mine = self.bits.borrow();
        let n_bytes = self.len.div_ceil(8);
        let mut out = Vec::with_capacity(n_bytes);
        for i in 0..n_bytes {
            let word = i / 8;
            let byte_in_word = i % 8;
            let b = if word < mine.len() {
                (mine[word] >> (8 * byte_in_word)) & 0xFF
            } else {
                0
            };
            out.push(b as u8);
        }
        out
    }
}

/// Protocol scheme for a `port!` (M113). Only `File` and `Http` are live in
/// v0.9 — the rest are reserved as `PortScheme` variants that error with
/// `NetError::UnsupportedInV09(scheme)` so the dispatch table is obviously
/// extensible for v0.10+ (which adds `ftp.rs`/`smtp.rs`/`dns.rs`/etc. under
/// `red-eval/src/net/`). `Http` covers both `http://` and `https://` — TLS
/// is a `NetworkOptions.tls` flag, not a separate scheme, matching `ureq`'s
/// URL-driven model.
#[derive(Clone, Debug, PartialEq, Eq, Copy)]
pub enum PortScheme {
    File,
    Http,
    Ftp,
    Smtp,
    Pop3,
    Nntp,
    Dns,
    Tcp,
    Udp,
    Whois,
    Finger,
    Daytime,
}

impl PortScheme {
    /// Lowercase name used in error messages and `mold` output.
    pub fn as_str(self) -> &'static str {
        match self {
            PortScheme::File => "file",
            PortScheme::Http => "http",
            PortScheme::Ftp => "ftp",
            PortScheme::Smtp => "smtp",
            PortScheme::Pop3 => "pop3",
            PortScheme::Nntp => "nntp",
            PortScheme::Dns => "dns",
            PortScheme::Tcp => "tcp",
            PortScheme::Udp => "udp",
            PortScheme::Whois => "whois",
            PortScheme::Finger => "finger",
            PortScheme::Daytime => "daytime",
        }
    }

    /// Is this scheme live (dispatched) in v0.9?
    pub fn is_supported_in_v09(self) -> bool {
        matches!(self, PortScheme::File | PortScheme::Http)
    }
}

/// Inner state of an open `port!` (M113). Held in a `RefCell` inside
/// `PortDef` so mutations (open/close, cursor advance) are visible to all
/// aliases — mirrors `MapDef`/`ObjectDef` interior-mutability.
///
/// `HttpBody` holds the `ureq::Response` body `Read` handle across multiple
/// `read port` calls so the body is *not* slurped at `open` time. The body
/// is read in 8 KiB chunks per `read port` call (POC deviation — see M113
/// open question 4); an empty chunk at EOF signals completion.
///
/// `FileHandle` holds an open `std::fs::File` for write/append ports; file
/// read ports slurp the whole file on the first `read port` call (matching
/// today's `read %file` behavior, unlike HTTP's streaming).
pub struct PortState {
    /// Whether the port is currently open. `false` after `close` or before
    /// `open`; reads/writes on a closed port raise `NetError::Closed`.
    pub open: bool,
    /// HTTP response body reader (held across `read port` calls). `None` for
    /// file ports or after the body has been fully consumed.
    pub http_body: Option<Box<dyn std::io::Read + Send>>,
    /// Open file handle for write/append file ports. `None` for read-only
    /// file ports (which slurp via `std::fs::read` on `read port`) and for
    /// HTTP ports.
    pub file_handle: Option<std::fs::File>,
    /// Buffering cursor for partial reads (unused in v0.9 — reserved for
    /// future chunk-aware `read/part` semantics).
    pub cursor: u64,
}

impl std::fmt::Debug for PortState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortState")
            .field("open", &self.open)
            .field("http_body", &self.http_body.as_ref().map(|_| "<reader>"))
            .field("file_handle", &self.file_handle.as_ref().map(|_| "<file>"))
            .field("cursor", &self.cursor)
            .finish()
    }
}

/// A `port!` definition (M113): a synchronous I/O handle over a `File` or
/// `Http` scheme. Synthetic — produced by the `open`/`create` natives (never
/// by the lexer); wrapped as `Value::Port(Rc<RefCell<PortDef>>)`.
///
/// `target` is the scheme-specific address: a filesystem path string for
/// `File` ports (relative paths resolved against `env.cwd`), a full URL for
/// `Http` ports (`http://host/path` or `https://host/path`).
///
/// `state` is interior-mutable so `open`/`close`/`read`/`write` can mutate
/// the handle in place without rebuilding the `Value::Port`.
#[derive(Debug)]
pub struct PortDef {
    pub scheme: PortScheme,
    pub target: Rc<str>,
    pub state: RefCell<PortState>,
}

impl PortDef {
    /// Construct a port definition with the port initially closed. `open`
    /// flips `state.open` to true and populates `http_body`/`file_handle`.
    pub fn new(scheme: PortScheme, target: Rc<str>) -> Self {
        PortDef {
            scheme,
            target,
            state: RefCell::new(PortState {
                open: false,
                http_body: None,
                file_handle: None,
                cursor: 0,
            }),
        }
    }
}

/// A `date!` payload (M45): a wall-clock `NaiveDateTime` plus an optional UTC
/// offset stored as `Option<i32>` minutes (matching Red's internal
/// `date!/zone` shape). `None` is zone-naive (no offset emitted on mold);
/// `Some(0)` is UTC (molds as `+00:00`); `Some(330)` is `+05:30`.
///
/// Three logical states all live in this single struct:
/// - **date-only**: `dt` is at midnight (`00:00:00`), `zone = None`.
/// - **date+time**: `dt` has a real time component, `zone = None`.
/// - **date+time+zone**: `dt` has a real time, `zone = Some(_)`.
///
/// (Red folds `time!` into `date!` — there is no separate `Value::Time`
/// variant; `time?` is a predicate on `date!`.)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DateValue {
    pub dt: NaiveDateTime,
    pub zone: Option<i32>,
}

impl DateValue {
    /// Build a `DateValue` from local wall-clock `dt` and an optional zone
    /// (minutes east of UTC). Caller supplies both; no automatic offset
    /// derivation here.
    pub fn from_local(dt: NaiveDateTime, zone: Option<i32>) -> Self {
        DateValue { dt, zone }
    }

    /// Date-only constructor (midnight, zone-naive).
    pub fn date_only(d: NaiveDate) -> Self {
        DateValue {
            dt: d.and_hms_opt(0, 0, 0).expect("midnight is always valid"),
            zone: None,
        }
    }

    /// True iff the value has a non-midnight time component. (`12:00:00` is
    /// midnight; `00:00:01` is not.)
    pub fn has_time(&self) -> bool {
        self.dt.time() != NaiveTime::default()
    }

    /// Apply `zone` to produce an absolute UTC instant. Zone-naive (`None`)
    /// is treated as UTC for arithmetic only (matching plan5.md M45: "None
    /// treated as UTC for arithmetic").
    pub fn to_offset_utc(&self) -> DateTime<Utc> {
        let offset_secs = self.zone.unwrap_or(0) * 60;
        let utc_naive = self.dt - chrono::Duration::seconds(offset_secs as i64);
        DateTime::<Utc>::from_naive_utc_and_offset(utc_naive, Utc)
    }

    /// Construct the `FixedOffset` for `zone` (`None` → UTC). Used transiently
    /// during mold/parse only.
    pub fn fixed_offset(&self) -> FixedOffset {
        FixedOffset::east_opt(self.zone.unwrap_or(0) * 60)
            .expect("zone minutes in range (lexer enforces |m| <= 14*60)")
    }

    /// Produce an absolute `DateTime<FixedOffset>` (the wall-clock instant
    /// with its zone attached). `None` zone → UTC offset.
    pub fn to_zoned(&self) -> DateTime<FixedOffset> {
        DateTime::<FixedOffset>::from_naive_utc_and_offset(self.dt, self.fixed_offset())
    }

    /// `now` constructor: current local time + the system's local UTC offset.
    /// The offset may differ between calls during DST transitions; that's the
    /// system's behavior, not a Red-parity issue.
    pub fn now_local() -> Self {
        let now = Local::now();
        let offset = now.offset().local_minus_utc() / 60;
        DateValue {
            dt: now.naive_local(),
            zone: Some(offset),
        }
    }

    /// `today` constructor: date-only at local midnight, `zone: None`.
    pub fn today_local() -> Self {
        Self::date_only(Local::now().date_naive())
    }

    /// Helper: `to-utc` shift-and-relabel. Subtracts `zone` minutes from `dt`
    /// (so the wall clock shows the UTC time), then sets `zone = Some(0)`.
    pub fn to_utc(&self) -> Self {
        let offset_secs = self.zone.unwrap_or(0) as i64 * 60;
        DateValue {
            dt: self.dt - chrono::Duration::seconds(offset_secs),
            zone: Some(0),
        }
    }

    /// Construct a `DateValue` from Unix epoch seconds (UTC). The result has
    /// `zone = Some(0)` (UTC). Returns `None` if `secs` is out of range.
    pub fn from_epoch(secs: i64) -> Option<Self> {
        let dt = DateTime::<Utc>::from_timestamp(secs, 0)?.naive_utc();
        Some(DateValue::from_local(dt, Some(0)))
    }

    /// Construct a `date!` from a `std::time::SystemTime` (e.g. a file's
    /// mtime), expressed in the **local** timezone with the system's local
    /// UTC offset attached. Used by `modified?` (M45).
    pub fn from_system_time_local(st: std::time::SystemTime) -> Self {
        let dt: DateTime<Local> = st.into();
        let offset = dt.offset().local_minus_utc() / 60;
        DateValue::from_local(dt.naive_local(), Some(offset))
    }

    /// Add `days` to the date portion. Zone is preserved. Used by
    /// `date + integer` arithmetic (M45).
    pub fn add_days(&self, days: i64) -> Self {
        DateValue::from_local(self.dt + chrono::Duration::days(days), self.zone)
    }

    /// Replace the time component, keeping the date + zone. Used by
    /// `date + time` arithmetic (M45).
    pub fn with_time(&self, time: NaiveTime) -> Self {
        DateValue::from_local(self.dt.date().and_time(time), self.zone)
    }

    // --- M45 path accessors (`date/year`, `date/zone`, etc.) ---

    pub fn year(&self) -> i32 {
        use chrono::Datelike;
        self.dt.year()
    }
    pub fn month(&self) -> u32 {
        use chrono::Datelike;
        self.dt.month()
    }
    pub fn day(&self) -> u32 {
        use chrono::Datelike;
        self.dt.day()
    }
    pub fn hour(&self) -> u32 {
        use chrono::Timelike;
        self.dt.hour()
    }
    pub fn minute(&self) -> u32 {
        use chrono::Timelike;
        self.dt.minute()
    }
    pub fn second(&self) -> u32 {
        use chrono::Timelike;
        self.dt.second()
    }
    /// ISO weekday (1=Monday .. 7=Sunday).
    pub fn weekday(&self) -> u32 {
        use chrono::Datelike;
        self.dt.weekday().number_from_monday()
    }
    /// Day of year (1..=366).
    pub fn yearday(&self) -> u32 {
        use chrono::Datelike;
        self.dt.ordinal()
    }
    /// ISO week number (1..=53).
    pub fn week(&self) -> u32 {
        use chrono::Datelike;
        self.dt.iso_week().week()
    }
    /// The time component as a `NaiveTime`.
    pub fn time(&self) -> NaiveTime {
        self.dt.time()
    }
    /// Relabel the zone offset only (does NOT shift the wall-clock `dt`).
    /// Matches Red's `date/zone:` set-path semantics.
    pub fn relabel_zone(&self, zone: Option<i32>) -> Self {
        DateValue::from_local(self.dt, zone)
    }
    /// The zone as a `time!`-shaped `DateValue` (date portion zeroed to epoch,
    /// time = |zone| as HH:MM, sign carried in the time value via negative
    /// hours). Used by `date/zone` path accessor.
    pub fn zone_as_time(&self) -> Option<Value> {
        let z = self.zone?;
        let abs = z.abs();
        let h: u32 = (abs / 60) as u32;
        let m: u32 = (abs % 60) as u32;
        let time = NaiveTime::from_hms_opt(h, m, 0)?;
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
        Some(Value::date(DateValue::from_local(
            epoch.and_time(time),
            None,
        )))
    }
}

impl Value {
    /// Span of this value in the original source. Every source-origin variant
    /// (`Integer`/`Float`/`String`/word-family/`Block`/`Paren`/`Path`/
    /// `Refinement`/`String8`/`Pair`/`Tuple`/`Date`) carries its token span;
    /// synthetic variants (`None`/`Logic`/`Func`) return `None`.
    pub fn span(&self) -> Option<Span> {
        match self {
            Value::Integer { span, .. }
            | Value::Float { span, .. }
            | Value::String { span, .. }
            | Value::Char { span, .. }
            | Value::Pair { span, .. }
            | Value::Tuple { span, .. }
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
            | Value::String8 { span, .. }
            | Value::Date { span, .. } => Some(*span),
            Value::None
            | Value::Logic(_)
            | Value::Func(_)
            | Value::Closure(_)
            | Value::Error(_)
            | Value::Object(_)
            | Value::Module(_)
            | Value::Map(_)
            | Value::Bitset(_)
            | Value::Port(_) => None,
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

    /// Constructor shorthand for a pair! value with a zero span (test/REPL
    /// use). Wraps `x`/`y` in `Rc<Value>`.
    pub fn pair(x: Value, y: Value) -> Self {
        Value::Pair {
            x: Rc::new(x),
            y: Rc::new(y),
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a tuple! value with a zero span
    /// (test/REPL use). `bytes` must be 3 or 4 elements (RGB or RGBA);
    /// construction does not enforce this — callers are responsible.
    pub fn tuple(bytes: Vec<u8>) -> Self {
        Value::Tuple {
            bytes: Rc::from(bytes.as_slice()),
            span: Span::default(),
        }
    }

    /// Constructor shorthand for an object wrapping `obj_def`.
    pub fn object(obj_def: ObjectDef) -> Self {
        Value::Object(Rc::new(RefCell::new(obj_def)))
    }

    /// Constructor shorthand for a module wrapping `module_def`. (M61.)
    pub fn module(module_def: ModuleDef) -> Self {
        Value::Module(Rc::new(RefCell::new(module_def)))
    }

    /// Constructor shorthand for a closure value wrapping `func` + the
    /// captured free-variable cells. (M60.)
    pub fn closure(func: Rc<FuncDef>, captures: Rc<Vec<RefCell<Value>>>) -> Self {
        Value::Closure(Rc::new(ClosureDef { func, captures }))
    }

    /// Constructor shorthand for a map wrapping `map_def`.
    pub fn map(map_def: MapDef) -> Self {
        Value::Map(Rc::new(RefCell::new(map_def)))
    }

    /// Constructor shorthand for a `date!` value with a zero span (test/REPL
    /// use).
    pub fn date(dt: DateValue) -> Self {
        Value::Date {
            dt: Rc::new(dt),
            span: Span::default(),
        }
    }

    /// Constructor shorthand for a bitset! value wrapping `bs_def`.
    pub fn bitset(bs_def: BitsetDef) -> Self {
        Value::Bitset(Rc::new(RefCell::new(bs_def)))
    }

    /// Constructor shorthand for a port! value wrapping `port_def`. (M113.)
    pub fn port(port_def: PortDef) -> Self {
        Value::Port(Rc::new(RefCell::new(port_def)))
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

        // M60: Closure binding is not Lexical.
        let closure = Binding::Closure(2);
        assert!(!closure.is_lexical());
        assert!(closure.as_lexical().is_none());
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
    fn value_closure_constructor() {
        // M60: `Value::closure` wraps a FuncDef + captures cell into a
        // `Closure` variant.
        let cd = Value::closure(Rc::new(FuncDef::default()), Rc::new(Vec::new()));
        match &cd {
            Value::Closure(c) => {
                assert!(c.captures.is_empty());
            }
            other => panic!("expected Closure, got {other:?}"),
        }
    }

    #[test]
    fn value_map_constructor() {
        let m = MapDef::new();
        match Value::map(m) {
            Value::Map(_) => {}
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn value_date_constructor() {
        let d = DateValue::default();
        match Value::date(d) {
            Value::Date { dt, span } => {
                assert!(span.is_default());
                assert_eq!(dt.zone, None);
            }
            other => panic!("expected Date, got {other:?}"),
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
        check!(Value::Pair {
            x: Rc::new(Value::integer(1)),
            y: Rc::new(Value::integer(2)),
            span: s
        });
        check!(Value::Tuple {
            bytes: Rc::from(&[1u8, 2, 3][..]),
            span: s
        });
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
        check!(Value::Date {
            dt: Rc::new(DateValue::default()),
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
        // M60: closure is synthetic — no span.
        assert!(
            Value::closure(Rc::new(FuncDef::default()), Rc::new(Vec::new()))
                .span()
                .is_none()
        );
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
            Value::Pair { x, y, .. } => Value::Pair { x, y, span: s },
            Value::Tuple { bytes, .. } => Value::Tuple { bytes, span: s },
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
            Value::Date { dt, .. } => Value::Date { dt, span: s },
            other => other,
        }
    }

    fn empty_compiled_block() -> CompiledBlock {
        CompiledBlock {
            instrs: Rc::from(&[][..]),
            pool: Rc::from(&[][..]),
            symbols: Rc::from(&Vec::<Symbol>::new()[..]),
            freevars_table: Rc::from(&Vec::<Vec<Symbol>>::new()[..]),
            captures_table: Rc::from(&Vec::<Vec<(Symbol, usize, usize)>>::new()[..]),
            n_locals: 0,
            freevars: Vec::new(),
            source_span: Span::default(),
            spans: Rc::from(&[][..]),
            needs_rebind: false,
            arity: 0,
        }
    }
}
