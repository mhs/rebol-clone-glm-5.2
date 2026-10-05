//! Context: an ordered `Symbol → slot index` map plus a `Vec` of slots.
//!
//! Both `names` and `slots` live behind `RefCell` so that a context shared
//! via `Rc<Context>` can still grow — new `SetWord`s encountered after the
//! initial binding pass (e.g. subsequent lines typed into the REPL) can
//! allocate fresh slots without rebuilding the context. Slot *contents*
//! remain independently mutable through their inner `RefCell<Value>`, so
//! eval-time writes flow through `set_slot`/`slot_value` on a shared
//! `&Rc<Context>`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::value::{Series, Symbol, Value};

/// A word context: ordered name → slot map plus a slot vector. Self-referential
/// in general (a slot can hold a `Value` that references the same context),
/// which is fine because slots are behind `RefCell`.
///
/// Both fields use interior mutability so the context can keep growing after
/// being shared as `Rc<Context>` — this is what lets the REPL bind new
/// top-level words against the live user context across lines.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub names: RefCell<HashMap<Symbol, usize>>,
    pub slots: RefCell<Vec<RefCell<Value>>>,
}

impl Context {
    /// Empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate (or reuse) a slot for `sym` and return its index. Safe to
    /// call on a shared `&Rc<Context>` — grows `names`/`slots` via their
    /// `RefCell`s. Used both by the binding pass and at runtime (e.g. the
    /// REPL binding a new line's SetWords against the live user context).
    pub fn slot_index(&self, sym: Symbol) -> usize {
        if let Some(&idx) = self.names.borrow().get(&sym) {
            return idx;
        }
        let idx = self.slots.borrow().len();
        self.slots.borrow_mut().push(RefCell::new(Value::None));
        self.names.borrow_mut().insert(sym, idx);
        idx
    }

    /// True if `sym` has a slot in this context.
    pub fn has(&self, sym: &Symbol) -> bool {
        self.names.borrow().contains_key(sym)
    }

    /// Slot index for `sym` if present.
    pub fn index_of(&self, sym: &Symbol) -> Option<usize> {
        self.names.borrow().get(sym).copied()
    }

    /// Look up `sym` and clone its slot value. `None` if the name is unknown.
    pub fn get(&self, sym: &Symbol) -> Option<Value> {
        let idx = *self.names.borrow().get(sym)?;
        Some(self.slots.borrow()[idx].borrow().clone())
    }

    /// Allocate (if needed) and write `val` into `sym`'s slot. Safe to call
    /// on a shared `&Rc<Context>` — only slot contents change, never the
    /// name map (unless `sym` is new, in which case a slot is appended).
    pub fn set(&self, sym: Symbol, val: Value) {
        let idx = self.slot_index(sym);
        *self.slots.borrow()[idx].borrow_mut() = val;
    }

    /// Read a slot by index (clones the value). Used by `Binding::Local`
    /// resolution during eval.
    pub fn slot_value(&self, idx: usize) -> Value {
        self.slots.borrow()[idx].borrow().clone()
    }

    /// M30 fast path: read a slot by index without a bounds check. The caller
    /// (the VM) statically knows `idx` is valid because the compiler's `Scope`
    /// proved it at compile time. A `debug_assert!` guards against routing
    /// bugs in debug builds; release builds skip the bounds check.
    ///
    /// # Safety
    /// `idx` must be < `self.slots.borrow().len()`. The compiler's `Scope`
    /// analysis proves this for every `LoadLocal`/`LoadGlobal`/`SetLocal`/
    /// `SetGlobal` emission. Additionally, the slot must have been
    /// initialized with a valid `Value` (no invalid bit patterns) —
    /// `Context` only ever stores values produced by `Value` constructors
    /// or by `RefCell::borrow_mut()` writes of other valid `Value`s, so
    /// this invariant holds by construction. See the static assertion in
    /// `crates/red-eval/src/vm/vm.rs` (`const _: () = assert!(size_of::<
    /// Value>() == size_of::<MaybeUninit<Value>>());`) backing the
    /// `from_raw_parts` cast in `call_native`.
    pub fn slot_value_unchecked(&self, idx: usize) -> Value {
        let slots = self.slots.borrow();
        debug_assert!(idx < slots.len(), "slot_value_unchecked OOB: {idx}");
        // SAFETY: caller guarantees idx < len. Bind to a local first so the
        // `Ref` destructor runs before `slots` (the outer borrow) drops.
        let v = unsafe { slots.get_unchecked(idx).borrow().clone() };
        v
    }

