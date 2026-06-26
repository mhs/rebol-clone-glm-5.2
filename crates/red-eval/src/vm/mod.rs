//! Bytecode VM (v0.3): compiler + stack machine, milestone-by-milestone.
//!
//! M22 (this milestone) ships only the type foundation — the IR types live
//! in `red-core::vm_ir` (so `FuncDef.compiled` can reference them without a
//! crate cycle), and this module re-exports them. The compiler (`compiler.rs`),
//! runtime (`vm.rs`), frame manager (`frame.rs`), and constant pool (`pool.rs`)
//! are stubs here; real code lands in M24/M25.
//!
//! Nothing under `vm/` is wired into `interp.rs` yet — the tree-walker
//! remains the sole evaluator until M29 flips the default.

pub mod compiler;
pub mod frame;
pub mod ir;
pub mod lex;
pub mod pool;
pub mod vm;

pub use ir::{CompiledBlock, Frame, Instr, disasm};
pub use lex::{AnalysisResult, Scope, analyze_block};
pub use vm::run;
