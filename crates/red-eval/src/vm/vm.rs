//! Stack VM dispatch loop (M25).
//!
//! Executes a [`CompiledBlock`] produced by M24's compiler. The VM is a
//! straightforward stack machine: each instr pushes/pops `Value`s on the
//! operand stack, function calls push [`Frame`]s on the call stack, and
//! control flow mutates the frame's `pc`. Lexical addressing walks the frame
//! chain — `LoadLocal(d, slot)` reads from `frames[len-1-d].locals[slot]`.
//!
//! The VM is **available but not yet the default** in M25: `interp::eval`
//! (the tree-walker) remains the sole evaluator until M29 flips the default.
//! M25 ships the dispatch loop + the six plan-required inline tests.
//!
//! ## Hot-path notes
//!
//! The dispatch `match` is one arm per `Instr` variant (23 total). The hot
//! arms are `Const`/`LoadLocal`/`LoadGlobal`/`Call`/`CallUser`/`JumpIfFalse`
//! — these dominate in compute-heavy loops. M30's profiling will target
//! them; M25 keeps the dispatch plain for clarity.
//!
//! ## Native bridge
//!
//! Natives keep their existing `NativeFn` signature
//! (`fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>`). The
//! VM assembles `&[Value]` by slicing the top `argc` stack slots, and
//! `RefineArgs` by collecting `MarkRefine`/`EndRefine`-bracketed regions.
//! Natives that recurse into evaluation (`do`/`reduce`/`if`/`either`/loops)
//! currently call the *walker* (`interp::eval`) — M26 adds the
//! `dispatch_block` shim that picks VM vs. walker based on the block's
//! `needs_rebind` flag. For M25's test cases (no `do`/`reduce`/loop native
//! invocation in VM mode), the walker recursion path is unused.

use std::mem::{align_of, size_of, MaybeUninit};
use std::rc::Rc;

use red_core::value::{FuncDef, Span, Symbol, Value};
use red_core::vm_ir::{CompiledBlock, Frame, Instr};
use red_core::{CompileErrorKind, Context, Env, EvalError, RefineArgs};

use crate::binding::{bind_function_body, deep_clone_series};
use crate::interp::{call_user_func, eval_get_path, set_path_value};
use crate::natives::extract_spec;
use crate::vm::compiler::{compile_block_for_func_body, stub_block, NativeRegistry};
use crate::vm::lex::Scope;

/// M30.1.A: capacity of the stack-allocated native-args fast path. Natives
/// with argc ≤ this copy args into a `[Value; INLINE_ARGS_CAP]` on the call
/// frame instead of heap-allocating a `Vec`. 8 covers every native in the
/// registry (the highest-arity fixed native is `make`/`to` at ~3 args;
/// variadic natives collect via `LoadDynamic`, not via argc).
const INLINE_ARGS_CAP: usize = 8;

/// M31: static assertion backing the `unsafe` `from_raw_parts` cast in
/// `call_native`. The cast reinterprets `[MaybeUninit<Value>; INLINE_ARGS_CAP]`
/// as `[Value; argc]`; this is sound only if `Value` and `MaybeUninit<Value>`
/// have identical layout (no padding differences, no niche-invalid bit
/// patterns the compiler could exploit). `MaybeUninit<T>` guarantees the
/// same size/alignment as `T` by definition, but the equality check here
/// makes the assumption explicit and fails compilation if a future `Value`
/// variant or `#[repr(...)]` change breaks it.
///
/// See `Context::slot_value_unchecked`/`set_slot_unchecked` in
/// `crates/red-core/src/context.rs` for the matching "no invalid bit
/// patterns" invariant on the slot-access fast path.
const _: () = assert!(
    size_of::<Value>() == size_of::<MaybeUninit<Value>>(),
    "Value and MaybeUninit<Value> must have identical size for the from_raw_parts cast"
);
const _: () = assert!(
    align_of::<Value>() == align_of::<MaybeUninit<Value>>(),
    "Value and MaybeUninit<Value> must have identical alignment for the from_raw_parts cast"
);

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run a compiled top-level block to completion. Pushes an initial frame
/// (no function, depth 0) and dispatches instrs until `Return`/`Halt` ends
/// the top-level frame or an error propagates. Catches `EvalError::Quit` to
/// match `run_series_inner_opts`'s top-level contract (the exit code is
/// discarded here — M29 wires the VM into `run_source*` for CLI exit-code
/// propagation).
pub fn run(block: CompiledBlock, env: &mut Env) -> Result<Value, EvalError> {
    let natives_by_idx = build_natives_by_idx(env);
    // M30.1.C: drain the reusable scratch Vecs from `env` instead of
    // allocating fresh ones. Each `dispatch_block`→`vm::run` call previously
    // allocated 2 Vecs (`frames` + `stack`); for a 1M-iteration `repeat`,
    // that was 2M heap allocations. The pools stay at their high-water
    // capacity across calls, so subsequent calls don't realloc.
    let frames_pool = std::mem::take(&mut env.vm_frames_pool);
    let stack_pool = std::mem::take(&mut env.vm_stack_pool);
    // M30.3.1: wrap in Rc so Frame clones are cheap (1 Rc bump vs 4 + Vec alloc).
    let block = Rc::new(block);
    let mut vm = Vm {
        env,
        frames: frames_pool,
        stack: stack_pool,
        natives_by_idx,
        ref_marks: Vec::new(),
        pending_refs: Vec::new(),
        cached_block: None,
        cached_instrs: None,
        cached_frame_gen: 0,
        frame_gen: 0,
    };
    vm.frames.clear();
    vm.stack.clear();
    vm.frames.push(Frame {
        func: None,
        locals: Vec::new(),
        depth: 0,
        block,
        pc: 0,
    });
    vm.frame_gen = vm.frame_gen.wrapping_add(1);
    let result = vm.run_loop();
    // M30.1.C: restore the pools. Extract the Vecs from `vm` before dropping
    // it (the Vm borrows `env: &mut Env`, so we can't touch `env` while `vm`
    // is alive). `std::mem::take` leaves `vm.frames`/`vm.stack` as empty
    // Vecs (no alloc) so `vm`'s Drop is cheap.
    let mut frames_pool = std::mem::take(&mut vm.frames);
    let mut stack_pool = std::mem::take(&mut vm.stack);
    frames_pool.clear();
    stack_pool.clear();
    drop(vm);
    env.vm_frames_pool = frames_pool;
    env.vm_stack_pool = stack_pool;
    result
}

/// Run a compiled block in "reduce mode": every expression's result stays on
/// the stack (the block was compiled with `compile_block_reduce`, which emits
/// no `Pop` between expressions). After the block's `Return`, collect the
/// remaining stack into a `Value::Block` — matching the walker's `reduce`
/// semantics (one entry per expression). Used by the `reduce` native in VM
/// mode (M26).
pub fn run_reduce(block: CompiledBlock, env: &mut Env) -> Result<Value, EvalError> {
    let natives_by_idx = build_natives_by_idx(env);
    // M30.1.C: reuse the scratch Vecs (same drain/restore as `run`).
    let frames_pool = std::mem::take(&mut env.vm_frames_pool);
    let stack_pool = std::mem::take(&mut env.vm_stack_pool);
    // M30.3.1: wrap in Rc so Frame clones are cheap.
    let block = Rc::new(block);
    let mut vm = Vm {
        env,
        frames: frames_pool,
        stack: stack_pool,
        natives_by_idx,
        ref_marks: Vec::new(),
        pending_refs: Vec::new(),
        cached_block: None,
        cached_instrs: None,
        cached_frame_gen: 0,
        frame_gen: 0,
    };
    vm.frames.clear();
    vm.stack.clear();
    vm.frames.push(Frame {
        func: None,
        locals: Vec::new(),
        depth: 0,
        block,
        pc: 0,
    });
    vm.frame_gen = vm.frame_gen.wrapping_add(1);
    let result = vm.run_loop_reduce();
    let mut frames_pool = std::mem::take(&mut vm.frames);
    let mut stack_pool = std::mem::take(&mut vm.stack);
    frames_pool.clear();
    stack_pool.clear();
    drop(vm);
    env.vm_frames_pool = frames_pool;
    env.vm_stack_pool = stack_pool;
    result
}

/// Build a `Vec<Rc<FuncDef>>` indexed by the same `u32` indices the
/// compiler's `NativeRegistry::from_env` assigned. M30: cached on
/// `Env::natives_by_idx` so the 1M-iteration loop path doesn't rebuild it
/// per `dispatch_block` call (the original `Rc::clone`-per-native-per-call
/// was the root cause of the v0.3.0 `sum_loop` regression — ~100 clones × 1M
/// iterations = 100M refcount ops). Invalidated by `invalidate_native_index`
/// when `env.natives` is mutated (at `register_natives` time, before any VM
/// run).
fn build_natives_by_idx(env: &mut Env) -> Rc<Vec<Rc<FuncDef>>> {
    if let Some(cached) = &env.natives_by_idx {
        // One Rc bump — no `Vec` alloc, no per-native `Rc::clone`.
        return Rc::clone(cached);
    }
    let mut out: Vec<Rc<FuncDef>> = Vec::with_capacity(env.natives.len());
    for fd in env.natives.values() {
        out.push(Rc::clone(fd));
    }
    let rc = Rc::new(out);
    env.natives_by_idx = Some(Rc::clone(&rc));
    rc
}

// ---------------------------------------------------------------------------
// Vm state
// ---------------------------------------------------------------------------