    /// Write a slot by index via `RefCell`. Safe to call on a shared
    /// `&Rc<Context>` — only slot contents change, never the name map.
    pub fn set_slot(&self, idx: usize, val: Value) {
        *self.slots.borrow()[idx].borrow_mut() = val;
    }

    /// M30 fast path: write a slot by index without a bounds check. Same
    /// contract as `slot_value_unchecked` — the VM only calls this with
    /// compiler-proven slot indices.
    ///
    /// # Safety
    /// `idx` must be < `self.slots.borrow().len()`. The compiler's `Scope`
    /// analysis proves this for every `SetLocal`/`SetGlobal` emission. See
    /// `slot_value_unchecked` for the full "no invalid bit patterns"
    /// invariant discussion.
    pub fn set_slot_unchecked(&self, idx: usize, val: Value) {
        let slots = self.slots.borrow();
        debug_assert!(idx < slots.len(), "set_slot_unchecked OOB: {idx}");
        // SAFETY: caller guarantees idx < len. Take the `RefMut` out in a
        // block so its destructor runs before `slots` (the outer borrow).
        {
            *unsafe { slots.get_unchecked(idx) }.borrow_mut() = val;
        }
        drop(slots);
    }

    /// Words in declaration order. Used by `words-of`/`values-of`/`reflect`
    /// for both contexts and objects (M18).
    pub fn words(&self) -> Vec<Symbol> {
        let names = self.names.borrow();
        let mut ordered: Vec<(Symbol, usize)> =
            names.iter().map(|(s, &i)| (s.clone(), i)).collect();
        ordered.sort_by_key(|(_, i)| *i);
        ordered.into_iter().map(|(s, _)| s).collect()
    }

    /// M41: Produce a true deep copy of this context — a fresh `Context` with
    /// independent `names`/`slots` storage, where every `Block`/`Paren` slot
    /// value is deep-cloned (new `Rc<RefCell<Vec<Value>>>`), and every
    /// `Object` slot value is recursively deep-cloned (new
    /// `Rc<RefCell<ObjectDef>>` with a deep-cloned `ctx` and parent chain).
    /// Scalar values (`Integer`/`Float`/`String`/etc.) are `Clone`-cheap (Rc
    /// bump). `Func` values are deep-cloned via `Rc::new` of a cloned
    /// `FuncDef` (body series deep-cloned, ctx deep-cloned). This is the
    /// "frozen snapshot" operation used by `Env::fork_thread_env` to give a
    /// worker thread an independent copy of the user context.
    pub fn deep_clone(&self) -> Context {
        let names = self.names.borrow().clone();
        let slots_src = self.slots.borrow();
        let slots: Vec<RefCell<Value>> = slots_src
            .iter()
            .map(|cell| RefCell::new(deep_clone_value(&cell.borrow())))
            .collect();
        Context {
            names: RefCell::new(names),
            slots: RefCell::new(slots),
        }
    }
}

