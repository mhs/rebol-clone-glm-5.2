//! Bytecode VM (v0.3): compiler + stack machine.
//!
//! Since M29 the VM is the default evaluator (`EvalMode::Vm`), wired into
//! `interp.rs` via `dispatch_block` (compile-on-demand + `vm::run`, with a
//! fallback to the tree-walker for `needs_rebind` / foreign-bound blocks).
//! The CLI `--walk` flag or the `force-walk` cargo feature override to
//! `EvalMode::Walk` for debugging and the golden parity baseline.
//!
//! The IR types (`CompiledBlock` / `Frame` / `Instr`) live in
//! `red-core::vm_ir` so `FuncDef.compiled` can reference them without a
//! crate cycle. This module holds the compiler (`compiler.rs`), the lexical
//! analysis / scope resolution (`lex.rs`), the constant pool (`pool.rs`),
//! and the runtime stack machine (`vm.rs`).

pub mod compiler;
pub mod lex;
pub mod pool;
// `module_inception`: the inner `vm` module is the runtime stack machine;
// the outer `vm` module groups the VM subsystem (compiler/lex/pool/runtime).
// Renaming would break the documented `red_eval::vm::run` public path.
#[allow(clippy::module_inception)]
pub mod vm;

pub use lex::{analyze_block, AnalysisResult, Scope};
pub use vm::{run, run_reduce};
