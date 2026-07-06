//! Block → `Instr` stream compiler (v0.3, M24).
//!
//! Walks a parsed, M23-analyzed `Series` and emits a flat `Vec<Instr>` plus a
//! constant pool, returning a [`CompiledBlock`]. The compiler is the second
//! stage of the v0.3 VM pipeline (`analyze_block` → `compile_block`); it is
//! **not wired into `interp::eval`** in M24 — it ships with its own tests
//! and is otherwise unused. M25 will dispatch `CompiledBlock`s from the VM.
//!
//! ## Emission rules (summary)
//!
//! | Source form | Emission |
//! |---|---|
//! | Literals (`Integer`/`Float`/`String`/`None`/`Logic`/`File`/`Url`/`LitWord`/`Refinement`/`Block`-as-data/`Func`/`String8`/`Error`/`Object`) | `Const(pool_idx)` |
//! | `Paren` | compile its series inline (eager), no `Return` |
//! | `Word` value-position | `LoadLocal` / `LoadGlobal` / `LoadDynamic` per binding |
//! | `Word` operator-position, native | collect args, `Call(native_idx, argc)` |
//! | `Word` operator-position, known user-func | collect args, `CallUser(slot, argc)` |
//! | `SetWord` | compile RHS, `SetLocal`/`SetGlobal`/`SetDynamic` |
//! | `GetWord` | load (no call) |
//! | `Path`/`GetPath`/`SetPath` | `GetPath`/`SetPath` (runtime M19 resolution) |
//! | `func`/`does`/`function` | `MakeFunc(spec_idx, body_idx, freevars)` |
//! | `use`/`make object!`/`object`/`context` | `needs_rebind = true`, `[Halt]` |
//! | `if cond block` (special) | `<cond>, JumpIfFalse(L), <then>, L:` |
//! | `either cond t f` (special) | `<cond>, JumpIfFalse(L_else), <t>, Jump(L_end), L_else: <f>, L_end:` |
//! | Non-tail expression (non-`SetWord`) | emits `Pop` after |
//! | Tail expression | no `Pop`; `Return` follows at block end |
//!
//! ## Non-goals (deferred)
//!
//! - Loop-body inlining (`while`/`until`/`loop`/`repeat`/`foreach`/`forall`)
//!   → generic `Call` in M24; inlining + tail-call optimization lands in M28.
//! - `compose`/`parse`/`do`/`reduce` on runtime-constructed blocks → generic
//!   `Call`; the native recurses via the walker (M26 bridges VM/walker).
//! - Pool dedup, small-value tagging → M30 if profiling warrants.
//! - `CallUser` global-vs-local disambiguation in the instr stream → M25.

use red_core::value::{Binding, FuncDef, Series, Span, Symbol, Value};
use red_core::vm_ir::{CompiledBlock, Instr};
use red_core::{CompileErrorKind, Context, Env};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::binding::{func_form_skip, use_body_index};
use crate::natives::extract_spec;
use crate::vm::lex::{analyze_block, AnalysisResult, Scope};
use crate::vm::pool::ConstantPool;

