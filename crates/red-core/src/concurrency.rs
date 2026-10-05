//! M40: The Send boundary — `SendValue` enum + marshalling.
//!
//! Every `Value` is `Rc`-backed (`Series = Rc<RefCell<Vec<Value>>>`, `Func(Rc<
//! FuncDef>)`, `Object(Rc<RefCell<ObjectDef>>)`, `Symbol(Rc<str>)`). All
//! `!Send`. Crossing a thread boundary requires marshalling into a `Send`-safe
//! form: the `SendValue` enum and its supporting mirror types.
//!
//! Soundness: `Symbol` is `Rc<str>` (non-atomic refcounts). Moving an `Rc`
//! across a thread boundary is UB. `SendValue` therefore stores `Arc<str>`
//! everywhere the plan says `Symbol`; `marshal_send` extracts the string and
//! allocates a fresh `Arc`, and `unmarshal` allocates a fresh `Rc` on the
//! receiver side. No `Rc` ever crosses a thread boundary.
//!
//! Marshalable types: `None`, `Unset`, `Logic`, `Integer`, `Float`, `Decimal`,
//! `Percent`, `Money`, `Issue`, `Email`, `Tag`, `String`, `Char`, `Pair`,
//! `Tuple`, word variants (`Word`/`SetWord`/`GetWord`/`LitWord`/`Refinement`),
//! `Block`/`Paren` (deep-cloned to flat `Vec<SendValue>` from the cursor),
//! path variants (`Path`/`GetPath`/`LitPath`/`SetPath`), `File`/`Url`,
//! `Object` (deep-cloned to `SendObject` — frozen snapshot with prototype
//! chain preserved), `Error` (deep-cloned to `SendError`), `Channel` (shared
//! — both ends live in one `Arc`), and all aggregate types (`Module`, `Map`,
//! `Hash`, `Vector`, `Image`, `Bitset`, `Port`, `Typeset`, `SemanticType`,
//! `Closure`, `Date`, `Duration`).
//!
//! Rejected types: `Func` (closures over `Rc<Context>` — `!Send`; workers
//! reference funcs by name resolved via their `ThreadEnv`'s `user_ctx`
//! snapshot, not by value), `String8` (POC stub, defer with `binary!`).
//! Sending these raises `EvalError::Native`.

use std::cell::RefCell;
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::context::Context;
use crate::env::EvalError;
use crate::value::{
    Binding, BitsetDef, ClosureDef, DateValue, ErrorValue, FuncDef, HashDef, ImageDef, MapDef,
    MapKey, ModuleDef, MoneyValue, ObjectDef, PortDef, PortScheme, SemanticShape, SemanticTypeDef,
    Series, Span, Symbol, TypesetDef, Value, VectorDef,
};

// ===========================================================================
// ChannelInner — the shared channel primitive (fully defined here, wired
// into Value::Channel in this milestone; channel natives arrive in M42).
// ===========================================================================

/// Go-style bidirectional channel: both ends in one value. Cloning a Channel
/// value = Arc bump (cheap; both tx and rx ride along). Closing one end via
/// `close` marks the channel half-closed; subsequent `send` errors, `recv`
/// returns `none` when drained.
///
/// `pub(crate)` until M42 wires it into `Value::Channel` natives. The struct
/// is `Send` + `Sync` (all fields are `Send`+`Sync`): `Mutex` wraps the
/// `Sender`/`Receiver`, and `AtomicBool` is inherently `Send`+`Sync`.
#[allow(dead_code)] // M42 wires tx/rx into channel natives
pub struct ChannelInner {
    pub tx: Mutex<Option<Sender<SendValue>>>,
    pub rx: Mutex<Receiver<SendValue>>,
    pub closed: AtomicBool,
}

impl std::fmt::Debug for ChannelInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelInner")
            .field("closed", &self.closed.load(std::sync::atomic::Ordering::Relaxed))
            .finish()
    }
}

// ===========================================================================
// ObjectKind — stub for the actor-library distinction (M45).
// ===========================================================================

/// Discriminator for `SendObject`. Only `Plain` in M40; `Actor` arrives in
/// M45 when the cooperative actor library lands. Preserved across the Send
/// boundary so an object snapshot carries its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Plain,
}

// ===========================================================================
// SendBlock — immutable snapshot of a Series.
// ===========================================================================

/// Owned, `Send`-safe mirror of a `Series` snapshot. Flat `Vec<SendValue>`
/// with no cursor — positioned views are a thread-local construct on the
/// receiver (the receiver constructs a fresh `Series` whose `index = 0`).
#[derive(Clone, Debug)]
pub struct SendBlock {
    pub data: Vec<SendValue>,
}

// ===========================================================================
// SendContext — helper for flattening a Context into Send-safe form.
// ===========================================================================

/// Flattened `Context` snapshot: ordered word names + parallel slot values.
/// Used by `SendObject` and `SendModule` to mirror the `Rc<Context>` storage
/// without interior mutability.
#[derive(Clone, Debug)]
pub struct SendContext {
    pub words: Vec<Arc<str>>,
    pub slots: Vec<SendValue>,
}

// ===========================================================================
// SendObject — immutable snapshot of an ObjectDef.
// ===========================================================================

/// Owned, `Send`-safe mirror of an `ObjectDef`. The prototype chain is
/// preserved (immutably) via `Option<Arc<SendObject>>`. No `RefCell`, no
/// `Rc` — a frozen snapshot, same model as `SendBlock`.
#[derive(Clone, Debug)]
pub struct SendObject {
    pub words: Vec<Arc<str>>,
    pub slots: Vec<SendValue>,
    pub parent: Option<Arc<SendObject>>,
    pub self_word: Arc<str>,
    pub kind: ObjectKind,
}

// ===========================================================================
// SendError — mirror of ErrorValue's actual field set.
// ===========================================================================

/// Owned, `Send`-safe mirror of an `ErrorValue`. Mirrors the real `ErrorValue`
/// fields (`message`, `code`, `kind`, `args`, `near`, `cause`, `by`) rather
/// than the plan's aspirational `payload: Arc<SendObject>` shape, which has
/// no correspondent in the actual `ErrorValue` struct. Marshalable iff the
/// structured fields (`args`, `near`) are marshalable; an `Error` wrapping a
/// `Func` value in `args` or `near` (rare) is rejected at marshal time.
#[derive(Clone, Debug)]
pub struct SendError {
    pub message: Arc<str>,
    pub code: Option<i64>,
    pub kind: Option<Arc<str>>,
    pub args: Vec<SendValue>,
    pub near: Option<Box<SendValue>>,
    pub cause: Option<Arc<str>>,
    pub by: Option<Arc<str>>,
}

// ===========================================================================
// Aggregate mirror types.
// ===========================================================================

/// `Send`-safe mirror of `MoneyValue`.
#[derive(Clone, Debug)]
pub struct SendMoneyValue {
    pub cents: i64,
    pub currency: Arc<str>,
}

/// `Send`-safe mirror of `DateValue`. All fields are inherently `Send`.
#[derive(Clone, Debug)]
pub struct SendDateValue {
    pub dt: chrono::NaiveDateTime,
    pub zone: Option<i32>,
}

/// `Send`-safe mirror of `MapKey`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SendMapKey {
    Sym(Arc<str>),
    Int(i64),
    Str(Arc<str>),
    Char(char),
    Bool(bool),
    None,
}

/// `Send`-safe mirror of `MapDef` (insertion-ordered key→value table).
#[derive(Clone, Debug)]
pub struct SendMap {
    pub entries: Vec<(SendMapKey, SendValue)>,
}

