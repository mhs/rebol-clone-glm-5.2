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

use std::cell::RefCell;
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
/// Variant groups (mirroring `docs/plans/plan3.md`'s design summary):
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
    /// M60: build a `Value::Closure` at runtime. Like `MakeFunc` but also
    /// snapshots the free-variable *values* into a `ClosureDef.captures`
    /// cell. `captures_idx` indexes into `CompiledBlock::captures_table`
    /// (a `Vec<Vec<(Symbol, usize, usize)>>` — each entry is
    /// `(freevar_name, depth, slot)` so the VM can read
    /// `self.frames[len-1-depth].locals[slot]` at `MakeClosure` time).
    MakeClosure(u32, u32, u32),
    /// M60: read `captures[idx]` of the current frame's closure cell. The
    /// frame's `captures` field is `Some` iff the frame was pushed by a
    /// `Value::Closure` call. Emits for body words with `Binding::Closure(idx)`.
    LoadCapture(u32),
    /// M60: write `captures[idx]` of the current frame's closure cell.
    SetCapture(u32),
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
    /// M60: captures table for `MakeClosure`. Indexed by the `captures_idx`
    /// carried by `MakeClosure`. Each entry is the per-closure
    /// `Vec<(Symbol, depth, slot)>` capture list: for each freevar, the
    /// `(depth, slot)` to read from the current frame chain at snapshot time
    /// (`self.frames[len-1-depth].locals[slot]`). The `Symbol` is kept for
    /// `bind_closure_body` (which keys on name) and for disasm readability.
    /// `Rc`-backed (same reason as `symbols`/`freevars_table`).
    pub captures_table: Rc<[Vec<(Symbol, usize, usize)>]>,
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
    /// M60: closure capture cell, present iff the frame was pushed by a
    /// `Value::Closure` call (`call_user`/`call_user_global` set this to
    /// `Some(Rc::clone(&cd.captures))`). `LoadCapture`/`SetCapture` read/write
    /// this; `None` for plain `func` frames (those instrs never execute on a
    /// plain-func frame — the body's freevar words have `Binding::Lexical` /
    /// `Binding::Local`, not `Binding::Closure`).
    pub captures: Option<Rc<Vec<RefCell<Value>>>>,
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
            Instr::MakeClosure(s, b, cap_idx) => {
                // M60: show the captures list (sym @ depth:slot) for readability.
                let caps = block
                    .captures_table
                    .get(*cap_idx as usize)
                    .map(|v| {
                        let names: Vec<String> = v
                            .iter()
                            .map(|(sym, d, slot)| format!("{:?}@{}:{}", sym.as_str(), d, slot))
                            .collect();
                        names.join(", ")
                    })
                    .unwrap_or_else(|| "<bad captures idx>".into());
                let _ = writeln!(out, "MakeClosure({s}, {b}, [{caps}])");
            }
            Instr::LoadCapture(idx) => {
                let _ = writeln!(out, "LoadCapture({idx})");
            }
            Instr::SetCapture(idx) => {
                let _ = writeln!(out, "SetCapture({idx})");
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
            captures_table: Rc::from(&Vec::<Vec<(Symbol, usize, usize)>>::new()[..]),
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
            captures_table: Rc::from(&Vec::<Vec<(Symbol, usize, usize)>>::new()[..]),
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
            captures_table: Rc::from(&Vec::<Vec<(Symbol, usize, usize)>>::new()[..]),
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
            captures_table: Rc::from(&Vec::<Vec<(Symbol, usize, usize)>>::new()[..]),
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

    // -------------------------------------------------------------------------
    // M135: coverage-focused disasm tests. The original suite exercised only
    // ~7 of the 28 `Instr` variants; these build `CompiledBlock`s by hand to
    // drive every match arm + the `<bad idx>` fallbacks + `position_prefix`
    // short-circuits. No VM/compiler/runtime is involved — `disasm` is pure
    // string formatting over the IR types.
    // -------------------------------------------------------------------------

    /// Helper: build a `CompiledBlock` with the given instrs and side tables
    /// sized so every index is in range. Spans default to `Span::default()`
    /// so the position prefix is blank (matches `disasm`'s no-source form).
    fn block_with(
        instrs: &[Instr],
        pool: &[Value],
        symbols: &[&str],
        freevars_table: &[Vec<&str>],
        captures_table: &[Vec<(String, usize, usize)>],
    ) -> CompiledBlock {
        let pool: Rc<[Value]> = Rc::from(pool);
        let symbols: Rc<[Symbol]> =
            Rc::from(symbols.iter().map(|s| Symbol::new(s)).collect::<Vec<_>>());
        let freevars_table: Rc<[Vec<Symbol>]> = Rc::from(
            freevars_table
                .iter()
                .map(|v| v.iter().map(|s| Symbol::new(s)).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
        );
        let captures_table: Rc<[Vec<(Symbol, usize, usize)>]> = Rc::from(
            captures_table
                .iter()
                .map(|v| {
                    v.iter()
                        .map(|(s, d, slot)| (Symbol::new(s), *d, *slot))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>(),
        );
        let n = instrs.len();
        CompiledBlock {
            instrs: Rc::from(instrs),
            pool,
            symbols,
            freevars_table,
            captures_table,
            n_locals: 0,
            freevars: Vec::new(),
            source_span: Span::new(0, 0),
            spans: Rc::from(vec![Span::new(0, 0); n]),
            needs_rebind: false,
            arity: 0,
        }
    }

    /// One of every `Instr` variant: `disasm` must emit a line for each, with
    /// the documented mnemonic. Drives all 28 match arms in `disasm_with_spans`
    /// (the file/file-with-spans forms are equivalent when spans are default —
    /// the position prefix is blank either way).
    #[test]
    fn disasm_renders_every_instr_variant() {
        let instrs: Vec<Instr> = vec![
            Instr::Const(0),
            Instr::ConstInt(42),
            Instr::ConstNone,
            Instr::ConstBool(true),
            Instr::LoadLocal(0, 1),
            Instr::LoadGlobal(2),
            Instr::LoadDynamic(0),
            Instr::SetLocal(0, 1),
            Instr::SetGlobal(2),
            Instr::SetDynamic(0),
            Instr::Call(3, 2),
            Instr::CallUser(0, 1),
            Instr::CallUserGlobal(1, 1),
            Instr::TailCall(0, 1),
            Instr::TailReenter(0, 1),
            Instr::Jump(5),
            Instr::JumpIfFalse(5),
            Instr::Pop,
            Instr::Return,
            Instr::MakeFunc(0, 1, 0),
            Instr::MakeClosure(0, 1, 0),
            Instr::LoadCapture(0),
            Instr::SetCapture(0),
            Instr::EnterBlock,
            Instr::DropTo(0),
            Instr::GetPath,
            Instr::SetPath,
            Instr::MarkRefine(0),
            Instr::EndRefine,
            Instr::Halt,
        ];
        let block = block_with(
            &instrs,
            &[Value::integer(5)],
            &["foo"],
            &[vec!["x", "y"]],
            &[vec![("z".to_string(), 1, 2)]],
        );
        let out = disasm(&block);
        // Every mnemonic must appear. Index-armed variants render their args.
        for needle in [
            "Const(0)  ; Integer",
            "ConstInt(42)",
            "ConstNone",
            "ConstBool(true)",
            "LoadLocal(0, 1)",
            "LoadGlobal(2)",
            "LoadDynamic(0)  ; \"foo\"",
            "SetLocal(0, 1)",
            "SetGlobal(2)",
            "SetDynamic(0)  ; \"foo\"",
            "Call(3, 2)",
            "CallUser(0, 1)",
            "CallUserGlobal(1, 1)",
            "TailCall(0, 1)",
            "TailReenter(0, 1)",
            "Jump(5)",
            "JumpIfFalse(5)",
            "Pop",
            "Return",
            "MakeFunc(0, 1, [\"x\", \"y\"])",
            "MakeClosure(0, 1, [\"z\"@1:2])",
            "LoadCapture(0)",
            "SetCapture(0)",
            "EnterBlock",
            "DropTo(0)",
            "GetPath",
            "SetPath",
            "MarkRefine(0)  ; \"foo\"",
            "EndRefine",
            "Halt",
        ] {
            assert!(
                out.contains(needle),
                "disasm missing {needle:?}\nfull output:\n{out}"
            );
        }
        assert_eq!(out.lines().count(), instrs.len());
    }

    /// Out-of-range pool index → `<bad pool idx>` fallback (covers the
    /// `unwrap_or_else` branch on the `Const` arm).
    #[test]
    fn disasm_bad_pool_idx_fallback() {
        let block = block_with(&[Instr::Const(99)], &[], &[], &[], &[]);
        let out = disasm(&block);
        assert!(out.contains("Const(99)  ; <bad pool idx>"), "got: {out}");
    }

    /// Out-of-range symbol index → `<bad sym idx>` fallback. Covers the
    /// `LoadDynamic`/`SetDynamic`/`MarkRefine` lookup branches.
    #[test]
    fn disasm_bad_sym_idx_fallback() {
        let block = block_with(
            &[Instr::LoadDynamic(7), Instr::SetDynamic(7), Instr::MarkRefine(7)],
            &[],
            &[],
            &[],
            &[],
        );
        let out = disasm(&block);
        assert!(out.contains("LoadDynamic(7)  ; <bad sym idx>"), "got: {out}");
        assert!(out.contains("SetDynamic(7)  ; <bad sym idx>"), "got: {out}");
        assert!(out.contains("MarkRefine(7)  ; <bad sym idx>"), "got: {out}");
    }

    /// Out-of-range freevars/captures index → `<bad fv idx>` / `<bad captures
    /// idx>` fallback. Covers the `MakeFunc`/`MakeClosure` lookup branches.
    #[test]
    fn disasm_bad_fv_and_captures_idx_fallback() {
        let block = block_with(
            &[Instr::MakeFunc(0, 0, 9), Instr::MakeClosure(0, 0, 9)],
            &[],
            &[],
            &[],
            &[],
        );
        let out = disasm(&block);
        assert!(out.contains("MakeFunc(0, 0, [<bad fv idx>])"), "got: {out}");
        assert!(
            out.contains("MakeClosure(0, 0, [<bad captures idx>])"),
            "got: {out}"
        );
    }

    /// `position_prefix` short-circuits: a `None` span, a default span, a
    /// missing `line_map`, and an overlong position all return a blank or
    /// un-truncated prefix of the right shape. Built via `disasm_with_spans`
    /// with carefully chosen span/src/file inputs.
    #[test]
    fn position_prefix_short_circuits() {
        // (a) None span: spans table shorter than instrs → `spans.get(i)` is
        //     `None`. Disasm must still emit the instr (with a blank prefix).
        let pool: Rc<[Value]> = Rc::new([Value::integer(5)]);
        let block = CompiledBlock {
            instrs: Rc::new([Instr::Const(0)]),
            pool,
            symbols: Rc::from(&Vec::<Symbol>::new()[..]),
            freevars_table: Rc::from(&Vec::<Vec<Symbol>>::new()[..]),
            captures_table: Rc::from(&Vec::<Vec<(Symbol, usize, usize)>>::new()[..]),
            n_locals: 0,
            freevars: Vec::new(),
            source_span: Span::new(0, 0),
            // Empty spans table: `spans.get(0)` returns `None`.
            spans: Rc::from(&Vec::<Span>::new()[..]),
            needs_rebind: false,
            arity: 0,
        };
        let out = disasm_with_spans(&block, Some("x: 5"), Some("test.red"));
        assert!(out.contains("Const(0)  ; Integer"));
        assert!(
            !out.contains("test.red:"),
            "None span should not annotate: {out}"
        );

        // (b) Default span (zero) with src+file present: `is_default()` true →
        //     blank prefix.
        let block_default = block_with(&[Instr::Const(0)], &[Value::integer(5)], &[], &[], &[]);
        let out_default = disasm_with_spans(&block_default, Some("x: 5"), Some("test.red"));
        assert!(
            !out_default.contains("test.red:"),
            "default span should not annotate: {out_default}"
        );

        // (c) Non-default span but src=None → `line_map` is None → blank prefix.
        let pool2: Rc<[Value]> = Rc::new([Value::integer(5)]);
        let block_no_src = CompiledBlock {
            instrs: Rc::new([Instr::Const(0)]),
            pool: pool2,
            symbols: Rc::from(&Vec::<Symbol>::new()[..]),
            freevars_table: Rc::from(&Vec::<Vec<Symbol>>::new()[..]),
            captures_table: Rc::from(&Vec::<Vec<(Symbol, usize, usize)>>::new()[..]),
            n_locals: 0,
            freevars: Vec::new(),
            source_span: Span::new(3, 4),
            spans: Rc::from(&[Span::new(3, 4)][..]),
            needs_rebind: false,
            arity: 0,
        };
        let out_no_src = disasm_with_spans(&block_no_src, None, Some("test.red"));
        assert!(
            !out_no_src.contains("test.red:"),
            "missing line_map should not annotate: {out_no_src}"
        );
        assert!(out_no_src.contains("Const(0)  ; Integer"));

        // (d) Overlong file path: position longer than WIDTH (24) hits the
        //     `pos.len() >= WIDTH` arm and emits `pos` + a single space (no
        //     left-pad). A short file path takes the `format!({pos:<WIDTH})`
        //     arm. Both must still prefix the instr line.
        let long_file = "a-very-long-source-file-name-here.red";
        let out_long = disasm_with_spans(&block_no_src, Some("x: 5"), Some(long_file));
        assert!(
            out_long.contains(&format!("{long_file}:1:4")),
            "overlong path should still annotate: {out_long}"
        );
        let short_out = disasm_with_spans(&block_no_src, Some("x: 5"), Some("t.red"));
        assert!(
            short_out.contains("t.red:1:4"),
            "short path should annotate: {short_out}"
        );
    }
}
