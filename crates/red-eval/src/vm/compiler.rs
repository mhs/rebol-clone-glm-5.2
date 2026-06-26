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

use std::collections::HashMap;
use std::rc::Rc;

use red_core::value::{Binding, FuncDef, Series, Span, Symbol, Value};
use red_core::vm_ir::{CompiledBlock, Instr};
use red_core::{Context, Env};

use crate::binding::{func_form_skip, use_body_index};
use crate::natives::extract_spec;
use crate::vm::lex::{AnalysisResult, Scope, analyze_block};
use crate::vm::pool::ConstantPool;

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
}

impl NativeRegistry {
    /// Build a snapshot from `env.natives`. Stable insertion order (the
    /// `HashMap` iteration order is deterministic within a single process
    /// run — for the M24 inline tests this is fine; M27's cache invalidation
    /// handles cross-run drift).
    pub fn from_env(env: &Env) -> Self {
        let mut map = HashMap::new();
        let mut idx: u32 = 0;
        for (sym, fd) in env.natives.iter() {
            map.insert(sym.clone(), (idx, Rc::clone(fd)));
            idx += 1;
        }
        Self { map }
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
/// source value so the CLI's `render_error` can localize it (M29+).
#[derive(Debug)]
pub struct CompileError {
    pub span: Span,
    pub kind: CompileErrorKind,
}

#[derive(Debug)]
pub enum CompileErrorKind {
    /// A `Word` in operator position (followed by args) couldn't be resolved
    /// to a native or a known user-func — so the compiler can't determine
    /// arity. The tree-walker would defer this to runtime; M24 surfaces it.
    UnboundInOperatorPosition,
    /// `func`/`does`/`function` invoked with non-block args (malformed spec
    /// or body). The runtime native reports a clearer error; M24 bails.
    MalformedSpec,
    /// Too few values remaining in the block to satisfy a native/func's arity.
    ArityMismatch,
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
}

/// Compile state: the in-progress instr stream + pool + reference data.
struct Compiler<'a> {
    instrs: Vec<Instr>,
    pool: ConstantPool,
    natives: &'a NativeRegistry,
    /// Slot of the enclosing `func` being defined (for recursive self-call
    /// detection — set by `compile_make_func`, read by `compile_word`).
    func_arities: FuncArityTable,
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
    compile_block_inner(block, scope, natives, None)
}

/// Like `compile_block` but pre-seeds the `FuncArityTable` with the enclosing
/// function's own slot (`self_func` = `(slot, arity)`), so recursive self-calls
/// inside the body emit `CallUser(slot, arity)` instead of degrading to
/// `LoadDynamic`. Used by the M25 VM's lazy func-body compilation path.
pub(crate) fn compile_block_for_func_body(
    block: &Series,
    scope: &mut Scope,
    natives: &NativeRegistry,
    self_func: (u32, usize),
) -> Result<CompiledBlock, CompileError> {
    compile_block_inner(block, scope, natives, Some(self_func))
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
        pool: ConstantPool::new(),
        natives,
        func_arities,
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
    compiler.emit(Instr::Return);
    let span = block_source_span(block);
    Ok(CompiledBlock {
        instrs: Rc::from(compiler.instrs.as_slice()),
        pool: compiler.pool.into_rc(),
        n_locals: scope_locals_count(scope),
        freevars: analysis.freevars,
        source_span: span,
        needs_rebind: false,
        arity: 0,
    })
}

fn compile_block_inner(
    block: &Series,
    scope: &mut Scope,
    natives: &NativeRegistry,
    self_func: Option<(u32, usize)>,
) -> Result<CompiledBlock, CompileError> {
    // Phase 1: lexical analysis (M23). Attaches `Binding::Lexical` to
    // function-local words and computes `freevars` + `needs_rebind`.
    let analysis = analyze_block(block, scope);

    // `needs_rebind` short-circuit: emit a stub. The VM will defer to the
    // walker for this block (and any nested `use`/object forms).
    if analysis.needs_rebind {
        return Ok(stub_block(block, analysis));
    }

    let mut func_arities = FuncArityTable::default();
    if let Some((slot, arity)) = self_func {
        // Self-recursion: the func's own slot is at depth 0 (global) when the
        // SetWord defining it is top-level; M25's lazy-compile path passes the
        // actual slot. The depth is 0 relative to the body's *parent* scope,
        // which is what the body's `Scope::child` parent represents.
        func_arities.record(0, slot as usize, arity);
    }
    let mut compiler = Compiler {
        instrs: Vec::new(),
        pool: ConstantPool::new(),
        natives,
        func_arities,
    };

    let data = block.data.borrow();
    let n = data.len();
    let mut i = block.index;
    while i < n {
        compile_expr(&mut compiler, &data, &mut i, scope, /*tail*/ false)?;
        // If `compile_expr` consumed the last values, this was the final
        // expression — its result stays on the stack as the block's return
        // value. Otherwise, pop the intermediate result. (We can't compute
        // `is_last` before compiling because `if`/`either`/native calls
        // consume a variable number of values. M28 will rework the `tail`
        // flag when it adds tail-call optimization.)
        if i < n {
            compiler.emit(Instr::Pop);
        }
    }
    drop(data);

    // Block ends with `Return`: the VM pops the frame, returning top-of-stack
    // (or `None` if the stack is empty — matches the walker's `last` value).
    compiler.emit(Instr::Return);

    let span = block_source_span(block);
    Ok(CompiledBlock {
        instrs: Rc::from(compiler.instrs.as_slice()),
        pool: compiler.pool.into_rc(),
        n_locals: scope_locals_count(scope),
        freevars: analysis.freevars,
        source_span: span,
        needs_rebind: false,
        arity: 0,
    })
}

/// Build a `needs_rebind` stub block: instrs `[Halt]`, pool empty, the
/// analysis's freevars preserved (in case the walker path inspects them).
fn stub_block(block: &Series, analysis: AnalysisResult) -> CompiledBlock {
    CompiledBlock {
        instrs: Rc::from([Instr::Halt]),
        pool: Rc::from([]),
        n_locals: 0,
        freevars: analysis.freevars,
        source_span: block_source_span(block),
        needs_rebind: true,
        arity: 0,
    }
}

// ---------------------------------------------------------------------------
// Per-expression compilation
// ---------------------------------------------------------------------------

impl<'a> Compiler<'a> {
    fn emit(&mut self, instr: Instr) {
        self.instrs.push(instr);
    }