/// `Send`-safe mirror of `HashDef` (unordered key→value + insertion-order list).
#[derive(Clone, Debug)]
pub struct SendHash {
    pub entries: Vec<(SendMapKey, SendValue)>,
    pub key_order: Vec<SendMapKey>,
}

/// `Send`-safe mirror of `VectorDef`.
#[derive(Clone, Debug)]
pub struct SendVector {
    pub kind: Arc<str>,
    pub elems: Vec<SendValue>,
    pub cursor: usize,
}

/// `Send`-safe mirror of `ImageDef`. Pure data — all fields inherently `Send`.
#[derive(Clone, Debug)]
pub struct SendImage {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[u8; 4]>,
}

/// `Send`-safe mirror of `BitsetDef`.
#[derive(Clone, Debug)]
pub struct SendBitset {
    pub bits: Vec<u64>,
    pub len: usize,
}

/// `Send`-safe mirror of `PortDef`. `PortState.http_body` (a `Box<dyn Read +
/// Send>`) is dropped on marshal — only `open`/`cursor` survive, since a
/// `Read` handle is not cloneable and the receiver must re-`open` if it
/// needs to continue streaming.
#[derive(Clone, Debug)]
pub struct SendPort {
    pub scheme: PortScheme,
    pub target: Arc<str>,
    pub open: bool,
    pub cursor: u64,
}

/// `Send`-safe mirror of `TypesetDef`.
#[derive(Clone, Debug)]
pub struct SendTypeset {
    pub types: Vec<Arc<str>>,
    pub semantic: Option<Arc<SendSemanticType>>,
}

/// `Send`-safe mirror of `SemanticTypeDef`. The `compiled` parse-rule cache
/// is dropped (it's a lazily-built `Rc<Series>` — not cloneable across the
/// boundary, and the receiver rebuilds it on first use).
#[derive(Clone, Debug)]
pub struct SendSemanticType {
    pub name: Arc<str>,
    pub base: Arc<str>,
    pub shape: SemanticShape,
    pub schema: SendBlock,
}

/// `Send`-safe mirror of `FuncDef` (helper for `SendClosure`). Drops the
/// `native` function pointer (not meaningful on the receiver — the worker
/// resolves natives by name via its `ThreadEnv`), the `compiled` bytecode
/// cache (`Rc` — not `Send`), and the `param_types` typesets (contain
/// `Rc<TypesetDef>`). The body and ctx are deep-cloned.
#[derive(Clone, Debug)]
pub struct SendFuncDef {
    pub params: Vec<Arc<str>>,
    pub refinements: Vec<(Arc<str>, Vec<Arc<str>>)>,
    pub locals: Vec<Arc<str>>,
    pub freevars: Vec<Arc<str>>,
    pub body: SendBlock,
    pub ctx: SendContext,
    pub variadic: bool,
    pub infix: bool,
}

/// `Send`-safe mirror of `ClosureDef`. The underlying `FuncDef` is
/// deep-cloned into a `SendFuncDef` (body/ctx only; native handler and
/// compiled caches dropped — the closure is **not invocable** on the
/// receiver, but can be inspected/molded/passed as data). Captures are
/// deep-cloned into `Vec<SendValue>`.
#[derive(Clone, Debug)]
pub struct SendClosure {
    pub func: SendFuncDef,
    pub captures: Vec<SendValue>,
}

/// `Send`-safe mirror of `ModuleDef`.
#[derive(Clone, Debug)]
pub struct SendModule {
    pub ctx: SendContext,
    pub exports: Vec<Arc<str>>,
    pub name: Option<Arc<str>>,
    pub source: Option<Arc<str>>,
    pub parent: Option<SendContext>,
}

// ===========================================================================
// SendValue — the owned, `Send`-safe mirror of the marshalable `Value` subset.
// ===========================================================================

/// Owned, `Send`-safe mirror of the marshalable `Value` subset. `Arc`-backed
/// instead of `Rc`; no `RefCell` (channels are owned, not shared). Derives
/// `Debug`. Automatically `Send` (all fields are `Send` — no `Rc`, no raw
/// pointers, no `!Send` function pointers).
#[derive(Clone, Debug)]
pub enum SendValue {
    None,
    Unset,
    Logic(bool),
    Integer(i64),
    Float(f64),
    Decimal(rust_decimal::Decimal),
    Percent(f64),
    Money(SendMoneyValue),
    Issue(Arc<str>),
    Email(Arc<str>),
    Tag(Arc<str>),
    String(Arc<str>),
    Char(char),
    Pair(Box<SendValue>, Box<SendValue>),
    Tuple(Arc<[u8]>),
    Word(Arc<str>),
    SetWord(Arc<str>),
    GetWord(Arc<str>),
    LitWord(Arc<str>),
    Block(Arc<SendBlock>),
    Paren(Arc<SendBlock>),
    Path(Vec<SendValue>),
    GetPath(Vec<SendValue>),
    LitPath(Vec<SendValue>),
    SetPath(Vec<SendValue>),
    Refinement(Arc<str>),
    File(Arc<str>),
    Url(Arc<str>),
    Object(Arc<SendObject>),
    Error(Arc<SendError>),
    Module(Arc<SendModule>),
    Map(Arc<SendMap>),
    Hash(Arc<SendHash>),
    Vector(Arc<SendVector>),
    Image(Arc<SendImage>),
    Bitset(Arc<SendBitset>),
    Port(Arc<SendPort>),
    Typeset(Arc<SendTypeset>),
    SemanticType(Arc<SendSemanticType>),
    /// M175: a tagged semantic value — the semantic-type *name* plus the
    /// marshalled inner value. The `SemanticTypeDef` itself is NOT needed
    /// on the receiver side (a tagged value carries only the tag), so the
    /// round-trip is lossless even though the def's compiled parse rule
    /// isn't Send-safe.
    SemanticTagged {
        tag: Arc<str>,
        value: Box<SendValue>,
    },
    Closure(Arc<SendClosure>),
    Date(Arc<SendDateValue>),
    Duration(chrono::Duration),
    /// Shared — both ends travel. `Arc::clone` on marshal (cheap bump);
    /// `Arc::clone` on unmarshal (same Arc). Pointer-identity is preserved.
    Channel(Arc<ChannelInner>),
}

// ===========================================================================
// marshal_send — deep-clone the marshalable subset into Send-safe form.
// ===========================================================================

impl Value {
    /// Deep-clone this value into a `Send`-safe `SendValue` form, or reject
    /// with `EvalError::Native` if the value (or any nested value) contains a
    /// non-marshalable type (`Func` or `String8`).
    ///
    /// For `Block`/`Paren`, walks the `Series.data` from `index..` (positions
    /// are preserved as a `Vec<SendValue>` starting at the cursor — the
    /// receiver gets a positioned view by constructing a fresh `Series` whose
    /// `index = 0`).
    ///
    /// For `Object`, walks `ctx.words()`/`ctx.slot_value(i)` into a
    /// `SendObject` (recursively marshalling slot values; prototype chain is
    /// deep-cloned as `Option<Arc<SendObject>>`).
    ///
    /// For `Error`, mirrors the `ErrorValue` fields; if `args` or `near`
    /// contains a `Func` (rare), rejects with the standard message naming
    /// `function!`.
    ///
    /// For `Channel`, shares the `Arc<ChannelInner>` (cheap Arc bump — both
    /// ends travel). Pointer-identity is preserved across the boundary.
    pub fn marshal_send(&self) -> Result<SendValue, EvalError> {
        marshal_value(self)
    }
}