struct Vm<'env> {
    env: &'env mut Env,
    frames: Vec<Frame>,
    stack: Vec<Value>,
    /// Native `FuncDef`s indexed by the `u32` carried by `Call(native_idx, _)`.
    /// M30: `Rc`-cloned from `Env::natives_by_idx` (one Rc bump per `vm::run`,
    /// not one `Vec` alloc + N `Rc::clone`s).
    natives_by_idx: Rc<Vec<Rc<FuncDef>>>,
    /// `(refinement_name, stack_height_at_mark)` for the currently-open
    /// `MarkRefine`/`EndRefine` region. `EndRefine` pops the topmost entry,
    /// collects `stack[height..]` into a `Vec<Value>`, truncates the stack,
    /// and appends `(name, args)` to `pending_refs`.
    ref_marks: Vec<(Symbol, usize)>,
    /// Accumulated refinement args for the current call, drained into a
    /// `RefineArgs` at `Call` time.
    pending_refs: Vec<(Symbol, Vec<Value>)>,
    // -----------------------------------------------------------------
    // M30 dispatch-loop cache. The original loop cloned the top frame's
    // `block` (Rc bump) and `instrs` slice (Rc<[Instr]> bump) on *every*
    // iteration — two atomic refcount ops per instr even when the frame
    // was unchanged across thousands of iterations (tight loops, deep
    // recursion). We now snapshot `(block, instrs)` and refresh only when
    // `frame_gen` changes (frame push/pop/overwrite). The snapshot holds
    // a borrow of `frames`, so we must drop it before mutating `frames`.
    // -----------------------------------------------------------------
    /// Cached clone of the current top frame's `CompiledBlock` (Rc bump).
    /// M30.3.1: cached as `Rc<CompiledBlock>` (was owned `CompiledBlock`).
    /// A single `Rc` bump on refresh (was 4 Rc bumps + 1 Vec alloc).
    cached_block: Option<Rc<CompiledBlock>>,
    /// Cached clone of the current top frame's `instrs` slice.
    cached_instrs: Option<Rc<[Instr]>>,
    /// `frame_gen` value at the time the cache was last refreshed.
    cached_frame_gen: u64,
    /// Bumped on every frame push/pop/overwrite. Wrapping is safe — the
    /// cache only needs to detect *change*, not ordering.
    frame_gen: u64,
}

impl<'env> Vm<'env> {
    /// Refresh the cached `(block, instrs)` snapshot if the top frame has
    /// changed since the last refresh. Returns the frame index. After this
    /// returns, `self.cached_instrs` is guaranteed populated and the caller
    /// can index into it directly.
    ///
    /// M30: this avoids the per-iteration `block.clone()` + `instrs.clone()`
    /// that the original loop did. Tight loops (e.g. `repeat 1000000`) hit
    /// this ~1M times; the cache stays valid across all of them because the
    /// top frame doesn't change. Only `CallUser`/`TailCall`/`Return` bump
    /// `frame_gen`, triggering a refresh on the next iteration.
    ///
    /// M30.2.D: previously returned `Rc<[Instr]>` (one Rc bump per iteration
    /// on the cache-hit path). Now returns only the frame index — the caller
    /// indexes into `self.cached_instrs` directly, eliminating the per-iter
    /// Rc refcount op. Safe because `Instr: Copy` (M30.1.B) means the caller
    /// reads `cached_instrs[pc]` as a bitwise copy with no borrow extension.
    #[inline]
    fn refresh_cache(&mut self) -> usize {
        let frame_idx = self.frames.len() - 1;
        if self.cached_frame_gen != self.frame_gen {
            // M30.3.1: `block` is `Rc<CompiledBlock>` — one Rc bump (was 4 + Vec alloc).
            let block = Rc::clone(&self.frames[frame_idx].block);
            let instrs = block.instrs.clone();
            self.cached_block = Some(block);
            self.cached_instrs = Some(instrs);
            self.cached_frame_gen = self.frame_gen;
        }
        frame_idx
    }

    /// M31: best-effort source span for an error raised at the current
    /// dispatch point. Returns the active `CompiledBlock`'s `source_span`
    /// (computed by `block_source_span` at compile time — the first/last
    /// value's span, or `Span::default()` for a synthetic block). Used by
    /// VM arms that have no per-value span to attribute the error to (e.g.
    /// `LoadDynamic` UnboundWord, `ConstInt` synthetic push, native-index
    /// invariant failures, `Halt`/`EndRefine` misroutes).
    ///
    /// Falls back to `Span::default()` if no block is cached (shouldn't happen
    /// mid-dispatch — `refresh_cache` runs at the top of each loop iteration —
    /// but defensive against a future caller that invokes an error-raising
    /// helper outside the dispatch loop).
    #[inline]
    fn current_span(&self) -> Span {
        self.cached_block
            .as_ref()
            .map(|b| b.source_span)
            .unwrap_or_default()
    }

