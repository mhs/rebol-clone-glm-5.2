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
/// `pool`, the function-locals vector, or the block's side tables) to keep
/// the enum compact.
///
/// **M30.1.B:** the enum is `Copy` (no `Vec`/`Rc`/`Symbol` payloads). Variable-
/// sized payloads are table-indexed:
/// - `LoadDynamic(sym_idx)` / `SetDynamic(sym_idx)` / `MarkRefine(sym_idx)`
///   reference `CompiledBlock::symbols[sym_idx]` (a `Vec<Symbol>`).
/// - `MakeFunc(spec_idx, body_idx, freevars_idx)` references
///   `CompiledBlock::freevars_table[freevars_idx]` (a `Vec<Vec<Symbol>>`).
///
/// This shrinks the enum from ~40 bytes (bloated by `MakeFunc`'s `Vec<Symbol>`)
/// to 16 bytes (tag + u64 payload), and eliminates `Rc` refcount ops on the
/// `Symbol`-carrying variants. The dispatch loop's per-iteration `instrs[pc]`
/// read becomes a cheap bitwise copy.
///
/// Variant groups (mirroring `plan3.md`'s design summary):
/// - Constants: `Const(i)` pushes `pool[i]`. The small-value fast paths
///   `ConstInt(n)`/`ConstNone`/`ConstBool(b)` (M30) skip the pool indirection
///   for the common literal kinds, avoiding a `block_pool` lookup + `Rc`
///   clone on the hot `Const` arm. The compiler emits these in preference to
///   `Const` whenever the literal fits (see `compiler.rs::emit_const`).
/// - Loads: `LoadLocal(depth, slot)`, `LoadGlobal(slot)`, `LoadDynamic(sym_idx)`.
/// - Stores: `SetLocal(d, slot)`, `SetGlobal(slot)`, `SetDynamic(sym_idx)`.
/// - Calls: `Call(native_idx, argc)`, `CallUser(func_slot, argc)`,
///   `TailCall(...)`, `TailReenter(...)` for tail-position calls.
/// - Control: `Jump(target)`, `JumpIfFalse(target)`, `Pop`, `Return`, `Halt`.
/// - Functions: `MakeFunc(spec_idx, body_idx, freevars_idx)` builds a `FuncDef`
///   at runtime when `func`/`does`/`function` is invoked on literal-block args.
///   The freevar list is looked up from `CompiledBlock::freevars_table`.
/// - Blocks: `EnterBlock`, `DropTo(n)` for nested `reduce`-style evaluation.
/// - Paths: `GetPath`, `SetPath` delegate to the M19 path resolver.
/// - Refinements: `MarkRefine(sym_idx)` + `EndRefine` bracket a refinement's
///   args on the stack so the VM can assemble `RefineArgs` for the native.
#[derive(Clone, Copy, Debug)]
pub enum Instr {
    Const(u32),
    /// M30 small-value fast path: push `Integer(n)` without a pool lookup.
    /// Emitted by the compiler for any `Value::Integer` literal (the most
    /// common constant kind in compute-heavy loops like `fib`/`sum_loop`).
    ConstInt(i64),
    /// M30 fast path: push `None`. (`if true [...]`/`if false [...]` tails
    /// frequently leave `none` on the stack.)
    ConstNone,
    /// M30 fast path: push `Logic(b)`. (`true`/`false` literals and the
    /// results of comparison natives that the compiler can statically fold.)
    ConstBool(bool),
    LoadLocal(u32, u32),
    LoadGlobal(u32),
    /// M30.1.B: index into `CompiledBlock::symbols`. (Was `LoadDynamic(Symbol)`
    /// — the `Rc<str>` clone per dispatch iteration was a hot-path overhead.)
    LoadDynamic(u32),
    SetLocal(u32, u32),
    SetGlobal(u32),
    /// M30.1.B: index into `CompiledBlock::symbols`.
    SetDynamic(u32),
    Call(u32, u32),
    CallUser(u32, u32),
    /// M30.3.4: like `CallUser` but the func is known to be in a global slot
    /// (depth 0, `Binding::Local(user_ctx, slot)`). The VM skips the
    /// `frames.last().and_then(|f| f.locals.get(slot))` check (which always
    /// fails for globals) and calls `slot_value_unchecked` directly. Emitted
    /// by the compiler when `slot_coords` reports depth 0.
    CallUserGlobal(u32, u32),
    TailCall(u32, u32),
    TailReenter(u32, u32),
    Jump(u32),
    JumpIfFalse(u32),
    Pop,
    Return,
    /// M30.1.B: `freevars_idx` indexes into `CompiledBlock::freevars_table`.
    /// (Was `MakeFunc(u32, u32, Vec<Symbol>)` — the `Vec` bloated the enum to
    /// ~40 bytes and forced a heap alloc per `MakeFunc` emission.)
    MakeFunc(u32, u32, u32),
    EnterBlock,
    DropTo(u32),
    GetPath,
    SetPath,
    /// M30.1.B: index into `CompiledBlock::symbols`.
    MarkRefine(u32),
    EndRefine,
    Halt,
}