/// Free-function form of the marshal pass (used by `Value::marshal_send` and
/// recursively for nested values).
fn marshal_value(v: &Value) -> Result<SendValue, EvalError> {
    match v {
        Value::None => Ok(SendValue::None),
        Value::Unset => Ok(SendValue::Unset),
        Value::Logic(b) => Ok(SendValue::Logic(*b)),
        Value::Integer { n, .. } => Ok(SendValue::Integer(*n)),
        Value::Float { f, .. } => Ok(SendValue::Float(*f)),
        Value::Decimal { d, .. } => Ok(SendValue::Decimal(*d)),
        Value::Percent { value, .. } => Ok(SendValue::Percent(*value)),
        Value::Money { amount, .. } => Ok(SendValue::Money(SendMoneyValue {
            cents: amount.cents,
            currency: Arc::from(amount.currency.as_ref()),
        })),
        Value::Issue { s, .. } => Ok(SendValue::Issue(Arc::from(s.as_ref()))),
        Value::Email { addr, .. } => Ok(SendValue::Email(Arc::from(addr.as_ref()))),
        Value::Tag { text, .. } => Ok(SendValue::Tag(Arc::from(text.as_ref()))),
        Value::String { s, .. } => Ok(SendValue::String(Arc::from(s.as_ref()))),
        Value::Char { c, .. } => Ok(SendValue::Char(*c)),
        Value::Pair { x, y, .. } => Ok(SendValue::Pair(
            Box::new(marshal_value(x)?),
            Box::new(marshal_value(y)?),
        )),
        Value::Tuple { bytes, .. } => Ok(SendValue::Tuple(Arc::from(bytes.as_ref()))),
        Value::Word { sym, .. } => Ok(SendValue::Word(Arc::from(sym.as_str()))),
        Value::SetWord { sym, .. } => Ok(SendValue::SetWord(Arc::from(sym.as_str()))),
        Value::GetWord { sym, .. } => Ok(SendValue::GetWord(Arc::from(sym.as_str()))),
        Value::LitWord { sym, .. } => Ok(SendValue::LitWord(Arc::from(sym.as_str()))),
        Value::Refinement { sym, .. } => Ok(SendValue::Refinement(Arc::from(sym.as_str()))),
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            let mut out = Vec::with_capacity(data.len() - series.index);
            for item in data.iter().skip(series.index) {
                out.push(marshal_value(item)?);
            }
            Ok(SendValue::Block(Arc::new(SendBlock { data: out })))
        }
        Value::Paren { series, .. } => {
            let data = series.data.borrow();
            let mut out = Vec::with_capacity(data.len() - series.index);
            for item in data.iter().skip(series.index) {
                out.push(marshal_value(item)?);
            }
            Ok(SendValue::Paren(Arc::new(SendBlock { data: out })))
        }
        Value::Path { parts, .. } => {
            let mut out = Vec::with_capacity(parts.len());
            for p in parts {
                out.push(marshal_value(p)?);
            }
            Ok(SendValue::Path(out))
        }
        Value::GetPath { parts, .. } => {
            let mut out = Vec::with_capacity(parts.len());
            for p in parts {
                out.push(marshal_value(p)?);
            }
            Ok(SendValue::GetPath(out))
        }
        Value::LitPath { parts, .. } => {
            let mut out = Vec::with_capacity(parts.len());
            for p in parts {
                out.push(marshal_value(p)?);
            }
            Ok(SendValue::LitPath(out))
        }
        Value::SetPath { parts, .. } => {
            let mut out = Vec::with_capacity(parts.len());
            for p in parts {
                out.push(marshal_value(p)?);
            }
            Ok(SendValue::SetPath(out))
        }
        Value::File { path, .. } => Ok(SendValue::File(Arc::from(path.as_ref()))),
        Value::Url { url, .. } => Ok(SendValue::Url(Arc::from(url.as_ref()))),
        Value::Object(obj) => {
            let obj_ref = obj.borrow();
            Ok(SendValue::Object(Arc::new(marshal_object(&obj_ref)?)))
        }
        Value::Error(err) => Ok(SendValue::Error(Arc::new(marshal_error(err)?))),
        Value::Channel(arc) => Ok(SendValue::Channel(Arc::clone(arc))),
        Value::Module(m) => {
            let m_ref = m.borrow();
            Ok(SendValue::Module(Arc::new(marshal_module(&m_ref)?)))
        }
        Value::Map(m) => {
            let m_ref = m.borrow();
            let entries = m_ref.entries.borrow();
            let mut out = Vec::with_capacity(entries.len());
            for (k, v) in entries.iter() {
                out.push((marshal_map_key(k), marshal_value(v)?));
            }
            Ok(SendValue::Map(Arc::new(SendMap { entries: out })))
        }
        Value::Hash(h) => {
            let h_ref = h.borrow();
            let entries_ref = h_ref.entries.borrow();
            let key_order_ref = h_ref.key_order.borrow();
            let mut entries = Vec::with_capacity(entries_ref.len());
            for (k, v) in entries_ref.iter() {
                entries.push((marshal_map_key(k), marshal_value(v)?));
            }
            let key_order: Vec<SendMapKey> =
                key_order_ref.iter().map(marshal_map_key).collect();
            Ok(SendValue::Hash(Arc::new(SendHash { entries, key_order })))
        }
        Value::Vector(v) => {
            let v_ref = v.borrow();
            let kind = Arc::from(v_ref.kind.borrow().as_str());
            let elems: Vec<SendValue> = v_ref
                .elems
                .borrow()
                .iter()
                .map(marshal_value)
                .collect::<Result<_, _>>()?;
            let cursor = *v_ref.cursor.borrow();
            Ok(SendValue::Vector(Arc::new(SendVector {
                kind,
                elems,
                cursor,
            })))
        }
        Value::Image(im) => {
            let im_ref = im.borrow();
            let width = im_ref.width;
            let height = im_ref.height;
            let pixels = im_ref.pixels.borrow().clone();
            drop(im_ref);
            Ok(SendValue::Image(Arc::new(SendImage {
                width,
                height,
                pixels,
            })))
        }
        Value::Bitset(b) => {
            let b_ref = b.borrow();
            let bits = b_ref.bits.borrow().clone();
            let len = b_ref.len;
            drop(b_ref);
            Ok(SendValue::Bitset(Arc::new(SendBitset { bits, len })))
        }
        Value::Port(p) => {
            let p_ref = p.borrow();
            let state = p_ref.state.borrow();
            Ok(SendValue::Port(Arc::new(SendPort {
                scheme: p_ref.scheme,
                target: Arc::from(p_ref.target.as_ref()),
                open: state.open,
                cursor: state.cursor,
            })))
        }
        Value::Typeset(t) => {
            let types: Vec<Arc<str>> = t
                .types
                .borrow()
                .iter()
                .map(|s| Arc::from(s.as_str()))
                .collect();
            let semantic = match t.semantic.borrow().as_ref() {
                Some(sem) => Some(Arc::new(marshal_semantic_type(sem)?)),
                None => None,
            };
            Ok(SendValue::Typeset(Arc::new(SendTypeset { types, semantic })))
        }
        Value::SemanticType(t) => Ok(SendValue::SemanticType(Arc::new(
            marshal_semantic_type(t)?,
        ))),
        // M175: marshal the inner value; keep the tag name only.
        Value::SemanticTagged { tag, value, .. } => Ok(SendValue::SemanticTagged {
            tag: Arc::from(tag.as_str()),
            value: Box::new(marshal_value(value)?),
        }),
        Value::Closure(cl) => Ok(SendValue::Closure(Arc::new(marshal_closure(cl)?))),
        Value::Date { dt, .. } => Ok(SendValue::Date(Arc::new(SendDateValue {
            dt: dt.dt,
            zone: dt.zone,
        }))),
        Value::Duration { d, .. } => Ok(SendValue::Duration(*d)),
        // Rejected types:
        Value::Func(_) => Err(EvalError::Native {
            message: "cannot send function! across thread boundary".to_string(),
            span: Span::default(),
        }),
        Value::String8 { .. } => Err(EvalError::Native {
            message: "cannot send binary! across thread boundary".to_string(),
            span: Span::default(),
        }),
    }
}