// Test-only per-thread counter of `compile_block_inner` invocations. M27
// tests assert that a func body is compiled exactly once across multiple
// calls (cache hit on the second call). Thread-local so parallel `cargo
// test` threads don't interfere. Reset via `reset_compile_counter()` before
// a test.
#[cfg(test)]
thread_local! {
    static COMPILE_COUNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_compile_counter() {
    COMPILE_COUNT.with(|c| c.set(0));
}

#[cfg(test)]
pub(crate) fn read_compile_counter() -> u32 {
    COMPILE_COUNT.with(|c| c.get())
}

// ---------------------------------------------------------------------------
// Native registry snapshot
// ---------------------------------------------------------------------------

/// A compile-time snapshot of `env.natives`: `Symbol -> (idx, FuncDef)`.
///
/// `idx` is the `u32` carried by the `Call(native_idx, argc)` instr; it's the
/// insertion order index, stable for the lifetime of one compiled block.
/// Built by [`NativeRegistry::from_env`] before `compile_block` runs; the VM
/// (M25) indexes its own `Vec<Rc<FuncDef>>` (or `env.natives` directly) by
/// the same index when dispatching `Call`.
///
/// The snapshot is taken once at the top of a compile; if `env.natives`
/// changes after compilation (natives added/removed at runtime), the compiled
/// block's indices may be stale — M27 invalidates the cache in that case.
#[derive(Debug)]
pub struct NativeRegistry {
    /// `Symbol -> (idx, FuncDef)` — `idx` matches the VM's `Call` operand.
    map: HashMap<Symbol, (u32, Rc<FuncDef>)>,
    /// M29: Snapshot of `user_ctx` for `FuncArityTable::populate_from_user_ctx`.
    /// Stored so the compiler can discover user-func arities for slots defined
    /// outside the current compile unit (e.g. a `does` func defined at the top
    /// level, referenced inside a `repeat` body compiled separately).
    user_ctx: Option<Rc<Context>>,
}

impl NativeRegistry {
    /// Build a snapshot from `env.natives`. Stable insertion order (the
    /// `HashMap` iteration order is deterministic within a single process
    /// run — for the M24 inline tests this is fine; M27's cache invalidation
    /// handles cross-run drift).
    pub fn from_env(env: &Env) -> Self {
        let mut map = HashMap::new();
        for (idx, (sym, fd)) in (0_u32..).zip(env.natives.iter()) {
            map.insert(sym.clone(), (idx, Rc::clone(fd)));
        }
        Self {
            map,
            user_ctx: Some(Rc::clone(&env.user_ctx)),
        }
    }

    /// Construct an empty registry (used by tests that don't need native
    /// dispatch). M29: `user_ctx` is `None` — `populate_from_user_ctx` is a
    /// no-op.
    pub fn empty() -> Self {
        Self {
            map: HashMap::new(),
            user_ctx: None,
        }
    }

    /// Look up a native by name. Returns `(native_idx, &FuncDef)`.
    pub fn get(&self, sym: &Symbol) -> Option<(u32, &Rc<FuncDef>)> {
        self.map.get(sym).map(|(idx, fd)| (*idx, fd))
    }

    /// True iff `sym` names a registered native. Equivalent to the
    /// tree-walker's `is_native_word` check (used to terminate variadic arg
    /// collection at the next native call).
    pub fn contains(&self, sym: &Symbol) -> bool {
        self.map.contains_key(sym)
    }
}

// ---------------------------------------------------------------------------
// Compile errors
// ---------------------------------------------------------------------------

/// Errors raised during compilation. Each carries the `Span` of the offending
/// source value so the CLI's `render_error` can localize it (M29+). The
/// `kind` enum is defined in `red-core::env` (re-exported as
/// `red_core::CompileErrorKind`) so `EvalError::Compile` can name it without
/// a red-eval dependency.
#[derive(Debug)]
pub struct CompileError {
    pub span: Span,
    pub kind: CompileErrorKind,
}

// ---------------------------------------------------------------------------
// Compiler state
// ---------------------------------------------------------------------------

/// Tracks per-slot user-func arity so a later `CallUser` to the same slot
/// knows how many args to collect (e.g. recursive `fact n - 1` → `fact`'s
/// arity was recorded when its `SetWord` compiled the `MakeFunc`).
///
/// Keyed by `(depth, slot)` — depth 0 = root/global, >=1 = function-local.
/// Set when a `SetWord`'s RHS emits `MakeFunc`; read when the same slot is
/// referenced in operator position by a `Word`.
#[derive(Default)]
struct FuncArityTable {
    /// `(depth, slot) -> arity` (positional param count).
    arities: HashMap<(usize, usize), usize>,
}

impl FuncArityTable {
    fn record(&mut self, depth: usize, slot: usize, arity: usize) {
        self.arities.insert((depth, slot), arity);
    }

    fn get(&self, depth: usize, slot: usize) -> Option<usize> {
        self.arities.get(&(depth, slot)).copied()
    }

    /// M29: Pre-populate from `user_ctx` slots that hold `Value::Func`. This
    /// lets the compiler emit `CallUser` for user-func calls even when the
    /// func was defined outside the current compile unit (e.g. `f: does [1]`
    /// at the top level, then `acc: f` inside a `repeat` body compiled
    /// separately via `dispatch_block`). Without this, the `repeat` body's
    /// `FuncArityTable` is empty, so `f` degrades to `LoadGlobal` (value
    /// load) instead of `CallUser` (function call) — returning `#[function]`
    /// instead of the call result.
    fn populate_from_user_ctx(&mut self, user_ctx: &Rc<Context>) {
        let names = user_ctx.names.borrow();
        for (_, idx) in names.iter() {
            let val = user_ctx.slot_value(*idx);
            match val {
                // M60: closures are callable like funcs — record their arity
                // so `f 5` emits `CallUser` instead of `LoadGlobal`.
                Value::Func(fd) => {
                    self.arities.insert((0, *idx), fd.params.len());
                }
                Value::Closure(cd) => {
                    self.arities.insert((0, *idx), cd.func.params.len());
                }
                _ => {}
            }
        }
    }
}

/// Compile state: the in-progress instr stream + pool + reference data.
struct Compiler<'a> {
    instrs: Vec<Instr>,
    /// M31: per-instr source span, parallel to `instrs`. Each entry is the
    /// `Span` of the source value that produced the corresponding instr.
    /// Synthesized instrs (trailing `Return`, `Jump` patch targets, the
    /// `ConstNone` false-branch push) inherit the nearest source-value span
    /// via `emit_with_span(.., span)` calls threaded through the compile
    /// helpers. The compiler's `emit(instr)` convenience uses the latest
    /// `current_span` set by `compile_prefix`/`compile_expr` from the value
    /// being compiled.
    spans: Vec<Span>,
    /// The span of the source value currently being compiled. Set by
    /// `compile_expr`/`compile_prefix` from `data[*i].span_or_default()`
    /// before any `emit` call. Read by `emit(instr)` (which doesn't take an
    /// explicit span). `emit_with_span` overrides this for synthesized instrs.
    current_span: Span,
    pool: ConstantPool,
    natives: &'a NativeRegistry,
    /// Slot of the enclosing `func` being defined (for recursive self-call
    /// detection — set by `compile_make_func`, read by `compile_word`).
    func_arities: FuncArityTable,
    /// M30.1.B: side table for `LoadDynamic`/`SetDynamic`/`MarkRefine`.
    /// Symbols are interned here by `intern_symbol`; the instr carries the
    /// index instead of the `Symbol` itself, so `Instr` can be `Copy`.
    symbols: Vec<Symbol>,
    /// M30.1.B: side table for `MakeFunc`. Each entry is one func's freevar
    /// capture list. The instr carries the index instead of the `Vec<Symbol>`,
    /// so `Instr` can be `Copy`.
    freevars_table: Vec<Vec<Symbol>>,
    /// M60: side table for `MakeClosure`. Each entry is one closure's
    /// captures list: `Vec<(Symbol, depth, slot)>` — for each freevar, the
    /// `(depth, slot)` to read at snapshot time (`depth == 0` → read from
    /// `env.user_ctx`, `depth >= 1` → read from `frames[len-1-depth].locals`).
    /// Populated by `compile_make_closure` via `intern_captures`.
    captures_table: Vec<Vec<(Symbol, usize, usize)>>,
    /// Slots that *might* hold a `Value::Func` at runtime but whose arity
    /// wasn't statically recorded in `func_arities`. Populated for:
    ///  - SetWord targets whose RHS is a `GetWord` (`g: :dbl`) or a `get
    ///    'word` native call (`f: get 'inc`) — the resolved value is a Func
    ///    whose arity the compiler can't see.
    ///  - Function parameters (params are dynamically typed; a param may hold
    ///    a Func passed by the caller, e.g. `apply-twice`'s `f`).
    ///
    /// When such a slot is referenced in operator position with trailing
    /// values that could be args, `compile_word` falls back to the walker
    /// (the VM can't statically decide arg count). Keyed by `(depth, slot)`.
    dynamic_func_slots: HashSet<(usize, usize)>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Compile a parsed, M23-analyzed block into a [`CompiledBlock`].
///
/// The caller seeds `scope` via `Scope::root(&env.user_ctx)` for the top-level
/// script body, or `Scope::child(&parent_scope)` for a function body.
/// `natives` is a snapshot built from `env.natives` via
/// [`NativeRegistry::from_env`]; it lets the compiler resolve `Word`s in
/// operator position to native indices for `Call(native_idx, argc)`.
///
/// The block's `AnalysisResult.needs_rebind` (computed by [`analyze_block`])
/// gates compilation: if `true`, the block contains `use`/`make object!`/
/// `object`/`context` forms that the walker must handle — `compile_block`
/// returns a stub `CompiledBlock` with `needs_rebind = true` and a `[Halt]`
/// instr stream; the VM (M25) falls back to the walker for such blocks.
pub fn compile_block(
    block: &Series,
    scope: &mut Scope,
    natives: &NativeRegistry,
) -> Result<CompiledBlock, CompileError> {
    compile_block_inner(block, scope, natives, None, 0)
}

/// Like `compile_block` but pre-seeds the `FuncArityTable` with the enclosing
/// function's own slot (`self_func` = `(slot, arity)`), so recursive self-calls
/// inside the body emit `CallUser(slot, arity)` instead of degrading to
/// `LoadDynamic`. Used by the M25 VM's lazy func-body compilation path.
///
/// `param_count` is the func's positional parameter count. Those slots
/// (0..param_count in the body's local frame) are marked as
/// `dynamic_func_slots`: params are dynamically typed, so a param may hold
/// a `Value::Func` passed by the caller (e.g. `apply-twice`'s `f`). When such
/// a param is referenced in operator position with trailing args, the
/// compiler falls back to the walker rather than emitting a plain load.
pub(crate) fn compile_block_for_func_body(
    block: &Series,
    scope: &mut Scope,
    natives: &NativeRegistry,
    self_func: (u32, usize),
    param_count: usize,
) -> Result<CompiledBlock, CompileError> {
    compile_block_inner(block, scope, natives, Some(self_func), param_count)
}

/// Like `compile_block` but emits **no `Pop` between expressions** — every
/// expression's result stays on the stack. Used by the `reduce` native in VM
/// mode: the VM runs the block, then `run_reduce` collects the stack into a
/// `Value::Block`. Matches the walker's `reduce` semantics (one result per
/// expression). `needs_rebind` short-circuits just like `compile_block`.
pub(crate) fn compile_block_reduce(
    block: &Series,
    scope: &mut Scope,
    natives: &NativeRegistry,
) -> Result<CompiledBlock, CompileError> {
    let analysis = analyze_block(block, scope);
    if analysis.needs_rebind {
        return Ok(stub_block(block, analysis));
    }
    let func_arities = FuncArityTable::default();
    let mut compiler = Compiler {
        instrs: Vec::new(),
        spans: Vec::new(),
        current_span: Span::default(),
        pool: ConstantPool::new(),
        natives,
        func_arities,
        symbols: Vec::new(),
        freevars_table: Vec::new(),
        captures_table: Vec::new(),
        dynamic_func_slots: HashSet::new(),
    };
    let data = block.data.borrow();
    let n = data.len();
    let mut i = block.index;
    while i < n {
        compile_expr(&mut compiler, &data, &mut i, scope, /*tail*/ false)?;
        // No Pop: every expression's result stays on the stack. The final
        // `Return` pops the frame; `run_reduce` collects what remains.
    }
    drop(data);
    compiler.emit_with_span(Instr::Return, block_source_span(block));
    let span = block_source_span(block);
    Ok(CompiledBlock {
        instrs: Rc::from(compiler.instrs.as_slice()),
        pool: compiler.pool.into_rc(),
        symbols: Rc::from(compiler.symbols.as_slice()),
        freevars_table: Rc::from(compiler.freevars_table.as_slice()),
        captures_table: Rc::from(compiler.captures_table.as_slice()),
        n_locals: scope_locals_count(scope),
        freevars: analysis.freevars,
        source_span: span,
        spans: Rc::from(compiler.spans.as_slice()),
        needs_rebind: false,
        arity: 0,
    })
}

fn compile_block_inner(
    block: &Series,
    scope: &mut Scope,
    natives: &NativeRegistry,
    self_func: Option<(u32, usize)>,
    param_count: usize,
) -> Result<CompiledBlock, CompileError> {
    #[cfg(test)]
    COMPILE_COUNT.with(|c| c.set(c.get() + 1));
    // Phase 1: lexical analysis (M23). Attaches `Binding::Lexical` to
    // function-local words and computes `freevars` + `needs_rebind`.
    let analysis = analyze_block(block, scope);

    // `needs_rebind` short-circuit: emit a stub. The VM will defer to the
    // walker for this block (and any nested `use`/object forms).
    if analysis.needs_rebind {
        return Ok(stub_block(block, analysis));
    }

    let mut func_arities = FuncArityTable::default();
    // M29: pre-populate from `user_ctx` so user-func calls inside blocks
    // compiled separately (e.g. `repeat`/`if`/`foreach` bodies) emit
    // `CallUser` instead of degrading to `LoadGlobal`. Without this, `f: does [1]`
    // at the top level followed by `acc: f` inside a `repeat` body would
    // load the func value (`#[function]`) instead of calling it.
    if let Some(ref user_ctx) = natives.user_ctx {
        func_arities.populate_from_user_ctx(user_ctx);
    }
    if let Some((slot, arity)) = self_func {
        // Self-recursion: the func's own slot is at depth 0 (global) when the
        // SetWord defining it is top-level; M25's lazy-compile path passes the
        // actual slot. The depth is 0 relative to the body's *parent* scope,
        // which is what the body's `Scope::child` parent represents.
        func_arities.record(0, slot as usize, arity);
    }
    // Mark the func's param slots as dynamic-func. Params are dynamically
    // typed: a caller may pass a `Value::Func` (e.g. `apply-twice get 'inc 5`
    // passes the `inc` func into param `f`). When the body references such a
    // param in operator position with trailing args (`f x`), the compiler
    // can't statically know the arity → fall back to the walker. Slots are
    // local to this frame, so depth 0. (Top-level blocks pass param_count=0.)
    let mut dynamic_func_slots: HashSet<(usize, usize)> = HashSet::new();
    for p in 0..param_count {
        dynamic_func_slots.insert((0, p));
    }
    let mut compiler = Compiler {
        instrs: Vec::new(),
        spans: Vec::new(),
        current_span: Span::default(),
        pool: ConstantPool::new(),
        natives,
        func_arities,
        symbols: Vec::new(),
        freevars_table: Vec::new(),
        captures_table: Vec::new(),
        dynamic_func_slots,
    };

    let data = block.data.borrow();
    let n = data.len();
    let mut i = block.index;
    while i < n {
        // M28: tail-position detection. We can't know whether the next
        // expression is the last until after `compile_expr` consumes its
        // values (expressions span a variable number of source values —
        // `n * fact n - 1` is 6 values but 1 expression). So we compile
        // first, then *retroactively* patch a trailing `CallUser` into a
        // `TailCall`/`TailReenter` if this turned out to be the last expr.
        compile_expr(&mut compiler, &data, &mut i, scope, /*tail*/ false)?;
        if i < n {
            // Non-tail: discard the intermediate result.
            compiler.emit(Instr::Pop);
        } else {
            // Tail position: if the last emitted instr is a `CallUser`,
            // promote it to `TailCall` (or `TailReenter` for self-recursion).
            patch_tail_call(&mut compiler, self_func);
        }
    }
    drop(data);

    // Block ends with `Return`: the VM pops the frame, returning top-of-stack
    // (or `None` if the stack is empty — matches the walker's `last` value).
    compiler.emit_with_span(Instr::Return, block_source_span(block));
    let span = block_source_span(block);
    Ok(CompiledBlock {
        instrs: Rc::from(compiler.instrs.as_slice()),
        pool: compiler.pool.into_rc(),
        symbols: Rc::from(compiler.symbols.as_slice()),
        freevars_table: Rc::from(compiler.freevars_table.as_slice()),
        captures_table: Rc::from(compiler.captures_table.as_slice()),
        n_locals: scope_locals_count(scope),
        freevars: analysis.freevars,
        source_span: span,
        spans: Rc::from(compiler.spans.as_slice()),
        needs_rebind: false,
        arity: 0,
    })
}

/// Build a `needs_rebind` stub block: instrs `[Halt]`, pool empty, the
/// analysis's freevars preserved (in case the walker path inspects them).
pub(crate) fn stub_block(block: &Series, analysis: AnalysisResult) -> CompiledBlock {
    let span = block_source_span(block);
    CompiledBlock {
        instrs: Rc::from([Instr::Halt]),
        pool: Rc::from([]),
        symbols: Rc::from(&Vec::<Symbol>::new()[..]),
        freevars_table: Rc::from(&Vec::<Vec<Symbol>>::new()[..]),
        captures_table: Rc::from(&Vec::<Vec<(Symbol, usize, usize)>>::new()[..]),
        n_locals: 0,
        freevars: analysis.freevars,
        source_span: span,
        spans: Rc::from(&[span][..]),
        needs_rebind: true,
        arity: 0,
    }
}

// ---------------------------------------------------------------------------
// Per-expression compilation
// ---------------------------------------------------------------------------

impl<'a> Compiler<'a> {
    fn emit(&mut self, instr: Instr) {
        self.emit_with_span(instr, self.current_span);
    }

    /// M31: emit `instr` with an explicit `span` (used by synthesized instrs
    /// like the trailing `Return`, `Jump` patch targets, the `ConstNone`
    /// false-branch push — these have no source value of their own, so they
    /// inherit the span of the nearest source value that triggered them).
    /// `emit` (above) reads `self.current_span`; this override lets callers
    /// pass a different span without disturbing `current_span`.
    fn emit_with_span(&mut self, instr: Instr, span: Span) {
        self.instrs.push(instr);
        self.spans.push(span);
    }

    fn push_const(&mut self, v: Value) -> u32 {
        self.pool.push(v)
    }

    /// M30.1.B: intern a `Symbol` into the block's symbol table and return
    /// its index. Used by `LoadDynamic`/`SetDynamic`/`MarkRefine` emission so
    /// the `Instr` variant carries a `u32` index (keeping `Instr: Copy`)
    /// instead of an `Rc<str>` clone (which would refcount per dispatch).
    fn intern_symbol(&mut self, sym: Symbol) -> u32 {
        // Linear scan — symbol tables are small (typically < 20 entries per
        // block; the user context's word count plus a few natives). A
        // `HashMap` would add allocation overhead without measurable win.
        if let Some(pos) = self.symbols.iter().position(|s| *s == sym) {
            return pos as u32;
        }
        let idx = self.symbols.len() as u32;
        self.symbols.push(sym);
        idx
    }

    /// M30.1.B: intern a freevar capture list (`Vec<Symbol>`) into the
    /// block's freevars table and return its index. Used by `MakeFunc`
    /// emission so the instr carries a `u32` index (keeping `Instr: Copy`)
    /// instead of the `Vec<Symbol>` inline (which bloated the enum to ~40
    /// bytes and forced a clone per dispatch iteration).
    fn intern_freevars(&mut self, fv: Vec<Symbol>) -> u32 {
        let idx = self.freevars_table.len() as u32;
        self.freevars_table.push(fv);
        idx
    }

    /// M60: intern a captures list `Vec<(Symbol, depth, slot)>` into the
    /// `captures_table` side table. Returns the index for `Instr::MakeClosure`.
    fn intern_captures(&mut self, caps: Vec<(Symbol, usize, usize)>) -> u32 {
        let idx = self.captures_table.len() as u32;
        self.captures_table.push(caps);
        idx
    }

    /// Detect an infix native at `data[i]` (unbound `Word`/`GetWord` whose
    /// `FuncDef.infix == true`). Returns the registered `(idx, fd)` pair.
    fn infix_native_at(&self, v: &Value) -> Option<(u32, Rc<FuncDef>)> {
        let sym = match v {
            Value::Word { sym, binding, .. } | Value::GetWord { sym, binding, .. } => {
                if !matches!(binding, Binding::Unbound) {
                    return None;
                }
                sym
            }
            _ => return None,
        };
        self.natives
            .get(sym)
            .filter(|(_, fd)| fd.infix)
            .map(|(idx, fd)| (idx, Rc::clone(fd)))
    }
}

/// Compile a single expression starting at `data[*i]`. Mirrors
/// `interp::eval_expression`: a prefix value followed by zero or more infix
/// native applications (Red's left-to-right, no-precedence rule). Advances
/// `*i` past the consumed expression.
///
/// `tail` marks tail position — the last expression in a block. Tail-position
/// `CallUser`s are candidates for `TailCall`/`TailReenter` in M28; M24 emits
/// a plain `CallUser` (the optimization lands later). `if`/`either` in tail
/// position propagate `tail` into their branches' last expressions.
fn compile_expr(
    c: &mut Compiler,
    data: &[Value],
    i: &mut usize,
    scope: &Scope,
    tail: bool,
) -> Result<(), CompileError> {
    // First: the prefix value (which may itself be a `func`/`does`/`if` form).
    let prefix_span = data[*i].span_or_default();
    compile_prefix(c, data, i, scope, tail)?;

    // Then: chain infix natives left-to-right. Each infix native consumes one
    // more prefix value as its right operand (mirrors `eval_expression`).
    while *i < data.len() {
        let (idx, fd) = match c.infix_native_at(&data[*i]) {
            Some(x) => x,
            None => break,
        };
        *i += 1; // consume the infix word
        let arity = fd.params.len();
        debug_assert!(arity >= 1, "infix native must take >=1 operand");
        // Remaining `arity - 1` operands (first is already on the stack).
        for _ in 1..arity {
            if *i >= data.len() {
                return Err(CompileError {
                    span: prefix_span,
                    kind: CompileErrorKind::ArityMismatch,
                });
            }
            // Infix operands are prefix values (no nested infix chain —
            // `1 + 2 * 3` chains at *this* loop's level, not recursively).
            compile_prefix(c, data, i, scope, /*tail*/ false)?;
        }
        c.emit(Instr::Call(idx, arity as u32));
    }
    Ok(())
}

/// Compile a single prefix value (no infix chaining). Mirrors
/// `interp::eval_prefix`. Advances `*i` past the consumed value (and any
/// native args / SetWord RHS / if/either body).
fn compile_prefix(
    c: &mut Compiler,
    data: &[Value],
    i: &mut usize,
    scope: &Scope,
    tail: bool,
) -> Result<(), CompileError> {
    let cur = &data[*i];
    let span = cur.span_or_default();
    // M31: thread the source value's span into `emit` so per-instr spans
    // localize to the value that produced them. Synthesized instrs emitted
    // by helpers below (e.g. `compile_if`'s `Jump`/`ConstNone`) call
    // `emit_with_span(.., span)` explicitly to inherit this span.
    c.current_span = span;
    // Special forms first — they need lookahead beyond the prefix value.
    // `use`/`make object!`/`object`/`context` should never reach here
    // (`compile_block` short-circuited on `needs_rebind`), but if one slipped
    // through (e.g. nested inside a compiled branch), surface it as an error.
    if use_body_index(data, *i).is_some() {
        return Err(CompileError {
            span,
            kind: CompileErrorKind::MalformedSpec,
        });
    }
    if is_make_object_form(data, *i) || is_object_keyword_form(data, *i) {
        return Err(CompileError {
            span,
            kind: CompileErrorKind::MalformedSpec,
        });
    }
    // `func`/`does`/`function`/`closure` — emit `MakeFunc`/`MakeClosure` and
    // advance `*i` past the form.
    if let Some(skip) = func_form_skip(data, *i) {
        let is_closure = matches!(
            cur,
            Value::Word { sym, .. } if sym.as_str() == "closure"
        );
        // M60: identify the closure's own name from the preceding SetWord
        // (if any). `fact: closure [n][body]` → closure_name = "fact". The
        // body's reference to `fact` (recursion) is NOT captured — it
        // resolves via the outer SetWord slot for late-binding.
        let closure_name = if is_closure && *i > 0 {
            match &data[*i - 1] {
                Value::SetWord { sym, .. } => Some(sym.clone()),
                _ => None,
            }
        } else {
            None
        };
        *i += 1; // consume the calling word itself
        if is_closure {
            compile_make_closure(c, data, i, scope, closure_name, span)?;
        } else {
            compile_make_func(c, data, i, scope, cur, span)?;
        }
        *i += skip - 1;
        return Ok(());
    }

    *i += 1; // consume the prefix value itself

    match cur {
        // Data / literals: push as `Const` (or the M30 small-value fast paths
        // `ConstInt`/`ConstNone`/`ConstBool` for the common kinds).
        Value::None => {
            c.emit(Instr::ConstNone);
        }
        // M86: `unset!` is a synthetic sentinel — push as `Const` (pool) so
        // the same `Value::Unset` flows through the VM.
        Value::Unset => {
            let idx = c.push_const(cur.clone());
            c.emit(Instr::Const(idx));
        }
        Value::Logic(b) => {
            c.emit(Instr::ConstBool(*b));
        }
        Value::Integer { n, .. } => {
            c.emit(Instr::ConstInt(*n));
        }
        Value::Float { .. }
        | Value::Decimal { .. }
        | Value::Percent { .. }
        | Value::Money { .. }
        | Value::Issue { .. }
        | Value::Email { .. }
        | Value::Tag { .. }
        | Value::String { .. }
        | Value::Char { .. }
        | Value::Pair { .. }
        | Value::Tuple { .. }
        | Value::String8 { .. }
        | Value::Date { .. }
        | Value::Duration { .. }
        | Value::LitWord { .. }
        | Value::Block { .. }
        | Value::Func(_)
        | Value::Closure(_)
        | Value::Refinement { .. }
        | Value::Error(_)
        | Value::File { .. }
        | Value::Url { .. }
        | Value::Object(_)
        | Value::Module(_)
        | Value::Map(_)
        | Value::Hash(_)
        | Value::Vector(_)
        | Value::Image(_)
        | Value::Bitset(_)
        | Value::Port(_)
        | Value::Typeset(_) => {
            let idx = c.push_const(cur.clone());
            c.emit(Instr::Const(idx));
        }

        // Path: either a data-headed path (`obj/field`, `block/2`) resolved at
        // runtime via `GetPath`, or a function-headed path (`copy/part x`)
        // compiled as a refined native call. The head determines which:
        // an unbound `Word` naming a registered native → refined `Call`;
        // a bound `Word` that might be a user func with refinements →
        //   `CompileError` (fall back to walker, which handles user-func
        //   refinement paths correctly — M29 pragmatic choice; full VM
        //   support for user-func refinements deferred to a later milestone);
        // anything else → `GetPath` (M19 runtime resolution).
        Value::Path { parts, .. } | Value::GetPath { parts, .. } => {
            if let Some((native_idx, fd, head_sym, leading_refs)) = function_path_info(c, parts) {
                // `*i` was already advanced past the path token by the
                // caller; `collect_args` reads args starting at `*i`.
                let (argc, _refs) =
                    collect_args(c, data, i, scope, &head_sym, &fd, &leading_refs, span)?;
                c.emit(Instr::Call(native_idx, argc as u32));
                return Ok(());
            }
            // M29: if the path head is a bound word (not Unbound), it might
            // be a user func called with refinements (`f/with 5 7`). The VM
            // doesn't support user-func refinement dispatch (the `CallUser`
            // handler has no refinement-arg collection path). Fall back to
            // the walker by returning a `MalformedSpec` compile error —
            // `dispatch_block` catches it and runs the walker, which handles
            // `eval_path_call` → `dispatch_call_with_refs` correctly. This is
            // a pragmatic M29 choice; a future milestone can add VM-side
            // user-func refinement dispatch.
            if let Some(
                Value::Word {
                    binding: Binding::Local(_, _),
                    ..
                }
                | Value::Word {
                    binding: Binding::Lexical(_, _),
                    ..
                },
            ) = parts.first()
            {
                if parts.len() > 1 {
                    return Err(CompileError {
                        span,
                        kind: CompileErrorKind::MalformedSpec,
                    });
                }
            }
            let idx = c.push_const(cur.clone());
            c.emit(Instr::Const(idx));
            c.emit(Instr::GetPath);
        }

        // LitPath `'foo/bar` — returned as data (mirrors `LitWord`).
        Value::LitPath { .. } => {
            let idx = c.push_const(cur.clone());
            c.emit(Instr::Const(idx));
        }

        // SetPath `obj/field: value` — push path, compile RHS, then `SetPath`.
        Value::SetPath { .. } => {
            let path_idx = c.push_const(cur.clone());
            c.emit(Instr::Const(path_idx));
            // RHS is the next expression.
            if *i >= data.len() {
                return Err(CompileError {
                    span,
                    kind: CompileErrorKind::ArityMismatch,
                });
            }
            compile_expr(c, data, i, scope, /*tail*/ false)?;
            // `SetPath` pops the path and the RHS; the written value remains
            // on the stack (matches the walker, which returns the RHS).
            c.emit(Instr::SetPath);
        }

        // Paren: compiled eagerly in place (its series is code, not data).
        Value::Paren { series, .. } => {
            let child = series.clone();
            let child_data = child.data.borrow();
            let n = child_data.len();
            let mut j = child.index;
            while j < n {
                let is_last = j + 1 == n;
                compile_expr(c, &child_data, &mut j, scope, is_last)?;
            }
            drop(child_data);
            // No `Return` — the paren's last value stays on the caller's
            // stack (the paren was an inline `do`).
        }

        // Word: the main dispatch point. Value position → load; operator
        // position → collect args + Call/CallUser. We peek the *binding* to
        // decide: Lexical/Local/Unbound each emit their own load, and if args
        // follow, the operator-position path supersedes.
        Value::Word {
            sym,
            binding,
            span: w_span,
        } => {
            compile_word(c, data, i, scope, tail, sym, binding, *w_span)?;
        }

        // SetWord: compile RHS expression, then store.
        Value::SetWord {
            sym,
            binding,
            span: w_span,
        } => {
            if *i >= data.len() {
                return Err(CompileError {
                    span: *w_span,
                    kind: CompileErrorKind::ArityMismatch,
                });
            }
            // If the RHS is a `func`/`does`/`function` form, peek its arity
            // now so subsequent `CallUser`s to this slot know how many args
            // to collect. This makes `square: func [x][x * x] square 5` emit
            // `CallUser(slot, 1)` rather than degrading to `LoadDynamic`.
            if let Some(arity) = peek_func_arity(data, *i) {
                let (depth, slot) = slot_coords(binding);
                c.func_arities.record(depth, slot, arity);
            } else if rhs_might_be_func(data, *i) {
                // RHS resolves to a Func at runtime but the compiler can't see
                // its arity (`f: get 'inc`, `g: :dbl`). Mark the slot so a
                // later operator-position reference falls back to the walker
                // (which dispatches dynamically) instead of emitting a plain
                // `LoadGlobal` that would return `#[function]`.
                let (depth, slot) = slot_coords(binding);
                c.dynamic_func_slots.insert((depth, slot));
            }
            // RHS is a full expression (so `x: 1 + 2` works).
            compile_expr(c, data, i, scope, /*tail*/ false)?;
            emit_store(c, binding, *w_span, sym)?;
            // `SetGlobal`/`SetLocal` (M25) pop the RHS and push the written
            // value back, so the walker's "SetWord returns the written value"
            // semantics hold. Non-last SetWords get a `Pop` from the enclosing
            // `compile_block_inner` loop.
        }

        // GetWord: load without calling (matches walker semantics).
        Value::GetWord {
            sym,
            binding,
            span: w_span,
        } => {
            emit_load(c, binding, *w_span, sym)?;
        }
    }

    Ok(())
}

/// Compile a `Word` in prefix position. The word may be:
/// - A known **native** (unbound + in `NativeRegistry`) in operator position
///   → collect args + `Call(native_idx, argc)`. Special-cases `if`/`either`
///   for inline `JumpIfFalse` emission.
/// - A known **user-func** bound to a slot with recorded arity → collect
///   args + `CallUser(slot, argc)`.
/// - Anything else → just load the value (no args collected). This is the
///   "value position" path; the walker's `dispatch_call` returns the value
///   as-is when it's not a Func.
#[allow(clippy::too_many_arguments)] // 4 args share mixed lifetimes; a CompileCtx struct would propagate to ~30 callsites
fn compile_word(
    c: &mut Compiler,
    data: &[Value],
    i: &mut usize,
    scope: &Scope,
    tail: bool,
    sym: &Symbol,
    binding: &Binding,
    span: Span,
) -> Result<(), CompileError> {
    // Operator-position detection: unbound word naming a native. (Bound
    // words resolve to user funcs via the slot table below.)
    if let Binding::Unbound = binding {
        if let Some((idx, fd)) = c.natives.get(sym) {
            // Special-case `if`/`either` for inline control flow.
            match sym.as_str() {
                "if" => return compile_if(c, data, i, scope, tail, span),
                "either" => return compile_either(c, data, i, scope, tail, span),
                _ => {}
            }
            // Generic native call.
            return compile_native_call(c, data, i, scope, sym, idx, fd, span);
        }
        // Bug 4: unbound non-native word. If it's in operator position
        // (followed by a potential argument and not the last value), we
        // can't statically know whether it will resolve at runtime to a
        // user-func (e.g. via `import` mid-eval) or to a plain value.
        // Return `MalformedSpec` so `dispatch_block`/`ensure_compiled`
        // route the enclosing block through the walker, whose
        // `dispatch_call` handles dynamic func dispatch correctly.
        // In value position (last value, or followed by an infix native),
        // emit `LoadDynamic` — the word is meant as a value, not a call.
        if *i < data.len() && c.infix_native_at(&data[*i]).is_none() {
            return Err(CompileError {
                span,
                kind: CompileErrorKind::MalformedSpec,
            });
        }
        let idx = c.intern_symbol(sym.clone());
        c.emit(Instr::LoadDynamic(idx));
        return Ok(());
    }

    // Bound word — is it a known user-func slot? If the slot was recorded by
    // an earlier `MakeFunc`, we know its arity and can emit `CallUser`.
    let (depth, slot) = match binding {
        Binding::Lexical(d, s) => (*d, *s),
        Binding::Local(_ctx, s) => (0, *s), // global (user-ctx) slot
        Binding::Func(s) => {
            // Function-local slot (set by `bind_function_body`'s older path).
            // M23 overwrites these with `Lexical` when it runs, but defensive.
            (0, *s)
        }
        // M60: closure capture — not a callable slot; the word resolves to the
        // captured value at runtime via `LoadCapture`. Emit `LoadCapture` here
        // and return (don't enter the `CallUser` path below).
        Binding::Closure(idx) => {
            c.emit(Instr::LoadCapture(*idx as u32));
            return Ok(());
        }
        // M31: was `unreachable!()`. The earlier `if let Binding::Unbound`
        // arm above returns for unbound words that name a native, or emits
        // `LoadDynamic` for unknown unbound words. Reaching here means a
        // routing bug in the compiler (or a future binding variant); surface
        // as a recoverable `CompileError` rather than panicking in release.
        // The span is the word's own source position.
        Binding::Unbound => {
            return Err(CompileError {
                span,
                kind: CompileErrorKind::UnboundWord,
            });
        }
    };
    // Is the resolved slot a known user-func? If so, emit `CallUser` — the
    // walker always calls a Func when a word resolves to one (even with 0
    // args), so the VM must match. For 0-arity funcs (`does [...]`), the word
    // IS the call (no args to collect); for non-zero arity, `collect_args`
    // collects the right number. If there aren't enough args, `collect_args`
    // returns `ArityMismatch`, which causes `dispatch_block` to fall back to
    // the walker (which produces the correct runtime `EvalError::Arity`).
    //
    // The old `if *i < data.len()` check was wrong for 0-arity funcs: it
    // prevented `CallUser` from being emitted when the word was the last
    // value, causing the VM to load the func value instead of calling it
    // (returning `#[function]` instead of the call result). (M29 fix.)
    if let Some(arity) = c.func_arities.get(depth, slot) {
        return compile_user_call(c, data, i, scope, slot, arity, depth, span);
    }
    // Unknown-arity bound word. If the slot is one that *might* hold a
    // `Value::Func` at runtime — assigned via `get 'word`/`:word` (GetWord
    // RHS) or a function parameter (params are dynamically typed) — and it's
    // followed by a value that could be an argument, the VM can't statically
    // decide how many trailing values to consume. Fall back to the walker by
    // returning a `MalformedSpec` CompileError: `dispatch_block` (top-level)
    // routes to the walker; `ensure_compiled` (func body) returns a
    // `needs_rebind` stub and `call_user` invokes the body via the walker.
    // Slots holding provably-non-Func values (ints, strings, etc.) aren't
    // marked dynamic-func, so they keep the fast `LoadGlobal`/`LoadLocal`
    // path — the fallback is scoped to genuine higher-order patterns.
    if c.dynamic_func_slots.contains(&(depth, slot))
        && *i < data.len()
        && c.infix_native_at(&data[*i]).is_none()
    {
        return Err(CompileError {
            span,
            kind: CompileErrorKind::MalformedSpec,
        });
    }
    // Value position — just load.
    emit_load(c, binding, span, sym)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// `if` / `either` inlining
// ---------------------------------------------------------------------------

/// M28: if the last instr emitted is a `CallUser(slot, argc)`, promote it to
/// `TailCall(slot, argc)`. If `self_func` matches the slot (recursive self-
/// call), promote further to `TailReenter(slot, argc)` — the VM reuses the
/// current frame's `FuncDef`, just resetting `locals`/`pc` (cheaper than a
/// `TailCall`, which still has to look up the func slot).
///
/// `self_func = Some((slot, _arity))` only when the block being compiled is a
/// func body and the slot is the func's own global slot (set up by
/// `compile_block_for_func_body`). For branch bodies (`if`/`either`) we don't
/// know `self_func` here, so we always emit `TailCall`; the VM's `TailCall`
/// handler detects self-recursion at runtime via `Rc::ptr_eq` on the
/// `FuncDef` and falls into the cheaper `TailReenter` path.
fn patch_tail_call(c: &mut Compiler, self_func: Option<(u32, usize)>) {
    let last = match c.instrs.last_mut() {
        Some(i) => i,
        None => return,
    };
    let (slot, argc) = match last {
        // M30.3.4: `CallUserGlobal` is also a candidate for tail-call promotion.
        Instr::CallUser(s, a) | Instr::CallUserGlobal(s, a) => (*s, *a),
        _ => return,
    };
    // Don't promote zero-argc "calls" (those are just value loads of the
    // func — no frame would be pushed anyway). `does` bodies and the like
    // take 0 args; tail-promoting them would be a no-op at best and a
    // misroute at worst (TailCall with argc=0).
    if argc == 0 {
        return;
    }
    if let Some((self_slot, _)) = self_func {
        if slot == self_slot {
            *last = Instr::TailReenter(slot, argc);
            return;
        }
    }
    *last = Instr::TailCall(slot, argc);
}

/// Compile `if cond block` as inline control flow:
/// ```text
/// <cond code>          ; pushes cond
/// JumpIfFalse(L_end)   ; pops cond; if false jump
/// <then-block inline> ; pushes value (or nothing if block empty)
/// L_end:
/// ```
/// Matches docs/plans/plan3.md's expected `[Const(true), JumpIfFalse(L1), Const(42), L1: Return]`.
///
/// `if` takes 2 args (cond + block); the block must be a literal `Block`.
/// If the shape doesn't match (block arg isn't a literal Block), we fall
/// back to a generic `Call(if, 2)` so the runtime native handles it.
fn compile_if(
    c: &mut Compiler,
    data: &[Value],
    i: &mut usize,
    scope: &Scope,
    tail: bool,
    span: Span,
) -> Result<(), CompileError> {
    // Cond expression. (`compile_prefix` already consumed the `if` word.)
    if *i >= data.len() {
        return Err(CompileError {
            span,
            kind: CompileErrorKind::ArityMismatch,
        });
    }
    compile_expr(c, data, i, scope, /*tail*/ false)?;
    // Then-block: must be a literal Block.
    if *i >= data.len() {
        return Err(CompileError {
            span,
            kind: CompileErrorKind::ArityMismatch,
        });
    }
    let then_block = match &data[*i] {
        Value::Block { series, .. } => Some(series.clone()),
        _ => None,
    };
    let Some(then_series) = then_block else {
        // Fallback: emit the block arg as Const and dispatch generically.
        let block_const = c.push_const(data[*i].clone());
        *i += 1;
        c.emit(Instr::Const(block_const));
        c.emit(Instr::Call(if_native_index(c), 2));
        return Ok(());
    };
    *i += 1; // consume the then-block

    // Structure:
    //   <cond>                ; pushes cond
    //   JumpIfFalse(L_none)  ; pops cond; if false jump to L_none
    //   <then-block>          ; pushes value
    //   Jump(L_end)           ; skip the `none` push
    // L_none:
    //   Const(none)           ; false branch: push `none` (walker parity)
    // L_end:
    //
    // Without the `none` push, `if false [...]` would leave the stack empty,
    // and `print if false [42]` would get 0 args instead of 1. (M29 fix.)
    let jump_idx = c.instrs.len();
    c.emit(Instr::JumpIfFalse(0)); // placeholder
                                   // Then-block: compile its series inline with the *same scope* (M23 already
                                   // analyzed its words; literal blocks are descended by `analyze_inner`).
    compile_block_series_inline(c, &then_series, scope, tail)?;
    // Jump past the `none` push (false branch falls through to `none`).
    let skip_none_idx = c.instrs.len();
    c.emit(Instr::Jump(0)); // placeholder — patched below
                            // False branch: `if` with a false condition returns `none`.
    let none_target = c.instrs.len() as u32;
    // M30: use `ConstNone` fast path instead of pool+`Const`.
    c.emit(Instr::ConstNone);
    // Patch JumpIfFalse to land at the `none` push.
    c.instrs[jump_idx] = Instr::JumpIfFalse(none_target);
    // Patch the Jump to land after the `none` push.
    let end_target = (c.instrs.len()) as u32;
    c.instrs[skip_none_idx] = Instr::Jump(end_target);
    Ok(())
}

/// Compile `either cond t-block f-block` as inline control flow:
/// ```text
/// <cond>, JumpIfFalse(L_else), <t>, Jump(L_end), L_else: <f>, L_end:
/// ```
fn compile_either(
    c: &mut Compiler,
    data: &[Value],
    i: &mut usize,
    scope: &Scope,
    tail: bool,
    span: Span,
) -> Result<(), CompileError> {
    // Cond expression. (`compile_prefix` already consumed the `either` word.)
    if *i >= data.len() {
        return Err(CompileError {
            span,
            kind: CompileErrorKind::ArityMismatch,
        });
    }
    compile_expr(c, data, i, scope, /*tail*/ false)?;
    // T-block.
    if *i + 1 >= data.len() {
        return Err(CompileError {
            span,
            kind: CompileErrorKind::ArityMismatch,
        });
    }
    let t_series = match &data[*i] {
        Value::Block { series, .. } => series.clone(),
        _ => {
            // Fallback generic call.
            let t_const = c.push_const(data[*i].clone());
            let f_const = c.push_const(data[*i + 1].clone());
            *i += 2;
            c.emit(Instr::Const(t_const));
            c.emit(Instr::Const(f_const));
            c.emit(Instr::Call(either_native_index(c), 3));
            return Ok(());
        }
    };
    let f_series = match &data[*i + 1] {
        Value::Block { series, .. } => series.clone(),
        _ => {
            let t_const = c.push_const(data[*i].clone());
            let f_const = c.push_const(data[*i + 1].clone());
            *i += 2;
            c.emit(Instr::Const(t_const));
            c.emit(Instr::Const(f_const));
            c.emit(Instr::Call(either_native_index(c), 3));
            return Ok(());
        }
    };
    *i += 2;

    let jfalse_idx = c.instrs.len();
    c.emit(Instr::JumpIfFalse(0)); // placeholder -> L_else
    compile_block_series_inline(c, &t_series, scope, tail)?;
    let jump_end_idx = c.instrs.len();
    c.emit(Instr::Jump(0)); // placeholder -> L_end
    let l_else = c.instrs.len() as u32;
    c.instrs[jfalse_idx] = Instr::JumpIfFalse(l_else);
    compile_block_series_inline(c, &f_series, scope, tail)?;
    let l_end = c.instrs.len() as u32;
    c.instrs[jump_end_idx] = Instr::Jump(l_end);
    Ok(())
}

/// Compile a block's series inline (no `Return`): each expression in order,
/// the last in tail position. Used by `if`/`either` branch bodies.
///
/// M28: the last expression of a branch is in tail position relative to the
/// *enclosing block* (Red tail-call semantics: the value flows up as the
/// branch result, then up as the block result). We promote a trailing
/// `CallUser` to `TailCall`/`TailReenter` so the VM reuses the frame.
fn compile_block_series_inline(
    c: &mut Compiler,
    series: &Series,
    scope: &Scope,
    tail: bool,
) -> Result<(), CompileError> {
    let data = series.data.borrow();
    let n = data.len();
    let mut j = series.index;
    while j < n {
        compile_expr(c, &data, &mut j, scope, tail)?;
        if j < n {
            c.emit(Instr::Pop);
        } else if tail {
            // Tail position of the enclosing block. Promote the trailing
            // `CallUser` (if any) to a tail call. `self_func` isn't known
            // here (branches live inside a func body whose `self_func` was
            // threaded into `compile_block_inner`); we pass `None` and rely
            // on the `TailCall` → `TailReenter` runtime check (the VM
            // compares the target `Rc<FuncDef>` identity to the current
            // frame's func).
            patch_tail_call(c, None);
        }
    }
    Ok(())
}

/// Look up the `if` native's index in the registry. Panics if missing
/// (`if` is always registered by `register_natives` before any compile).
fn if_native_index(c: &Compiler) -> u32 {
    c.natives
        .get(&Symbol::new("if"))
        .map(|(idx, _)| idx)
        .expect("`if` native must be registered before compilation")
}

/// Look up the `either` native's index.
fn either_native_index(c: &Compiler) -> u32 {
    c.natives
        .get(&Symbol::new("either"))
        .map(|(idx, _)| idx)
        .expect("`either` native must be registered before compilation")
}

// ---------------------------------------------------------------------------
// Native / user-func call compilation
// ---------------------------------------------------------------------------

/// Compile a generic native call: collect `fd.params.len()` args (honoring
/// `uneval_first` and variadic semantics), then emit `Call(native_idx, argc)`.
#[allow(clippy::too_many_arguments)] // mixed-lifetime args; see compile_word
fn compile_native_call(
    c: &mut Compiler,
    data: &[Value],
    i: &mut usize,
    scope: &Scope,
    sym: &Symbol,
    native_idx: u32,
    fd: &Rc<FuncDef>,
    span: Span,
) -> Result<(), CompileError> {
    // (`compile_prefix` already consumed the calling word.)
    let (argc, _refs_emitted) = collect_args(c, data, i, scope, sym, fd, &[], span)?;
    c.emit(Instr::Call(native_idx, argc as u32));
    Ok(())
}

/// If `parts` is a function-headed path (`copy/part`, `find/case`, ...) — i.e.
/// its head is an unbound `Word` naming a registered native — return
/// `(native_idx, fd, head_sym, leading_refs)` so the caller can emit a refined
/// `Call` instead of a `GetPath`. `leading_refs` is the list of tail `Word`
/// parts (refinement flags); non-Word tail parts (integer/paren) are dropped
/// (mirrors `interp::eval_path_call`'s refinement extraction). Returns `None`
/// for data-headed paths (`obj/field`, `block/2`) — those stay `GetPath`.
fn function_path_info(
    c: &Compiler,
    parts: &[Value],
) -> Option<(u32, Rc<FuncDef>, Symbol, Vec<Symbol>)> {
    let head = parts.first()?;
    let (head_sym, head_binding) = match head {
        Value::Word { sym, binding, .. } => (sym.clone(), binding.clone()),
        Value::GetWord { sym, binding, .. } => (sym.clone(), binding.clone()),
        Value::LitWord { sym, .. } => (sym.clone(), Binding::Unbound),
        _ => return None,
    };
    if !matches!(head_binding, Binding::Unbound) {
        return None;
    }
    let (native_idx, fd) = c.natives.get(&head_sym)?;
    let leading_refs: Vec<Symbol> = parts[1..]
        .iter()
        .filter_map(|p| match p {
            Value::Word { sym, .. } => Some(sym.clone()),
            _ => None,
        })
        .collect();
    // M45: if the native has no declared refinements and the path has word
    // parts in the tail, this is a data-path select on the native's return
    // value (e.g. `now/year`). Fall back to `GetPath` (runtime resolution)
    // instead of compiling as a refined call.
    if fd.refinements.is_empty() && !leading_refs.is_empty() {
        return None;
    }
    Some((native_idx, Rc::clone(fd), head_sym, leading_refs))
}

/// Compile a user-func call: collect `fd.params.len()` args, emit
/// `CallUser(slot, argc)`. The `slot` is the bound slot index (local or
/// global); M25's `CallUser` handler resolves it to the `Rc<FuncDef>`.
#[allow(clippy::too_many_arguments)] // mixed-lifetime args; see compile_word
fn compile_user_call(
    c: &mut Compiler,
    data: &[Value],
    i: &mut usize,
    scope: &Scope,
    slot: usize,
    arity: usize,
    depth: usize,
    span: Span,
) -> Result<(), CompileError> {
    // Synthesize a minimal FuncDef for arg-collection purposes — the real
    // FuncDef is fetched at runtime by `CallUser`'s slot lookup. We only need
    // arity (params count); refinements on user funcs are rare in the test
    // corpus so M24 collects positional args only.
    let fd = Rc::new(FuncDef {
        params: (0..arity)
            .map(|n| Symbol::new(&format!("__arg{n}")))
            .collect(),
        refinements: Vec::new(),
        locals: Vec::new(),
        freevars: Vec::new(),
        param_types: Vec::new(),
        compiled: None,
        body: Series::empty(),
        ctx: Context::new(),
        native: None,
        variadic: false,
        infix: false,
    });
    // (`compile_prefix` already consumed the calling word.)
    let (argc, _refs) = collect_args(c, data, i, scope, &Symbol::new("__user"), &fd, &[], span)?;
    // M29: if the next value after positional args is a `Value::Refinement`
    // token, this is a user-func called with inline refinements (`f 5 /with
    // 7`). The VM's `CallUser` handler doesn't support refinement dispatch
    // (the synthetic FuncDef above has empty refinements, so `collect_args`
    // skipped them). Fall back to the walker by returning a `CompileError` —
    // `dispatch_block` catches it and runs the walker, which handles user-
    // func refinements correctly. A future milestone can add VM-side
    // user-func refinement dispatch.
    if let Some(Value::Refinement { .. }) = data.get(*i) {
        return Err(CompileError {
            span,
            kind: CompileErrorKind::MalformedSpec,
        });
    }
    // M30.3.4: emit `CallUserGlobal` for depth-0 (global) slots — skips the
    // always-failing `frames.last().and_then(...)` check in `prepare_call`.
    if depth == 0 {
        c.emit(Instr::CallUserGlobal(slot as u32, argc as u32));
    } else {
        c.emit(Instr::CallUser(slot as u32, argc as u32));
    }
    Ok(())
}

/// Collect arguments for a native or user-func call, mirroring
/// `interp::collect_call_args` (lines 769-853). Returns `(argc, refs)`.
///
/// Honors:
/// - **Variadic** natives (`print`/`prin`/`probe`/`return`/`make`/`to`/
///   `cause-error`/`exit`/`quit` — `fd.variadic == true`): collect args until
///   the next value is an unbound `Word`/`GetWord` naming a native, or block end.
/// - **`uneval_first`** natives (`repeat`/`foreach`/`forall`/`make`/`to`/
///   `default`/`module`): first arg is pushed as `Const` (the literal value, not
///   evaluated) — matches the walker, which takes the word/name as-is.
///   (`import` is NOT in this set — `import m` needs `m` evaluated to the
///   module value, while `import 'name` is a LitWord that evaluates to itself.)
/// - **Refinements**: walked in `fd.refinements` spec order; for each, if it
///   appears in `leading_refs` (path form) or the next value is a matching
///   `Value::Refinement` token (spaced form), emit `MarkRefine(ref)` +
///   args + `EndRefine`.
#[allow(clippy::too_many_arguments)] // mixed-lifetime args; see compile_word
fn collect_args(
    c: &mut Compiler,
    data: &[Value],
    i: &mut usize,
    scope: &Scope,
    sym: &Symbol,
    fd: &Rc<FuncDef>,
    leading_refs: &[Symbol],
    span: Span,
) -> Result<(usize, usize), CompileError> {
    // Variadic: collect until next native word or end of block.
    if fd.variadic {
        let mut argc = 0;
        while *i < data.len() && !Compiler::is_native_word_at_dyn(c, data, *i) {
            compile_expr(c, data, i, scope, /*tail*/ false)?;
            argc += 1;
        }
        return Ok((argc, 0));
    }

    let arity = fd.params.len();
    let uneval_first = matches!(
        sym.as_str(),
        "repeat"
            | "foreach"
            | "forall"
            | "for"
            | "forskip"
            | "map-each"
            | "remove-each"
            | "make"
            | "to"
            | "default"
            | "module"
            | "bound?"
            | "bind?"
            | "context-of"
            | "bind-of"
            | "dump"
    );

    // M61: `module` variable-arity peek — 2 args if the next value is a
    // Word-family (the name), 1 arg if it's a Block (the body). Mirrors
    // the walker's `collect_call_args` override.
    let module_arity_override = if sym.as_str() == "module" {
        match data.get(*i) {
            Some(
                Value::Word { .. }
                | Value::GetWord { .. }
                | Value::LitWord { .. }
                | Value::SetWord { .. },
            ) => Some(2),
            Some(Value::Block { .. }) => Some(1),
            _ => None,
        }
    } else {
        None
    };
    // `loop count block` (arity 2) vs `loop block` (arity 1, infinite).
    // Peek the first arg: Integer → 2, Block → 1.
    let loop_arity_override = if sym.as_str() == "loop" {
        match data.get(*i) {
            Some(Value::Integer { .. }) | Some(Value::Float { .. }) => Some(2),
            Some(Value::Block { .. }) | Some(Value::Paren { .. }) => Some(1),
            _ => None,
        }
    } else {
        None
    };
    let arity = module_arity_override
        .or(loop_arity_override)
        .unwrap_or(arity);

    // `set 'word <func-form>`: the `set` native writes a Func value into the
    // named slot at runtime. If the second arg is a literal `func`/`does`/
    // `function`/`make function!` form, peek its arity now and record it for
    // the target slot, so a later `word 5` emits `CallUser` instead of
    // falling back to the walker. (Matches the `SetWord` RHS peek above.)
    if sym.as_str() == "set" && arity >= 2 && !uneval_first {
        record_set_func_arity(c, data, *i);
    }

    let mut argc = 0;
    for n in 0..arity {
        if *i >= data.len() {
            return Err(CompileError {
                span,
                kind: CompileErrorKind::ArityMismatch,
            });
        }
        if n == 0 && uneval_first {
            let idx = c.push_const(data[*i].clone());
            c.emit(Instr::Const(idx));
            *i += 1;
        } else {
            compile_expr(c, data, i, scope, /*tail*/ false)?;
        }
        argc += 1;
    }

    // Refinements in spec order.
    let mut refs_emitted = 0;
    for (ref_name, ref_args_spec) in &fd.refinements {
        let already_leading = leading_refs.iter().any(|r| r == ref_name);
        let mut active = already_leading;
        if !active {
            if let Some(Value::Refinement { sym: rname, .. }) = data.get(*i) {
                if rname == ref_name {
                    *i += 1;
                    active = true;
                }
            }
        }
        if active {
            let idx = c.intern_symbol(ref_name.clone());
            c.emit(Instr::MarkRefine(idx));
            for _ in 0..ref_args_spec.len() {
                if *i >= data.len() {
                    return Err(CompileError {
                        span,
                        kind: CompileErrorKind::ArityMismatch,
                    });
                }
                compile_expr(c, data, i, scope, /*tail*/ false)?;
            }
            c.emit(Instr::EndRefine);
            refs_emitted += 1;
        }
    }

    Ok((argc, refs_emitted))
}

impl<'a> Compiler<'a> {
    /// Dynamic form of `is_native_word_at` (takes `&self` for the registry).
    fn is_native_word_at_dyn(&self, data: &[Value], i: usize) -> bool {
        let sym = match &data[i] {
            Value::Word { sym, binding, .. } | Value::GetWord { sym, binding, .. } => {
                if !matches!(binding, Binding::Unbound) {
                    return false;
                }
                sym
            }
            _ => return false,
        };
        self.natives.contains(sym)
    }
}

// ---------------------------------------------------------------------------
// `MakeFunc` emission
// ---------------------------------------------------------------------------

/// Compile a `func`/`does`/`function` form: push spec + body blocks into the
/// pool, then emit `MakeFunc(spec_idx, body_idx, freevars)`. The freevar list
/// comes from a recursive `analyze_block` on the body with a fresh child scope
/// (matching M23's `analyze_func_form`).
///
/// Pre: `*i` points just past the calling word (`func`/`does`/`function`).
/// For `does`, the next value is the body; for `func`/`function`, the next
/// two are spec + body.
fn compile_make_func(
    c: &mut Compiler,
    data: &[Value],
    i: &mut usize,
    scope: &Scope,
    calling_word: &Value,
    span: Span,
) -> Result<(), CompileError> {
    let is_does = matches!(
        calling_word,
        Value::Word { sym, .. } if sym.as_str() == "does"
    );
    let (spec_val, body_val);
    if is_does {
        if *i >= data.len() {
            return Err(CompileError {
                span,
                kind: CompileErrorKind::MalformedSpec,
            });
        }
        body_val = data[*i].clone();
        spec_val = Value::block(Series::empty());
    } else {
        if *i + 1 >= data.len() {
            return Err(CompileError {
                span,
                kind: CompileErrorKind::MalformedSpec,
            });
        }
        spec_val = data[*i].clone();
        body_val = data[*i + 1].clone();
    }

    // Validate both are blocks.
    if !matches!(spec_val, Value::Block { .. }) || !matches!(body_val, Value::Block { .. }) {
        return Err(CompileError {
            span,
            kind: CompileErrorKind::MalformedSpec,
        });
    }

    // Compute freevars via a recursive analyze on the body (child scope).
    // M23 already did this once on the *outer* block; redoing it here gives
    // us the body's own freevar list (not the parent's).
    let spec = if is_does {
        crate::natives::FuncSpec {
            params: Vec::new(),
            refinements: Vec::new(),
            locals: Vec::new(),
            param_types: Vec::new(),
        }
    } else {
        extract_spec(&spec_val).map_err(|_| CompileError {
            span,
            kind: CompileErrorKind::MalformedSpec,
        })?
    };
    let mut child = Scope::child(scope);
    for p in &spec.params {
        child.slot_index(p.clone());
    }
    for (ref_name, ref_args) in &spec.refinements {
        child.slot_index(ref_name.clone());
        for arg in ref_args {
            child.slot_index(arg.clone());
        }
    }
    for local in &spec.locals {
        child.slot_index(local.clone());
    }
    let body_series = match &body_val {
        Value::Block { series, .. } => series.clone(),
        // M31: was `unreachable!()`. The caller (`compile_make_func`) is
        // `pub` and reachable from tests / future code paths; a non-Block
        // body (e.g. a `does` whose argument was a `Paren` or `String`)
        // should surface as a recoverable `CompileError` (MalformedBody)
        // rather than a release panic. The runtime `func`/`does` natives
        // already produce a clearer `EvalError::TypeError`; this is the
        // compile-time bail-out equivalent.
        _ => {
            return Err(CompileError {
                span: body_val.span_or_default(),
                kind: CompileErrorKind::MalformedBody,
            });
        }
    };
    // Pre-collect body SetWords (mirrors `analyze_func_form`).
    collect_setwords_inline(&body_series, &mut child);
    let body_analysis = analyze_block(&body_series, &mut child);

    // Push spec + body into the pool.
    let spec_idx = c.push_const(spec_val);
    let body_idx = c.push_const(body_val);
    let fv_idx = c.intern_freevars(body_analysis.freevars);
    c.emit(Instr::MakeFunc(spec_idx, body_idx, fv_idx));

    // Record the func's arity in the table so subsequent `CallUser`s to this
    // slot know how many args to collect. The slot is the *enclosing SetWord's*
    // slot — we don't have it here; the caller's `compile_prefix` (which
    // handled the SetWord) records it. For recursive self-calls inside the
    // body, the body would need its own slot table; M24 relies on the
    // global-slot path (recursive `fact` resolves as global → recorded by
    // the outer SetWord's MakeFunc).
    Ok(())
}

/// M60: compile a `closure [spec] [body]` form. Like `compile_make_func` but:
/// - Opens a `Scope::child_closure` (with `is_closure=true` + `closure_name`)
///   so `attach_lexical` captures outer-scope words into `result.captures`
///   and sets `Binding::Closure(idx)` on body freevar words.
/// - Stores the captures list `Vec<(Symbol, depth, slot)>` in the
///   `CompiledBlock.captures_table` side table.
/// - Emits `Instr::MakeClosure(spec_idx, body_idx, captures_idx)`.
///
/// Pre: `*i` points just past the `closure` calling word (at the spec block).
fn compile_make_closure(
    c: &mut Compiler,
    data: &[Value],
    i: &mut usize,
    scope: &Scope,
    closure_name: Option<Symbol>,
    span: Span,
) -> Result<(), CompileError> {
    if *i + 1 >= data.len() {
        return Err(CompileError {
            span,
            kind: CompileErrorKind::MalformedSpec,
        });
    }
    let spec_val = data[*i].clone();
    let body_val = data[*i + 1].clone();
    if !matches!(spec_val, Value::Block { .. }) || !matches!(body_val, Value::Block { .. }) {
        return Err(CompileError {
            span,
            kind: CompileErrorKind::MalformedSpec,
        });
    }
    let spec = extract_spec(&spec_val).map_err(|_| CompileError {
        span,
        kind: CompileErrorKind::MalformedSpec,
    })?;
    // Open a closure-aware child scope so `attach_lexical` captures outer
    // words instead of emitting `Lexical`/leaving `Local`.
    let mut child = Scope::child_closure(scope, closure_name);
    for p in &spec.params {
        child.slot_index(p.clone());
    }
    for (ref_name, ref_args) in &spec.refinements {
        child.slot_index(ref_name.clone());
        for arg in ref_args {
            child.slot_index(arg.clone());
        }
    }
    for local in &spec.locals {
        child.slot_index(local.clone());
    }
    let body_series = match &body_val {
        Value::Block { series, .. } => series.clone(),
        _ => {
            return Err(CompileError {
                span: body_val.span_or_default(),
                kind: CompileErrorKind::MalformedBody,
            });
        }
    };
    collect_setwords_inline(&body_series, &mut child);
    let body_analysis = analyze_block(&body_series, &mut child);

    let spec_idx = c.push_const(spec_val);
    let body_idx = c.push_const(body_val);
    let cap_idx = c.intern_captures(body_analysis.captures);
    c.emit(Instr::MakeClosure(spec_idx, body_idx, cap_idx));
    Ok(())
}

// ---------------------------------------------------------------------------
// Load / store emission
// ---------------------------------------------------------------------------

/// Emit a load instr for a `Word`/`GetWord` based on its `Binding`:
/// - `Lexical(d, slot)` → `LoadLocal(d, slot)`
/// - `Local(_, slot)` → `LoadGlobal(slot)` (user-ctx global)
/// - `Unbound` → `LoadDynamic(sym)` (resolved at VM entry from `env.user_ctx`)
/// - `Closure(idx)` → `LoadCapture(idx)` (M60)
fn emit_load(
    c: &mut Compiler,
    binding: &Binding,
    _span: Span,
    sym: &Symbol,
) -> Result<(), CompileError> {
    match binding {
        Binding::Lexical(d, s) => c.emit(Instr::LoadLocal(*d as u32, *s as u32)),
        Binding::Local(_ctx, s) => c.emit(Instr::LoadGlobal(*s as u32)),
        Binding::Unbound => {
            let idx = c.intern_symbol(sym.clone());
            c.emit(Instr::LoadDynamic(idx));
        }
        Binding::Func(s) => c.emit(Instr::LoadLocal(0, *s as u32)), // defensive
        Binding::Closure(idx) => c.emit(Instr::LoadCapture(*idx as u32)),
    }
    Ok(())
}

/// Emit a store instr for a `SetWord` based on its `Binding`:
/// - `Lexical(d, slot)` → `SetLocal(d, slot)`
/// - `Local(_, slot)` → `SetGlobal(slot)`
/// - `Unbound` → `SetDynamic(sym)`
/// - `Closure(idx)` → `SetCapture(idx)` (M60)
fn emit_store(
    c: &mut Compiler,
    binding: &Binding,
    _span: Span,
    sym: &Symbol,
) -> Result<(), CompileError> {
    match binding {
        Binding::Lexical(d, s) => c.emit(Instr::SetLocal(*d as u32, *s as u32)),
        Binding::Local(_ctx, s) => c.emit(Instr::SetGlobal(*s as u32)),
        Binding::Unbound => {
            let idx = c.intern_symbol(sym.clone());
            c.emit(Instr::SetDynamic(idx));
        }
        Binding::Func(s) => c.emit(Instr::SetLocal(0, *s as u32)), // defensive
        Binding::Closure(idx) => c.emit(Instr::SetCapture(*idx as u32)),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// If `data[i]` begins a `func`/`does`/`function` form, return the param
/// count (arity) by peeking at the spec block. Used by the SetWord arm so a
/// subsequent `CallUser` to the same slot knows how many args to collect.
/// Returns `None` for non-func forms or malformed specs (the runtime native
/// reports the error later).
fn peek_func_arity(data: &[Value], i: usize) -> Option<usize> {
    // `func`/`does`/`function` forms.
    if let Some(_skip) = func_form_skip(data, i) {
        let is_does = matches!(
            &data[i],
            Value::Word { sym, .. } if sym.as_str() == "does"
        );
        if is_does {
            return Some(0);
        }
        let spec_val = &data[i + 1];
        return extract_spec(spec_val).ok().map(|s| s.params.len());
    }
    // `make function! [[params][body]]` — the `make` native creates a FuncDef
    // at runtime, but we need the arity at compile time so subsequent calls
    // emit `CallUser` instead of falling back to `LoadGlobal`. (M29)
    if is_make_function_form(data, i) {
        let packed = &data[i + 2];
        if let Value::Block { series, .. } = packed {
            let inner = series.data.borrow();
            // The packed block is `[spec body]` — two blocks.
            if !inner.is_empty() {
                if let Value::Block { .. } = &inner[series.index] {
                    return extract_spec(&inner[series.index])
                        .ok()
                        .map(|s| s.params.len());
                }
            }
        }
    }
    None
}

/// Does the RHS starting at `data[i]` resolve to a `Value::Func` at runtime
/// without the compiler being able to see its arity? Returns true for:
/// - `:word` (GetWord) — fetches a Func value without invoking.
/// - `get 'word` / `get word` — the `get` native returns a slot's value,
///   which is frequently a Func (the canonical way to fetch a function value
///   for higher-order use).
///
/// Used by the SetWord arm to mark the target slot as `dynamic_func_slots`,
/// so a later operator-position reference (`f 10`) falls back to the walker
/// instead of emitting a plain load that would return `#[function]`.
///
/// Returns false for `func`/`does`/`make function!` forms (those have their
/// arity peeked by `peek_func_arity` and go through `CallUser`), and for
/// everything else (the slot holds a non-Func value).
/// Does the RHS starting at `data[i]` resolve to a `Value::Func`/
/// `Value::Closure` at runtime without the compiler being able to see its
/// arity? Returns true for:
/// - `:word` (GetWord) — fetches a Func value without invoking.
/// - `get 'word` / `get word` — the `get` native returns a slot's value,
///   which is frequently a Func (the canonical way to fetch a function value
///   for higher-order use).
/// - M60: `word args...` (a `CallUser` whose result might be a func/closure,
///   e.g. `add5: make-adder 5` where `make-adder` returns a closure). The
///   compiler can't see the return type, so mark the slot dynamic to force
///   the walker to dispatch at runtime.
///
/// Used by the SetWord arm to mark the target slot as `dynamic_func_slots`,
/// so a later operator-position reference (`f 10`) falls back to the walker
/// instead of emitting a plain load that would return `#[function]`/
/// `#[closure]`.
///
/// Returns false for `func`/`does`/`make function!`/`closure` forms (those
/// have their arity peeked by `peek_func_arity` and go through `CallUser`).
fn rhs_might_be_func(data: &[Value], i: usize) -> bool {
    match &data[i] {
        Value::GetWord { .. } => true,
        Value::Word { sym, binding, .. } => {
            // `get 'word` / `get word` — the `get` native.
            if matches!(binding, Binding::Unbound)
                && sym.as_str() == "get"
                && data.get(i + 1).is_some_and(|arg| {
                    matches!(
                        arg,
                        Value::LitWord { .. } | Value::Word { .. } | Value::GetWord { .. }
                    )
                })
            {
                return true;
            }
            // M60: a bound or unbound word in operator position (a `CallUser`
            // or native call) whose result might be a func/closure. Exclude
            // `func`/`does`/`function`/`closure` forms (arity recorded by
            // `peek_func_arity`) and `make function!` (arity recorded).
            if func_form_skip(data, i).is_some() || is_make_function_form(data, i) {
                return false;
            }
            // Require at least one trailing arg (a word with no args is just a
            // value load, not a func-producing call).
            matches!(
                binding,
                Binding::Local(_, _) | Binding::Lexical(_, _) | Binding::Unbound
            ) && data.get(i + 1).is_some()
        }
        _ => false,
    }
}

/// `set 'word <func-form>`: peek the func arity of the second arg and record
/// it for the slot named by the first arg (a LitWord). `i` points at the
/// first arg (`'word`); the func-form is at `i + 1`. Looks up the word's
/// global slot via `user_ctx` — `set` writes through `user_ctx.set_slot`, so
/// the slot must already exist (the word appeared as a SetWord at parse time,
/// per the `set` native's contract). No-op if the word isn't bound or the
/// second arg isn't a literal func form (those cases fall back to the walker
/// via `dynamic_func_slots`/runtime dispatch).
fn record_set_func_arity(c: &mut Compiler, data: &[Value], i: usize) {
    let word_sym = match &data[i] {
        Value::LitWord { sym, .. } => sym,
        _ => return,
    };
    let Some(func_arity) = peek_func_arity(data, i + 1) else {
        return;
    };
    let Some(ref user_ctx) = c.natives.user_ctx else {
        return;
    };
    let names = user_ctx.names.borrow();
    if let Some(&slot) = names.get(word_sym) {
        c.func_arities.record(0, slot, func_arity);
    }
}

/// Extract `(depth, slot)` from a `Binding` for `FuncArityTable` keying.
/// `Lexical(d, s)` and `Local(_, s)` map to `(d, s)`; `Func(s)` maps to
/// `(0, s)` (function-local slot, resolved via the active call frame).
fn slot_coords(binding: &Binding) -> (usize, usize) {
    match binding {
        Binding::Lexical(d, s) => (*d, *s),
        Binding::Local(_, s) => (0, *s),
        Binding::Func(s) => (0, *s),
        // M60: closure captures don't participate in func-arity keying —
        // a captured word is read via LoadCapture, not CallUser.
        Binding::Closure(_) => (0, 0),
        Binding::Unbound => (0, 0),
    }
}

/// Estimate the source span of a `Series` (used for `CompiledBlock.source_span`).
/// M24 doesn't need an exact span — just a fallback; M31 (disassembler) will
/// thread precise spans through.
pub(crate) fn block_source_span(block: &Series) -> Span {
    let data = block.data.borrow();
    let first = data
        .first()
        .map(|v| v.span_or_default())
        .unwrap_or_default();
    let last = data.last().map(|v| v.span_or_default()).unwrap_or_default();
    if first == Span::default() && last == Span::default() {
        Span::default()
    } else {
        Span::new(first.start, last.end.max(first.end))
    }
}

/// Count the locals in a scope (for `CompiledBlock.n_locals`). M23's `Scope`
/// doesn't expose this directly; we approximate via the root scope's child
/// slot count when possible. For the top-level block this is 0 (no
/// function-local slots); for a func body it's `params + refinements + locals`.
fn scope_locals_count(scope: &Scope) -> usize {
    // For a func body (depth >= 1), the scope's slot count is params +
    // refinements + locals + body-local SetWords — the frame's `locals` Vec
    // size at `CallUser` time. For the top-level script body (depth 0) there
    // are no function-local slots; all words live in the user context.
    if scope.depth() == 0 {
        0
    } else {
        scope.slot_count()
    }
}

/// Detect `make object! [spec]` — mirrors `lex.rs`'s `is_make_object_form`.
fn is_make_object_form(data: &[Value], i: usize) -> bool {
    if i + 2 >= data.len() {
        return false;
    }
    let Value::Word {
        sym: make_sym,
        binding: Binding::Unbound,
        ..
    } = &data[i]
    else {
        return false;
    };
    if make_sym.as_str() != "make" {
        return false;
    }
    matches!(
        &data[i + 1],
        Value::Word { sym, .. } | Value::LitWord { sym, .. }
            if sym.as_str() == "object!" || sym.as_str() == "object"
    ) && matches!(&data[i + 2], Value::Block { .. })
}

/// Detect `make function! [[params][body]]` — used by `peek_func_arity` so
/// `f: make function! [...] f 5` emits `CallUser` instead of `LoadGlobal`.
/// (M29)
fn is_make_function_form(data: &[Value], i: usize) -> bool {
    if i + 2 >= data.len() {
        return false;
    }
    let Value::Word {
        sym: make_sym,
        binding: Binding::Unbound,
        ..
    } = &data[i]
    else {
        return false;
    };
    if make_sym.as_str() != "make" {
        return false;
    }
    matches!(
        &data[i + 1],
        Value::Word { sym, .. } | Value::LitWord { sym, .. }
            if sym.as_str() == "function!"
    ) && matches!(&data[i + 2], Value::Block { .. })
}

/// Detect `object [spec]` / `context [spec]` — mirrors `lex.rs`.
fn is_object_keyword_form(data: &[Value], i: usize) -> bool {
    if i + 1 >= data.len() {
        return false;
    }
    let Value::Word {
        sym,
        binding: Binding::Unbound,
        ..
    } = &data[i]
    else {
        return false;
    };
    matches!(sym.as_str(), "object" | "context") && matches!(&data[i + 1], Value::Block { .. })
}

/// M31: public wrapper around `compile_block_for_func_body` for
/// `disasm_source` (in `interp_runner.rs`), which compiles a named func's
/// body with the func's own slot pre-recorded (so recursive calls emit
/// `CallUser`/`CallUserGlobal` instead of degrading to `LoadGlobal`).
pub(crate) fn compile_block_for_func_body_pub(
    block: &Series,
    scope: &mut Scope,
    natives: &NativeRegistry,
    self_func: (u32, usize),
    param_count: usize,
) -> Result<CompiledBlock, CompileError> {
    compile_block_for_func_body(block, scope, natives, self_func, param_count)
}

/// M31: public wrapper around `collect_setwords_inline` for `disasm_source`
/// (in `interp_runner.rs`), which needs to pre-collect a func body's
/// SetWords into a child scope before compiling — mirroring what
/// `compile_make_func` does internally. Without this, the func body's
/// body-local SetWords wouldn't resolve to `Lexical(0, slot)` and the
/// disassembly would show `LoadDynamic` for them.
pub(crate) fn collect_setwords_inline_pub(series: &Series, scope: &mut Scope) {
    collect_setwords_inline(series, scope);
}

/// Mirror of `lex.rs`'s `collect_setwords` — pre-collect body-local SetWords
/// into a child scope so subsequent references resolve to `Lexical(0, slot)`.
/// `lex.rs` keeps this private; we duplicate the small body here so the
/// compiler doesn't need to reach into `lex.rs`'s internals.
fn collect_setwords_inline(series: &Series, scope: &mut Scope) {
    use crate::binding::{func_form_skip, use_body_index};
    let data = series.data.borrow();
    let n = data.len();
    let mut i = 0;
    while i < n {
        if use_body_index(&data, i).is_some() {
            i += 3;
            continue;
        }
        if is_make_object_form(&data, i) {
            i += 3;
            continue;
        }
        if is_object_keyword_form(&data, i) {
            i += 2;
            continue;
        }
        if let Some(skip) = func_form_skip(&data, i) {
            i += skip;
            continue;
        }
        match &data[i] {
            Value::SetWord { sym, .. } => {
                if scope.lookup(sym).is_none() {
                    scope.slot_index(sym.clone());
                }
                i += 1;
            }
            Value::Block { series: s, .. } | Value::Paren { series: s, .. } => {
                let child = s.clone();
                collect_setwords_inline(&child, scope);
                i += 1;
            }
            _ => i += 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::bind_pass;
    use crate::natives::{install_constants, register_natives};
    use red_core::parser::load_source;
    use red_core::value::Value;
    use red_core::vm_ir::Instr;
    use red_core::{Context, Env};

    /// Build a fresh `Env` with natives + constants registered, run `bind_pass`
    /// on `src`, then return the body, the user-ctx `Rc<Context>`, and the
    /// `NativeRegistry` snapshot. Mirrors `lex.rs`'s `parse_and_bind` plus the
    /// native registry setup the compiler needs.
    fn parse_bind_and_registry(src: &str) -> (Series, Rc<Context>, NativeRegistry) {
        let body = load_source(src).expect("parse failed");
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let mut env = Env::new(Rc::clone(&ctx_rc));
        register_natives(&mut env);
        let registry = NativeRegistry::from_env(&env);
        (body, ctx_rc, registry)
    }

    /// Assert two instr slices are equal (by Debug string, since `Instr`
    /// doesn't derive `PartialEq`).
    fn assert_instrs(got: &[Instr], want: &[Instr], msg: &str) {
        assert_eq!(
            format!("{got:?}"),
            format!("{want:?}"),
            "{msg}: got {got:#?}\nwant {want:#?}"
        );
    }

    // --- Plan-required tests (docs/plans/plan3.md:307-318) --------------------------

    /// `5` -> `[ConstInt(5), Return]`. (docs/plans/plan3.md:307)
    ///
    /// M30: integer literals now emit `ConstInt(n)` (small-value fast path)
    /// instead of `Const(idx)` + a pool entry, skipping the pool indirection
    /// on the hot `Const` arm. The pool stays empty for an all-integer
    /// literal block.
    #[test]
    fn compile_literal() {
        let (body, ctx_rc, registry) = parse_bind_and_registry("5");
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile");
        assert!(!block.needs_rebind);
        assert_instrs(
            block.instrs.as_ref(),
            &[Instr::ConstInt(5), Instr::Return],
            "compile `5`",
        );
        assert_eq!(block.pool.len(), 0);
    }

    /// `foo: 5 foo` -> `[ConstInt(5), SetGlobal(slot), Pop, LoadGlobal(slot), Return]`.
    /// (docs/plans/plan3.md:308 — originally expected no `Pop`, but M25 adds `Pop` after
    /// non-last expressions to keep the VM stack disciplined. The SetWord is
    /// not the last expression, so its pushed-back value is popped before the
    /// `foo` load. The walker's `last = ...` overwrite is the equivalent.)
    ///
    /// M30: the `5` literal now emits `ConstInt(5)` (no pool entry).
    #[test]
    fn compile_setword_then_load() {
        let (body, ctx_rc, registry) = parse_bind_and_registry("foo: 5 foo");
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile");
        // `foo` is bound by `bind_pass` to user-ctx slot 0 (the first setword
        // after constants). Find its slot.
        let foo_slot = ctx_rc
            .names
            .borrow()
            .get(&Symbol::new("foo"))
            .copied()
            .expect("foo should be bound");
        assert_instrs(
            block.instrs.as_ref(),
            &[
                Instr::ConstInt(5),
                Instr::SetGlobal(foo_slot as u32),
                Instr::Pop,
                Instr::LoadGlobal(foo_slot as u32),
                Instr::Return,
            ],
            "compile `foo: 5 foo`",
        );
        assert_eq!(block.pool.len(), 0);
    }

    /// `1 + 2` -> `[ConstInt(1), ConstInt(2), Call(+, 2), Return]`. (docs/plans/plan3.md:310)
    ///
    /// M30: both operands are `ConstInt` (no pool entries).
    #[test]
    fn compile_infix_call() {
        let (body, ctx_rc, registry) = parse_bind_and_registry("1 + 2");
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile");
        let (plus_idx, _) = registry
            .get(&Symbol::new("+"))
            .expect("`+` native registered");
        assert_instrs(
            block.instrs.as_ref(),
            &[
                Instr::ConstInt(1),
                Instr::ConstInt(2),
                Instr::Call(plus_idx, 2),
                Instr::Return,
            ],
            "compile `1 + 2`",
        );
        assert_eq!(block.pool.len(), 0);
    }

    /// `if true [42]` -> `[LoadGlobal(true_slot), JumpIfFalse(4), ConstInt(42), Jump(5), ConstNone, Return]`.
    /// (docs/plans/plan3.md:312 expected `Const(true)`, but `true` is a context-stored constant
    /// via `install_constants`, so the compiler emits `LoadGlobal` — matching the
    /// walker, which resolves `true` as a word bound to the user context.)
    ///
    /// M30: `42` is now `ConstInt(42)` and the false-branch `none` is now
    /// `ConstNone` (both small-value fast paths, no pool entries).
    #[test]
    fn compile_if_true() {
        let (body, ctx_rc, registry) = parse_bind_and_registry("if true [42]");
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile");
        let true_slot = ctx_rc
            .names
            .borrow()
            .get(&Symbol::new("true"))
            .copied()
            .expect("`true` should be bound");
        // M30 instr layout (with `ConstNone` for the false branch):
        //   0: LoadGlobal(true_slot)  ; load `true` constant
        //   1: JumpIfFalse(4)         ; L_none = index 4
        //   2: ConstInt(42)           ; 42 (then-block)
        //   3: Jump(5)                ; skip the `none` push → L_end
        //   4: ConstNone              ; none (false branch) — L_none
        //   5: Return                 ; L_end
        assert_instrs(
            block.instrs.as_ref(),
            &[
                Instr::LoadGlobal(true_slot as u32),
                Instr::JumpIfFalse(4),
                Instr::ConstInt(42),
                Instr::Jump(5),
                Instr::ConstNone,
                Instr::Return,
            ],
            "compile `if true [42]`",
        );
        assert_eq!(block.pool.len(), 0);
    }

    /// `if 1 [42]` exercises the pure-Const(cond) path (literal cond, not a
    /// context-stored constant like `true`). Verifies the docs/plans/plan3.md:312
    /// instr shape with the M29 `none` push for the false branch.
    ///
    /// M30: `1`/`42` are `ConstInt`, `none` is `ConstNone`.
    #[test]
    fn compile_if_literal_cond() {
        let (body, ctx_rc, registry) = parse_bind_and_registry("if 1 [42]");
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile");
        // M30 layout:
        //   0: ConstInt(1)    ; 1 (cond)
        //   1: JumpIfFalse(4)  ; L_none = index 4
        //   2: ConstInt(42)    ; 42 (then-block)
        //   3: Jump(5)     ; skip `none` → L_end
        //   4: ConstNone   ; none (false branch)
        //   5: Return      ; L_end
        assert_instrs(
            block.instrs.as_ref(),
            &[
                Instr::ConstInt(1),
                Instr::JumpIfFalse(4),
                Instr::ConstInt(42),
                Instr::Jump(5),
                Instr::ConstNone,
                Instr::Return,
            ],
            "compile `if 1 [42]`",
        );
        assert_eq!(block.pool.len(), 0);
    }

    /// `func [x][x * x]` emits `MakeFunc` with freevars=[]. (docs/plans/plan3.md:314)
    #[test]
    fn compile_func_makefunc() {
        let (body, ctx_rc, registry) = parse_bind_and_registry("square: func [x][x * x]");
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile");
        // Locate the MakeFunc instr.
        let makefunc = block
            .instrs
            .iter()
            .find(|i| matches!(i, Instr::MakeFunc(_, _, _)))
            .expect("MakeFunc should be emitted");
        match makefunc {
            Instr::MakeFunc(_spec_idx, _body_idx, fv_idx) => {
                // M30.1.B: `fv_idx` is an index into `block.freevars_table`.
                let fv = &block.freevars_table[*fv_idx as usize];
                assert!(fv.is_empty(), "square should have no freevars, got {fv:?}");
            }
            _ => unreachable!(),
        }
    }

    /// A recursive factorial emits `CallUser(0, 1)` referencing its own slot. (docs/plans/plan3.md:316)
    #[test]
    fn compile_recursive_factorial_calluser() {
        // `fact: func [n][either n <= 1 [1][n * fact n - 1]]`
        // `fact` is a top-level SetWord (global slot), so the recursive call
        // resolves as a global LoadGlobal + CallUser with the fact slot.
        let (body, ctx_rc, registry) =
            parse_bind_and_registry("fact: func [n][either n <= 1 [1][n * fact n - 1]]");
        let mut scope = Scope::root(&ctx_rc);
        let _block = compile_block(&body, &mut scope, &registry).expect("compile");
        // The outer block compiles to: MakeFunc(...) + Return. The func body
        // is *not* compiled here — MakeFunc caches the body block; M25
        // lazily compiles it on first invocation. To verify CallUser emission,
        // we separately compile the body with a child scope.
        let fact_slot = ctx_rc
            .names
            .borrow()
            .get(&Symbol::new("fact"))
            .copied()
            .expect("fact should be bound");
        // Find the func body in the source: [fact: func [n] [body]] — index 3.
        let data = body.data.borrow();
        let Value::Block {
            series: func_body, ..
        } = &data[3]
        else {
            panic!("expected func body block at index 3");
        };
        // Manually record fact's slot as a global arity-1 func (simulating
        // what the SetWord path would have done in a full compile).
        let mut child = Scope::child(&Scope::root(&ctx_rc));
        child.slot_index(Symbol::new("n"));
        let mut c = Compiler {
            instrs: Vec::new(),
            spans: Vec::new(),
            current_span: Span::default(),
            pool: ConstantPool::new(),
            natives: &registry,
            func_arities: FuncArityTable::default(),
            symbols: Vec::new(),
            freevars_table: Vec::new(),
            captures_table: Vec::new(),
            dynamic_func_slots: HashSet::new(),
        };
        c.func_arities.record(0, fact_slot, 1); // fact is global arity-1
        let body_data = func_body.data.borrow();
        let n = body_data.len();
        let mut i = func_body.index;
        while i < n {
            let is_last = i + 1 == n;
            compile_expr(&mut c, &body_data, &mut i, &child, is_last).expect("compile body");
        }
        drop(body_data);
        drop(data);
        c.emit(Instr::Return);
        // Assert the body's instrs contain `CallUser(fact_slot, 1)` or
        // `CallUserGlobal(fact_slot, 1)` (M30.3.4: globals emit the latter).
        let has_calluser = c.instrs.iter().any(|instr| {
            matches!(
                instr,
                Instr::CallUser(slot, argc) | Instr::CallUserGlobal(slot, argc)
                if *slot as usize == fact_slot && *argc == 1
            )
        });
        assert!(
            has_calluser,
            "fact body should contain CallUser({}, 1) or CallUserGlobal, got {:?}",
            fact_slot, c.instrs
        );
    }

    // --- Extra sanity tests -----------------------------------------------

    /// `either true [1][2]` — both branches present with `JumpIfFalse` + `Jump`.
    #[test]
    fn compile_either() {
        let (body, ctx_rc, registry) = parse_bind_and_registry("either true [1][2]");
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile");
        let has_jfalse = block
            .instrs
            .iter()
            .any(|i| matches!(i, Instr::JumpIfFalse(_)));
        let has_jump = block.instrs.iter().any(|i| matches!(i, Instr::Jump(_)));
        assert!(has_jfalse, "either should emit JumpIfFalse");
        assert!(has_jump, "either should emit Jump");
    }

    /// `foo` (unbound) → `LoadDynamic(foo)`.
    #[test]
    fn compile_unbound_word() {
        let (body, ctx_rc, registry) = parse_bind_and_registry("foo");
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile");
        // M30.1.B: `LoadDynamic` now carries a `u32` symbol-table index
        // instead of the `Symbol` itself. `foo` is the first symbol interned
        // into the block's `symbols` table → index 0.
        assert_instrs(
            block.instrs.as_ref(),
            &[Instr::LoadDynamic(0), Instr::Return],
            "compile `foo`",
        );
        assert_eq!(block.symbols.len(), 1);
        assert_eq!(block.symbols[0].as_str(), "foo");
    }

    /// `use [x][x: 1 x]` → `needs_rebind == true`, instrs `[Halt]`.
    #[test]
    fn compile_needs_rebind_use() {
        let (body, ctx_rc, registry) = parse_bind_and_registry("use [x][x: 1 x]");
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile");
        assert!(block.needs_rebind, "use should set needs_rebind");
        assert_instrs(block.instrs.as_ref(), &[Instr::Halt], "use stub");
    }

    /// `does [42]` → `MakeFunc` with empty spec.
    #[test]
    fn compile_does_makefunc() {
        let (body, ctx_rc, registry) = parse_bind_and_registry("noop: does [42]");
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile");
        let makefunc = block
            .instrs
            .iter()
            .find(|i| matches!(i, Instr::MakeFunc(_, _, _)))
            .expect("MakeFunc should be emitted for does");
        if let Instr::MakeFunc(spec_idx, _body_idx, fv_idx) = makefunc {
            let fv = &block.freevars_table[*fv_idx as usize];
            assert!(
                fv.is_empty(),
                "does body should have no freevars, got {fv:?}"
            );
            // The does spec is an empty block — pool[spec_idx] is `Block([])`.
            let spec_val = &block.pool[*spec_idx as usize];
            assert!(
                matches!(spec_val, Value::Block { .. }),
                "does spec is a block"
            );
        }
    }
}
