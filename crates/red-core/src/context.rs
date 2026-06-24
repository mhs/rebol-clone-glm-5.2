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

use crate::value::{Symbol, Value};

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

    /// Write a slot by index via `RefCell`. Safe to call on a shared
    /// `&Rc<Context>` — only slot contents change, never the name map.
    pub fn set_slot(&self, idx: usize, val: Value) {
        *self.slots.borrow()[idx].borrow_mut() = val;
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
}