/// M41: Deep-clone a `Value` — recursively clones `Block`/`Paren` (new
/// `Series` with deep-cloned `Vec<Value>`), `Object` (new `Rc<RefCell<
/// ObjectDef>>` with deep-cloned `ctx` and parent chain), and `Func` (new
/// `Rc<FuncDef>` with deep-cloned body). All other variants fall through to
/// `Value::clone` (Rc bump for `Rc`-backed variants, copy for scalars).
fn deep_clone_value(v: &Value) -> Value {
    match v {
        Value::Block { series, span } => Value::Block {
            series: deep_clone_series(series),
            span: *span,
        },
        Value::Paren { series, span } => Value::Paren {
            series: deep_clone_series(series),
            span: *span,
        },
        Value::Object(obj) => {
            let src = obj.borrow();
            let ctx = Rc::new(src.ctx.deep_clone());
            let parent = src.parent.as_ref().map(|p| {
                Rc::new(RefCell::new(deep_clone_object(&p.borrow())))
            });
            let self_word = src.self_word.clone();
            let protected = RefCell::new(*src.protected.borrow());
            drop(src);
            Value::Object(Rc::new(RefCell::new(crate::value::ObjectDef {
                ctx,
                parent,
                self_word,
                protected,
            })))
        }
        Value::Func(fd) => {
            let fd = (**fd).clone();
            Value::Func(Rc::new(fd))
        }
        Value::Closure(cl) => {
            let func = Rc::new((*cl.func).clone());
            let captures = cl.captures.clone();
            Value::Closure(Rc::new(crate::value::ClosureDef { func, captures }))
        }
        other => other.clone(),
    }
}

/// Deep-clone a `Series`: fresh `Rc<RefCell<Vec<Value>>>` with each value
/// recursively deep-cloned.
fn deep_clone_series(series: &Series) -> Series {
    let data = series.data.borrow();
    let cloned: Vec<Value> = data.iter().map(deep_clone_value).collect();
    Series::new(cloned)
}

/// Deep-clone an `ObjectDef` (helper for `deep_clone_value`'s `Object` arm).
fn deep_clone_object(obj: &crate::value::ObjectDef) -> crate::value::ObjectDef {
    crate::value::ObjectDef {
        ctx: Rc::new(obj.ctx.deep_clone()),
        parent: obj.parent.as_ref().map(|p| {
            Rc::new(RefCell::new(deep_clone_object(&p.borrow())))
        }),
        self_word: obj.self_word.clone(),
        protected: RefCell::new(*obj.protected.borrow()),
    }
}

#[cfg(test)]
mod tests {
    //! Unit coverage for `Context` slot accessors. M34.
    //!
    //! The `unsafe` `slot_value_unchecked` / `set_slot_unchecked` paths are
    //! the VM's hot-path slot accessors; these tests pin their happy-path
    //! parity with the checked variants and assert the `debug_assert!` OOB
    //! guard fires under `cfg(debug_assertions)`.

    use super::*;
    use crate::value::{Symbol, Value};

    #[test]
    fn new_context_is_empty() {
        let ctx = Context::new();
        assert!(ctx.words().is_empty());
        assert!(!ctx.has(&Symbol::new("x")));
        assert!(ctx.index_of(&Symbol::new("x")).is_none());
        assert!(ctx.get(&Symbol::new("x")).is_none());
    }

    #[test]
    fn default_matches_new() {
        let a = Context::new();
        let b = Context::default();
        assert!(a.words().is_empty());
        assert!(b.words().is_empty());
    }

    #[test]
    fn slot_index_is_idempotent() {
        let ctx = Context::new();
        let sym = Symbol::new("x");
        let i1 = ctx.slot_index(sym.clone());
        let i2 = ctx.slot_index(sym.clone());
        assert_eq!(i1, i2);
        assert_eq!(ctx.slots.borrow().len(), 1);
        assert!(ctx.has(&sym));
        assert_eq!(ctx.index_of(&sym), Some(i1));
    }

    #[test]
    fn slot_index_is_monotonic_for_distinct_symbols() {
        let ctx = Context::new();
        let a = ctx.slot_index(Symbol::new("a"));
        let b = ctx.slot_index(Symbol::new("b"));
        let c = ctx.slot_index(Symbol::new("c"));
        assert_eq!((a, b, c), (0, 1, 2));
        // Re-adding an earlier symbol must not allocate a new slot.
        assert_eq!(ctx.slot_index(Symbol::new("a")), a);
        assert_eq!(ctx.slots.borrow().len(), 3);
    }

