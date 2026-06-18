//! Context: an ordered `Symbol → slot index` map plus a `Vec` of slots.
//!
//! The `names` map is populated during the binding pass (or via `set` at
//! runtime) and frozen once shared via `Rc<Context>`. Slot *contents* remain
//! mutable through their `RefCell` wrappers, so writes during eval flow
//! through `set_slot`/`slot_value` on a shared `&Rc<Context>`.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::value::{Symbol, Value};

/// A word context: ordered name → slot map plus a slot vector. Self-referential
/// in general (a slot can hold a `Value` that references the same context),
/// which is fine because slots are behind `RefCell`.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub names: HashMap<Symbol, usize>,
    pub slots: Vec<RefCell<Value>>,
}

impl Context {
    /// Empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate (or reuse) a slot for `sym` and return its index. Mutates
    /// `names`/`slots`; only callable while you own `&mut Context` (i.e.
    /// before wrapping in `Rc` for sharing).
    pub fn slot_index(&mut self, sym: Symbol) -> usize {
        if let Some(&idx) = self.names.get(&sym) {
            return idx;
        }
        let idx = self.slots.len();
        self.slots.push(RefCell::new(Value::None));
        self.names.insert(sym, idx);
        idx
    }

    /// True if `sym` has a slot in this context.
    pub fn has(&self, sym: &Symbol) -> bool {
        self.names.contains_key(sym)
    }

    /// Slot index for `sym` if present.
    pub fn index_of(&self, sym: &Symbol) -> Option<usize> {
        self.names.get(sym).copied()
    }

    /// Look up `sym` and clone its slot value. `None` if the name is unknown.
    pub fn get(&self, sym: &Symbol) -> Option<Value> {
        let idx = *self.names.get(sym)?;
        Some(self.slots[idx].borrow().clone())
    }

    /// Allocate (or reuse) a slot for `sym` and return a mutable reference
    /// to its `RefCell`. Mutates `names`/`slots`; only callable while you
    /// own `&mut Context` (i.e. before wrapping in `Rc` for sharing).
    pub fn slot_mut(&mut self, sym: Symbol) -> &mut RefCell<Value> {
        let idx = self.slot_index(sym);
        &mut self.slots[idx]
    }

    /// Allocate (if needed) and write `val` into `sym`'s slot. Mutates
    /// `names`/`slots`; only callable with `&mut self` (pre-`Rc` sharing).
    pub fn set(&mut self, sym: Symbol, val: Value) {
        let idx = self.slot_index(sym);
        self.slots[idx] = RefCell::new(val);
    }

    /// Read a slot by index (clones the value). Used by `Binding::Local`
    /// resolution during eval.
    pub fn slot_value(&self, idx: usize) -> Value {
        self.slots[idx].borrow().clone()
    }

    /// Write a slot by index via `RefCell`. Safe to call on a shared
    /// `&Rc<Context>` — only slot contents change, never the name map.
    pub fn set_slot(&self, idx: usize, val: Value) {
        *self.slots[idx].borrow_mut() = val;
    }
}
