//! Constant-pool builder for the bytecode VM (M24).
//!
//! A thin append-only `Vec<Value>` indexed by `u32`. No deduplication — every
//! literal gets its own pool slot. The compiler (`compiler.rs`) uses this to
//! intern `Const(i)` operands; M25's `Const(i)` instr pushes `pool[i]`.
//!
//! Dedup is intentionally omitted for M24: `Value` has no `PartialEq`/`Hash`,
//! so dedup would require a custom key type, and the plan3 checklist tests
//! (`pool=[5]`, `pool=[true]`) don't require it. Profile-guided dedup can land
//! in M30 if a hot path benefits.

use std::rc::Rc;

use red_core::value::Value;

/// Append-only constant pool. Push returns the `u32` index a `Const` instr
/// carries; `into_rc` freezes the pool into the `Rc<[Value]>` shape stored on
/// `CompiledBlock`.
pub struct ConstantPool {
    values: Vec<Value>,
}

impl ConstantPool {
    pub fn new() -> Self {
        Self { values: Vec::new() }
    }

    /// Push `v` onto the pool, returning its index. No dedup.
    pub fn push(&mut self, v: Value) -> u32 {
        let idx = self.values.len() as u32;
        self.values.push(v);
        idx
    }

    /// Freeze into an `Rc<[Value]>` for storage on `CompiledBlock.pool`.
    pub fn into_rc(self) -> Rc<[Value]> {
        Rc::from(self.values)
    }
}

impl Default for ConstantPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_push_returns_increasing_indices() {
        let mut pool = ConstantPool::new();
        assert_eq!(pool.push(Value::integer(1)), 0);
        assert_eq!(pool.push(Value::integer(2)), 1);
        assert_eq!(pool.push(Value::integer(3)), 2);
    }

    #[test]
    fn pool_into_rc_preserves_order() {
        let mut pool = ConstantPool::new();
        let _ = pool.push(Value::integer(10));
        let _ = pool.push(Value::integer(20));
        let rc = pool.into_rc();
        assert_eq!(rc.len(), 2);
        assert!(matches!(rc[0], Value::Integer { n: 10, .. }));
        assert!(matches!(rc[1], Value::Integer { n: 20, .. }));
    }
}
