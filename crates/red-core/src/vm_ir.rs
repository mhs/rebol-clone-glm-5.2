//! Bytecode VM IR types: `Instr`, `CompiledBlock`, `Frame`.
//!
//! v0.3 (Milestone 22) scope: types only — no compilation or execution. These
//! live in `red-core` (not `red-eval/src/vm`) so `FuncDef.compiled` can
//! reference `CompiledBlock` without a crate dependency cycle, mirroring how
//! `Env`/`EvalError` already live in `red-core` for the same reason. The VM
//! *machinery* (compiler, runtime, frame stack) stays in `red-eval::vm`.
//!
//! Nothing here is exercised at runtime yet — M23 (lexical analyzer) populates
//! `FuncDef::freevars`, M24 (compiler) emits `Instr` streams, and M25 (VM)
//! dispatches them. M22 just lays the type foundation so the value model
//! (`FuncDef`) can carry a compiled-cache slot.

use std::rc::Rc;

use crate::value::{FuncDef, Span, Symbol, Value};

/// A single bytecode instruction. Variants use `u32` indices (into the block's
/// `pool` or the function-locals vector) to keep the enum compact.
///
/// Variant groups (mirroring `plan3.md`'s design summary):
/// - Constants: `Const(i)` pushes `pool[i]`.
/// - Loads: `LoadLocal(depth, slot)`, `LoadGlobal(slot)`, `LoadDynamic(sym)`.
/// - Stores: `SetLocal(d, slot)`, `SetGlobal(slot)`, `SetDynamic(sym)`.
/// - Calls: `Call(native_idx, argc)`, `CallUser(func_slot, argc)`,
///   `TailCall(...)`, `TailReenter(...)` for tail-position calls.
/// - Control: `Jump(target)`, `JumpIfFalse(target)`, `Pop`, `Return`, `Halt`.
/// - Functions: `MakeFunc(spec_idx, body_idx, freevars)` builds a `FuncDef`
///   at runtime when `func`/`does`/`function` is invoked on literal-block args.
/// - Blocks: `EnterBlock`, `DropTo(n)` for nested `reduce`-style evaluation.
/// - Paths: `GetPath`, `SetPath` delegate to the M19 path resolver.
/// - Refinements: `MarkRefine(sym)` + `EndRefine` bracket a refinement's
///   args on the stack so the VM can assemble `RefineArgs` for the native.
#[derive(Clone, Debug)]
pub enum Instr {
    Const(u32),
    LoadLocal(u32, u32),
    LoadGlobal(u32),
    LoadDynamic(Symbol),
    SetLocal(u32, u32),
    SetGlobal(u32),
    SetDynamic(Symbol),
    Call(u32, u32),
    CallUser(u32, u32),
    TailCall(u32, u32),
    TailReenter(u32, u32),
    Jump(u32),
    JumpIfFalse(u32),
    Pop,
    Return,
    MakeFunc(u32, u32, Vec<Symbol>),
    EnterBlock,
    DropTo(u32),
    GetPath,
    SetPath,
    MarkRefine(Symbol),
    EndRefine,
    Halt,
}

/// A compiled block: an instruction stream plus its constant pool and
/// metadata. `Rc`-backed internally so cloning (e.g. across `MakeFunc` or
/// `CallUser`) is cheap. The `needs_rebind` flag marks blocks that must fall
/// back to the legacy tree-walker because `bind`/`use`/`make object!` mutated
/// their bindings after compilation (per M23/M27).
#[derive(Clone, Debug)]
pub struct CompiledBlock {
    pub instrs: Rc<[Instr]>,
    pub pool: Rc<[Value]>,
    pub n_locals: usize,
    pub freevars: Vec<Symbol>,
    pub source_span: Span,
    pub needs_rebind: bool,
    pub arity: usize,
}

/// A VM call frame. Pushed by `CallUser`, overwritten in place by
/// `TailCall`/`TailReenter` (constant-stack loops). `depth` is the lexical
/// distance from the top-level script frame — used by `LoadLocal`/`SetLocal`
/// to walk the frame chain to the defining scope.
#[derive(Clone, Debug)]
pub struct Frame {
    pub func: Option<Rc<FuncDef>>,
    pub locals: Vec<Value>,
    pub depth: usize,
    pub block: CompiledBlock,
    pub pc: usize,
}