/// Marshal an `ObjectDef` into a `SendObject`.
fn marshal_object(obj: &ObjectDef) -> Result<SendObject, EvalError> {
    // Skip the `self` slot — it holds a circular `Rc<RefCell<ObjectDef>>`
    // pointing back to this same object. Without skipping, `marshal_value`
    // would recurse into the same `Value::Object` → infinite recursion →
    // stack overflow. The printer's `mold_object` skips `self` for the
    // same reason (printer.rs:379).
    let self_str = obj.self_word.as_str();
    let words_syms: Vec<Symbol> = obj
        .ctx
        .words()
        .into_iter()
        .filter(|s| s.as_str() != self_str)
        .collect();
    let names = obj.ctx.names.borrow();
    let words: Vec<Arc<str>> = words_syms
        .iter()
        .map(|s| Arc::from(s.as_str()))
        .collect();
    let slots: Vec<SendValue> = words_syms
        .iter()
        .map(|s| {
            let idx = *names.get(s).unwrap();
            marshal_value(&obj.ctx.slot_value(idx))
        })
        .collect::<Result<_, _>>()?;
    let parent = match &obj.parent {
        Some(p) => Some(Arc::new(marshal_object(&p.borrow())?)),
        None => None,
    };
    Ok(SendObject {
        words,
        slots,
        parent,
        self_word: Arc::from(obj.self_word.as_str()),
        kind: ObjectKind::Plain,
    })
}

/// Marshal an `ErrorValue` into a `SendError`. Rejects if `args` or `near`
/// contains a `Func` value.
fn marshal_error(err: &ErrorValue) -> Result<SendError, EvalError> {
    let args: Vec<SendValue> = err
        .args
        .iter()
        .map(marshal_value)
        .collect::<Result<_, _>>()?;
    let near = match &err.near {
        Some(v) => Some(Box::new(marshal_value(v)?)),
        None => None,
    };
    Ok(SendError {
        message: Arc::from(err.message.as_str()),
        code: err.code,
        kind: err.kind.as_ref().map(|s| Arc::from(s.as_str())),
        args,
        near,
        cause: err.cause.as_ref().map(|s| Arc::from(s.as_str())),
        by: err.by.as_ref().map(|s| Arc::from(s.as_str())),
    })
}

/// Marshal a `ModuleDef` into a `SendModule`.
fn marshal_module(m: &ModuleDef) -> Result<SendModule, EvalError> {
    let ctx = marshal_context(&m.ctx)?;
    let exports: Vec<Arc<str>> = m
        .exports
        .borrow()
        .iter()
        .map(|s| Arc::from(s.as_str()))
        .collect();
    let parent = match &m.parent {
        Some(p) => Some(marshal_context(p)?),
        None => None,
    };
    Ok(SendModule {
        ctx,
        exports,
        name: m.name.as_ref().map(|s| Arc::from(s.as_str())),
        source: m.source.as_ref().map(|s| Arc::from(s.as_ref())),
        parent,
    })
}

/// Flatten a `Context` into a `SendContext` (ordered words + marshalled slots).
/// Non-marshalable slot values (`Func`, `String8`) are replaced with
/// `SendValue::None` — the worker rebuilds its own natives via
/// `register_natives`, and user-defined funcs can't cross the Send boundary
/// (they capture `Rc<Context>`). This prevents the stdlib's `Func` values
/// in `user_ctx` from blocking `fork_thread_env`.
pub fn marshal_context(ctx: &Context) -> Result<SendContext, EvalError> {
    let words_syms = ctx.words();
    let names = ctx.names.borrow();
    let words: Vec<Arc<str>> = words_syms
        .iter()
        .map(|s| Arc::from(s.as_str()))
        .collect();
    let slots: Vec<SendValue> = words_syms
        .iter()
        .map(|s| {
            let idx = *names.get(s).unwrap();
            let val = ctx.slot_value(idx);
            // Skip non-marshalable types (Func, String8) — replace with None.
            match marshal_value(&val) {
                Ok(sv) => sv,
                Err(_) => SendValue::None,
            }
        })
        .collect();
    Ok(SendContext { words, slots })
}

/// Marshal a `MapKey` into a `SendMapKey`.
fn marshal_map_key(k: &MapKey) -> SendMapKey {
    match k {
        MapKey::Sym(s) => SendMapKey::Sym(Arc::from(s.as_str())),
        MapKey::Int(n) => SendMapKey::Int(*n),
        MapKey::Str(s) => SendMapKey::Str(Arc::from(s.as_ref())),
        MapKey::Char(c) => SendMapKey::Char(*c),
        MapKey::Bool(b) => SendMapKey::Bool(*b),
        MapKey::None => SendMapKey::None,
    }
}

/// Marshal a `SemanticTypeDef` into a `SendSemanticType`.
fn marshal_semantic_type(t: &SemanticTypeDef) -> Result<SendSemanticType, EvalError> {
    let schema_data = t.schema.data.borrow();
    let mut schema_out = Vec::with_capacity(schema_data.len() - t.schema.index);
    for item in schema_data.iter().skip(t.schema.index) {
        schema_out.push(marshal_value(item)?);
    }
    Ok(SendSemanticType {
        name: Arc::from(t.name.as_str()),
        base: Arc::from(t.base.as_str()),
        shape: t.shape,
        schema: SendBlock { data: schema_out },
    })
}

/// Marshal a `ClosureDef` into a `SendClosure`.
fn marshal_closure(cl: &ClosureDef) -> Result<SendClosure, EvalError> {
    let func = marshal_func_def(&cl.func)?;
    let captures: Vec<SendValue> = cl
        .captures
        .iter()
        .map(|cell| marshal_value(&cell.borrow()))
        .collect::<Result<_, _>>()?;
    Ok(SendClosure { func, captures })
}

/// Marshal a `FuncDef` into a `SendFuncDef` (drops native/compiled caches).
fn marshal_func_def(f: &FuncDef) -> Result<SendFuncDef, EvalError> {
    let params: Vec<Arc<str>> = f.params.iter().map(|s| Arc::from(s.as_str())).collect();
    let refinements: Vec<(Arc<str>, Vec<Arc<str>>)> = f
        .refinements
        .iter()
        .map(|(r, args)| {
            (
                Arc::from(r.as_str()),
                args.iter().map(|a| Arc::from(a.as_str())).collect(),
            )
        })
        .collect();
    let locals: Vec<Arc<str>> = f.locals.iter().map(|s| Arc::from(s.as_str())).collect();
    let freevars: Vec<Arc<str>> = f.freevars.iter().map(|s| Arc::from(s.as_str())).collect();
    let body_data = f.body.data.borrow();
    let mut body_out = Vec::with_capacity(body_data.len() - f.body.index);
    for item in body_data.iter().skip(f.body.index) {
        body_out.push(marshal_value(item)?);
    }
    let ctx = marshal_context(&f.ctx)?;
    Ok(SendFuncDef {
        params,
        refinements,
        locals,
        freevars,
        body: SendBlock { data: body_out },
        ctx,
        variadic: f.variadic,
        infix: f.infix,
    })
}

// ===========================================================================
// unmarshal — rewrap into the Rc-backed forms on the receiver side.
// ===========================================================================