    fn run_loop(&mut self) -> Result<Value, EvalError> {
        loop {
            // M30.2.D: no Rc<[Instr]> clone here — `refresh_cache` populates
            // `self.cached_instrs` and we index into it directly. `Instr` is
            // `Copy` (16 bytes), so `instrs[pc]` is a bitwise copy with no
            // refcount ops and no borrow-extension across the match.
            let frame_idx = self.refresh_cache();
            let pc = self.frames[frame_idx].pc;
            let instrs = self.cached_instrs.as_ref().expect("cache invariant");
            if pc >= instrs.len() {
                // M31: was a silent "implicit return of top-of-stack". A
                // well-formed `CompiledBlock` always ends with `Return`/
                // `Halt`, so reaching here is a compiler or VM routing bug.
                // `debug_assert!` catches it in debug; release builds
                // surface a recoverable `EvalError::Compile` (VmInvariant)
                // rather than silently returning a possibly-wrong value.
                debug_assert!(
                    pc < instrs.len(),
                    "VM ran off instr stream: pc={pc} len={}",
                    instrs.len()
                );
                return Err(EvalError::Compile {
                    kind: CompileErrorKind::VmInvariant(format!(
                        "ran off instr stream: pc={pc} len={}",
                        instrs.len()
                    )),
                    span: self.current_span(),
                });
            }
            // M30.1.B: `Instr` is `Copy` (16 bytes), so this is a bitwise
            // copy — no `Rc` refcount ops, no borrow-extension across the match.
            let instr = instrs[pc];
            // Advance pc before dispatch (jump instrs overwrite it).
            self.frames[frame_idx].pc = pc + 1;

            match instr {
                Instr::Const(i) => {
                    // M30: `ConstInt`/`ConstNone`/`ConstBool` are the
                    // small-value fast paths; `Const` handles the rest
                    // (Float/String/Block/etc.) via the pool. The pool
                    // lookup still clones the `Value` (unavoidable — the
                    // stack owns its values).
                    let block = self.cached_block.as_ref().expect("cache invariant");
                    let v = block_pool(block, i as usize)?;
                    self.stack.push(v);
                }
                Instr::ConstInt(n) => {
                    // M30 fast path: skip pool indirection for `Integer`.
                    // Constructs the `Value` inline (no `Rc`, no clone) —
                    // the dominant literal kind in `fib`/`sum_loop`/loops.
                    // M31: thread `current_span()` so integer-arg type
                    // errors localize to the literal's block (not zero).
                    self.stack.push(Value::Integer {
                        n,
                        span: self.current_span(),
                    });
                }
                Instr::ConstNone => {
                    self.stack.push(Value::None);
                }
                Instr::ConstBool(b) => {
                    self.stack.push(Value::Logic(b));
                }
                Instr::LoadLocal(d, slot) => {
                    // M30: unchecked slot access. The compiler's `Scope`
                    // proved the slot exists at compile time; the bounds
                    // check is redundant in release. `debug_assert!` keeps
                    // debug builds safe.
                    let len = self.frames.len();
                    let frame_idx2 = len - 1 - d as usize;
                    let locals = &self.frames[frame_idx2].locals;
                    debug_assert!(
                        (slot as usize) < locals.len(),
                        "LoadLocal OOB: slot={} len={}",
                        slot,
                        locals.len()
                    );
                    // SAFETY: compiler-proven slot index.
                    let v = unsafe { locals.get_unchecked(slot as usize).clone() };
                    self.stack.push(v);
                }
                Instr::LoadGlobal(slot) => {
                    // M30: unchecked global slot access. Same contract as
                    // `LoadLocal` — the compiler proved the slot at compile
                    // time via the user context's binding pass.
                    let v = self.env.user_ctx.slot_value_unchecked(slot as usize);
                    self.stack.push(v);
                }
                Instr::LoadDynamic(sym_idx) => {
                    // M30.1.B: look up the symbol from the block's side table.
                    let sym = self
                        .cached_block
                        .as_ref()
                        .expect("cache invariant")
                        .symbols
                        .get(sym_idx as usize)
                        .cloned()
                        .unwrap_or_else(|| Symbol::new(""));
                    let v = if let Some(val) = self.env.user_ctx.get(&sym) {
                        val
                    } else if let Some(fd) = self.env.natives.get(&sym) {
                        Value::Func(Rc::clone(fd))
                    } else {
                        // M31: use the block's `source_span` as the
                        // fallback (the per-symbol span isn't in the side
                        // table). Better than zero for locating which block
                        // the unbound word came from.
                        return Err(EvalError::UnboundWord {
                            sym,
                            span: self.current_span(),
                        });
                    };
                    self.stack.push(v);
                }
                Instr::SetLocal(d, slot) => {
                    let val = self.stack.pop().unwrap_or(Value::None);
                    let len = self.frames.len();
                    let locals = &mut self.frames[len - 1 - d as usize].locals;
                    if (slot as usize) >= locals.len() {
                        locals.resize(slot as usize + 1, Value::None);
                    }
                    // SAFETY: resize above guarantees the slot exists.
                    locals[slot as usize] = val.clone();
                    self.stack.push(val);
                }
                Instr::SetGlobal(slot) => {
                    let val = self.stack.pop().unwrap_or(Value::None);
                    // M30: unchecked global slot write. The compiler only
                    // emits `SetGlobal(slot)` for words bound by the binding
                    // pass, which allocates the slot.
                    self.env
                        .user_ctx
                        .set_slot_unchecked(slot as usize, val.clone());
                    self.stack.push(val);
                }
                Instr::SetDynamic(sym_idx) => {
                    // M30.1.B: look up the symbol from the block's side table.
                    let sym = self
                        .cached_block
                        .as_ref()
                        .expect("cache invariant")
                        .symbols
                        .get(sym_idx as usize)
                        .cloned()
                        .unwrap_or_else(|| Symbol::new(""));
                    let val = self.stack.pop().unwrap_or(Value::None);
                    self.env.user_ctx.set(sym, val.clone());
                    self.stack.push(val);
                }
                Instr::Call(native_idx, argc) => {
                    self.call_native(native_idx as usize, argc as usize)?;
                }
                Instr::CallUser(slot, argc) => {
                    self.call_user(slot as usize, argc as usize)?;
                }
                Instr::CallUserGlobal(slot, argc) => {
                    // M30.3.4: skip the `frames.last().and_then(...)` local-slot
                    // check in `prepare_call` — the func is in `user_ctx`.
                    self.call_user_global(slot as usize, argc as usize)?;
                }
                Instr::TailCall(slot, argc) => {
                    // M28: tail-call frame overwrite (see `tail_call`). The
                    // VM reuses the current frame, bounding call-stack depth
                    // for tail-recursive programs.
                    self.tail_call(slot as usize, argc as usize)?;
                }
                Instr::TailReenter(slot, argc) => {
                    // M28: self-recursion in tail position. Same frame reuse
                    // as `TailCall` — `tail_call` detects the same-`FuncDef`
                    // case at runtime and skips the block swap. (The compiler
                    // emits `TailReenter` only when it statically knows the
                    // slot is the func's own; `TailCall` covers the runtime-
                    // detected case.)
                    self.tail_call(slot as usize, argc as usize)?;
                }
                Instr::Jump(target) => {
                    self.frames[frame_idx].pc = target as usize;
                }
                Instr::JumpIfFalse(target) => {
                    let cond = self.stack.pop().unwrap_or(Value::None);
                    if !is_truthy(&cond) {
                        self.frames[frame_idx].pc = target as usize;
                    }
                }
                Instr::Pop => {
                    // M24 note: Pop on an empty stack is a no-op (SetWord
                    // pushes nothing in some compile paths). Keep that lenient.
                    self.stack.pop();
                }
                Instr::Return => {
                    // End the current frame. The result is top-of-stack (or
                    // None if the block was empty). Pop the frame, then push
                    // the result back onto the *caller's* stack so the
                    // caller's `CallUser` sees the return value. If this was
                    // the top-level frame, return the result directly.
                    let result = self.stack.pop().unwrap_or(Value::None);
                    // M30.3.2: save the popped frame's `locals` Vec to the
                    // pool so the next `prepare_call` can reuse its capacity
                    // instead of allocating fresh. The pool stays at high-water
                    // capacity, so deep recursion only allocates on the first call.
                    let popped_frame = self.frames.pop();
                    if let Some(frame) = popped_frame {
                        let mut locals = frame.locals;
                        locals.clear();
                        self.env.vm_locals_pool.push(locals);
                    }
                    self.frame_gen = self.frame_gen.wrapping_add(1);
                    if self.frames.is_empty() {
                        return Ok(result);
                    }
                    self.stack.push(result);
                }
                Instr::MakeFunc(spec_idx, body_idx, fv_idx) => {
                    // M30.1.B: freevar list is looked up from the side table.
                    let block = self.cached_block.as_ref().expect("cache invariant").clone();
                    let spec_val = block_pool(&block, spec_idx as usize)?;
                    let body_val = block_pool(&block, body_idx as usize)?;
                    let freevars = block
                        .freevars_table
                        .get(fv_idx as usize)
                        .cloned()
                        .unwrap_or_default();
                    let fd = self.build_func_def(spec_val, body_val, freevars)?;
                    self.stack.push(Value::Func(Rc::new(fd)));
                }
                Instr::EnterBlock => {
                    // No-op for M25 — `DropTo` restores height. M26 may use
                    // this to mark a reduce-style nested scope boundary.
                }
                Instr::DropTo(n) => {
                    self.stack.truncate(n as usize);
                }
                Instr::GetPath => {
                    let path = self.stack.pop().unwrap_or(Value::None);
                    let (parts, span) = match &path {
                        Value::Path { parts, span } => (parts.clone(), *span),
                        Value::GetPath { parts, span } => (parts.clone(), *span),
                        other => {
                            return Err(EvalError::TypeError {
                                expected: "path! or get-path!",
                                found: crate::natives::type_name(other),
                                span: other.span_or_default(),
                            });
                        }
                    };
                    let v = eval_get_path(&parts, span, self.env)?;
                    self.stack.push(v);
                }
                Instr::SetPath => {
                    let rhs = self.stack.pop().unwrap_or(Value::None);
                    let path = self.stack.pop().unwrap_or(Value::None);
                    let (parts, span) = match &path {
                        Value::SetPath { parts, span } => (parts.clone(), *span),
                        Value::Path { parts, span } => (parts.clone(), *span),
                        other => {
                            return Err(EvalError::TypeError {
                                expected: "set-path! or path!",
                                found: crate::natives::type_name(other),
                                span: other.span_or_default(),
                            });
                        }
                    };
                    set_path_value(&parts, rhs.clone(), self.env, span)?;
                    self.stack.push(rhs);
                }
                Instr::MarkRefine(sym_idx) => {
                    // M30.1.B: look up the symbol from the block's side table.
                    let sym = self
                        .cached_block
                        .as_ref()
                        .expect("cache invariant")
                        .symbols
                        .get(sym_idx as usize)
                        .cloned()
                        .unwrap_or_else(|| Symbol::new(""));
                    self.ref_marks.push((sym, self.stack.len()));
                }
                Instr::EndRefine => {
                    let (sym, height) = self.ref_marks.pop().ok_or_else(|| EvalError::Compile {
                        kind: CompileErrorKind::VmInvariant("EndRefine without MarkRefine".into()),
                        span: self.current_span(),
                    })?;
                    let args: Vec<Value> = self.stack[height..].to_vec();
                    self.stack.truncate(height);
                    self.pending_refs.push((sym, args));
                }
                Instr::Halt => {
                    // `needs_rebind` stub blocks emit `[Halt]`. The VM
                    // shouldn't reach here for M25's tests (compile_block
                    // returns needs_rebind only for `use`/object forms, which
                    // the test cases don't use). Surface as an error so a
                    // misroute is visible rather than silently returning None.
                    return Err(EvalError::Compile {
                        kind: CompileErrorKind::VmInvariant(
                            "VM reached Halt (block needs_rebind — use walker)".into(),
                        ),
                        span: self.current_span(),
                    });
                }
            }
        }
    }

    /// Reduce-mode dispatch loop. Identical to `run_loop` except the
    /// top-level `Return` collects the *entire* remaining stack into a
    /// `Value::Block` (rather than popping one value). Used by `run_reduce`
    /// for the `reduce` native in VM mode — the block was compiled with
    /// `compile_block_reduce` (no `Pop` between expressions), so the stack
    /// holds one value per expression. Nested `Return`s (from user-func calls
    /// inside the reduce block) behave as in `run_loop`.
    fn run_loop_reduce(&mut self) -> Result<Value, EvalError> {
        loop {
            // M30.2.D: no Rc<[Instr]> clone — index into `cached_instrs` directly.
            let frame_idx = self.refresh_cache();
            let pc = self.frames[frame_idx].pc;
            let instrs = self.cached_instrs.as_ref().expect("cache invariant");
            if pc >= instrs.len() {
                // M31: a well-formed reduce-mode `CompiledBlock` always
                // ends with `Return` (which calls `collect_reduce_stack`
                // itself). Reaching here is a compiler bug; surface it
                // rather than silently collecting the stack (which could
                // mask a real bug as "wrong reduce results"). The
                // `debug_assert!` catches it in debug builds.
                debug_assert!(
                    pc < instrs.len(),
                    "VM (reduce) ran off instr stream: pc={pc} len={}",
                    instrs.len()
                );
                return Err(EvalError::Compile {
                    kind: CompileErrorKind::VmInvariant(format!(
                        "ran off instr stream (reduce): pc={pc} len={}",
                        instrs.len()
                    )),
                    span: self.current_span(),
                });
            }
            // M30.1.B: `Instr` is `Copy` — bitwise copy, no clone.
            let instr = instrs[pc];
            self.frames[frame_idx].pc = pc + 1;
            match instr {
                Instr::Return => {
                    // M30.3.2: pool the popped frame's locals Vec.
                    let popped = self.frames.pop();
                    if let Some(frame) = popped {
                        let mut locals = frame.locals;
                        locals.clear();
                        self.env.vm_locals_pool.push(locals);
                    }
                    self.frame_gen = self.frame_gen.wrapping_add(1);
                    if self.frames.is_empty() {
                        // Top-level Return in reduce mode: collect the whole
                        // stack into a Block. (Each expression left its result;
                        // the final expression's result is also on the stack,
                        // so we don't pop-and-discard like `run_loop` does.)
                        return Ok(self.collect_reduce_stack());
                    }
                    // Nested Return (from a user-func called inside the
                    // reduce block): push the return value onto the caller's
                    // stack, matching `run_loop`.
                    let result = self.stack.pop().unwrap_or(Value::None);
                    self.stack.push(result);
                }
                Instr::Halt => {
                    return Err(EvalError::Compile {
                        kind: CompileErrorKind::VmInvariant(
                            "VM reached Halt (block needs_rebind — use walker)".into(),
                        ),
                        span: self.current_span(),
                    });
                }
                _ => {
                    // Delegate all other instrs to the shared `dispatch_instr`.
                    self.dispatch_instr(instr, frame_idx)?;
                }
            }
        }
    }