/// A compiled block: an instruction stream plus its constant pool, symbol
/// table, freevar table, and metadata. `Rc`-backed internally so cloning
/// (e.g. across `MakeFunc` or `CallUser`) is cheap. The `needs_rebind` flag
/// marks blocks that must fall back to the legacy tree-walker because
/// `bind`/`use`/`make object!` mutated their bindings after compilation
/// (per M23/M27).
///
/// **M30.1.B:** added `symbols` and `freevars_table` side tables so `Instr`
/// can be `Copy` (no `Vec`/`Symbol` payloads inline). Populated at compile
/// time; never mutated post-compile.
#[derive(Clone, Debug)]
pub struct CompiledBlock {
    pub instrs: Rc<[Instr]>,
    pub pool: Rc<[Value]>,
    /// M30.1.B: symbol table for `LoadDynamic`/`SetDynamic`/`MarkRefine`.
    /// Indexed by the `u32` carried by those instr variants. Populated by
    /// the compiler when it emits a dynamic-binding instr.
    /// M30.2.E: `Rc`-backed so `CompiledBlock::clone` is cheap (one Rc bump,
    /// not a `Vec` alloc). The Tier 2.E loop natives clone the block per
    /// iteration, so this must be allocation-free.
    pub symbols: Rc<[Symbol]>,
    /// M30.1.B: freevar-list table for `MakeFunc`. Indexed by the `freevars_idx`
    /// carried by `MakeFunc`. Each entry is the `Vec<Symbol>` freevar capture
    /// list for one `MakeFunc` emission. M30.2.E: `Rc`-backed (same reason
    /// as `symbols`).
    pub freevars_table: Rc<[Vec<Symbol>]>,
    pub n_locals: usize,
    pub freevars: Vec<Symbol>,
    pub source_span: Span,
    /// M31: per-instr source span, parallel to `instrs`. Each entry is the
    /// `Span` of the source value that produced the corresponding `Instr`.
    /// Synthesized instrs (the trailing `Return`, `Jump` patch targets, the
    /// `ConstNone` false-branch push) inherit the nearest source-value span
    /// or fall back to `source_span`. Used by `disasm` to annotate each line
    /// with `file:line:col`, and by the VM to localize `EvalError`s to the
    /// offending instr rather than the whole block.
    pub spans: Rc<[Span]>,
    pub needs_rebind: bool,
    pub arity: usize,
}

/// A VM call frame. Pushed by `CallUser`, overwritten in place by
/// `TailCall`/`TailReenter` (constant-stack loops). `depth` is the lexical
/// distance from the top-level script frame — used by `LoadLocal`/`SetLocal`
/// to walk the frame chain to the defining scope.
///
/// M30.3.1: `block` is `Rc<CompiledBlock>` (was owned `CompiledBlock`). This
/// makes `CallUser`'s frame push a single `Rc` bump (was 4 Rc bumps + 1
/// `Vec<Symbol>` alloc for the `freevars` field via `#[derive(Clone)]`).
/// `Return`'s frame pop drops one `Rc` (was 4 decrements + 1 Vec drop).
/// `refresh_cache` clones one `Rc` (was 4 bumps + 1 Vec alloc).
#[derive(Clone, Debug)]
pub struct Frame {
    pub func: Option<Rc<FuncDef>>,
    pub locals: Vec<Value>,
    pub depth: usize,
    pub block: Rc<CompiledBlock>,
    pub pc: usize,
}

/// Format a `CompiledBlock` for debugging: one instr per line, with pool
/// values and symbol-table entries inlined for readability. Used by the
/// `--disasm` CLI flag (M31) and disassembler tests. Equivalent to
/// [`disasm_with_spans`] called with `src = None` and `file = None` (no
/// source-position annotation).
pub fn disasm(block: &CompiledBlock) -> String {
    disasm_with_spans(block, None, None)
}