impl SendValue {
    /// ReWrap this `SendValue` into the `Rc`-backed `Value` form on the
    /// receiver side. All spans become `Span::default()`; all word bindings
    /// become `Binding::Unbound`. `Channel` unmarshals via `Arc::clone` (cheap
    /// Arc bump — both ends travel). `Object` unmarshals to a fresh
    /// `Rc<RefCell<ObjectDef>>` (independent storage, with the prototype
    /// chain rebuilt as nested `Rc`s).
    pub fn unmarshal(&self) -> Value {
        unmarshal_value(self)
    }
}

/// Free-function form of the unmarshal pass.
fn unmarshal_value(sv: &SendValue) -> Value {
    match sv {
        SendValue::None => Value::None,
        SendValue::Unset => Value::Unset,
        SendValue::Logic(b) => Value::Logic(*b),
        SendValue::Integer(n) => Value::Integer {
            n: *n,
            span: Span::default(),
        },
        SendValue::Float(f) => Value::Float {
            f: *f,
            span: Span::default(),
        },
        SendValue::Decimal(d) => Value::Decimal {
            d: *d,
            span: Span::default(),
        },
        SendValue::Percent(value) => Value::Percent {
            value: *value,
            span: Span::default(),
        },
        SendValue::Money(m) => Value::Money {
            amount: Rc::new(MoneyValue {
                cents: m.cents,
                currency: Rc::from(m.currency.as_ref()),
            }),
            span: Span::default(),
        },
        SendValue::Issue(s) => Value::Issue {
            s: Rc::from(s.as_ref()),
            span: Span::default(),
        },
        SendValue::Email(s) => Value::Email {
            addr: Rc::from(s.as_ref()),
            span: Span::default(),
        },
        SendValue::Tag(s) => Value::Tag {
            text: Rc::from(s.as_ref()),
            span: Span::default(),
        },
        SendValue::String(s) => Value::String {
            s: Rc::from(s.as_ref()),
            span: Span::default(),
        },
        SendValue::Char(c) => Value::Char {
            c: *c,
            span: Span::default(),
        },
        SendValue::Pair(x, y) => Value::Pair {
            x: Rc::new(unmarshal_value(x)),
            y: Rc::new(unmarshal_value(y)),
            span: Span::default(),
        },
        SendValue::Tuple(bytes) => Value::Tuple {
            bytes: Rc::<[u8]>::from(bytes.as_ref()),
            span: Span::default(),
        },
        SendValue::Word(s) => Value::Word {
            sym: Symbol(Rc::from(s.as_ref())),
            binding: Binding::Unbound,
            span: Span::default(),
        },
        SendValue::SetWord(s) => Value::SetWord {
            sym: Symbol(Rc::from(s.as_ref())),
            binding: Binding::Unbound,
            span: Span::default(),
        },
        SendValue::GetWord(s) => Value::GetWord {
            sym: Symbol(Rc::from(s.as_ref())),
            binding: Binding::Unbound,
            span: Span::default(),
        },
        SendValue::LitWord(s) => Value::LitWord {
            sym: Symbol(Rc::from(s.as_ref())),
            span: Span::default(),
        },
        SendValue::Block(b) => Value::Block {
            series: unmarshal_block(b),
            span: Span::default(),
        },
        SendValue::Paren(b) => Value::Paren {
            series: unmarshal_block(b),
            span: Span::default(),
        },
        SendValue::Path(parts) => Value::Path {
            parts: parts.iter().map(unmarshal_value).collect(),
            span: Span::default(),
        },
        SendValue::GetPath(parts) => Value::GetPath {
            parts: parts.iter().map(unmarshal_value).collect(),
            span: Span::default(),
        },
        SendValue::LitPath(parts) => Value::LitPath {
            parts: parts.iter().map(unmarshal_value).collect(),
            span: Span::default(),
        },
        SendValue::SetPath(parts) => Value::SetPath {
            parts: parts.iter().map(unmarshal_value).collect(),
            span: Span::default(),
        },
        SendValue::Refinement(s) => Value::Refinement {
            sym: Symbol(Rc::from(s.as_ref())),
            span: Span::default(),
        },
        SendValue::File(s) => Value::File {
            path: Rc::from(s.as_ref()),
            span: Span::default(),
        },
        SendValue::Url(s) => Value::Url {
            url: Rc::from(s.as_ref()),
            span: Span::default(),
        },
        SendValue::Object(o) => Value::Object(Rc::new(RefCell::new(unmarshal_object(o)))),
        SendValue::Error(e) => Value::Error(Rc::new(unmarshal_error(e))),
        SendValue::Channel(arc) => Value::Channel(Arc::clone(arc)),
        SendValue::Module(m) => Value::Module(Rc::new(RefCell::new(unmarshal_module(m)))),
        SendValue::Map(m) => {
            let map = MapDef::new();
            for (k, v) in &m.entries {
                map.set(unmarshal_map_key(k), unmarshal_value(v));
            }
            Value::Map(Rc::new(RefCell::new(map)))
        }
        SendValue::Hash(h) => {
            let hash = HashDef::new();
            // Insert in `key_order` to preserve the original insertion order.
            let entries = &h.entries;
            for k in &h.key_order {
                let v = entries
                    .iter()
                    .find(|(ek, _)| ek == k)
                    .map(|(_, v)| v)
                    .expect("key_order key missing from entries");
                hash.set(unmarshal_map_key(k), unmarshal_value(v));
            }
            Value::Hash(Rc::new(RefCell::new(hash)))
        }
        SendValue::Vector(v) => {
            let elems: Vec<Value> = v.elems.iter().map(unmarshal_value).collect();
            Value::Vector(Rc::new(RefCell::new(VectorDef::new(
                Symbol(Rc::from(v.kind.as_ref())),
                elems,
            ))))
        }
        SendValue::Image(im) => Value::Image(Rc::new(RefCell::new(ImageDef::new(
            im.width,
            im.height,
            im.pixels.clone(),
        )))),
        SendValue::Bitset(b) => {
            let bs = BitsetDef::new(b.len);
            *bs.bits.borrow_mut() = b.bits.clone();
            Value::Bitset(Rc::new(RefCell::new(bs)))
        }
        SendValue::Port(p) => {
            let port = PortDef::new(p.scheme, Rc::from(p.target.as_ref()));
            port.state.borrow_mut().open = p.open;
            port.state.borrow_mut().cursor = p.cursor;
            Value::Port(Rc::new(RefCell::new(port)))
        }
        SendValue::Typeset(t) => {
            let types: Vec<Symbol> = t.types.iter().map(|s| Symbol(Rc::from(s.as_ref()))).collect();
            let ts = TypesetDef::new(types);
            if let Some(sem) = &t.semantic {
                *ts.semantic.borrow_mut() = Some(Rc::new(unmarshal_semantic_type(sem)));
            }
            Value::Typeset(Rc::new(ts))
        }
        SendValue::SemanticType(t) => {
            Value::SemanticType(Rc::new(unmarshal_semantic_type(t)))
        }
        SendValue::SemanticTagged { tag, value } => {
            Value::semantic_tagged(Symbol::new(tag.as_ref()), unmarshal_value(value), Span::default())
        }
        SendValue::Closure(cl) => {
            let func = unmarshal_func_def(&cl.func);
            let captures: Vec<RefCell<Value>> =
                cl.captures.iter().map(|v| RefCell::new(unmarshal_value(v))).collect();
            Value::Closure(Rc::new(ClosureDef {
                func: Rc::new(func),
                captures: Rc::new(captures),
            }))
        }
        SendValue::Date(dt) => Value::Date {
            dt: Rc::new(DateValue {
                dt: dt.dt,
                zone: dt.zone,
            }),
            span: Span::default(),
        },
        SendValue::Duration(d) => Value::Duration {
            d: *d,
            span: Span::default(),
        },
    }
}