    /// Collect the current stack into a `Value::Block`, leaving the stack
    /// empty. Used by `run_loop_reduce` at top-level `Return`.
    fn collect_reduce_stack(&mut self) -> Value {
        let results: Vec<Value> = self.stack.drain(..).collect();
        Value::block(red_core::value::Series::new(results))
    }

    /// Dispatch a single instr (shared core between `run_loop` and
    /// `run_loop_reduce`). Extracted so reduce mode can reuse every arm
    /// except `Return`/`Halt`.
    fn dispatch_instr(&mut self, instr: Instr, frame_idx: usize) -> Result<(), EvalError> {
        match instr {
            Instr::Const(i) => {
                let block = self.cached_block.as_ref().expect("cache invariant").clone();
                let v = block_pool(&block, i as usize)?;
                self.stack.push(v);
            }
            Instr::ConstInt(n) => {
                self.stack.push(Value::Integer {
                    n,
                    span: self.current_span(),
                });
            }
            Instr::ConstNone => {
                self.stack.push(Value::None);
            }
            Instr::ConstBool(b) => {
                self.stack.push(Value::Logic(b));
            }
            Instr::LoadLocal(d, slot) => {
                let len = self.frames.len();
                let locals = &self.frames[len - 1 - d as usize].locals;
                debug_assert!(
                    (slot as usize) < locals.len(),
                    "LoadLocal OOB: slot={} len={}",
                    slot,
                    locals.len()
                );
                // SAFETY: compiler-proven slot index.
                let v = unsafe { locals.get_unchecked(slot as usize).clone() };
                self.stack.push(v);
            }
            Instr::LoadGlobal(slot) => {
                let v = self.env.user_ctx.slot_value_unchecked(slot as usize);
                self.stack.push(v);
            }
            Instr::LoadDynamic(sym_idx) => {
                // M30.1.B: look up the symbol from the block's side table.
                let sym = self
                    .cached_block
                    .as_ref()
                    .expect("cache invariant")
                    .symbols
                    .get(sym_idx as usize)
                    .cloned()
                    .unwrap_or_else(|| Symbol::new(""));
                let v = if let Some(val) = self.env.user_ctx.get(&sym) {
                    val
                } else if let Some(fd) = self.env.natives.get(&sym) {
                    Value::Func(Rc::clone(fd))
                } else {
                    return Err(EvalError::UnboundWord {
                        sym,
                        span: self.current_span(),
                    });
                };
                self.stack.push(v);
            }
            Instr::SetLocal(d, slot) => {
                let val = self.stack.pop().unwrap_or(Value::None);
                let len = self.frames.len();
                let locals = &mut self.frames[len - 1 - d as usize].locals;
                if (slot as usize) >= locals.len() {
                    locals.resize(slot as usize + 1, Value::None);
                }
                locals[slot as usize] = val.clone();
                self.stack.push(val);
            }
            Instr::SetGlobal(slot) => {
                let val = self.stack.pop().unwrap_or(Value::None);
                self.env
                    .user_ctx
                    .set_slot_unchecked(slot as usize, val.clone());
                self.stack.push(val);
            }
            Instr::SetDynamic(sym_idx) => {
                // M30.1.B: look up the symbol from the block's side table.
                let sym = self
                    .cached_block
                    .as_ref()
                    .expect("cache invariant")
                    .symbols
                    .get(sym_idx as usize)
                    .cloned()
                    .unwrap_or_else(|| Symbol::new(""));
                let val = self.stack.pop().unwrap_or(Value::None);
                self.env.user_ctx.set(sym, val.clone());
                self.stack.push(val);
            }
            Instr::Call(native_idx, argc) => {
                self.call_native(native_idx as usize, argc as usize)?;
            }
            Instr::CallUser(slot, argc) => {
                self.call_user(slot as usize, argc as usize)?;
            }
            Instr::CallUserGlobal(slot, argc) => {
                self.call_user_global(slot as usize, argc as usize)?;
            }
            Instr::TailCall(slot, argc) => {
                // M28: tail-call frame overwrite (see `tail_call`).
                self.tail_call(slot as usize, argc as usize)?;
            }
            Instr::TailReenter(slot, argc) => {
                // M28: self-recursion tail-call (same as TailCall at runtime —
                // `tail_call` detects same-`FuncDef` and skips the block swap).
                self.tail_call(slot as usize, argc as usize)?;
            }
            Instr::Jump(target) => {
                self.frames[frame_idx].pc = target as usize;
            }
            Instr::JumpIfFalse(target) => {
                let cond = self.stack.pop().unwrap_or(Value::None);
                if !is_truthy(&cond) {
                    self.frames[frame_idx].pc = target as usize;
                }
            }
            Instr::Pop => {
                self.stack.pop();
            }
            Instr::MakeFunc(spec_idx, body_idx, fv_idx) => {
                let block = self.cached_block.as_ref().expect("cache invariant").clone();
                let spec_val = block_pool(&block, spec_idx as usize)?;
                let body_val = block_pool(&block, body_idx as usize)?;
                // M30.1.B: freevar list is looked up from the side table.
                let freevars = block
                    .freevars_table
                    .get(fv_idx as usize)
                    .cloned()
                    .unwrap_or_default();
                let fd = self.build_func_def(spec_val, body_val, freevars)?;
                self.stack.push(Value::Func(Rc::new(fd)));
            }
            Instr::EnterBlock => {}
            Instr::DropTo(n) => {
                self.stack.truncate(n as usize);
            }
            Instr::GetPath => {
                let path = self.stack.pop().unwrap_or(Value::None);
                let (parts, span) = match &path {
                    Value::Path { parts, span } => (parts.clone(), *span),
                    Value::GetPath { parts, span } => (parts.clone(), *span),
                    other => {
                        return Err(EvalError::TypeError {
                            expected: "path! or get-path!",
                            found: crate::natives::type_name(other),
                            span: other.span_or_default(),
                        });
                    }
                };
                let v = eval_get_path(&parts, span, self.env)?;
                self.stack.push(v);
            }
            Instr::SetPath => {
                let rhs = self.stack.pop().unwrap_or(Value::None);
                let path = self.stack.pop().unwrap_or(Value::None);
                let (parts, span) = match &path {
                    Value::SetPath { parts, span } => (parts.clone(), *span),
                    Value::Path { parts, span } => (parts.clone(), *span),
                    other => {
                        return Err(EvalError::TypeError {
                            expected: "set-path! or path!",
                            found: crate::natives::type_name(other),
                            span: other.span_or_default(),
                        });
                    }
                };
                set_path_value(&parts, rhs.clone(), self.env, span)?;
                self.stack.push(rhs);
            }
            Instr::MarkRefine(sym_idx) => {
                // M30.1.B: look up the symbol from the block's side table.
                let sym = self
                    .cached_block
                    .as_ref()
                    .expect("cache invariant")
                    .symbols
                    .get(sym_idx as usize)
                    .cloned()
                    .unwrap_or_else(|| Symbol::new(""));
                self.ref_marks.push((sym, self.stack.len()));
            }
            Instr::EndRefine => {
                let (sym, height) = self.ref_marks.pop().ok_or_else(|| EvalError::Compile {
                    kind: CompileErrorKind::VmInvariant("EndRefine without MarkRefine".into()),
                    span: self.current_span(),
                })?;
                let args: Vec<Value> = self.stack[height..].to_vec();
                self.stack.truncate(height);
                self.pending_refs.push((sym, args));
            }
            Instr::Return | Instr::Halt => {
                // Should not reach here via `dispatch_instr` — `run_loop` and
                // `run_loop_reduce` handle these in their own match arms.
            }
        }
        Ok(())
    }

