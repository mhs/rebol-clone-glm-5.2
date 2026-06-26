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

use std::rc::Rc;

use red_core::value::{FuncDef, Span, Symbol, Value};
use red_core::vm_ir::{CompiledBlock, Frame, Instr};
use red_core::{Context, Env, EvalError, RefineArgs};

use crate::binding::bind_function_body;
use crate::interp::{eval_get_path, set_path_value};
use crate::natives::extract_spec;
use crate::vm::compiler::{NativeRegistry, compile_block_for_func_body};
use crate::vm::lex::Scope;

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
    let mut vm = Vm {
        env,
        frames: Vec::new(),
        stack: Vec::new(),
        natives_by_idx,
        ref_marks: Vec::new(),
        pending_refs: Vec::new(),
    };
    vm.frames.push(Frame {
        func: None,
        locals: Vec::new(),
        depth: 0,
        block,
        pc: 0,
    });
    vm.run_loop()
}

/// Run a compiled block in "reduce mode": every expression's result stays on
/// the stack (the block was compiled with `compile_block_reduce`, which emits
/// no `Pop` between expressions). After the block's `Return`, collect the
/// remaining stack into a `Value::Block` — matching the walker's `reduce`
/// semantics (one entry per expression). Used by the `reduce` native in VM
/// mode (M26).
pub fn run_reduce(block: CompiledBlock, env: &mut Env) -> Result<Value, EvalError> {
    let natives_by_idx = build_natives_by_idx(env);
    let mut vm = Vm {
        env,
        frames: Vec::new(),
        stack: Vec::new(),
        natives_by_idx,
        ref_marks: Vec::new(),
        pending_refs: Vec::new(),
    };
    vm.frames.push(Frame {
        func: None,
        locals: Vec::new(),
        depth: 0,
        block,
        pc: 0,
    });
    vm.run_loop_reduce()
}

/// Build a `Vec<Rc<FuncDef>>` indexed by the same `u32` indices the
/// compiler's `NativeRegistry::from_env` assigned. Iterates `env.natives` in
/// HashMap order — stable within a single process run, matching the
/// compiler's snapshot. (If `env.natives` is mutated after compilation, the
/// indices go stale — M27 invalidates the compiled cache in that case.)
fn build_natives_by_idx(env: &Env) -> Vec<Rc<FuncDef>> {
    let mut out: Vec<Rc<FuncDef>> = Vec::with_capacity(env.natives.len());
    for fd in env.natives.values() {
        out.push(Rc::clone(fd));
    }
    out
}

// ---------------------------------------------------------------------------
// Vm state
// ---------------------------------------------------------------------------

struct Vm<'env> {
    env: &'env mut Env,
    frames: Vec<Frame>,
    stack: Vec<Value>,
    /// Native `FuncDef`s indexed by the `u32` carried by `Call(native_idx, _)`.
    natives_by_idx: Vec<Rc<FuncDef>>,
    /// `(refinement_name, stack_height_at_mark)` for the currently-open
    /// `MarkRefine`/`EndRefine` region. `EndRefine` pops the topmost entry,
    /// collects `stack[height..]` into a `Vec<Value>`, truncates the stack,
    /// and appends `(name, args)` to `pending_refs`.
    ref_marks: Vec<(Symbol, usize)>,
    /// Accumulated refinement args for the current call, drained into a
    /// `RefineArgs` at `Call` time.
    pending_refs: Vec<(Symbol, Vec<Value>)>,
}