    fn push_const(&mut self, v: Value) -> u32 {
        self.pool.push(v)
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
    // `func`/`does`/`function` — emit `MakeFunc` and advance `*i` past the form.
    if let Some(skip) = func_form_skip(data, *i) {
        *i += 1; // consume the calling word itself; `compile_make_func` reads spec/body from `*i`
        compile_make_func(c, data, i, scope, cur, span)?;
        // `func_form_skip` returned the *total* skip including the calling word.
        // We've consumed `1 + (spec + body)` = `skip` values total.
        // Adjust: we already advanced 1; advance the remaining `skip - 1`.
        *i = *i + (skip - 1);
        // SetWord-RHS path: the MakeFunc pushes a `Value::Func` onto the stack,
        // which the enclosing SetWord stores. In value position, the func
        // is left on the stack — `Pop` if non-tail (handled by the caller).
        return Ok(());
    }

    *i += 1; // consume the prefix value itself

    match cur {
        // Data / literals: push as `Const`.
        Value::None
        | Value::Logic(_)
        | Value::Integer { .. }
        | Value::Float { .. }
        | Value::String { .. }
        | Value::String8(_)
        | Value::LitWord { .. }
        | Value::Block { .. }
        | Value::Func(_)
        | Value::Refinement { .. }
        | Value::Error(_)
        | Value::File { .. }
        | Value::Url { .. }
        | Value::Object(_) => {
            let idx = c.push_const(cur.clone());
            c.emit(Instr::Const(idx));
        }

        // Path: either a data-headed path (`obj/field`, `block/2`) resolved at
        // runtime via `GetPath`, or a function-headed path (`copy/part x`)
        // compiled as a refined native call. The head determines which:
        // an unbound `Word` naming a registered native → refined `Call`;
        // anything else → `GetPath` (M19 runtime resolution).
        Value::Path { parts, .. } | Value::GetPath { parts, .. } => {
            if let Some((native_idx, fd, head_sym, leading_refs)) =
                function_path_info(c, parts)
            {
                // `*i` was already advanced past the path token by the
                // caller; `collect_args` reads args starting at `*i`.
                let (argc, _refs) = collect_args(
                    c, data, i, scope, &head_sym, &fd, &leading_refs, span,
                )?;
                c.emit(Instr::Call(native_idx, argc as u32));
                return Ok(());
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

        // SetPath `obj/field: value` — compile RHS, then `SetPath`.
        Value::SetPath { .. } => {
            let path_idx = c.push_const(cur.clone());
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
            // The path Const was pushed but consumed by SetPath; mark it used
            // by emitting nothing further — the RHS value is the result.
            let _ = path_idx; // (path Const is consumed by the SetPath handler in M25)
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
        // Unbound non-native word in operator position: load as dynamic and
        // let the VM/runtime resolve it (M25 falls back to walker-style
        // dispatch). For M24 we emit `LoadDynamic` with no args.
        c.emit(Instr::LoadDynamic(sym.clone()));
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
        Binding::Unbound => unreachable!(),
    };
    // Is there at least one following value, AND is the resolved slot a known
    // user-func? If yes and we can determine arity, emit CallUser. Otherwise
    // just load (the walker returns the value as-is when not a Func).
    if let Some(arity) = c.func_arities.get(depth, slot) {
        if *i < data.len() {
            return compile_user_call(c, data, i, scope, slot, arity, span);
        }
    }
    // Value position — just load.
    emit_load(c, binding, span, sym)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// `if` / `either` inlining
// ---------------------------------------------------------------------------

/// Compile `if cond block` as inline control flow:
/// ```text
/// <cond code>          ; pushes cond
/// JumpIfFalse(L_end)   ; pops cond; if false jump
/// <then-block inline> ; pushes value (or nothing if block empty)
/// L_end:
/// ```
/// Matches plan3.md's expected `[Const(true), JumpIfFalse(L1), Const(42), L1: Return]`.
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

    // JumpIfFalse placeholder. We'll patch the target after emitting the then-block.
    let jump_idx = c.instrs.len();
    c.emit(Instr::JumpIfFalse(0)); // placeholder
    // Then-block: compile its series inline with the *same scope* (M23 already
    // analyzed its words; literal blocks are descended by `analyze_inner`).
    compile_block_series_inline(c, &then_series, scope, tail)?;
    // Patch the JumpIfFalse to land here.
    let end_target = c.instrs.len() as u32;
    c.instrs[jump_idx] = Instr::JumpIfFalse(end_target);
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
/// the last in `tail` position. Used by `if`/`either` branch bodies.
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
        // If `compile_expr` consumed the last values, this was the final
        // expression — its result stays on the stack as the branch's result.
        // Otherwise, pop the intermediate result. (Can't compute `is_last`
        // before compiling because expressions span a variable number of
        // values — e.g. `n * fact n - 1` is 6 values but 1 expression.)
        if j < n {
            c.emit(Instr::Pop);
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
    Some((native_idx, Rc::clone(fd), head_sym, leading_refs))
}

/// Compile a user-func call: collect `fd.params.len()` args, emit
/// `CallUser(slot, argc)`. The `slot` is the bound slot index (local or
/// global); M25's `CallUser` handler resolves it to the `Rc<FuncDef>`.
fn compile_user_call(
    c: &mut Compiler,
    data: &[Value],
    i: &mut usize,
    scope: &Scope,
    slot: usize,
    arity: usize,
    span: Span,
) -> Result<(), CompileError> {
    // Synthesize a minimal FuncDef for arg-collection purposes — the real
    // FuncDef is fetched at runtime by `CallUser`'s slot lookup. We only need
    // arity (params count); refinements on user funcs are rare in the test
    // corpus so M24 collects positional args only.
    let fd = Rc::new(FuncDef {
        params: (0..arity).map(|n| Symbol::new(&format!("__arg{n}"))).collect(),
        refinements: Vec::new(),
        locals: Vec::new(),
        freevars: Vec::new(),
        compiled: None,
        body: Series::empty(),
        ctx: Context::new(),
        native: None,
        variadic: false,
        infix: false,
    });
    // (`compile_prefix` already consumed the calling word.)
    let (argc, _refs) = collect_args(c, data, i, scope, &Symbol::new("__user"), &fd, &[], span)?;
    c.emit(Instr::CallUser(slot as u32, argc as u32));
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
///   `default`): first arg is pushed as `Const` (the literal value, not
///   evaluated) — matches the walker, which takes the word/name as-is.
/// - **Refinements**: walked in `fd.refinements` spec order; for each, if it
///   appears in `leading_refs` (path form) or the next value is a matching
///   `Value::Refinement` token (spaced form), emit `MarkRefine(ref)` +
///   args + `EndRefine`.
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
        "repeat" | "foreach" | "forall" | "make" | "to" | "default"
    );

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
            c.emit(Instr::MarkRefine(ref_name.clone()));
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
        spec_val = Value::Block {
            series: Series::empty(),
            span: Span::new(0, 0),
        };
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
        }
    } else {
        extract_spec(&spec_val).map_err(|_| CompileError {
            span,
            kind: CompileErrorKind::MalformedSpec,
        })?
    };
    let mut child = Scope::child(scope);
    for p in &spec.params {
        child.slot_index_pub(p.clone());
    }
    for (ref_name, ref_args) in &spec.refinements {
        child.slot_index_pub(ref_name.clone());
        for arg in ref_args {
            child.slot_index_pub(arg.clone());
        }
    }
    for local in &spec.locals {
        child.slot_index_pub(local.clone());
    }
    let body_series = match &body_val {
        Value::Block { series, .. } => series.clone(),
        _ => unreachable!(),
    };
    // Pre-collect body SetWords (mirrors `analyze_func_form`).
    collect_setwords_inline(&body_series, &mut child);
    let body_analysis = analyze_block(&body_series, &mut child);

    // Push spec + body into the pool.
    let spec_idx = c.push_const(spec_val);
    let body_idx = c.push_const(body_val);
    c.emit(Instr::MakeFunc(spec_idx, body_idx, body_analysis.freevars));

    // Record the func's arity in the table so subsequent `CallUser`s to this
    // slot know how many args to collect. The slot is the *enclosing SetWord's*
    // slot — we don't have it here; the caller's `compile_prefix` (which
    // handled the SetWord) records it. For recursive self-calls inside the
    // body, the body would need its own slot table; M24 relies on the
    // global-slot path (recursive `fact` resolves as global → recorded by
    // the outer SetWord's MakeFunc).
    Ok(())
}

// ---------------------------------------------------------------------------
// Load / store emission
// ---------------------------------------------------------------------------

/// Emit a load instr for a `Word`/`GetWord` based on its `Binding`:
/// - `Lexical(d, slot)` → `LoadLocal(d, slot)`
/// - `Local(_, slot)` → `LoadGlobal(slot)` (user-ctx global)
/// - `Unbound` → `LoadDynamic(sym)` (resolved at VM entry from `env.user_ctx`)
fn emit_load(c: &mut Compiler, binding: &Binding, _span: Span, sym: &Symbol) -> Result<(), CompileError> {
    match binding {
        Binding::Lexical(d, s) => c.emit(Instr::LoadLocal(*d as u32, *s as u32)),
        Binding::Local(_ctx, s) => c.emit(Instr::LoadGlobal(*s as u32)),
        Binding::Unbound => c.emit(Instr::LoadDynamic(sym.clone())),
        Binding::Func(s) => c.emit(Instr::LoadLocal(0, *s as u32)), // defensive
    }
    Ok(())
}

/// Emit a store instr for a `SetWord` based on its `Binding`:
/// - `Lexical(d, slot)` → `SetLocal(d, slot)`
/// - `Local(_, slot)` → `SetGlobal(slot)`
/// - `Unbound` → `SetDynamic(sym)`
fn emit_store(
    c: &mut Compiler,
    binding: &Binding,
    _span: Span,
    sym: &Symbol,
) -> Result<(), CompileError> {
    match binding {
        Binding::Lexical(d, s) => c.emit(Instr::SetLocal(*d as u32, *s as u32)),
        Binding::Local(_ctx, s) => c.emit(Instr::SetGlobal(*s as u32)),
        Binding::Unbound => c.emit(Instr::SetDynamic(sym.clone())),
        Binding::Func(s) => c.emit(Instr::SetLocal(0, *s as u32)), // defensive
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
    let _skip = func_form_skip(data, i)?;
    let is_does = matches!(
        &data[i],
        Value::Word { sym, .. } if sym.as_str() == "does"
    );
    if is_does {
        return Some(0);
    }
    let spec_val = &data[i + 1];
    extract_spec(spec_val).ok().map(|s| s.params.len())
}

/// Extract `(depth, slot)` from a `Binding` for `FuncArityTable` keying.
/// `Lexical(d, s)` and `Local(_, s)` map to `(d, s)`; `Func(s)` maps to
/// `(0, s)` (function-local slot, resolved via the active call frame).
fn slot_coords(binding: &Binding) -> (usize, usize) {
    match binding {
        Binding::Lexical(d, s) => (*d, *s),
        Binding::Local(_, s) => (0, *s),
        Binding::Func(s) => (0, *s),
        Binding::Unbound => (0, 0),
    }
}

/// Estimate the source span of a `Series` (used for `CompiledBlock.source_span`).
/// M24 doesn't need an exact span — just a fallback; M31 (disassembler) will
/// thread precise spans through.
fn block_source_span(block: &Series) -> Span {
    let data = block.data.borrow();
    let first = data.first().map(|v| v.span_or_default()).unwrap_or_default();
    let last = data
        .last()
        .map(|v| v.span_or_default())
        .unwrap_or_default();
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
    matches!(sym.as_str(), "object" | "context")
        && matches!(&data[i + 1], Value::Block { .. })
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
                if scope.lookup_pub(sym).is_none() {
                    scope.slot_index_pub(sym.clone());
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
    use red_core::{Context, Env};
    use red_core::vm_ir::Instr;

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

    // --- Plan-required tests (plan3.md:307-318) --------------------------

    /// `5` -> `[Const(0), Return]`, pool=[5]. (plan3.md:307)
    #[test]
    fn compile_literal() {
        let (body, ctx_rc, registry) = parse_bind_and_registry("5");
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile");
        assert!(!block.needs_rebind);
        assert_instrs(
            block.instrs.as_ref(),
            &[Instr::Const(0), Instr::Return],
            "compile `5`",
        );
        assert_eq!(block.pool.len(), 1);
        assert!(matches!(block.pool[0], Value::Integer { n: 5, .. }));
    }

    /// `foo: 5 foo` -> `[Const(0), SetGlobal(slot), Pop, LoadGlobal(slot), Return]`.
    /// (plan3.md:308 — originally expected no `Pop`, but M25 adds `Pop` after
    /// non-last expressions to keep the VM stack disciplined. The SetWord is
    /// not the last expression, so its pushed-back value is popped before the
    /// `foo` load. The walker's `last = ...` overwrite is the equivalent.)
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
                Instr::Const(0),
                Instr::SetGlobal(foo_slot as u32),
                Instr::Pop,
                Instr::LoadGlobal(foo_slot as u32),
                Instr::Return,
            ],
            "compile `foo: 5 foo`",
        );
        assert!(matches!(block.pool[0], Value::Integer { n: 5, .. }));
    }

    /// `1 + 2` -> `[Const(0), Const(1), Call(+, 2), Return]`. (plan3.md:310)
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
                Instr::Const(0),
                Instr::Const(1),
                Instr::Call(plus_idx, 2),
                Instr::Return,
            ],
            "compile `1 + 2`",
        );
        assert!(matches!(block.pool[0], Value::Integer { n: 1, .. }));
        assert!(matches!(block.pool[1], Value::Integer { n: 2, .. }));
    }

    /// `if true [42]` -> `[LoadGlobal(true_slot), JumpIfFalse(L1), Const(0), L1: Return]`.
    /// (plan3.md:312 expected `Const(true)`, but `true` is a context-stored constant
    /// via `install_constants`, so the compiler emits `LoadGlobal` — matching the
    /// walker, which resolves `true` as a word bound to the user context.)
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
        // Expected instr layout:
        //   0: LoadGlobal(true_slot)  ; load `true` constant
        //   1: JumpIfFalse(3)         ; L1 = index 3
        //   2: Const(0)               ; 42
        //   3: Return                 ; L1 lands here
        assert_instrs(
            block.instrs.as_ref(),
            &[
                Instr::LoadGlobal(true_slot as u32),
                Instr::JumpIfFalse(3),
                Instr::Const(0),
                Instr::Return,
            ],
            "compile `if true [42]`",
        );
        assert!(matches!(block.pool[0], Value::Integer { n: 42, .. }));
    }

    /// `if 1 [42]` exercises the pure-Const(cond) path (literal cond, not a
    /// context-stored constant like `true`). Verifies the plan3.md:312
    /// instr shape `[Const(0), JumpIfFalse(L1), Const(1), L1: Return]`.
    #[test]
    fn compile_if_literal_cond() {
        let (body, ctx_rc, registry) = parse_bind_and_registry("if 1 [42]");
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile");
        assert_instrs(
            block.instrs.as_ref(),
            &[
                Instr::Const(0),
                Instr::JumpIfFalse(3),
                Instr::Const(1),
                Instr::Return,
            ],
            "compile `if 1 [42]`",
        );
        assert!(matches!(block.pool[0], Value::Integer { n: 1, .. }));
        assert!(matches!(block.pool[1], Value::Integer { n: 42, .. }));
    }

    /// `func [x][x * x]` emits `MakeFunc` with freevars=[]. (plan3.md:314)
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
            Instr::MakeFunc(_spec_idx, _body_idx, freevars) => {
                assert!(
                    freevars.is_empty(),
                    "square should have no freevars, got {freevars:?}"
                );
            }
            _ => unreachable!(),
        }
    }

    /// A recursive factorial emits `CallUser(0, 1)` referencing its own slot. (plan3.md:316)
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
        let Value::Block { series: func_body, .. } = &data[3] else {
            panic!("expected func body block at index 3");
        };
        // Manually record fact's slot as a global arity-1 func (simulating
        // what the SetWord path would have done in a full compile).
        let mut child = Scope::child(&Scope::root(&ctx_rc));
        child.slot_index_pub(Symbol::new("n"));
        let mut c = Compiler {
            instrs: Vec::new(),
            pool: ConstantPool::new(),
            natives: &registry,
            func_arities: FuncArityTable::default(),
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
        // Assert the body's instrs contain `CallUser(fact_slot, 1)`.
        let has_calluser = c
            .instrs
            .iter()
            .any(|instr| matches!(instr, Instr::CallUser(slot, argc) if *slot as usize == fact_slot && *argc == 1));
        assert!(
            has_calluser,
            "fact body should contain CallUser({}, 1), got {:?}",
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
        assert_instrs(
            block.instrs.as_ref(),
            &[Instr::LoadDynamic(Symbol::new("foo")), Instr::Return],
            "compile `foo`",
        );
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
        if let Instr::MakeFunc(spec_idx, _body_idx, freevars) = makefunc {
            assert!(
                freevars.is_empty(),
                "does body should have no freevars, got {freevars:?}"
            );
            // The does spec is an empty block — pool[spec_idx] is `Block([])`.
            let spec_val = &block.pool[*spec_idx as usize];
            assert!(matches!(spec_val, Value::Block { .. }), "does spec is a block");
        }
    }
}