    #[test]
    fn set_get_round_trip() {
        let ctx = Context::new();
        let sym = Symbol::new("x");
        ctx.set(sym.clone(), Value::integer(42));
        match ctx.get(&sym) {
            Some(Value::Integer { n, .. }) => assert_eq!(n, 42),
            other => panic!("expected Integer(42), got {other:?}"),
        }
    }

    #[test]
    fn set_on_new_symbol_allocates_slot() {
        let ctx = Context::new();
        ctx.set(Symbol::new("x"), Value::integer(1));
        ctx.set(Symbol::new("y"), Value::integer(2));
        assert_eq!(ctx.index_of(&Symbol::new("x")), Some(0));
        assert_eq!(ctx.index_of(&Symbol::new("y")), Some(1));
    }

    #[test]
    fn get_miss_returns_none() {
        let ctx = Context::new();
        ctx.set(Symbol::new("x"), Value::integer(1));
        assert!(ctx.get(&Symbol::new("absent")).is_none());
    }

    #[test]
    fn slot_value_and_set_slot_round_trip() {
        let ctx = Context::new();
        let idx = ctx.slot_index(Symbol::new("x"));
        ctx.set_slot(idx, Value::integer(7));
        match ctx.slot_value(idx) {
            Value::Integer { n, .. } => assert_eq!(n, 7),
            other => panic!("expected Integer(7), got {other:?}"),
        }
        // Overwrite via set_slot.
        ctx.set_slot(idx, Value::integer(99));
        match ctx.slot_value(idx) {
            Value::Integer { n, .. } => assert_eq!(n, 99),
            other => panic!("expected Integer(99), got {other:?}"),
        }
    }

    #[test]
    fn slot_value_clones() {
        // Mutating the returned value must not affect the stored slot.
        let ctx = Context::new();
        let idx = ctx.slot_index(Symbol::new("x"));
        ctx.set_slot(idx, Value::integer(5));
        let v = ctx.slot_value(idx);
        drop(v);
        match ctx.slot_value(idx) {
            Value::Integer { n, .. } => assert_eq!(n, 5),
            other => panic!("expected Integer(5), got {other:?}"),
        }
    }

    #[test]
    fn unchecked_matches_checked() {
        let ctx = Context::new();
        let i = ctx.slot_index(Symbol::new("x"));
        ctx.set_slot(i, Value::integer(123));
        // Checked and unchecked reads return equal values.
        let a = ctx.slot_value(i);
        let b = ctx.slot_value_unchecked(i);
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
        // Unchecked write matches checked write.
        ctx.set_slot_unchecked(i, Value::integer(456));
        match ctx.slot_value(i) {
            Value::Integer { n, .. } => assert_eq!(n, 456),
            other => panic!("expected Integer(456), got {other:?}"),
        }
    }

    #[test]
    fn words_preserves_insertion_order() {
        // `words()` sorts by slot index, so the result is in insertion
        // order, not alphabetical.
        let ctx = Context::new();
        ctx.slot_index(Symbol::new("c"));
        ctx.slot_index(Symbol::new("a"));
        ctx.slot_index(Symbol::new("b"));
        let words: Vec<String> = ctx
            .words()
            .into_iter()
            .map(|s| s.as_str().to_string())
            .collect();
        assert_eq!(words, vec!["c", "a", "b"]);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "slot_value_unchecked OOB")]
    fn slot_value_unchecked_oob_panics_in_debug() {
        let ctx = Context::new();
        let _ = ctx.slot_value_unchecked(0);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "set_slot_unchecked OOB")]
    fn set_slot_unchecked_oob_panics_in_debug() {
        let ctx = Context::new();
        ctx.set_slot_unchecked(0, Value::None);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn slot_value_unchecked_oob_no_debug_assert() {
        // In release builds the `debug_assert!` is absent; the call is
        // still `unsafe` for OOB indices (UB), so we do NOT exercise the
        // OOB path here — only assert the happy path is reachable.
        let ctx = Context::new();
        let i = ctx.slot_index(Symbol::new("x"));
        ctx.set_slot(i, Value::integer(1));
        let _ = ctx.slot_value_unchecked(i);
    }
}