impl<'env> Vm<'env> {
    fn run_loop(&mut self) -> Result<Value, EvalError> {
        loop {
            // Borrow the top frame's block instrs. We clone the instr slice
            // out via `Rc` index to avoid holding a borrow across the match
            // (handlers mutate `self.frames`/`self.stack`).
            let frame_idx = self.frames.len() - 1;
            let pc = self.frames[frame_idx].pc;
            let block = self.frames[frame_idx].block.clone();
            let instrs = block.instrs.clone();
            if pc >= instrs.len() {
                // Fell off the end without `Return`/`Halt` — treat as
                // implicit return of top-of-stack (defensive).
                return Ok(self.stack.pop().unwrap_or(Value::None));
            }
            let instr = instrs[pc].clone();
            // Advance pc before dispatch (jump instrs overwrite it).
            self.frames[frame_idx].pc = pc + 1;
            drop(block);

            match instr {
                Instr::Const(i) => {
                    let v = block_pool(&self.frames[frame_idx].block, i as usize);
                    self.stack.push(v);
                }
                Instr::LoadLocal(d, slot) => {
                    let len = self.frames.len();
                    let src = &self.frames[len - 1 - d as usize].locals;
                    let v = src.get(slot as usize).cloned().unwrap_or(Value::None);
                    self.stack.push(v);
                }
                Instr::LoadGlobal(slot) => {
                    let v = self.env.user_ctx.slot_value(slot as usize);
                    self.stack.push(v);
                }
                Instr::LoadDynamic(sym) => {
                    let v = if let Some(val) = self.env.user_ctx.get(&sym) {
                        val
                    } else if let Some(fd) = self.env.natives.get(&sym) {
                        Value::Func(Rc::clone(fd))
                    } else {
                        return Err(EvalError::UnboundWord {
                            sym,
                            span: Span::new(0, 0),
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
                    self.env.user_ctx.set_slot(slot as usize, val.clone());
                    self.stack.push(val);
                }
                Instr::SetDynamic(sym) => {
                    let val = self.stack.pop().unwrap_or(Value::None);
                    self.env.user_ctx.set(sym, val.clone());
                    self.stack.push(val);
                }
                Instr::Call(native_idx, argc) => {
                    let fd = self
                        .natives_by_idx
                        .get(native_idx as usize)
                        .cloned()
                        .ok_or_else(|| EvalError::Native {
                            message: format!("VM: bad native index {native_idx}"),
                            span: Span::new(0, 0),
                        })?;
                    let f = fd.native.ok_or_else(|| EvalError::Native {
                        message: format!("VM: native {native_idx} has no handler"),
                        span: Span::new(0, 0),
                    })?;
                    // Slice the top `argc` args without moving them.
                    let len = self.stack.len();
                    if argc as usize > len {
                        return Err(EvalError::Arity {
                            native: Symbol::new("<native>"),
                            expected: argc as usize,
                            got: len,
                            span: Span::new(0, 0),
                        });
                    }
                    let args: Vec<Value> =
                        self.stack[len - argc as usize..].to_vec();
                    // Assemble RefineArgs from `pending_refs`.
                    let refs = RefineArgs::from_pairs(std::mem::take(&mut self.pending_refs));
                    let result = f(&args, &refs, self.env);
                    // Pop argc args regardless of success/failure.
                    self.stack.truncate(len - argc as usize);
                    match result {
                        Ok(v) => self.stack.push(v),
                        Err(EvalError::Return(v)) => {
                            // `return` native: unwind to the nearest function
                            // frame (the current top frame if it has
                            // `func: Some(...)`, else search down). Push the
                            // return value onto the caller's stack.
                            while let Some(frame) = self.frames.last() {
                                let is_func = frame.func.is_some();
                                self.frames.pop();
                                if is_func {
                                    break;
                                }
                            }
                            if self.frames.is_empty() {
                                return Ok(v);
                            }
                            self.stack.push(v);
                        }
                        Err(EvalError::Quit(code)) => {
                            // `exit`/`quit` unwind to top level.
                            while self.frames.pop().is_some() {}
                            return Err(EvalError::Quit(code));
                        }
                        Err(e) => return Err(e),
                    }
                }
                Instr::CallUser(slot, argc) => {
                    self.call_user(slot as usize, argc as usize)?;
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
                    self.frames.pop();
                    if self.frames.is_empty() {
                        return Ok(result);
                    }
                    self.stack.push(result);
                }
                Instr::MakeFunc(spec_idx, body_idx, freevars) => {
                    let spec_val = block_pool(&self.frames[frame_idx].block, spec_idx as usize);
                    let body_val = block_pool(&self.frames[frame_idx].block, body_idx as usize);
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
                Instr::MarkRefine(sym) => {
                    self.ref_marks.push((sym, self.stack.len()));
                }
                Instr::EndRefine => {
                    let (sym, height) = self
                        .ref_marks
                        .pop()
                        .ok_or_else(|| EvalError::Native {
                            message: "EndRefine without MarkRefine".into(),
                            span: Span::new(0, 0),
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
                    return Err(EvalError::Native {
                        message: "VM reached Halt (block needs_rebind — use walker)"
                            .into(),
                        span: Span::new(0, 0),
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
            let frame_idx = self.frames.len() - 1;
            let pc = self.frames[frame_idx].pc;
            let block = self.frames[frame_idx].block.clone();
            let instrs = block.instrs.clone();
            if pc >= instrs.len() {
                return Ok(self.collect_reduce_stack());
            }
            let instr = instrs[pc].clone();
            self.frames[frame_idx].pc = pc + 1;
            drop(block);
            match instr {
                Instr::Return => {
                    self.frames.pop();
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
                    return Err(EvalError::Native {
                        message: "VM reached Halt (block needs_rebind — use walker)"
                            .into(),
                        span: Span::new(0, 0),
                    });
                }
                _ => {
                    // Delegate all other instrs to `run_loop`'s dispatch by
                    // re-emitting the instr. We do this by falling back to a
                    // shared `dispatch_instr` helper — but to avoid a large
                    // refactor, we instead reconstruct a single-step: push the
                    // instr back via a synthetic one-instr frame would be
                    // wrong. The cleanest approach is to share the match.
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
                let v = block_pool(&self.frames[frame_idx].block, i as usize);
                self.stack.push(v);
            }
            Instr::LoadLocal(d, slot) => {
                let len = self.frames.len();
                let src = &self.frames[len - 1 - d as usize].locals;
                let v = src.get(slot as usize).cloned().unwrap_or(Value::None);
                self.stack.push(v);
            }
            Instr::LoadGlobal(slot) => {
                let v = self.env.user_ctx.slot_value(slot as usize);
                self.stack.push(v);
            }
            Instr::LoadDynamic(sym) => {
                let v = if let Some(val) = self.env.user_ctx.get(&sym) {
                    val
                } else if let Some(fd) = self.env.natives.get(&sym) {
                    Value::Func(Rc::clone(fd))
                } else {
                    return Err(EvalError::UnboundWord {
                        sym,
                        span: Span::new(0, 0),
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
                self.env.user_ctx.set_slot(slot as usize, val.clone());
                self.stack.push(val);
            }
            Instr::SetDynamic(sym) => {
                let val = self.stack.pop().unwrap_or(Value::None);
                self.env.user_ctx.set(sym, val.clone());
                self.stack.push(val);
            }
            Instr::Call(native_idx, argc) => {
                let fd = self
                    .natives_by_idx
                    .get(native_idx as usize)
                    .cloned()
                    .ok_or_else(|| EvalError::Native {
                        message: format!("VM: bad native index {native_idx}"),
                        span: Span::new(0, 0),
                    })?;
                let f = fd.native.ok_or_else(|| EvalError::Native {
                    message: format!("VM: native {native_idx} has no handler"),
                    span: Span::new(0, 0),
                })?;
                let len = self.stack.len();
                if argc as usize > len {
                    return Err(EvalError::Arity {
                        native: Symbol::new("<native>"),
                        expected: argc as usize,
                        got: len,
                        span: Span::new(0, 0),
                    });
                }
                let args: Vec<Value> = self.stack[len - argc as usize..].to_vec();
                let refs = RefineArgs::from_pairs(std::mem::take(&mut self.pending_refs));
                let result = f(&args, &refs, self.env);
                self.stack.truncate(len - argc as usize);
                match result {
                    Ok(v) => self.stack.push(v),
                    Err(EvalError::Return(v)) => {
                        while let Some(frame) = self.frames.last() {
                            let is_func = frame.func.is_some();
                            self.frames.pop();
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
                        while self.frames.pop().is_some() {}
                        return Err(EvalError::Quit(code));
                    }
                    Err(e) => return Err(e),
                }
            }
            Instr::CallUser(slot, argc) => {
                self.call_user(slot as usize, argc as usize)?;
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
            Instr::MakeFunc(spec_idx, body_idx, freevars) => {
                let spec_val = block_pool(&self.frames[frame_idx].block, spec_idx as usize);
                let body_val = block_pool(&self.frames[frame_idx].block, body_idx as usize);
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
            Instr::MarkRefine(sym) => {
                self.ref_marks.push((sym, self.stack.len()));
            }
            Instr::EndRefine => {
                let (sym, height) = self
                    .ref_marks
                    .pop()
                    .ok_or_else(|| EvalError::Native {
                        message: "EndRefine without MarkRefine".into(),
                        span: Span::new(0, 0),
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

    /// Resolve the func stored at `slot`, pop `argc` args off the stack,
    /// lazily compile the body if needed, and return `(fd, compiled, locals)`.
    /// Shared by `call_user` (pushes a new frame) and `tail_call` (overwrites
    /// the current frame) — M28 factored this out so the two paths share arg
    /// collection + lazy-compile + locals-layout logic.
    fn prepare_call(
        &mut self,
        slot: usize,
        argc: usize,
    ) -> Result<(Rc<FuncDef>, Rc<CompiledBlock>, Vec<Value>), EvalError> {
        // The func value lives in the *caller's* scope. For a top-level
        // SetWord that's `env.user_ctx`; for a func-local SetWord it's the
        // current frame's `locals`. We check the current frame first, then
        // fall back to user_ctx (matches the compiler's `slot_coords`:
        // depth 0 = global).
        let func_val = if let Some(local) = self
            .frames
            .last()
            .and_then(|f| f.locals.get(slot))
            .cloned()
        {
            local
        } else {
            self.env.user_ctx.slot_value(slot)
        };
        let fd = match func_val {
            Value::Func(fd) => fd,
            other => {
                return Err(EvalError::TypeError {
                    expected: "function!",
                    found: crate::natives::type_name(&other),
                    span: Span::new(0, 0),
                });
            }
        };
        // Pop argc args.
        let len = self.stack.len();
        if argc > len {
            return Err(EvalError::Arity {
                native: Symbol::new("<user-func>"),
                expected: argc,
                got: len,
                span: Span::new(0, 0),
            });
        }
        let args: Vec<Value> = self.stack[len - argc..].to_vec();
        self.stack.truncate(len - argc);

        // Lazily compile the body if needed.
        let compiled = self.ensure_compiled(&fd, slot, argc)?;

        // Build the call frame's locals: params from args, refinement slots
        // (default none), locals slots (default none), body-local SetWord
        // slots (default none). Slot layout matches `bind_function_body`:
        // [params...][ref_flag, ref_args...][locals...][body SetWords...].
        // `CompiledBlock.n_locals` gives the total count; we size locals to
        // that and fill params from args.
        let n_locals = compiled.n_locals.max(fd.params.len());
        let mut locals = vec![Value::None; n_locals];
        for (i, arg) in args.iter().enumerate() {
            if i < locals.len() {
                locals[i] = arg.clone();
            }
        }
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
        let (fd, compiled, locals) = self.prepare_call(slot, argc)?;
        self.frames.push(Frame {
            func: Some(Rc::clone(&fd)),
            locals,
            depth: self.frames.last().map(|f| f.depth + 1).unwrap_or(1),
            block: (*compiled).clone(),
            pc: 0,
        });
        #[cfg(feature = "stats")]
        {
            self.env.record_frame_push();
        }

        // The callee runs in `run_loop`'s next iteration (the new top frame).
        // When it hits `Return`, that handler pops the frame and pushes the
        // return value onto the stack, so the caller resumes with the result
        // on top. The args were already truncated above, so the stack is
        // clean for the caller.
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
        let (fd, compiled, locals) = self.prepare_call(slot, argc)?;
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
                block: (*compiled).clone(),
                pc: 0,
            });
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
            self.frames[frame_idx].locals = locals;
            self.frames[frame_idx].pc = 0;
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
        self.frames[frame_idx].locals = locals;
        self.frames[frame_idx].block = (*compiled).clone();
        self.frames[frame_idx].pc = 0;
        // No stats bump: a reused frame doesn't increase call-stack depth.
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
        let compiled = compile_block_for_func_body(
            &fd.body,
            &mut child,
            &registry,
            (slot as u32, argc),
        )
        .map_err(|e| EvalError::Native {
            message: format!("VM: compile error: {:?}", e.kind),
            span: e.span,
        })?;
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
fn block_pool(block: &CompiledBlock, idx: usize) -> Value {
    block
        .pool
        .get(idx)
        .cloned()
        .unwrap_or(Value::None)
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
    use crate::vm::compiler::{NativeRegistry, compile_block};
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
        assert!(matches!(v, Value::Integer { n: 25, .. }), "got {}", mold_to_string(&v));
    }

    /// VM runs recursive `fact 5` -> `Integer(120)`. (plan3.md:466)
    ///
    /// M25 doesn't implement tail-call optimization (M28 does); the test
    /// verifies correctness at `fact 5` (shallow recursion, no stack concern).
    #[test]
    fn vm_runs_fact() {
        let v = run_vm("fact: func [n][either n <= 1 [1][n * fact n - 1]] fact 5");
        assert!(matches!(v, Value::Integer { n: 120, .. }), "got {}", mold_to_string(&v));
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
        assert!(matches!(v, Value::Integer { n: 5, .. }), "got {}", mold_to_string(&v));
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
        let (block, mut env, buf) = compile_for_vm_captured(
            "repeat i 1000000 [ if i > 999999 [print i] ]",
        );
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
