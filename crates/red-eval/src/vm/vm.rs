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
                    // M25 stub: behave like CallUser (no frame reuse optimization
                    // yet — M28 implements true tail-call frame overwrite).
                    self.call_user(slot as usize, argc as usize)?;
                }
                Instr::TailReenter(slot, argc) => {
                    // M25 stub: same as TailCall.
                    self.call_user(slot as usize, argc as usize)?;
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

    /// Invoke a user function stored in a slot. Reads `Value::Func(fd)` from
    /// the slot (global or local depending on the current frame's depth),
    /// pops `argc` args, lazily compiles the body if needed, pushes a new
    /// `Frame`, and returns. The callee runs in `run_loop`'s next iteration;
    /// `EvalError::Return(v)` from the `return` native is caught by the
    /// `Call` handler (not here), which pops the function frame and pushes
    /// `v` onto the caller's stack.
    fn call_user(&mut self, slot: usize, argc: usize) -> Result<(), EvalError> {
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

        // Push the call frame and recurse into the dispatch loop.
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
        // Fast path: already compiled (cached by a prior call or by MakeFunc).
        if let Some(c) = fd.compiled.clone() {
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
        // M25 does not cache the compiled block on the (shared) `Rc<FuncDef>`
        // — `slot_value` clones the `Rc`, bumping the refcount, so
        // `Rc::get_mut` would fail. The body recompiles on each call (correct,
        // just slower). M27 adds proper cache management with invalidation
        // when `bind` mutates the body. For M25's test cases (shallow
        // recursion like `fact 5`) the recompile cost is negligible.
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
    use red_core::{Context, Env};

    /// Parse + bind + register natives, then return the compiled block + env
    /// ready for `run`. Mirrors the walker's `run_series_inner_opts` setup.
    fn compile_for_vm(src: &str) -> (CompiledBlock, Env) {
        let body = load_source(src).expect("parse failed");
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let mut env = Env::new(Rc::clone(&ctx_rc));
        register_natives(&mut env);
        let registry = NativeRegistry::from_env(&env);
        let mut scope = Scope::root(&ctx_rc);
        let block = compile_block(&body, &mut scope, &registry).expect("compile failed");
        (block, env)
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
}