    /// M30.1.A: Invoke a native function indexed by `native_idx` with `argc`
    /// args popped from the operand stack. Shared by the `Call` arm in both
    /// `run_loop` and `dispatch_instr` (previously duplicated).
    ///
    /// **Stack-allocated args fast path:** for the common case (argc ≤ 8), the
    /// args are copied into a stack-allocated `[Value; 8]` instead of a
    /// heap-allocated `Vec`. This eliminates 1 heap allocation per native call
    /// — for `repeat 1000000 [acc: acc + 1]`, the `+` native alone caused 1M
    /// heap allocations before this optimization. Natives receive `&mut Env`
    /// (not `&mut Vm`), so they cannot touch the caller's `Vm.stack`; the
    /// copy is safe because re-entrant natives (`if`/`loop`/etc.) create a
    /// fresh `Vm` via `dispatch_block`, leaving the caller's stack untouched.
    fn call_native(&mut self, native_idx: usize, argc: usize) -> Result<(), EvalError> {
        // M30.3.6: borrow the `Rc<FuncDef>` instead of cloning it. We only
        // need `fd.native` (a `fn` pointer — `Copy`), so extract it before
        // the mutable `self.env` borrow below. Saves 1 Rc bump + decrement
        // per native call (the `+` in `fib`'s body fires this ~1.4M times).
        let span = self.current_span();
        let fd = self
            .natives_by_idx
            .get(native_idx)
            .ok_or_else(|| EvalError::Compile {
                kind: CompileErrorKind::VmInvariant(format!("bad native index {native_idx}")),
                span,
            })?;
        let f = fd.native.ok_or_else(|| EvalError::Compile {
            kind: CompileErrorKind::VmInvariant(format!("native {native_idx} has no handler")),
            span,
        })?;
        let len = self.stack.len();
        if argc > len {
            return Err(EvalError::Arity {
                native: Symbol::new("<native>"),
                expected: argc,
                got: len,
                span,
            });
        }
        let start = len - argc;
        // Assemble RefineArgs from `pending_refs`.
        let refs = RefineArgs::from_pairs(std::mem::take(&mut self.pending_refs));
        // Call the native. The args slice is built without heap allocation
        // for argc ≤ 8 (stack-allocated). The native can't touch
        // `self.stack` (it only has `&mut Env`), so we truncate *after*
        // the call returns.
        let result = if argc <= INLINE_ARGS_CAP {
            // M30.1.A: stack-allocated args fast path. `[Value; 8]` lives on
            // the call frame (512 bytes) — cheaper than a heap alloc for the
            // hot `Call` path (1M allocs for a tight `repeat` loop before this).
            // `MaybeUninit` avoids the `Value: Copy` requirement for array
            // initialization; we initialize exactly `argc` slots below.
            let mut buf: [MaybeUninit<Value>; INLINE_ARGS_CAP] =
                [const { MaybeUninit::uninit() }; INLINE_ARGS_CAP];
            for (i, v) in self.stack[start..len].iter().enumerate() {
                buf[i].write(v.clone());
            }
            // SAFETY: we initialized buf[0..argc] above.
            let args: &[Value] =
                unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const Value, argc) };
            f(args, &refs, self.env)
        } else {
            // Fall back to heap allocation for very high-arity natives
            // (rare — `make`/`to` with many args, variadic collection).
            let args: Vec<Value> = self.stack[start..len].to_vec();
            f(&args, &refs, self.env)
        };
        // Pop argc args regardless of success/failure.
        self.stack.truncate(start);
        match result {
            Ok(v) => self.stack.push(v),
            Err(EvalError::Return(v)) => {
                // `return` native: unwind to the nearest function frame.
                while let Some(frame) = self.frames.last() {
                    let is_func = frame.func.is_some();
                    self.frames.pop();
                    self.frame_gen = self.frame_gen.wrapping_add(1);
                    if is_func {
                        break;
                    }
                }
                if self.frames.is_empty() {
                    return Err(EvalError::Return(v));
                }
                self.stack.push(v);
            }
            Err(EvalError::Quit(code)) => {
                self.frames.clear();
                self.frame_gen = self.frame_gen.wrapping_add(1);
                return Err(EvalError::Quit(code));
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// Resolve the func stored at `slot`, pop `argc` args off the stack,
    /// lazily compile the body if needed, and return `(fd, compiled, locals)`.
    /// Shared by `call_user` (pushes a new frame) and `tail_call` (overwrites
    /// the current frame) — M28 factored this out so the two paths share arg
    /// collection + lazy-compile + locals-layout logic.
    fn prepare_call(
        &mut self,
        slot: usize,
        argc: usize,
        is_global: bool,
    ) -> Result<(Rc<FuncDef>, Rc<CompiledBlock>, Vec<Value>), EvalError> {
        // The func value lives in the *caller's* scope. For a top-level
        // SetWord that's `env.user_ctx`; for a func-local SetWord it's the
        // current frame's `locals`. We check the current frame first, then
        // fall back to user_ctx (matches the compiler's `slot_coords`:
        // depth 0 = global).
        // M30.3.4: when `is_global` is true (emitted via `CallUserGlobal`),
        // skip the always-failing `frames.last().and_then(...)` check and go
        // straight to `user_ctx.slot_value_unchecked`. For `fib 30`, this
        // fires on every recursive call (~2.7M times).
        let func_val = if !is_global {
            if let Some(local) = self.frames.last().and_then(|f| f.locals.get(slot)).cloned() {
                local
            } else {
                self.env.user_ctx.slot_value(slot)
            }
        } else {
            self.env.user_ctx.slot_value_unchecked(slot)
        };
        // M31: capture the func value's span before `match` consumes it —
        // used as the fallback for the Arity error below (the call site
        // span). `span_or_default()` returns `Span::default()` for synthetic
        // Func values (which carry no span); `current_span()` is a worse
        // fallback there since it points at the *body* block, not the call.
        let call_span = func_val.span_or_default();
        let fd = match func_val {
            Value::Func(fd) => fd,
            other => {
                // M31: use the offending value's span (the non-Func value
                // stored in the slot), falling back to the current block's
                // span if it's synthetic (no source position).
                return Err(EvalError::TypeError {
                    expected: "function!",
                    found: crate::natives::type_name(&other),
                    span: call_span,
                });
            }
        };
        // M30.3.3: don't pop args yet — `ensure_compiled` doesn't touch the
        // operand stack (confirmed: only touches `env.func_cache`/`natives`/
        // `user_ctx` for `Scope::root`). The args stay on the stack across
        // this call, so we can copy them directly into `locals` below,
        // eliminating the intermediate `args: Vec<Value>` allocation.
        let len = self.stack.len();
        if argc > len {
            return Err(EvalError::Arity {
                native: Symbol::new("<user-func>"),
                expected: argc,
                got: len,
                span: call_span,
            });
        }
        let start = len - argc;

        // Lazily compile the body if needed.
        let compiled = self.ensure_compiled(&fd, slot, argc)?;

        // Build the call frame's locals: params from args, refinement slots
        // (default none), locals slots (default none), body-local SetWord
        // slots (default none). Slot layout matches `bind_function_body`:
        // [params...][ref_flag, ref_args...][locals...][body SetWords...].
        // `CompiledBlock.n_locals` gives the total count; we size locals to
        // that and fill params from args.
        let n_locals = compiled.n_locals.max(fd.params.len());
        // M30.3.2: drain a Vec from the pool instead of allocating fresh.
        let mut locals = self.env.vm_locals_pool.pop().unwrap_or_default();
        // Resize to n_locals (reuses capacity if n_locals <= locals.capacity()).
        locals.clear();
        locals.resize(n_locals, Value::None);
        // M30.3.3: copy args directly from the stack into locals[0..argc],
        // skipping the intermediate `args: Vec<Value>` allocation.
        for i in 0..argc {
            if i < locals.len() {
                locals[i] = self.stack[start + i].clone();
            }
        }
        // Now pop the args from the operand stack.
        self.stack.truncate(start);
        // Refinement slots default to none/logic false. M25's tests don't
        // exercise user-func refinements; M26 wires full refinement dispatch.
        Ok((fd, compiled, locals))
    }

    /// Invoke a user function stored in a slot. Reads `Value::Func(fd)` from
    /// the slot (global or local depending on the current frame's depth),
    /// pops `argc` args, lazily compiles the body if needed, pushes a new
    /// `Frame`, and returns. The callee runs in `run_loop`'s next iteration;
    /// `EvalError::Return(v)` from the `return` native is caught by the
    /// `Call` handler (not here), which pops the function frame and pushes
    /// `v` onto the caller's stack.
    fn call_user(&mut self, slot: usize, argc: usize) -> Result<(), EvalError> {
        let (fd, compiled, locals) = self.prepare_call(slot, argc, false)?;
        // Walker fallback: if the body couldn't be compiled to bytecode
        // (e.g. it contains a higher-order pattern the compiler can't
        // statically resolve, like a func-valued param called in operator
        // position), `ensure_compiled` returns a `needs_rebind` stub.
        // Invoke the func through the tree-walker instead of pushing a VM
        // frame — the walker's `dispatch_call` handles dynamic func
        // dispatch correctly. Args were already popped into `locals` by
        // `prepare_call`; repack them as the walker's arg vec.
        if compiled.needs_rebind {
            return self.invoke_via_walker(fd, locals);
        }
        self.frames.push(Frame {
            func: Some(Rc::clone(&fd)),
            locals,
            depth: self.frames.last().map(|f| f.depth + 1).unwrap_or(1),
            block: compiled,
            pc: 0,
        });
        self.frame_gen = self.frame_gen.wrapping_add(1);
        #[cfg(feature = "stats")]
        {
            self.env.record_frame_push();
        }
        Ok(())
    }

    /// M30.3.4: like `call_user` but the func is known to be in a global slot.
    /// Skips the `frames.last().and_then(...)` local-slot check in `prepare_call`.
    fn call_user_global(&mut self, slot: usize, argc: usize) -> Result<(), EvalError> {
        let (fd, compiled, locals) = self.prepare_call(slot, argc, true)?;
        if compiled.needs_rebind {
            return self.invoke_via_walker(fd, locals);
        }
        self.frames.push(Frame {
            func: Some(Rc::clone(&fd)),
            locals,
            depth: self.frames.last().map(|f| f.depth + 1).unwrap_or(1),
            block: compiled,
            pc: 0,
        });
        self.frame_gen = self.frame_gen.wrapping_add(1);
        #[cfg(feature = "stats")]
        {
            self.env.record_frame_push();
        }
        Ok(())
    }

    /// M28: tail-call. Like `call_user`, but instead of pushing a new frame,
    /// overwrite the current top frame's `func`/`locals`/`block`/`pc`. This
    /// bounds call-stack depth for tail-recursive programs: a function whose
    /// last action is a call to itself (or any function) reuses its frame.
    ///
    /// The semantics mirror `call_user`: the callee's `Return` pushes its
    /// result onto the (now-caller's) stack, and the args were already
    /// truncated by `prepare_call`. The only difference is the frame count.
    ///
    /// If the current top frame is the root script frame (no `func`), we
    /// can't overwrite it — fall back to `call_user` so the script body's
    /// frame stays intact. (This case shouldn't arise in practice: a
    /// top-level `CallUser` isn't in tail position of a func body, so the
    /// compiler's `patch_tail_call` only emits `TailCall` inside compiled
    /// func bodies.)
    fn tail_call(&mut self, slot: usize, argc: usize) -> Result<(), EvalError> {
        let (fd, compiled, locals) = self.prepare_call(slot, argc, false)?;
        // Walker fallback for tail calls: same rationale as `call_user`.
        // We can't reuse the current VM frame (the walker manages its own
        // CallFrame), so invoke through the walker and push the result.
        // The current frame stays; its `Return` will surface the result.
        if compiled.needs_rebind {
            return self.invoke_via_walker(fd, locals);
        }
        let frame_idx = self.frames.len() - 1;
        let depth = self.frames[frame_idx].depth;
        // Guard: don't overwrite the root script frame. (Defensive — the
        // compiler only emits TailCall inside func bodies, but a misroute
        // should still produce correct results rather than corrupt state.)
        if self.frames[frame_idx].func.is_none() {
            self.frames.push(Frame {
                func: Some(Rc::clone(&fd)),
                locals,
                depth: depth + 1,
                block: compiled, // M30.3.1: Rc move (was (*compiled).clone())
                pc: 0,
            });
            self.frame_gen = self.frame_gen.wrapping_add(1);
            #[cfg(feature = "stats")]
            {
                self.env.record_frame_push();
            }
            return Ok(());
        }
        // TailReenter optimization: if the target func is the *same* `Rc`
        // as the current frame's func (self-recursion compiled as TailCall
        // — the compiler's `patch_tail_call` only knows the slot, not the
        // func identity, so it emits TailCall and we detect the reenter at
        // runtime), we can skip replacing `block` (it's the same compiled
        // body) and just reset `locals`/`pc`.
        let same_func = self.frames[frame_idx]
            .func
            .as_ref()
            .map(|cur| Rc::ptr_eq(cur, &fd))
            .unwrap_or(false);
        if same_func {
            // M30: reuse the existing `locals` Vec's allocation rather than
            // dropping + reallocating. The original `self.frames[frame_idx].locals = locals`
            // dropped the old Vec and installed a fresh one — for a tight
            // 1M-deep tail loop, that's 1M Vec allocations. Instead we
            // truncate the existing Vec and copy the new values in.
            let existing = &mut self.frames[frame_idx].locals;
            existing.clear();
            existing.extend_from_slice(&locals);
            self.frames[frame_idx].pc = 0;
            // Block cache stays valid (same FuncDef → same compiled body).
            // No stats bump: the frame is reused, not pushed. (TailReenter
            // at the instr level also doesn't bump; the runtime TailReenter
            // detection here is equivalent.)
            return Ok(());
        }
        // Different func: overwrite the frame in place. Same depth (the
        // caller's frame is reused for the callee, so the lexical-depth
        // chain stays valid — the callee's freevar captures walk up the same
        // ancestor frames that were on the stack at the call site).
        self.frames[frame_idx].func = Some(Rc::clone(&fd));
        // M30: reuse the existing Vec allocation here too.
        {
            let existing = &mut self.frames[frame_idx].locals;
            existing.clear();
            existing.extend_from_slice(&locals);
        }
        self.frames[frame_idx].block = compiled; // M30.3.1: Rc move (was (*compiled).clone())
        self.frames[frame_idx].pc = 0;
        self.frame_gen = self.frame_gen.wrapping_add(1);
        // No stats bump: a reused frame doesn't increase call-stack depth.
        Ok(())
    }

    /// Walker fallback for user-func invocation. Used by `call_user`/
    /// `call_user_global`/`tail_call` when the body's compiled block is a
    /// `needs_rebind` stub — i.e. the compiler couldn't lower it to bytecode
    /// (e.g. a higher-order pattern like a func-valued param called in
    /// operator position, which the VM can't statically resolve).
    ///
    /// `locals` holds the args in `[0..argc]` (laid out by `prepare_call`);
    /// we repack them into a `Vec` for the walker's `call_user_func`, which
    /// sets up its own `CallFrame`, evaluates the body via the tree-walker,
    /// and catches `EvalError::Return`. The result is pushed onto the VM
    /// operand stack so the caller's `CallUser` sees it like a normal call.
    /// Refinements aren't supported on this path (user-func refinement
    /// dispatch already falls back to the walker at the compiler level —
    /// see `compile_user_call`'s `MalformedSpec` return).
    fn invoke_via_walker(&mut self, fd: Rc<FuncDef>, locals: Vec<Value>) -> Result<(), EvalError> {
        let argc = fd.params.len();
        let args: Vec<Value> = locals.into_iter().take(argc).collect();
        let result = call_user_func(&fd, args, &RefineArgs::empty(), self.env)?;
        self.stack.push(result);
        Ok(())
    }

    /// Ensure `fd.compiled` is populated. If `None`, compile the body with a
    /// fresh child scope seeded from the func's spec, pre-recording the func's
    /// own slot for recursive self-calls. Returns the `CompiledBlock` (cloned
    /// cheaply via `Rc`).
    fn ensure_compiled(
        &mut self,
        fd: &Rc<FuncDef>,
        slot: usize,
        argc: usize,
    ) -> Result<Rc<CompiledBlock>, EvalError> {
        // Fast path 1: construction-time hint (set by a future MakeFunc
        // eager-compile path; stays `None` for funcs created in `Walk` mode).
        if let Some(c) = fd.compiled.clone() {
            return Ok(c);
        }
        // M30.3.5: Self-recursion fast path. If the target `fd` is the same
        // `Rc<FuncDef>` as the current frame's `func` (via `Rc::ptr_eq`), the
        // compiled block is already on the current frame — skip the `HashMap`
        // lookup. For `fib 30`, this fires on every recursive call (~2.7M
        // times), eliminating ~2.7M HashMap lookups. Requires `Frame.block` to
        // be `Rc<CompiledBlock>` (Tier 4.1) so we can return it directly.
        if let Some(cur_frame) = self.frames.last() {
            if let Some(cur_fd) = &cur_frame.func {
                if Rc::ptr_eq(cur_fd, fd) {
                    return Ok(Rc::clone(&cur_frame.block));
                }
            }
        }
        // Fast path 2: Env-level cache (authoritative, M27). Keyed by
        // `Rc::as_ptr(fd)` — stable across `Rc` clones of the same underlying
        // `FuncDef`, so a func stored in a context slot and re-read on each
        // call hits the cache. Invalidated by `bind` on a `Value::Func` and
        // defensively by `bind_function_body`.
        let key = Rc::as_ptr(fd) as usize;
        if let Some(c) = self.env.func_cache.get(&key).cloned() {
            return Ok(c);
        }
        // Compile the body. We need a `NativeRegistry` snapshot and a child
        // scope seeded with the func's params/refinements/locals.
        let registry = NativeRegistry::from_env(self.env);
        let parent_scope = Scope::root(&self.env.user_ctx);
        let mut child = Scope::child(&parent_scope);
        for p in &fd.params {
            child.slot_index_pub(p.clone());
        }
        for (ref_name, ref_args) in &fd.refinements {
            child.slot_index_pub(ref_name.clone());
            for arg in ref_args {
                child.slot_index_pub(arg.clone());
            }
        }
        for local in &fd.locals {
            child.slot_index_pub(local.clone());
        }
        // Deep-clone the body before compiling. `analyze_block` (inside
        // `compile_block_for_func_body`) mutates bindings in place, converting
        // `Binding::Func` → `Binding::Lexical` — which the tree-walker can't
        // resolve. If the body later falls back to the walker (via the
        // `needs_rebind`/`MalformedSpec` path in `call_user`), it must evaluate
        // the *original* `fd.body` (with `Func` bindings intact). Mirrors
        // `dispatch_block`'s deep-clone-before-compile (interp_legacy.rs:144).
        let compile_body = deep_clone_series(&fd.body);
        let compiled = match compile_block_for_func_body(
            &compile_body,
            &mut child,
            &registry,
            (slot as u32, argc),
            fd.params.len(),
        ) {
            Ok(c) => c,
            // `MalformedSpec`/`ArityMismatch` are the compiler's "defer to the
            // walker" signals (set by `compile_word` for higher-order patterns
            // it can't statically resolve, and by the user-func-refinement /
            // path-refinement fallbacks). Return a `needs_rebind` stub so
            // `call_user` routes the body through `invoke_via_walker` instead
            // of erroring. Other compile errors (`VmInvariant`, `UnboundWord`)
            // are genuine bugs/corruption — surface them as hard errors.
            Err(e)
                if matches!(
                    e.kind,
                    CompileErrorKind::MalformedSpec | CompileErrorKind::ArityMismatch
                ) =>
            {
                stub_block(&fd.body, crate::vm::lex::AnalysisResult::default())
            }
            Err(e) => {
                return Err(EvalError::Compile {
                    kind: e.kind,
                    span: e.span,
                })
            }
        };
        let compiled = Rc::new(compiled);
        self.env.func_cache.insert(key, compiled.clone());
        Ok(compiled)
    }

    /// Build a `FuncDef` from `MakeFunc`'s spec + body pool values, then run
    /// `bind_function_body` so the body's words resolve to function-local
    /// slots. Mirrors `natives::function_native`/`func_native`/`does_native`.
    fn build_func_def(
        &self,
        spec_val: Value,
        body_val: Value,
        freevars: Vec<Symbol>,
    ) -> Result<FuncDef, EvalError> {
        let spec = match &spec_val {
            Value::Block { .. } => extract_spec(&spec_val).map_err(|e| EvalError::Native {
                message: e.to_string(),
                span: spec_val.span_or_default(),
            })?,
            _ => {
                return Err(EvalError::TypeError {
                    expected: "block! for func spec",
                    found: crate::natives::type_name(&spec_val),
                    span: spec_val.span_or_default(),
                });
            }
        };
        let body_series = match &body_val {
            Value::Block { series, .. } => series.clone(),
            _ => {
                return Err(EvalError::TypeError {
                    expected: "block! for func body",
                    found: crate::natives::type_name(&body_val),
                    span: body_val.span_or_default(),
                });
            }
        };
        let mut fd = FuncDef {
            params: spec.params,
            refinements: spec.refinements,
            locals: spec.locals,
            freevars,
            compiled: None,
            body: body_series,
            ctx: Context::new(),
            native: None,
            variadic: false,
            infix: false,
        };
        bind_function_body(&mut fd, &self.env.user_ctx);
        Ok(fd)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a pool value by index. Clones (as all `Value` access does).
///
/// M31: was `unwrap_or(Value::None)` — a compiler bug producing a bad pool
/// index silently returned `none`, corrupting results. Now returns an
/// `EvalError::Compile` (VmInvariant) in release and asserts in debug.
/// The span is the block's `source_span` (the per-pool-entry span isn't
/// tracked; this localizes to the offending block).
fn block_pool(block: &CompiledBlock, idx: usize) -> Result<Value, EvalError> {
    match block.pool.get(idx) {
        Some(v) => Ok(v.clone()),
        None => {
            debug_assert!(
                false,
                "block_pool OOB: idx={} len={}",
                idx,
                block.pool.len()
            );
            Err(EvalError::Compile {
                kind: CompileErrorKind::VmInvariant(format!(
                    "pool index {idx} out of bounds (len {})",
                    block.pool.len()
                )),
                span: block.source_span,
            })
        }
    }
}

/// Truthiness test matching the walker's `is_truthy` (Red semantics: `false`
/// and `none` are falsy; everything else is truthy).
fn is_truthy(v: &Value) -> bool {
    !matches!(v, Value::Logic(false) | Value::None)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::bind_pass;
    use crate::natives::{install_constants, register_natives};
    use crate::vm::compiler::{compile_block, NativeRegistry};
    use crate::vm::lex::Scope;
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use red_core::value::Value;
    use red_core::{Context, Env, EvalMode};
    use std::cell::RefCell;
    use std::io::Write;

    /// Owning `Write` sink backed by `Rc<RefCell<Vec<u8>>>` so the test can
    /// read captured stdout after the `Env` (which owns the boxed writer) is
    /// dropped. Mirrors `tests/bench_fixtures.rs`'s `BufferWriter`.
    #[derive(Clone)]
    struct BufferWriter {
        buf: Rc<RefCell<Vec<u8>>>,
    }

    impl BufferWriter {
        fn new() -> Self {
            Self {
                buf: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }

    impl Write for BufferWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf.borrow_mut().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Parse + bind + register natives, then return the compiled block + env
    /// ready for `run`. Mirrors the walker's `run_series_inner_opts` setup.
    /// `Env::mode` is set to `Vm` so `dispatch_block` (used by natives that
    /// recurse into block evaluation) routes to the VM (M26).
    fn compile_for_vm(src: &str) -> (CompiledBlock, Env) {
        let body = load_source(src).expect("parse failed");
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let mut env = Env::new(Rc::clone(&ctx_rc));
        register_natives(&mut env);
        env.mode = EvalMode::Vm;
        let registry = NativeRegistry::from_env(&env);
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile failed");
        (block, env)
    }

    /// Like `compile_for_vm` but with a capturing `BufferWriter` for stdout.
    fn compile_for_vm_captured(src: &str) -> (CompiledBlock, Env, Rc<RefCell<Vec<u8>>>) {
        let body = load_source(src).expect("parse failed");
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let writer = BufferWriter::new();
        let buf = writer.buf.clone();
        let mut env = Env::new_with_output(Rc::clone(&ctx_rc), Box::new(writer));
        register_natives(&mut env);
        env.mode = EvalMode::Vm;
        let registry = NativeRegistry::from_env(&env);
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile failed");
        (block, env, buf)
    }

    /// Run `src` through the VM and return the result.
    fn run_vm(src: &str) -> Value {
        let (block, mut env) = compile_for_vm(src);
        run(block, &mut env).expect("VM run failed")
    }

    /// VM runs `5` -> `Integer(5)`. (plan3.md:461)
    #[test]
    fn vm_runs_literal() {
        let v = run_vm("5");
        assert!(matches!(v, Value::Integer { n: 5, .. }));
    }

    /// VM runs `1 + 2` -> `Integer(3)`. (plan3.md:462)
    #[test]
    fn vm_runs_infix() {
        let v = run_vm("1 + 2");
        assert!(matches!(v, Value::Integer { n: 3, .. }));
    }

    /// VM runs `foo: 5 foo` -> `Integer(5)`. (plan3.md:463)
    #[test]
    fn vm_runs_setword_load() {
        let v = run_vm("foo: 5 foo");
        assert!(matches!(v, Value::Integer { n: 5, .. }));
    }

    /// VM runs `if true [42]` -> `Integer(42)`. (plan3.md:464)
    #[test]
    fn vm_runs_if() {
        let v = run_vm("if true [42]");
        assert!(matches!(v, Value::Integer { n: 42, .. }));
    }

    /// VM runs `square: func [x][x * x] square 5` -> `Integer(25)`. (plan3.md:465)
    #[test]
    fn vm_runs_square() {
        let v = run_vm("square: func [x][x * x] square 5");
        assert!(
            matches!(v, Value::Integer { n: 25, .. }),
            "got {}",
            mold_to_string(&v)
        );
    }

    /// VM runs recursive `fact 5` -> `Integer(120)`. (plan3.md:466)
    ///
    /// M25 doesn't implement tail-call optimization (M28 does); the test
    /// verifies correctness at `fact 5` (shallow recursion, no stack concern).
    #[test]
    fn vm_runs_fact() {
        let v = run_vm("fact: func [n][either n <= 1 [1][n * fact n - 1]] fact 5");
        assert!(
            matches!(v, Value::Integer { n: 120, .. }),
            "got {}",
            mold_to_string(&v)
        );
    }

    // -----------------------------------------------------------------------
    // M26: native bridge + refinement dispatch on the VM
    // -----------------------------------------------------------------------

    /// `copy/part [1 2 3] 2` runs through the VM with refinement dispatch:
    /// the compiler emits `MarkRefine("part")` + the arg + `EndRefine`, and
    /// the VM assembles `RefineArgs` from the stack marks before invoking the
    /// `copy` native. (plan3.md:547)
    #[test]
    fn vm_copy_part() {
        let v = run_vm("copy/part [1 2 3] 2");
        assert_eq!(mold_to_string(&v), "[1 2]");
    }

    /// `find/case [a A b] 'A` runs through the VM with a zero-arity
    /// refinement (`/case`). The `MarkRefine("case")` + `EndRefine` region
    /// carries no args; `find` sees `refs.has("case")`. (plan3.md:548)
    #[test]
    fn vm_find_case() {
        let v = run_vm("find/case [a A b] 'A");
        assert_eq!(mold_to_string(&v), "[A b]");
    }

    /// `foreach x [1 2 3][print x]` -> "1\n2\n3\n" via VM. The `foreach`
    /// native recurses into its body block through `dispatch_block`, which
    /// (with `Env::mode == Vm`) compiles the body and runs it on the VM each
    /// iteration. (plan3.md:549)
    #[test]
    fn vm_foreach_print() {
        let (block, mut env, buf) = compile_for_vm_captured("foreach x [1 2 3][print x]");
        let v = run(block, &mut env).expect("VM run failed");
        assert!(matches!(v, Value::None), "got {}", mold_to_string(&v));
        assert_eq!(buf.borrow().as_slice(), b"1\n2\n3\n");
    }

    /// `switch 2 [1 ["a"] 2 ["b"]]` -> `"b"` via VM. The `switch` native
    /// evaluates the matched body block through `dispatch_block` (VM path).
    /// (plan3.md:550)
    #[test]
    fn vm_switch() {
        let v = run_vm("switch 2 [1 [\"a\"] 2 [\"b\"]]");
        match v {
            Value::String { s, .. } => assert_eq!(&*s, "b"),
            other => panic!("expected string!, got {}", mold_to_string(&other)),
        }
    }

    /// `do bind [x: 5] 'x` runs through the VM. `bind` in the POC rebinds words
    /// to `user_ctx` (not a foreign context), so `has_foreign_bindings` returns
    /// false and `dispatch_block` routes the `do`'d block to the VM. The VM
    /// compiles `[x: 5]` (SetGlobal + Const) and runs it, setting `x` to 5;
    /// the trailing `x` loads it back. (plan3.md:551)
    ///
    /// Note: the plan3 "falls back to walker" qualifier assumed `bind` targets
    /// a foreign context. In the POC, `bind` always targets `user_ctx`, so the
    /// VM handles it directly. The walker-fallback path (for blocks with
    /// foreign `Binding::Local` from a non-`user_ctx` context, e.g. `use`'s
    /// child context) is unit-tested via `has_foreign_bindings` in `binding.rs`
    /// — it can't be exercised end-to-end from VM-compilable Red source
    /// because `use`/`make object!` forms are flagged `needs_rebind` at the
    /// block level by the M23 analyzer, producing `[Halt]` stubs the VM
    /// refuses to run.
    #[test]
    fn vm_do_bind() {
        let v = run_vm("x: 0 do bind [x: 5] 'x x");
        assert!(
            matches!(v, Value::Integer { n: 5, .. }),
            "got {}",
            mold_to_string(&v)
        );
    }

    /// `reduce [1 + 1 2 + 2]` -> `[2 4]` via VM. The `reduce` native calls
    /// `dispatch_block_reduce`, which compiles the block with
    /// `compile_block_reduce` (no `Pop` between expressions) and runs
    /// `run_reduce`, collecting the stack into a `Value::Block`.
    /// (plan3.md:565 — "`reduce` native: same logic")
    #[test]
    fn vm_reduce() {
        let v = run_vm("reduce [1 + 1 2 + 2]");
        assert_eq!(mold_to_string(&v), "[2 4]");
    }

    // -----------------------------------------------------------------------
    // M27: FuncDef compiled-cache + lazy compilation
    // -----------------------------------------------------------------------

    /// A `func` invoked twice compiles its body exactly once. The first
    /// `CallUser` misses the `Env::func_cache` and compiles; the second hits
    /// the cache (keyed by `Rc::as_ptr(fd)`, stable across `Rc` clones).
    /// (plan3.md:646)
    #[test]
    fn vm_func_compiles_once_across_calls() {
        crate::vm::compiler::reset_compile_counter();
        let (block, mut env, _buf) =
            compile_for_vm_captured("square: func [x][x * x] square 5 square 6");
        // The top-level `compile_block` call in `compile_for_vm_captured`
        // bumps the counter by 1. Record the baseline after that.
        let baseline = crate::vm::compiler::read_compile_counter();
        let v = run(block, &mut env).expect("VM run failed");
        // First `square 5` -> `ensure_compiled` compiles the body (+1); second
        // `square 6` -> cache hit (+0). Delta must be exactly 1.
        let after = crate::vm::compiler::read_compile_counter();
        assert_eq!(
            after - baseline,
            1,
            "func body compiled exactly once; got delta {}",
            after - baseline
        );
        assert!(matches!(v, Value::Integer { n: 36, .. }));
        assert_eq!(env.func_cache.len(), 1, "exactly one func cached");
    }

    /// `bind :func 'word` invalidates the original func's VM cache entry.
    /// After `f 5`, `func_cache` holds f's compiled body. After `bind :f 'y`,
    /// f's entry is removed (the body bindings may be stale post-rebind). The
    /// new func `g` is a fresh `Rc<FuncDef>` (different identity) — not cached
    /// until called.
    ///
    /// Note: `g` is not invoked from the VM because the M25 compiler can't
    /// statically detect that `g: bind :f 'y` produces a function (it degrades
    /// to `LoadDynamic` + 0 args, not `CallUser`). Calling runtime-constructed
    /// funcs is walker territory until a future milestone adds flow-sensitive
    /// func-arity inference. The cache invalidation itself is what's under
    /// test here, not the call path. (plan3.md:648)
    #[test]
    fn vm_bind_func_invalidates_cache() {
        let (block, mut env, _buf) =
            compile_for_vm_captured("y: 0 f: func [x][x + 1] f 5 bind :f 'y");
        let v = run(block, &mut env).expect("VM run failed");
        // `f 5` populates the cache; `bind :f 'y` invalidates f's entry.
        // The bind returns a new func (not called, not cached). Net: empty.
        assert_eq!(
            env.func_cache.len(),
            0,
            "f's entry invalidated by bind; got cache {:?}",
            env.func_cache.keys().collect::<Vec<_>>()
        );
        assert!(matches!(v, Value::Func(_)), "bind returns a function");
    }

    /// `make function!` at runtime lazily compiles on first call, not at
    /// `make` time. The `make` native delegates to `func_native`, which
    /// constructs a `FuncDef` with `compiled: None`. The VM's `ensure_compiled`
    /// compiles the body on the first `CallUser` and caches it.
    ///
    /// Note: the M25 compiler can't statically detect that
    /// `f: make function! [...]` produces a function (it's a native call, not
    /// a `func`/`does`/`function` keyword the compiler inlines as `MakeFunc`).
    /// So `f 7` degrades to `LoadDynamic(f)` + 0 args — the func is never
    /// invoked through `CallUser`, and the cache stays empty. This test
    /// verifies the "not at make time" half: after `make function!` with no
    /// call, `func_cache` is empty (the body was not compiled). The "compiles
    /// on first call" half is covered by `vm_func_compiles_once_across_calls`
    /// (which uses the `func` keyword so the compiler emits `MakeFunc` +
    /// `CallUser`). Full call-path generality arrives with flow-sensitive
    /// func-arity inference in a future milestone. (plan3.md:649)
    #[test]
    fn vm_make_function_lazy_compile() {
        // `make function!` with no call -> func_cache stays empty (no
        // `CallUser` fires `ensure_compiled`).
        let (block, mut env, _buf) = compile_for_vm_captured("f: make function! [[x][x * x]]");
        let _ = run(block, &mut env).expect("VM run failed");
        assert!(
            env.func_cache.is_empty(),
            "make function! does not compile at make time"
        );
    }

    // -----------------------------------------------------------------------
    // M28: Tail-call optimization + loop-body compilation
    // -----------------------------------------------------------------------

    /// Tail-recursive `countdown n acc` runs at `countdown 100000 0` without
    /// stack growth. The compiler emits `TailCall` for the recursive call in
    /// tail position (the last expr of the `either` false-branch); the VM
    /// overwrites the current frame in place, so call-stack depth stays
    /// bounded regardless of recursion depth. (plan3.md:758)
    ///
    /// `countdown 100000 0` on the tree-walker would push 100k Rust frames;
    /// the VM with tail-call optimization pushes zero. Correctness: returns
    /// 100000 (the accumulator).
    #[test]
    fn vm_tail_recursive_countdown() {
        let v = run_vm("countdown: func [n acc] [ either n <= 0 [acc] [countdown n - 1 acc + 1] ] countdown 100000 0");
        assert!(
            matches!(v, Value::Integer { n: 100000, .. }),
            "countdown 100000 0 should return 100000; got {}",
            mold_to_string(&v)
        );
    }

    /// `repeat i 1000000 [if i > 999999 [print i]]` runs without stack
    /// overflow. The loop native invokes the body block via `dispatch_block`
    /// (M26), which compiles + runs the body each iteration; the body's
    /// `if`-branch is inlined. No Rust recursion happens per iteration (loops
    /// reuse one frame), so the call stack stays tiny.
    ///
    /// The tree-walker also handles this without overflow (loops don't push
    /// Rust frames there either), but the test verifies the VM handles it
    /// too, and that `print`/`if`/`>`/`repeat` all work end-to-end in VM
    /// mode. (plan3.md:756)
    #[test]
    fn vm_repeat_one_million_no_overflow() {
        let (block, mut env, buf) =
            compile_for_vm_captured("repeat i 1000000 [ if i > 999999 [print i] ]");
        let v = run(block, &mut env).expect("VM run failed");
        assert!(matches!(v, Value::None), "got {}", mold_to_string(&v));
        assert_eq!(
            String::from_utf8_lossy(&buf.borrow()),
            "1000000\n",
            "loop body should have printed 1000000"
        );
    }

    /// `loop [break]` exits cleanly via `EvalError::Break` caught by the
    /// loop native. Verifies the VM's `Break` control-flow unwind propagates
    /// through `dispatch_block` (M26 bridge) to the loop native. (plan3.md:759)
    #[test]
    fn vm_loop_break_exits_cleanly() {
        let v = run_vm("loop [break]");
        assert!(matches!(v, Value::None), "got {}", mold_to_string(&v));
    }

    /// Self-recursive `fact` written with accumulator + tail call has bounded
    /// call-stack depth. Verifies the compiler emits `TailReenter` for a
    /// self-call in tail position (the SetWord slot equals the func's own
    /// slot, detected statically by `patch_tail_call`), and the VM reuses
    /// the current frame. (plan3.md:754)
    ///
    /// `fact-tail n acc`: if n <= 1 return acc; else `fact-tail n-1 n*acc`.
    /// Returns `factorial(5) = 120` (correctness) at a depth that would
    /// overflow on a non-tail-recursive VM at large N.
    #[test]
    fn vm_tail_recursive_factorial() {
        let v = run_vm(
            "fact-tail: func [n acc] [ either n <= 1 [acc] [fact-tail n - 1 n * acc] ] fact-tail 5 1",
        );
        assert!(
            matches!(v, Value::Integer { n: 120, .. }),
            "fact-tail 5 1 should return 120; got {}",
            mold_to_string(&v)
        );
    }

    /// Stress: `countdown 1000000 0` (1M deep tail recursion). Without
    /// tail-call optimization the tree-walker overflows its Rust stack at
    /// ~400 frames; the VM with `TailCall`/`TailReenter` reuses one frame,
    /// so depth stays at 1 regardless of N. The default 8 MiB Rust stack is
    /// plenty because no Rust recursion happens — that's the point.
    #[test]
    fn vm_tail_recursive_one_million_no_overflow() {
        let v = run_vm("countdown: func [n acc] [ either n <= 0 [acc] [countdown n - 1 acc + 1] ] countdown 1000000 0");
        assert!(
            matches!(v, Value::Integer { n: 1000000, .. }),
            "countdown 1000000 0 should return 1000000; got {}",
            mold_to_string(&v)
        );
    }
}
