//! v0.4 Cranelift JIT compiler for hot function bodies.
//!
//! When a user function's call count crosses `JIT_THRESHOLD`, the VM calls
//! [`JitCompiler::compile_func`] to translate the func's `CompiledBlock` to
//! native machine code via Cranelift.
//!
//! **Integer-only specialization**: all args must be `Value::Integer`. The
//! return value is an `i64`. Self-recursion compiles to a direct native
//! `call` — no VM frame overhead.

use std::rc::Rc;

use cranelift_codegen::ir::{types, AbiParam, Block, InstBuilder, MemFlagsData, Signature, Value as CLValue};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

use red_core::value::{FuncDef, Value};
use red_core::vm_ir::{CompiledBlock, Instr};
use red_core::Env;

pub const JIT_THRESHOLD: u32 = 10;
pub type JitFn = unsafe extern "C" fn(*mut Env, i64) -> i64;

/// Try to compile a function body to native code. Returns `Ok(JitFn)` on
/// success, or `Err(msg)` if the body contains unsupported instructions.
/// Uses a thread-local `JitCompiler` instance (the Cranelift `JITModule`
/// owns executable memory and must be kept alive for the fn pointers to
/// remain valid).
pub fn try_compile(
    fd: &Rc<FuncDef>,
    compiled: &Rc<CompiledBlock>,
    self_slot: usize,
    env: &Env,
) -> Result<JitFn, String> {
    thread_local! {
        static JIT: std::cell::RefCell<Option<JitCompiler>> = std::cell::RefCell::new(None);
    }
    JIT.with(|jit| {
        let mut j = jit.borrow_mut();
        if j.is_none() {
            let natives = crate::vm::compiler::NativeRegistry::from_env(env);
            *j = Some(JitCompiler::new(&natives));
        }
        j.as_mut().unwrap().compile_func(fd, compiled, self_slot)
    })
}

// --- Trampolines ---

#[no_mangle]
pub extern "C" fn jit_slot_read(env: *mut Env, slot: u32) -> i64 {
    unsafe {
        match (*env).user_ctx.slot_value_unchecked(slot as usize) {
            Value::Integer { n, .. } => n,
            _ => 0,
        }
    }
}

#[no_mangle]
pub extern "C" fn jit_slot_write(env: *mut Env, slot: u32, val: i64) {
    unsafe {
        (*env).user_ctx.set_slot_unchecked(slot as usize, Value::integer(val));
    }
}

#[no_mangle]
pub extern "C" fn jit_native_call(env: *mut Env, native_idx: u32, argc: u32, args: *const i64) -> i64 {
    unsafe {
        let env = &mut *env;
        let args_slice = std::slice::from_raw_parts(args, argc as usize);
        let args_val: Vec<Value> = args_slice.iter().map(|&n| Value::integer(n)).collect();
        let natives_by_idx = env.natives_by_idx.as_ref().expect("natives_by_idx cached");
        let fd = &natives_by_idx[native_idx as usize];
        let f = fd.native.expect("native has handler");
        let refs = red_core::RefineArgs::empty();
        match f(&args_val, &refs, env) {
            Ok(Value::Integer { n, .. }) => n,
            _ => 0,
        }
    }
}

#[no_mangle]
pub extern "C" fn jit_vm_call_user(env: *mut Env, slot: u32, argc: u32, args: *const i64) -> i64 {
    unsafe {
        let env = &mut *env;
        let args_slice = std::slice::from_raw_parts(args, argc as usize);
        let result = crate::vm::vm::jit_reenter_interpreter(env, slot as usize, argc as usize, args_slice);
        match result {
            Ok(Value::Integer { n, .. }) => n,
            Ok(_) => 0,
            Err(_) => 0,
        }
    }
}

// --- JitCompiler ---

struct Trampolines {
    slot_read: cranelift_module::FuncId,
    slot_write: cranelift_module::FuncId,
    native_call: cranelift_module::FuncId,
    vm_call_user: cranelift_module::FuncId,
}

