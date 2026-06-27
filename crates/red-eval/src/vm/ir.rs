//! IR re-exports. The actual type definitions live in `red-core::vm_ir` so
//! `FuncDef.compiled` (in `red-core`) can name `CompiledBlock` without a
//! crate dependency cycle — same pattern as `Env`/`EvalError`.

pub use red_core::vm_ir::{disasm, CompiledBlock, Frame, Instr};