/// Unmarshal a `SendBlock` into a fresh `Series` (index = 0).
pub fn unmarshal_block(b: &SendBlock) -> Series {
    let data: Vec<Value> = b.data.iter().map(unmarshal_value).collect();
    Series::new(data)
}

/// Unmarshal a `SendObject` into a fresh `ObjectDef` (independent `Rc`s).
fn unmarshal_object(o: &SendObject) -> ObjectDef {
    let ctx = Context::new();
    for (w, slot) in o.words.iter().zip(o.slots.iter()) {
        let sym = Symbol(Rc::from(w.as_ref()));
        ctx.set(sym, unmarshal_value(slot));
    }
    let parent = o.parent.as_ref().map(|p| {
        Rc::new(RefCell::new(unmarshal_object(p)))
    });
    ObjectDef {
        ctx: Rc::new(ctx),
        parent,
        self_word: Symbol(Rc::from(o.self_word.as_ref())),
        protected: RefCell::new(false),
    }
}

/// Unmarshal a `SendError` into a fresh `ErrorValue`.
fn unmarshal_error(e: &SendError) -> ErrorValue {
    let args: Vec<Value> = e.args.iter().map(unmarshal_value).collect();
    let near = e.near.as_ref().map(|v| unmarshal_value(v));
    ErrorValue {
        message: String::from(e.message.as_ref()),
        code: e.code,
        kind: e.kind.as_ref().map(|s| Symbol(Rc::from(s.as_ref()))),
        args,
        near,
        cause: e.cause.as_ref().map(|s| Symbol(Rc::from(s.as_ref()))),
        by: e.by.as_ref().map(|s| Symbol(Rc::from(s.as_ref()))),
    }
}

/// Unmarshal a `SendModule` into a fresh `ModuleDef`.
fn unmarshal_module(m: &SendModule) -> ModuleDef {
    let ctx = unmarshal_context(&m.ctx);
    let exports: HashSet<Symbol> = m
        .exports
        .iter()
        .map(|s| Symbol(Rc::from(s.as_ref())))
        .collect();
    let parent = m.parent.as_ref().map(unmarshal_context);
    ModuleDef {
        ctx: Rc::new(ctx),
        exports: RefCell::new(exports),
        name: m.name.as_ref().map(|s| Symbol(Rc::from(s.as_ref()))),
        source: m.source.as_ref().map(|s| Rc::from(s.as_ref())),
        parent: parent.map(Rc::new),
    }
}

/// Unmarshal a `SendContext` into a fresh `Context`.
pub fn unmarshal_context(sc: &SendContext) -> Context {
    let ctx = Context::new();
    for (w, slot) in sc.words.iter().zip(sc.slots.iter()) {
        let sym = Symbol(Rc::from(w.as_ref()));
        ctx.set(sym, unmarshal_value(slot));
    }
    ctx
}

/// Unmarshal a `SendMapKey` into a `MapKey`.
fn unmarshal_map_key(k: &SendMapKey) -> MapKey {
    match k {
        SendMapKey::Sym(s) => MapKey::Sym(Symbol(Rc::from(s.as_ref()))),
        SendMapKey::Int(n) => MapKey::Int(*n),
        SendMapKey::Str(s) => MapKey::Str(Rc::from(s.as_ref())),
        SendMapKey::Char(c) => MapKey::Char(*c),
        SendMapKey::Bool(b) => MapKey::Bool(*b),
        SendMapKey::None => MapKey::None,
    }
}

/// Unmarshal a `SendSemanticType` into a fresh `SemanticTypeDef`.
fn unmarshal_semantic_type(t: &SendSemanticType) -> SemanticTypeDef {
    SemanticTypeDef::new(
        Symbol(Rc::from(t.name.as_ref())),
        Symbol(Rc::from(t.base.as_ref())),
        t.shape,
        unmarshal_block(&t.schema),
    )
}

/// Unmarshal a `SendFuncDef` into a fresh `FuncDef`.
fn unmarshal_func_def(f: &SendFuncDef) -> FuncDef {
    let params: Vec<Symbol> = f.params.iter().map(|s| Symbol(Rc::from(s.as_ref()))).collect();
    let refinements: Vec<(Symbol, Vec<Symbol>)> = f
        .refinements
        .iter()
        .map(|(r, args)| {
            (
                Symbol(Rc::from(r.as_ref())),
                args.iter().map(|a| Symbol(Rc::from(a.as_ref()))).collect(),
            )
        })
        .collect();
    let locals: Vec<Symbol> = f.locals.iter().map(|s| Symbol(Rc::from(s.as_ref()))).collect();
    let freevars: Vec<Symbol> = f.freevars.iter().map(|s| Symbol(Rc::from(s.as_ref()))).collect();
    FuncDef {
        params,
        refinements,
        locals,
        freevars,
        param_types: Vec::new(),
        // Type annotations (positional + refinement) are dropped at the
        // marshal boundary alongside `param_types` — `Rc<TypesetDef>` isn't
        // Send-safe, and the unmarshaled func isn't invocable anyway.
        refinement_types: Vec::new(),
        compiled: None,
        body: unmarshal_block(&f.body),
        ctx: unmarshal_context(&f.ctx),
        native: None,
        variadic: f.variadic,
        infix: f.infix,
    }
}

// ===========================================================================
// M41: MutexWrite adapter — wraps an Arc<Mutex<Box<dyn Write + Send>>> behind
// the std::io::Write interface so the main thread's Env::out can share a
// sink with worker threads (lock-per-print, Erlang-style).
// ===========================================================================

/// `Write` adapter that locks an `Arc<Mutex<Box<dyn Write + Send>>>` per write
/// call. Used so the main thread's `Env::out: Box<dyn Write>` can share an
/// underlying sink (stdout or a test buffer) with worker `ThreadEnv`s. Each
/// `write!`/`writeln!` acquires the mutex, writes, and releases — correct
/// under contention, with per-call lock granularity (Erlang's model).
pub struct MutexWrite {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl MutexWrite {
    pub fn new(inner: Arc<Mutex<Box<dyn Write + Send>>>) -> Self {
        Self { inner }
    }
}

impl std::io::Write for MutexWrite {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.lock().unwrap().flush()
    }
}

impl Clone for MutexWrite {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

// ===========================================================================
// M41: ThreadEnv — the per-worker Send-safe environment snapshot.
// ===========================================================================

/// Per-worker `Send`-safe environment snapshot. Created by
/// `Env::fork_thread_env` on the main thread; moved into the worker thread,
/// which reconstructs a full `Env` from it (all `Rc`s created on the worker
/// thread — no `Rc` crosses a thread boundary).
///
/// **Soundness note:** The plan specified `natives: Arc<HashMap<Symbol,
/// Rc<FuncDef>>>` and `user_ctx: Context`, but both are `!Send` (`Rc` is
/// `!Send`). Instead, `ThreadEnv` carries `Send`-safe mirror types from M40
/// (`SendContext` for the user context snapshot, `Arc<Mutex<...>>` for the
/// output sink). The worker thread reconstructs a full `Env` on its own
/// thread, calling `register_natives` to build a fresh `HashMap<Symbol,
/// Rc<FuncDef>>` (all `Rc`s created and dropped on the worker thread — sound).
/// This costs ~140 `Rc::clone`s per spawn (cheap relative to thread creation)
/// and avoids a retrospective `Rc`→`Arc` refactor of the value model.
#[derive(Clone)]
pub struct ThreadEnv {
    /// `Send`-safe snapshot of the user context (frozen at spawn time).
    /// Unmarshalled on the worker thread into a fresh `Rc<Context>`.
    pub user_ctx: SendContext,
    /// `Send`-safe snapshot of the body block to evaluate. Unmarshalled on
    /// the worker thread into a fresh `Series`.
    pub body: SendBlock,
    /// Shared output sink (lock-per-print). Cloned cheaply (Arc bump).
    pub out: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Thread-local copy of the working directory.
    pub cwd: PathBuf,
    /// Thread-local copy of the shell-exec permission flag.
    pub allow_shell: bool,
    /// Thread-local copy of the network I/O permission flag.
    pub allow_network: bool,
}

// `ThreadEnv` is `Send` because all fields are `Send`:
// - `SendContext`: `Vec<Arc<str>>` + `Vec<SendValue>` — all `Send`
// - `SendBlock`: `Vec<SendValue>` — `Send`
// - `Arc<Mutex<Box<dyn Write + Send>>>`: `Send` + `Sync`
// - `PathBuf`, `bool`: `Send`

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printer::mold_to_string;
    use crate::value::{ObjectDef, Series, Symbol, Value};
    use std::cell::RefCell;
    use std::rc::Rc;