pub struct JitCompiler {
    module: JITModule,
    builder_ctx: FunctionBuilderContext,
    trampolines: Trampolines,
    call_conv: CallConv,
    /// Reverse native index → symbol name. Built from `NativeRegistry` at
    /// construction time. Used to inline arithmetic natives (+, -, *, <, etc.)
    /// instead of going through the trampoline.
    native_names: Vec<String>,
}

impl JitCompiler {
    pub fn new(natives: &crate::vm::compiler::NativeRegistry) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        // cranelift-jit requires is_pic=false (JIT'd code uses absolute addresses).
        flag_builder.set("is_pic", "false").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        let isa_builder = cranelift_native::builder().expect("host not supported");
        let isa = isa_builder.finish(settings::Flags::new(flag_builder)).unwrap();
        let libcall_names: Box<dyn Fn(cranelift_codegen::ir::LibCall) -> String + Send + Sync> =
            Box::new(|lc| format!("{lc:?}"));
        let builder = JITBuilder::with_isa(isa, libcall_names);
        let mut module = JITModule::new(builder);

        let ptr_type = module.target_config().pointer_type();
        let i64_t = types::I64;
        let i32_t = types::I32;
        let call_conv = module.isa().default_call_conv();

        let mk_sig = |params: &[cranelift_codegen::ir::Type], returns: &[cranelift_codegen::ir::Type]| {
            Signature {
                params: params.iter().map(|&t| AbiParam::new(t)).collect(),
                returns: returns.iter().map(|&t| AbiParam::new(t)).collect(),
                call_conv,
            }
        };

        let sig_read = mk_sig(&[ptr_type, i32_t], &[i64_t]);
        let sig_write = mk_sig(&[ptr_type, i32_t, i64_t], &[]);
        let sig_native = mk_sig(&[ptr_type, i32_t, i32_t, ptr_type], &[i64_t]);
        let sig_vm = mk_sig(&[ptr_type, i32_t, i32_t, ptr_type], &[i64_t]);

        let slot_read = module.declare_function("jit_slot_read", Linkage::Import, &sig_read).unwrap();
        let slot_write = module.declare_function("jit_slot_write", Linkage::Import, &sig_write).unwrap();
        let native_call = module.declare_function("jit_native_call", Linkage::Import, &sig_native).unwrap();
        let vm_call_user = module.declare_function("jit_vm_call_user", Linkage::Import, &sig_vm).unwrap();

        // Build reverse native-name index for inlining arithmetic ops.
        let native_names = natives.reverse_names();

