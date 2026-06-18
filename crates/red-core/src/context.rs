//! Context: an ordered `Symbol → slot index` map plus a `Vec` of slots.
//!
//! Milestone 2 ships only the skeleton (`new` + fields). Slot access, lookup,
//! and mutation land in Milestone 5 alongside the evaluator.

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
}