/// M31: like [`disasm`] but annotates each instr line with its source
/// position (`file:line:col` or `line:col`), read from `block.spans` (the
/// per-instr span table populated by the compiler). When `src` is `None`
/// (or a span is the zero default), no position is emitted and the line is
/// identical to [`disasm`]'s output.
///
/// `file`: the source file path (`Some("examples/fib.red")`) or `None` for
/// the REPL / unnamed source. When `Some`, the path is prepended to the
/// `line:col`. `src`: the source text the block was compiled from, used to
/// build a `LineMap` translating byte offsets to `line:col`. Both are
/// optional so a caller with only the `CompiledBlock` (e.g. an inline test)
/// still gets the unannotated form.
pub fn disasm_with_spans(block: &CompiledBlock, src: Option<&str>, file: Option<&str>) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let pool = block.pool.as_ref();
    let line_map = src.map(crate::source::LineMap::new);
    let spans = block.spans.as_ref();
    for (i, instr) in block.instrs.as_ref().iter().enumerate() {
        // M31: emit the source-position prefix (if available) before the
        // instr text. Format: `  [file:line:col]` or `  [line:col]`. When
        // the span is default (zero) or no `src` was provided, emit a blank
        // prefix of the same width so the instr column stays aligned.
        let pos_prefix = position_prefix(spans.get(i).copied(), line_map.as_ref(), file);
        let _ = write!(out, "{i:4}: {pos_prefix}");
        match instr {
            Instr::Const(idx) => {
                let v = pool
                    .get(*idx as usize)
                    .map(|v| format!("{v:?}"))
                    .unwrap_or_else(|| "<bad pool idx>".into());
                let _ = writeln!(out, "Const({idx})  ; {v}");
            }
            Instr::ConstInt(n) => {
                let _ = writeln!(out, "ConstInt({n})");
            }
            Instr::ConstNone => {
                let _ = writeln!(out, "ConstNone");
            }
            Instr::ConstBool(b) => {
                let _ = writeln!(out, "ConstBool({b})");
            }
            Instr::LoadLocal(d, s) => {
                let _ = writeln!(out, "LoadLocal({d}, {s})");
            }
            Instr::LoadGlobal(s) => {
                let _ = writeln!(out, "LoadGlobal({s})");
            }
            Instr::LoadDynamic(idx) => {
                let sym = block
                    .symbols
                    .get(*idx as usize)
                    .map(|s| format!("{:?}", s.as_str()))
                    .unwrap_or_else(|| "<bad sym idx>".into());
                let _ = writeln!(out, "LoadDynamic({idx})  ; {sym}");
            }
            Instr::SetLocal(d, s) => {
                let _ = writeln!(out, "SetLocal({d}, {s})");
            }
            Instr::SetGlobal(s) => {
                let _ = writeln!(out, "SetGlobal({s})");
            }
            Instr::SetDynamic(idx) => {
                let sym = block
                    .symbols
                    .get(*idx as usize)
                    .map(|s| format!("{:?}", s.as_str()))
                    .unwrap_or_else(|| "<bad sym idx>".into());
                let _ = writeln!(out, "SetDynamic({idx})  ; {sym}");
            }
            Instr::Call(n, a) => {
                let _ = writeln!(out, "Call({n}, {a})");
            }
            Instr::CallUser(f, a) => {
                let _ = writeln!(out, "CallUser({f}, {a})");
            }
            Instr::CallUserGlobal(f, a) => {
                let _ = writeln!(out, "CallUserGlobal({f}, {a})");
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
            Instr::MakeFunc(s, b, fv_idx) => {
                let fv = block
                    .freevars_table
                    .get(*fv_idx as usize)
                    .map(|v| {
                        let names: Vec<String> =
                            v.iter().map(|f| format!("{:?}", f.as_str())).collect();
                        names.join(", ")
                    })
                    .unwrap_or_else(|| "<bad fv idx>".into());
                let _ = writeln!(out, "MakeFunc({s}, {b}, [{fv}])");
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
            Instr::MarkRefine(idx) => {
                let sym = block
                    .symbols
                    .get(*idx as usize)
                    .map(|s| format!("{:?}", s.as_str()))
                    .unwrap_or_else(|| "<bad sym idx>".into());
                let _ = writeln!(out, "MarkRefine({idx})  ; {sym}");
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

/// M31: build the `file:line:col` (or `line:col`, or blank) prefix for a
/// disasm line. Returns a fixed-width string so the instr column stays
/// aligned regardless of whether a position is available. When `span` is
/// `None` or `Span::default()`, or `line_map` is `None`, returns a blank
/// prefix of `position_prefix_width` spaces (so the line starts at the same
/// column as an annotated line).
fn position_prefix(
    span: Option<crate::value::Span>,
    line_map: Option<&crate::source::LineMap>,
    file: Option<&str>,
) -> String {
    const WIDTH: usize = 24;
    let Some(s) = span else {
        return " ".repeat(WIDTH);
    };
    if s.is_default() {
        return " ".repeat(WIDTH);
    }
    let Some(map) = line_map else {
        return " ".repeat(WIDTH);
    };
    let (line, col) = map.line_col(s.start);
    let pos = match file {
        Some(path) => format!("[{path}:{line}:{col}]"),
        None => format!("[{line}:{col}]"),
    };
    if pos.len() >= WIDTH {
        format!("{pos} ")
    } else {
        format!("{pos:<WIDTH$}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Instr` variants derive `Debug` and format without panicking.
    #[test]
    fn instr_debug_roundtrip() {
        let instrs: Vec<Instr> = vec![
            Instr::Const(0),
            Instr::ConstInt(42),
            Instr::ConstNone,
            Instr::ConstBool(true),
            Instr::LoadLocal(0, 1),
            Instr::Call(2, 2),
            Instr::Return,
        ];
        let s = format!("{instrs:?}");
        assert!(s.contains("Const(0)"));
        assert!(s.contains("ConstInt(42)"));
        assert!(s.contains("ConstNone"));
        assert!(s.contains("ConstBool(true)"));
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
            symbols: Rc::from(&Vec::<Symbol>::new()[..]),
            freevars_table: Rc::from(&Vec::<Vec<Symbol>>::new()[..]),
            n_locals: 0,
            freevars: Vec::new(),
            source_span: Span::new(0, 0),
            spans: Rc::from(&[Span::new(0, 0), Span::new(0, 0)][..]),
            needs_rebind: false,
            arity: 0,
        };
        let out = disasm(&block);
        assert!(out.contains("Const(0)  ; Integer"));
        assert!(out.contains("Return"));
        assert_eq!(out.lines().count(), 2);
    }

    /// M31: `disasm_with_spans` annotates each line with `file:line:col`
    /// when given a source string and file path. The span table is parallel
    /// to `instrs`; a non-default span produces a position prefix.
    #[test]
    fn disasm_with_spans_annotates_lines() {
        let pool: Rc<[Value]> = Rc::new([Value::integer(5)]);
        // Two instrs with spans pointing into "x: 5" (offset 3..4 = the `5`).
        let block = CompiledBlock {
            instrs: Rc::new([Instr::Const(0), Instr::Return]),
            pool,
            symbols: Rc::from(&Vec::<Symbol>::new()[..]),
            freevars_table: Rc::from(&Vec::<Vec<Symbol>>::new()[..]),
            n_locals: 0,
            freevars: Vec::new(),
            source_span: Span::new(3, 4),
            spans: Rc::from(&[Span::new(3, 4), Span::new(3, 4)][..]),
            needs_rebind: false,
            arity: 0,
        };
        let out = disasm_with_spans(&block, Some("x: 5"), Some("test.red"));
        // The Const line should carry the file:line:col prefix.
        assert!(
            out.contains("test.red:1:4"),
            "expected position prefix in disasm output; got:\n{out}"
        );
        assert!(out.contains("Const(0)  ; Integer"));
        assert!(out.contains("Return"));
    }

    /// M31: `disasm` (the no-source wrapper) emits no position prefix —
    /// identical to the pre-M31 format. Confirms the wrapper delegates
    /// correctly.
    #[test]
    fn disasm_no_source_has_no_position_prefix() {
        let pool: Rc<[Value]> = Rc::new([Value::integer(5)]);
        let block = CompiledBlock {
            instrs: Rc::new([Instr::Const(0), Instr::Return]),
            pool,
            symbols: Rc::from(&Vec::<Symbol>::new()[..]),
            freevars_table: Rc::from(&Vec::<Vec<Symbol>>::new()[..]),
            n_locals: 0,
            freevars: Vec::new(),
            source_span: Span::new(3, 4),
            spans: Rc::from(&[Span::new(3, 4), Span::new(3, 4)][..]),
            needs_rebind: false,
            arity: 0,
        };
        let out = disasm(&block);
        // No `[...]` position prefix should appear.
        assert!(
            !out.contains("]  Const"),
            "unexpected position prefix: {out}"
        );
        assert!(out.contains("Const(0)  ; Integer"));
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
            symbols: Rc::from(&Vec::<Symbol>::new()[..]),
            freevars_table: Rc::from(&Vec::<Vec<Symbol>>::new()[..]),
            n_locals: 0,
            freevars: Vec::new(),
            source_span: Span::new(0, 0),
            spans: Rc::from(&[Span::new(0, 0), Span::new(0, 0)][..]),
            needs_rebind: false,
            arity: 0,
        };
        let clone = block.clone();
        assert!(Rc::ptr_eq(&block.instrs, &clone.instrs));
        assert!(Rc::ptr_eq(&block.pool, &clone.pool));
    }

    /// M30.1.B: `Instr` is `Copy` (no `Vec`/`Symbol` payloads inline). This
    /// test confirms the enum is `Copy` — if a future variant adds a non-Copy
    /// payload, this fails to compile.
    #[test]
    fn instr_is_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<Instr>();
    }
}