        Self {
            module,
            builder_ctx: FunctionBuilderContext::new(),
            trampolines: Trampolines { slot_read, slot_write, native_call, vm_call_user },
            call_conv,
            native_names,
        }
    }

    pub fn compile_func(
        &mut self,
        fd: &Rc<FuncDef>,
        compiled: &Rc<CompiledBlock>,
        self_slot: usize,
    ) -> Result<JitFn, String> {
        if fd.params.len() != 1 {
            return Err(format!("JIT MVP: only 1-arg funcs, got {}", fd.params.len()));
        }

        let ptr_type = self.module.target_config().pointer_type();
        let i64_t = types::I64;
        let sig = Signature {
            params: vec![AbiParam::new(ptr_type), AbiParam::new(i64_t)],
            returns: vec![AbiParam::new(i64_t)],
            call_conv: self.call_conv,
        };
        let func_id = self.module
            .declare_function("jit_func", Linkage::Local, &sig)
            .map_err(|e| format!("declare: {e}"))?;

        let mut ctx = self.module.make_context();
        ctx.func = cranelift_codegen::ir::Function::with_name_signature(
            cranelift_codegen::ir::UserFuncName::user(0, func_id.as_u32()),
            sig,
        );

        {
            let mut builder = FunctionBuilder::new(&mut ctx.func, &mut self.builder_ctx);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);

            let env_param = builder.block_params(entry)[0];
            let arg0 = builder.block_params(entry)[1];

            // Pre-declare all trampoline FuncRefs (avoids borrowing `self`
            // inside the builder loop — `declare_func_in_func` borrows the
            // module, which is on `self`).
            let fr_slot_read = self.module.declare_func_in_func(self.trampolines.slot_read, &mut builder.func);
            let fr_slot_write = self.module.declare_func_in_func(self.trampolines.slot_write, &mut builder.func);
            let fr_native_call = self.module.declare_func_in_func(self.trampolines.native_call, &mut builder.func);
            let fr_vm_call_user = self.module.declare_func_in_func(self.trampolines.vm_call_user, &mut builder.func);
            let fr_self = self.module.declare_func_in_func(func_id, &mut builder.func);

            // Declare Cranelift variables for local slots.
            let n_locals = compiled.n_locals.max(fd.params.len());
            let mut var_map: std::collections::HashMap<u32, Variable> = std::collections::HashMap::new();
            for slot in 0..n_locals {
                let var = builder.declare_var(i64_t);
                let init = if slot == 0 { arg0 } else { builder.ins().iconst(i64_t, 0) };
                builder.def_var(var, init);
                var_map.insert(slot as u32, var);
            }

            let instrs = compiled.instrs.as_ref();
            let n = instrs.len();

            // Scan for jump targets — each gets its own Cranelift block.
            let mut jump_targets: std::collections::HashSet<usize> = std::collections::HashSet::new();
            jump_targets.insert(0); // entry
            for (pc, instr) in instrs.iter().enumerate() {
                match instr {
                    Instr::Jump(t) => { jump_targets.insert(*t as usize); }
                    Instr::JumpIfFalse(t) => {
                        jump_targets.insert(*t as usize);
                        jump_targets.insert(pc + 1); // fall-through after brif
                    }
                    _ => {}
                }
            }

            // Create all blocks upfront.
            let mut block_map: std::collections::HashMap<usize, Block> = std::collections::HashMap::new();
            for &target in &jump_targets {
                let blk = builder.create_block();
                block_map.insert(target, blk);
            }
            // Start in the entry block (pc=0).
            builder.switch_to_block(block_map[&0]);
            builder.seal_block(block_map[&0]);

            let mut stk: Vec<CLValue> = Vec::new();
            let mut prev_terminated = false;

            for (pc, instr) in instrs.iter().enumerate() {
                // If the previous instruction terminated the block (Jump/
                // brif/Return/Halt), this pc must be a jump target to be
                // reachable. If it IS a jump target, switch to its block.
                // If it's NOT a jump target, it's dead code — skip it.
                if prev_terminated {
                    prev_terminated = false;
                    if !jump_targets.contains(&pc) {
                        continue; // dead code — skip
                    }
                    let blk = block_map[&pc];
                    builder.switch_to_block(blk);
                    builder.seal_block(blk);
                } else if pc > 0 && jump_targets.contains(&pc) {
                    // We fell through from the previous instruction into a
                    // jump target. Need to create a new block and jump to it.
                    let blk = block_map[&pc];
                    builder.ins().jump(blk, &[]);
                    builder.switch_to_block(blk);
                    builder.seal_block(blk);
                }

                match instr {
                    Instr::ConstInt(v) => { stk.push(builder.ins().iconst(i64_t, *v)); }
                    Instr::ConstNone => { stk.push(builder.ins().iconst(i64_t, i64::MIN)); }
                    Instr::ConstBool(b) => { stk.push(builder.ins().iconst(i64_t, if *b {1} else {0})); }
                    Instr::LoadLocal(_, slot) => {
                        let var = var_map[slot];
                        stk.push(builder.use_var(var));
                    }
                    Instr::SetLocal(_, slot) => {
                        let val = stk.pop().unwrap();
                        let var = var_map[slot];
                        builder.def_var(var, val);
                    }
                    Instr::LoadGlobal(slot) => {
                        let sv = builder.ins().iconst(types::I32, *slot as i64);
                        let fr = fr_slot_read;
                        let c = builder.ins().call(fr, &[env_param, sv]);
                        stk.push(builder.inst_results(c)[0]);
                    }
                    Instr::SetGlobal(slot) => {
                        let val = stk.pop().unwrap();
                        let sv = builder.ins().iconst(types::I32, *slot as i64);
                        let fr = fr_slot_write;
                        builder.ins().call(fr, &[env_param, sv, val]);
                    }
                    Instr::Call(ni, ac) => {
                        // Inline arithmetic natives by name.
                        let name = self.native_names.get(*ni as usize).map(|s| s.as_str());
                        match (name, *ac) {
                            (Some("+"), 2) => {
                                let b = stk.pop().unwrap();
                                let a = stk.pop().unwrap();
                                stk.push(builder.ins().iadd(a, b));
                            }
                            (Some("-"), 2) => {
                                let b = stk.pop().unwrap();
                                let a = stk.pop().unwrap();
                                stk.push(builder.ins().isub(a, b));
                            }
                            (Some("*"), 2) => {
                                let b = stk.pop().unwrap();
                                let a = stk.pop().unwrap();
                                stk.push(builder.ins().imul(a, b));
                            }
                            (Some("/"), 2) => {
                                let b = stk.pop().unwrap();
                                let a = stk.pop().unwrap();
                                stk.push(builder.ins().sdiv(a, b));
                            }
                            (Some("//"), 2) => {
                                let b = stk.pop().unwrap();
                                let a = stk.pop().unwrap();
                                stk.push(builder.ins().srem(a, b));
                            }
                            (Some("<"), 2) => {
                                let b = stk.pop().unwrap();
                                let a = stk.pop().unwrap();
                                let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedLessThan, a, b);
                                stk.push(builder.ins().uextend(i64_t, cmp));
                            }
                            (Some("<="), 2) => {
                                let b = stk.pop().unwrap();
                                let a = stk.pop().unwrap();
                                let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedLessThanOrEqual, a, b);
                                stk.push(builder.ins().uextend(i64_t, cmp));
                            }
                            (Some(">"), 2) => {
                                let b = stk.pop().unwrap();
                                let a = stk.pop().unwrap();
                                let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedGreaterThan, a, b);
                                stk.push(builder.ins().uextend(i64_t, cmp));
                            }
                            (Some(">="), 2) => {
                                let b = stk.pop().unwrap();
                                let a = stk.pop().unwrap();
                                let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::SignedGreaterThanOrEqual, a, b);
                                stk.push(builder.ins().uextend(i64_t, cmp));
                            }
                            (Some("="), 2) => {
                                let b = stk.pop().unwrap();
                                let a = stk.pop().unwrap();
                                let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::Equal, a, b);
                                stk.push(builder.ins().uextend(i64_t, cmp));
                            }
                            (Some("<>"), 2) => {
                                let b = stk.pop().unwrap();
                                let a = stk.pop().unwrap();
                                let cmp = builder.ins().icmp(cranelift_codegen::ir::condcodes::IntCC::NotEqual, a, b);
                                stk.push(builder.ins().uextend(i64_t, cmp));
                            }
                            _ => {
                                // Fall back to trampoline for non-arithmetic natives.
                                let iv = builder.ins().iconst(types::I32, *ni as i64);
                                let av = builder.ins().iconst(types::I32, *ac as i64);
                                let ap = alloc_args(&mut builder, &mut stk, *ac as usize, ptr_type);
                                let c = builder.ins().call(fr_native_call, &[env_param, iv, av, ap]);
                                stk.push(builder.inst_results(c)[0]);
                            }
                        }
                    }
                    Instr::CallUserGlobal(slot, ac) => {
                        if *slot as usize == self_slot {
                            // Self-recursion: direct native call.
                            let arg = stk.pop().unwrap();
                            let callee = fr_self;
                            let c = builder.ins().call(callee, &[env_param, arg]);
                            stk.push(builder.inst_results(c)[0]);
                        } else {
                            let sv = builder.ins().iconst(types::I32, *slot as i64);
                            let av = builder.ins().iconst(types::I32, *ac as i64);
                            let ap = alloc_args(&mut builder, &mut stk, *ac as usize, ptr_type);
                            let fr = fr_vm_call_user;
                            let c = builder.ins().call(fr, &[env_param, sv, av, ap]);
                            stk.push(builder.inst_results(c)[0]);
                        }
                    }
                    Instr::Jump(t) => {
                        let target = *t as usize;
                        let blk = block_map.get(&target).copied().unwrap_or_else(|| {
                            let b = builder.create_block();
                            block_map.insert(target, b);
                            b
                        });
                        builder.ins().jump(blk, &[]);
                        prev_terminated = true;
                    }
                    Instr::JumpIfFalse(t) => {
                        let target = *t as usize;
                        let cond = stk.pop().unwrap();
                        let is_zero = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, cond, 0);
                        let is_none = builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, cond, i64::MIN);
                        let falsy = builder.ins().bor(is_zero, is_none);
                        let true_blk = block_map.get(&target).copied().unwrap_or_else(|| {
                            let b = builder.create_block();
                            block_map.insert(target, b);
                            b
                        });
                        let fallthrough = pc + 1;
                        let false_blk = block_map.get(&fallthrough).copied().unwrap_or_else(|| {
                            let b = builder.create_block();
                            block_map.insert(fallthrough, b);
                            b
                        });
                        builder.ins().brif(falsy, true_blk, &[], false_blk, &[]);
                        prev_terminated = true;
                    }
                    Instr::Pop => { stk.pop(); }
                    Instr::Return => {
                        let r = stk.pop().unwrap_or_else(|| builder.ins().iconst(i64_t, 0));
                        builder.ins().return_(&[r]);
                        prev_terminated = true;
                    }
                    Instr::Halt => {
                        let z = builder.ins().iconst(i64_t, 0);
                        builder.ins().return_(&[z]);
                        prev_terminated = true;
                    }
                    _ => return Err(format!("JIT: unsupported instr {:?} at pc {pc}", instr)),
                }
            }

            // If the last instruction wasn't Return, add one.
            if n == 0 || !matches!(instrs.last(), Some(Instr::Return)) {
                let r = stk.pop().unwrap_or_else(|| builder.ins().iconst(i64_t, 0));
                builder.ins().return_(&[r]);
            }

            builder.seal_all_blocks();
            builder.finalize();
        }

        self.module.define_function(func_id, &mut ctx).map_err(|e| format!("define: {e}"))?;
        self.module.clear_context(&mut ctx);
        self.module.finalize_definitions().map_err(|e| format!("finalize: {e}"))?;
        let ptr = self.module.get_finalized_function(func_id);
        Ok(unsafe { std::mem::transmute::<*const u8, JitFn>(ptr) })
    }
}

fn alloc_args(builder: &mut FunctionBuilder, stk: &mut Vec<CLValue>, argc: usize, ptr_type: cranelift_codegen::ir::Type) -> CLValue {
    let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (argc * 8) as u32,
        8,
    ));
    let args: Vec<CLValue> = (0..argc).map(|_| stk.pop().unwrap()).collect();
    for (i, arg) in args.into_iter().enumerate() {
        let addr = builder.ins().stack_addr(ptr_type, slot, 0);
        builder.ins().store(MemFlagsData::new(), arg, addr, (i * 8) as i32);
    }
    builder.ins().stack_addr(ptr_type, slot, 0)
}