/// Format a `CompiledBlock` for debugging: one instr per line, with pool
/// values inlined for readability. Used by later-milestone disassembler tests
/// and the `--disasm` CLI flag (M31).
pub fn disasm(block: &CompiledBlock) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let pool = block.pool.as_ref();
    for (i, instr) in block.instrs.as_ref().iter().enumerate() {
        let _ = write!(out, "{i:4}: ");
        match instr {
            Instr::Const(idx) => {
                let v = pool.get(*idx as usize).map(|v| format!("{v:?}")).unwrap_or_else(|| "<bad pool idx>".into());
                let _ = writeln!(out, "Const({idx})  ; {v}");
            }
            Instr::LoadLocal(d, s) => {
                let _ = writeln!(out, "LoadLocal({d}, {s})");
            }
            Instr::LoadGlobal(s) => {
                let _ = writeln!(out, "LoadGlobal({s})");
            }
            Instr::LoadDynamic(sym) => {
                let _ = writeln!(out, "LoadDynamic({:?})", sym.as_str());
            }
            Instr::SetLocal(d, s) => {
                let _ = writeln!(out, "SetLocal({d}, {s})");
            }
            Instr::SetGlobal(s) => {
                let _ = writeln!(out, "SetGlobal({s})");
            }
            Instr::SetDynamic(sym) => {
                let _ = writeln!(out, "SetDynamic({:?})", sym.as_str());
            }
            Instr::Call(n, a) => {
                let _ = writeln!(out, "Call({n}, {a})");
            }
            Instr::CallUser(f, a) => {
                let _ = writeln!(out, "CallUser({f}, {a})");
            }
            Instr::TailCall(f, a) => {
                let _ = writeln!(out, "TailCall({f}, {a})");
            }
            Instr::TailReenter(f, a) => {
                let _ = writeln!(out, "TailReenter({f}, {a})");
            }
            Instr::Jump(t) => {
                let _ = writeln!(out, "Jump({t})");
            }
            Instr::JumpIfFalse(t) => {
                let _ = writeln!(out, "JumpIfFalse({t})");
            }
            Instr::Pop => {
                let _ = writeln!(out, "Pop");
            }
            Instr::Return => {
                let _ = writeln!(out, "Return");
            }
            Instr::MakeFunc(s, b, fv) => {
                let names: Vec<String> = fv.iter().map(|f| format!("{:?}", f.as_str())).collect();
                let _ = writeln!(out, "MakeFunc({s}, {b}, [{}])", names.join(", "));
            }
            Instr::EnterBlock => {
                let _ = writeln!(out, "EnterBlock");
            }
            Instr::DropTo(n) => {
                let _ = writeln!(out, "DropTo({n})");
            }
            Instr::GetPath => {
                let _ = writeln!(out, "GetPath");
            }
            Instr::SetPath => {
                let _ = writeln!(out, "SetPath");
            }
            Instr::MarkRefine(sym) => {
                let _ = writeln!(out, "MarkRefine({:?})", sym.as_str());
            }
            Instr::EndRefine => {
                let _ = writeln!(out, "EndRefine");
            }
            Instr::Halt => {
                let _ = writeln!(out, "Halt");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Instr` variants derive `Debug` and format without panicking.
    #[test]
    fn instr_debug_roundtrip() {
        let instrs: Vec<Instr> = vec![
            Instr::Const(0),
            Instr::LoadLocal(0, 1),
            Instr::Call(2, 2),
            Instr::Return,
        ];
        let s = format!("{instrs:?}");
        assert!(s.contains("Const(0)"));
        assert!(s.contains("LoadLocal(0, 1)"));
        assert!(s.contains("Call(2, 2)"));
        assert!(s.contains("Return"));
    }

    /// `disasm` of a minimal block inlines the pool value and emits one line
    /// per instruction. Used by later-milestone disassembler tests.
    #[test]
    fn disasm_basic() {
        let pool: Rc<[Value]> = Rc::new([Value::integer(5)]);
        let block = CompiledBlock {
            instrs: Rc::new([Instr::Const(0), Instr::Return]),
            pool,
            n_locals: 0,
            freevars: Vec::new(),
            source_span: Span::new(0, 0),
            needs_rebind: false,
            arity: 0,
        };
        let out = disasm(&block);
        assert!(out.contains("Const(0)  ; Integer"));
        assert!(out.contains("Return"));
        assert_eq!(out.lines().count(), 2);
    }

    /// `CompiledBlock` clones cheaply: `instrs` and `pool` are `Rc`-backed, so
    /// a clone shares the allocation rather than copying. Asserted via
    /// `Rc::ptr_eq` so later milestones can rely on this property (e.g. for
    /// cache-invalidation pointer comparisons in M27).
    #[test]
    fn compiled_block_clones_cheaply() {
        let pool: Rc<[Value]> = Rc::new([Value::integer(5)]);
        let block = CompiledBlock {
            instrs: Rc::new([Instr::Const(0), Instr::Return]),
            pool,
            n_locals: 0,
            freevars: Vec::new(),
            source_span: Span::new(0, 0),
            needs_rebind: false,
            arity: 0,
        };
        let clone = block.clone();
        assert!(Rc::ptr_eq(&block.instrs, &clone.instrs));
        assert!(Rc::ptr_eq(&block.pool, &clone.pool));
    }
}