    // ---- Round-trip tests per marshalable type ----

    #[test]
    fn round_trip_integer() {
        let v = Value::integer(5);
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_string() {
        let v = Value::string("hello");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_float() {
        let v = Value::float(2.5);
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_char() {
        let v = Value::char('a');
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_none() {
        let v = Value::None;
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_logic() {
        for b in [true, false] {
            let v = Value::Logic(b);
            let sv = v.marshal_send().unwrap();
            let back = sv.unmarshal();
            assert_eq!(mold_to_string(&back), mold_to_string(&v));
        }
    }

    #[test]
    fn round_trip_word() {
        let v = Value::word("foo");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
        // Binding is dropped on marshal, set to Unbound on unmarshal.
        match back {
            Value::Word { binding, .. } => assert!(matches!(binding, Binding::Unbound)),
            _ => panic!("expected Word"),
        }
    }

    #[test]
    fn round_trip_setword() {
        let v = Value::set_word("x");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_getword() {
        let v = Value::get_word("x");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_litword() {
        let v = Value::lit_word("foo");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_refinement() {
        let v = Value::refinement("only");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_file() {
        let v = Value::file("path/to/file.txt");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_url() {
        let v = Value::url("http://example.com/x");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_pair() {
        let v = Value::pair(Value::integer(1), Value::integer(2));
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_tuple() {
        let v = Value::tuple(vec![255, 0, 0]);
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_issue() {
        let v = Value::issue("ABC");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_email() {
        let v = Value::email("foo@bar.com");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_tag() {
        let v = Value::tag("b");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_money() {
        let v = Value::money(1000, "USD");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_block() {
        let v = Value::block(Series::new(vec![
            Value::integer(1),
            Value::integer(2),
            Value::integer(3),
        ]));
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_paren() {
        let v = Value::paren(Series::new(vec![Value::integer(1), Value::integer(2)]));
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_path() {
        let v = Value::path(vec![Value::word("foo"), Value::word("bar")]);
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    // ---- Channel round-trip (pointer-identity preserved) ----

    #[test]
    fn round_trip_channel_preserves_pointer_identity() {
        let inner = Arc::new(ChannelInner {
            tx: Mutex::new(None),
            rx: Mutex::new(std::sync::mpsc::channel().1),
            closed: AtomicBool::new(false),
        });
        let v = Value::Channel(Arc::clone(&inner));
        let sv = v.marshal_send().unwrap();
        match &sv {
            SendValue::Channel(arc) => assert!(Arc::ptr_eq(arc, &inner)),
            _ => panic!("expected SendValue::Channel"),
        }
        let back = sv.unmarshal();
        match back {
            Value::Channel(arc) => assert!(Arc::ptr_eq(&arc, &inner)),
            _ => panic!("expected Value::Channel"),
        }
    }

    // ---- Object round-trip ----

    #[test]
    fn round_trip_object_basic() {
        let obj = ObjectDef::new();
        obj.ctx.set(Symbol::new("x"), Value::integer(5));
        obj.ctx.set(Symbol::new("y"), Value::string("hello"));
        let v = Value::object(obj);
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn object_round_trip_independent_storage() {
        let obj = ObjectDef::new();
        obj.ctx.set(Symbol::new("x"), Value::integer(5));
        let original = Value::object(obj);
        let sv = original.marshal_send().unwrap();
        let back = sv.unmarshal();
        // The receiver's Rc<RefCell<ObjectDef>> must NOT be ptr-equal.
        match (&original, &back) {
            (Value::Object(o1), Value::Object(o2)) => {
                assert!(!Rc::ptr_eq(o1, o2), "Object should have independent storage");
            }
            _ => panic!("expected Object"),
        }
    }

    #[test]
    fn object_round_trip_preserves_prototype_chain() {
        // Create a parent object.
        let parent_obj = ObjectDef::new();
        parent_obj.ctx.set(Symbol::new("base"), Value::integer(100));
        let parent_rc = Rc::new(RefCell::new(parent_obj));

        // Create a child object with the parent link.
        let mut child = ObjectDef::new();
        child.ctx.set(Symbol::new("x"), Value::integer(5));
        child.parent = Some(Rc::clone(&parent_rc));
        let v = Value::object(child);

        let sv = v.marshal_send().unwrap();
        // Verify the prototype chain is preserved in SendObject.
        match &sv {
            SendValue::Object(o) => {
                assert!(o.parent.is_some(), "prototype chain should be preserved");
                assert_eq!(o.kind, ObjectKind::Plain);
            }
            _ => panic!("expected SendValue::Object"),
        }
        let back = sv.unmarshal();
        // Verify the receiver's object has a parent and inherited fields.
        match &back {
            Value::Object(o) => {
                let o_ref = o.borrow();
                assert!(
                    o_ref.parent.is_some(),
                    "prototype chain should be reconstructed"
                );
                // The parent's fields should be accessible (copy-based inheritance
                // would have copied them into ctx, but we don't do that here;
                // we just preserve the parent link).
            }
            _ => panic!("expected Object"),
        }
        // Mold equality checks the visible fields.
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    // ---- Error round-trip ----

    #[test]
    fn round_trip_error_message_only() {
        let v = Value::error("something went wrong");
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_error_structured() {
        let v = Value::error_structed(
            "division by zero",
            Some(400),
            Some(Symbol::new("math")),
            vec![Value::integer(1), Value::integer(0)],
            Some(Value::block(Series::new(vec![Value::word("divide")]))),
            Some(Symbol::new("divide")),
            Some(Symbol::new("main")),
        );
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    // ---- Rejection tests ----

    #[test]
    fn marshal_rejects_func() {
        let f = FuncDef::default();
        let v = Value::Func(Rc::new(f));
        let err = v.marshal_send().unwrap_err();
        match err {
            EvalError::Native { message, .. } => {
                assert!(message.contains("function!"), "message: {message}");
            }
            _ => panic!("expected EvalError::Native"),
        }
    }

    #[test]
    fn marshal_rejects_string8() {
        let v = Value::binary(vec![0xDE, 0xAD]);
        let err = v.marshal_send().unwrap_err();
        match err {
            EvalError::Native { message, .. } => {
                assert!(message.contains("binary!"), "message: {message}");
            }
            _ => panic!("expected EvalError::Native"),
        }
    }

    #[test]
    fn marshal_rejects_error_wrapping_func_in_args() {
        let v = Value::error_structed(
            "oops",
            None,
            None,
            vec![Value::Func(Rc::new(FuncDef::default()))],
            None,
            None,
            None,
        );
        let err = v.marshal_send().unwrap_err();
        match err {
            EvalError::Native { message, .. } => {
                assert!(message.contains("function!"), "message: {message}");
            }
            _ => panic!("expected EvalError::Native"),
        }
    }

    #[test]
    fn marshal_rejects_error_wrapping_func_in_near() {
        let v = Value::error_structed(
            "oops",
            None,
            None,
            vec![],
            Some(Value::Func(Rc::new(FuncDef::default()))),
            None,
            None,
        );
        let err = v.marshal_send().unwrap_err();
        match err {
            EvalError::Native { message, .. } => {
                assert!(message.contains("function!"), "message: {message}");
            }
            _ => panic!("expected EvalError::Native"),
        }
    }

    // ---- Nested deep-clone tests ----

    #[test]
    fn nested_block_deep_clones_inner_blocks() {
        let inner = Series::new(vec![Value::integer(1), Value::integer(2)]);
        let outer = Series::new(vec![
            Value::block(inner.clone()),
            Value::block(Series::new(vec![Value::integer(3), Value::integer(4)])),
        ]);
        let v = Value::block(outer);
        let sv = v.marshal_send().unwrap();
        match &sv {
            SendValue::Block(b) => {
                // Inner blocks should be SendValue::Block, not Rc aliases.
                assert!(b.data.iter().all(|e| matches!(e, SendValue::Block(_))));
            }
            _ => panic!("expected SendValue::Block"),
        }
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn nested_object_deep_clones_inner_object() {
        let inner = ObjectDef::new();
        inner.ctx.set(Symbol::new("val"), Value::integer(42));
        let inner_val = Value::object(inner);

        let outer = ObjectDef::new();
        outer.ctx.set(Symbol::new("child"), inner_val);
        let v = Value::object(outer);

        let sv = v.marshal_send().unwrap();
        match &sv {
            SendValue::Object(o) => {
                // The inner object should be a SendValue::Object, not an Rc alias.
                let child = o.slots.iter().find(|s| matches!(s, SendValue::Object(_)));
                assert!(child.is_some(), "inner object should be deep-cloned");
            }
            _ => panic!("expected SendValue::Object"),
        }
        let back = sv.unmarshal();
        // Verify the inner object has independent storage.
        match (&v, &back) {
            (Value::Object(o1), Value::Object(o2)) => {
                assert!(!Rc::ptr_eq(o1, o2));
                // Check inner object independence.
                let o1_ref = o1.borrow();
                let o2_ref = o2.borrow();
                let inner1 = o1_ref.ctx.get(&Symbol::new("child")).unwrap();
                let inner2 = o2_ref.ctx.get(&Symbol::new("child")).unwrap();
                match (&inner1, &inner2) {
                    (Value::Object(i1), Value::Object(i2)) => {
                        assert!(!Rc::ptr_eq(i1, i2), "inner object should be independent");
                    }
                    _ => panic!("expected inner Object"),
                }
            }
            _ => panic!("expected Object"),
        }
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    // ---- Positioned series test ----

    #[test]
    fn positioned_series_marshal_starts_at_cursor() {
        // `next [1 2 3]` → series with index=1, so marshal should produce [2 3].
        let series = Series {
            data: Rc::new(RefCell::new(vec![
                Value::integer(1),
                Value::integer(2),
                Value::integer(3),
            ])),
            index: 1,
        };
        let v = Value::block(series);
        let sv = v.marshal_send().unwrap();
        match &sv {
            SendValue::Block(b) => {
                assert_eq!(b.data.len(), 2);
                match &b.data[0] {
                    SendValue::Integer(n) => assert_eq!(*n, 2),
                    _ => panic!("expected Integer(2)"),
                }
                match &b.data[1] {
                    SendValue::Integer(n) => assert_eq!(*n, 3),
                    _ => panic!("expected Integer(3)"),
                }
            }
            _ => panic!("expected SendValue::Block"),
        }
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), "[2 3]");
    }

    // ---- Aggregate round-trip tests ----

    #[test]
    fn round_trip_map() {
        let map = MapDef::new();
        map.set(MapKey::Str(Rc::from("a")), Value::integer(1));
        map.set(MapKey::Int(2), Value::string("two"));
        let v = Value::map(map);
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_hash() {
        let hash = HashDef::new();
        hash.set(MapKey::Str(Rc::from("x")), Value::integer(10));
        hash.set(MapKey::Int(5), Value::string("five"));
        let v = Value::hash(hash);
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_vector() {
        let v = Value::vector(VectorDef::new(
            Symbol::new("integer!"),
            vec![Value::integer(1), Value::integer(2), Value::integer(3)],
        ));
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_image() {
        let v = Value::image(ImageDef::new(2, 1, vec![
            [255, 0, 0, 255],
            [0, 255, 0, 255],
        ]));
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_bitset() {
        let bs = BitsetDef::new(256);
        bs.set(65); // 'A'
        bs.set(66); // 'B'
        let v = Value::bitset(bs);
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_typeset() {
        let v = Value::typeset(TypesetDef::from_words(&["integer!", "float!"]));
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_date() {
        use chrono::NaiveDate;
        let dt = NaiveDate::from_ymd_opt(2024, 6, 29)
            .unwrap()
            .and_hms_opt(12, 30, 0)
            .unwrap();
        let v = Value::date(DateValue { dt, zone: Some(0) });
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_duration() {
        let v = Value::duration(chrono::Duration::seconds(90));
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_percent() {
        let v = Value::percent(0.5);
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_decimal() {
        use rust_decimal::Decimal;
        let v = Value::decimal(Decimal::new(314, 2));
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_unset() {
        let v = Value::Unset;
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    #[test]
    fn round_trip_closure() {
        // Build a minimal closure with a body and one capture.
        let mut func = FuncDef::default();
        func.params.push(Symbol::new("x"));
        func.body = Series::new(vec![Value::word("x")]);
        func.ctx.set(Symbol::new("x"), Value::None);
        let captures = Rc::new(vec![RefCell::new(Value::integer(42))]);
        let v = Value::closure(Rc::new(func), captures);
        let sv = v.marshal_send().unwrap();
        match &sv {
            SendValue::Closure(cl) => {
                assert_eq!(cl.captures.len(), 1);
                match &cl.captures[0] {
                    SendValue::Integer(n) => assert_eq!(*n, 42),
                    _ => panic!("expected Integer capture"),
                }
            }
            _ => panic!("expected SendValue::Closure"),
        }
        let back = sv.unmarshal();
        match back {
            Value::Closure(cl) => {
                assert_eq!(cl.captures.len(), 1);
                match &*cl.captures[0].borrow() {
                    Value::Integer { n, .. } => assert_eq!(*n, 42),
                    _ => panic!("expected Integer capture"),
                }
            }
            _ => panic!("expected Value::Closure"),
        }
    }

    // ---- Module round-trip ----

    #[test]
    fn round_trip_module() {
        let mut m = ModuleDef::new();
        m.ctx.set(Symbol::new("foo"), Value::integer(1));
        m.exports.borrow_mut().insert(Symbol::new("foo"));
        m.name = Some(Symbol::new("mymod"));
        let v = Value::module(m);
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }

    // ---- Port round-trip (state is lossy) ----

    #[test]
    fn round_trip_port() {
        let port = PortDef::new(PortScheme::File, Rc::from("test.txt"));
        let v = Value::port(port);
        let sv = v.marshal_send().unwrap();
        let back = sv.unmarshal();
        assert_eq!(mold_to_string(&back), mold_to_string(&v));
    }
}
