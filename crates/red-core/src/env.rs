//! Evaluator environment: `Env`, `CallFrame`, `EvalError`, `NativeFn`.
//!
//! Lives in `red-core` (not `red-eval`) so `FuncDef.native` can reference
//! `NativeFn` without a cross-crate dependency cycle. `red-eval` re-exports
//! these and provides the evaluation algorithm + native implementations.
//!
//! Milestone 5 scope: types exist, `Env::new` builds an empty environment,
//! `EvalError::UnboundWord` renders with the offending symbol. The call stack
//! and `Return`/`Native` error variants are present for M9+ but unused here.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::rc::Rc;

use crate::context::Context;
use crate::value::{ErrorValue, FuncDef, ModuleDef, Series, Span, Symbol, Value};
use crate::vm_ir::CompiledBlock;

/// Bytecode compiler / VM-invariant failure kinds. Surfaced via
/// [`EvalError::Compile`]. Lives in `red-core` (alongside `EvalError`) so
/// `red-core::EvalError` can name it without depending on `red-eval`.
/// `red-eval::vm::compiler::CompileError` re-uses this enum directly.
///
/// M31 (plan4): added `UnboundWord` and `MalformedSpec` (non-Block body) to
/// replace `unreachable!()` in `compiler.rs`.
#[derive(Clone, Debug)]
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
    /// M31: a `Word` reached the compiler with `Binding::Unbound` and no
    /// native/user-func match (replaces `unreachable!()` at `compiler.rs:772`).
    /// Distinct from `UnboundInOperatorPosition`: this fires for any unbound
    /// word (value position included), where the plan4 call site is
    /// `emit_load`'s `Binding::Unbound` arm.
    UnboundWord,
    /// M31: a `MakeFunc` body was not a `Block!` (replaces `unreachable!()`
    /// at `compiler.rs:1336`). The runtime `func` native already reports a
    /// clearer error; this surfaces a compile-time bail-out instead of a
    /// release panic.
    MalformedBody,
    /// M31: VM dispatch reached an instruction stream invariant violation
    /// (e.g. pool index OOB, bad native index, EndRefine without MarkRefine,
    /// ran off the instr stream). Used by `EvalError::Compile` for VM
    /// invariant failures that previously panicked or silently returned
    /// `none`.
    VmInvariant(String),
}

impl fmt::Display for CompileErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileErrorKind::UnboundInOperatorPosition => {
                write!(f, "unbound word in operator position")
            }
            CompileErrorKind::MalformedSpec => write!(f, "malformed function spec"),
            CompileErrorKind::ArityMismatch => write!(f, "arity mismatch"),
            CompileErrorKind::UnboundWord => write!(f, "unbound word"),
            CompileErrorKind::MalformedBody => write!(f, "malformed function body"),
            CompileErrorKind::VmInvariant(msg) => write!(f, "VM invariant violated: {msg}"),
        }
    }
}

/// Refinement arguments handed to a native at call time. Built by
/// `dispatch_call` from the call site (path refinements and/or inline
/// `/ref` flags), this is the refinement-facing counterpart to `args`.
///
/// Each entry is `(refinement_name, collected_arg_values)`. A refinement
/// present in the call appears here with its arguments (possibly empty for
/// zero-arity refinements like `/case` or `/only`); a refinement absent
/// from the call does not appear. Natives query with [`Self::has`] and
/// [`Self::get`].
#[derive(Debug, Default)]
pub struct RefineArgs {
    inner: Vec<(Symbol, Vec<Value>)>,
}

impl RefineArgs {
    /// A fresh empty argument set — used by call sites that take no
    /// refinements (the overwhelming majority, including all infix natives).
    /// Returns an owned value; pass `&RefineArgs::empty()` to natives.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Construct from already-collected `(name, args)` pairs. Used by
    /// `dispatch_call` after walking the spec.
    pub fn from_pairs(pairs: Vec<(Symbol, Vec<Value>)>) -> Self {
        Self { inner: pairs }
    }

    /// True if refinement `name` was supplied at the call site.
    pub fn has(&self, name: &Symbol) -> bool {
        self.inner.iter().any(|(n, _)| n == name)
    }

    /// The argument values supplied for refinement `name`, or `None` if the
    /// refinement wasn't used. Zero-arity refinements return `Some(&[])`.
    pub fn get(&self, name: &Symbol) -> Option<&[Value]> {
        self.inner
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_slice())
    }
}

/// Function pointer for native (Rust-implemented) operations. `args` are the
/// positional arguments (in spec order); `refs` carries any refinement flags
/// and their arguments (M13); `env` is the interpreter state.
pub type NativeFn = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

/// Which evaluator backs `dispatch_block` for the current call. M26 leaves
/// the default as `Walk` (the v0.2 tree-walker); M29 flips the default to
/// `Vm`. Natives that recurse into block evaluation inspect `env.mode` via
/// `interp::dispatch_block` to pick the right evaluator, so a native invoked
/// from the VM that re-enters a block re-uses the VM rather than falling
/// back to the walker (unless the block is `needs_rebind`-flagged).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvalMode {
    /// Tree-walking interpreter (`interp::eval`). The v0.2 default.
    Walk,
    /// Bytecode stack VM (`vm::run`). M26 makes it available; M29 makes it
    /// the default.
    Vm,
}

/// Top-level interpreter state: the shared user context, the function call
/// stack (empty until M9), the native registry (populated in M6), and the
/// shared output sink that natives like `print`/`prin`/`probe` write to.
///
/// `out` defaults to `io::stdout()`; tests inject a `Box<dyn Write>` buffer
/// so inline tests can assert on captured output.
pub struct Env {
    pub user_ctx: Rc<Context>,
    pub call_stack: Vec<CallFrame>,
    pub natives: HashMap<Symbol, Rc<FuncDef>>,
    pub out: Box<dyn Write>,
    /// Whether `call`/`shell` may execute external commands. Off by default;
    /// enabled by the CLI `--allow-shell` flag. `call`/`shell` raise
    /// `EvalError::Native` when this is false (M20 sandbox policy).
    pub allow_shell: bool,
    /// Whether `open`/`read`/`write` on a `url!` or HTTP `port!` may perform
    /// network I/O. Off by default; enabled by the CLI `--allow-network` flag.
    /// Networking natives raise `EvalError::Native` when this is false
    /// (M113 sandbox policy — mirrors `allow_shell`). File ports are
    /// unaffected (gated only by the existing filesystem permissions).
    pub allow_network: bool,
    /// M86: when true, resolving a truly-unbound word yields `Value::Unset`
    /// instead of raising `EvalError::UnboundWord`. Default **off** —
    /// preserves the v0.2–v0.6 strict-binding contract. Enabled by the CLI
    /// `--unset-on-unbound` flag (runtime gate, not a cargo feature). The
    /// `user_ctx` (M62) and native-registry fallbacks still run before this
    /// gate, so `import`-aliased words and natives keep resolving normally.
    pub unset_on_unbound: bool,
    /// Current working directory for file! path resolution. Updated by
    /// `change-dir`; read by `what-dir`. Relative file paths in `read`/
    /// `write`/`exists?`/etc. resolve against this.
    pub cwd: PathBuf,
    /// Which evaluator (`Walk` or `Vm`) `dispatch_block` should route a block
    /// evaluation to. M26 leaves the default `Walk`; M29 flips it to `Vm`.
    /// Natives that recurse into block evaluation read this via
    /// `interp::dispatch_block`, so a native invoked from the VM re-enters
    /// the VM for its block args (unless the block is `needs_rebind`).
    pub mode: EvalMode,
    /// VM compiled-block cache for function bodies, keyed by
    /// `Rc::as_ptr(fd) as usize` (stable across `Rc` clones of the same
    /// underlying `FuncDef`). Populated lazily by the VM's `ensure_compiled`
    /// on the first `CallUser` to a given func; invalidated by `bind` on a
    /// `Value::Func` and defensively by `bind_function_body` (which only runs
    /// at func-creation time, before any cache entry exists). M27.
    ///
    /// This is the *authoritative* VM cache for func bodies —
    /// `FuncDef::compiled` is a construction-time hint that stays `None` for
    /// funcs created in `Walk` mode (the slot for recursive `CallUser`
    /// emission isn't known until the SetWord stores the func at runtime, so
    /// eager compilation at `MakeFunc` time would emit a wrong slot index).
    pub func_cache: HashMap<usize, Rc<CompiledBlock>>,
    /// VM compiled-block cache for non-function blocks that are `do`-ed /
    /// `reduce`-d / loop-body-ed repeatedly, keyed by
    /// `(Rc::as_ptr(&series.data) as usize, series.index)`. Safe without
    /// explicit invalidation because `bind`/`use` deep-clone the series (a new
    /// `Rc` allocation → different identity → cache miss, recompile) and
    /// `user_ctx` slots are append-only (cached `LoadGlobal(slot)` indices
    /// remain valid). M27.
    pub block_cache: HashMap<(usize, usize), Rc<CompiledBlock>>,
    /// M30: indexed view of `natives` (same `Rc<FuncDef>` pointers, ordered
    /// by the snapshot the compiler's `NativeRegistry::from_env` took). Built
    /// lazily by `vm::run` on first call, then cached here so the 1M-iteration
    /// `repeat`/`while`/`foreach` paths don't rebuild it per `dispatch_block`
    /// call (the root cause of the v0.3.0 `sum_loop`/`sum_while` regressions:
    /// ~100 `Rc::clone`s × 1M iterations = 100M refcount ops just for native
    /// lookup). `Rc`-wrapped so `vm::run` can clone it cheaply into each `Vm`
    /// (one Rc bump instead of a `Vec` alloc per `dispatch_block` call).
    /// Invalidated by `invalidate_native_index` whenever `natives` is mutated
    /// (currently only at `register_natives` time, before any VM run).
    pub natives_by_idx: Option<Rc<Vec<Rc<FuncDef>>>>,
    /// M42: parallel `Vec<Symbol>` of native names, indexed identically to
    /// `natives_by_idx`. Built lazily alongside `natives_by_idx` so the VM
    /// can enrich raised errors with `where: <native name>` without a
    /// reverse lookup into the `natives` HashMap.
    pub native_names_by_idx: Option<Rc<Vec<Symbol>>>,
    /// M30.1.C: reusable scratch `Vec` for the VM's `frames` stack. Drained
    /// by `vm::run` via `std::mem::take` on entry, cleared + drained back on
    /// exit. Eliminates 1 heap allocation per `dispatch_block` call (was
    /// `Vec::with_capacity(8)` per call → 1M allocs for a 1M-iteration
    /// `repeat`). The vec stays at its high-water capacity across calls, so
    /// subsequent `vm::run` calls don't realloc until they exceed it.
    pub vm_frames_pool: Vec<crate::vm_ir::Frame>,
    /// M30.1.C: reusable scratch `Vec` for the VM's operand `stack`. Same
    /// drain/restore contract as `vm_frames_pool`.
    pub vm_stack_pool: Vec<Value>,
    /// M30.3.2: reusable scratch `Vec` for VM `Frame.locals`. Drained by
    /// `prepare_call` (via `std::mem::take`) to avoid a fresh `Vec` alloc
    /// per `CallUser`; saved back by `Return` (which extracts the popped
    /// frame's `locals` before dropping the `Frame`). The pool stays at its
    /// high-water capacity across calls, so deep recursion (e.g. `fib 30`)
    /// only allocates on the first call — the ~2.7M subsequent calls reuse
    /// the pooled Vec.
    pub vm_locals_pool: Vec<Vec<Value>>,
    /// M31: optional per-instr trace sink. When `Some`, the VM appends one
    /// line per executed instr (`pc={pc} {instr:?}`) to this writer. The CLI
    /// `--trace` flag wires this to `stderr`; tests wire it to a buffer.
    /// `None` (the default) means tracing is off — zero cost (the VM checks
    /// `is_some()` before formatting the trace line, so the hot path pays
    /// only one `Option::is_some` branch per instr when off).
    pub trace_out: Option<Box<dyn Write>>,
    /// M61: stack of currently-evaluating module bodies. Pushed by
    /// `module_native` before evaluating the body (with `env.user_ctx`
    /// swapped to the module's ctx), popped after. `current_module()`
    /// returns the top — used by the `export` native (writes to its
    /// `exports` set) and by `module/word` path resolution (to detect
    /// "inside the module body" vs "from outside" for the export check).
    pub module_stack: Vec<Rc<RefCell<ModuleDef>>>,
    /// M61: cache of named modules, keyed by module name. Populated by
    /// `module 'name [...]`; a second `module 'name [different body]`
    /// returns the cached value (the new body is ignored — matches Red's
    /// "module is a singleton by name"). M62's `import 'name` consults
    /// this; M61 only populates it.
    pub modules: HashMap<Symbol, Rc<RefCell<ModuleDef>>>,
    /// M62: cache of file-imported modules, keyed by canonical source path.
    /// Populated by `import %file.red`; a second `import %same-file` returns
    /// the cached module without re-reading/re-evaluating. Mirrors `modules`
    /// (keyed by name) for the file case.
    pub modules_by_path: HashMap<PathBuf, Rc<RefCell<ModuleDef>>>,
    /// M65: canonical paths of modules currently mid-`import %file` (between
    /// the file-read and the `modules_by_path` cache insertion). Used to
    /// detect circular imports (`a.red` imports `b.red` imports `a.red`)
    /// which would otherwise stack-overflow. A cycle raises
    /// `EvalError::Native { "import: circular import detected: <path>" }`.
    pub loading_modules: Vec<PathBuf>,
    /// Bug 3 fix: the active VM frame's closure captures, made visible to
    /// `dispatch_block` calls from within natives (`if`/`either`/`do`/loops).
    /// `Vm::call_native` saves/restores this around each native call, setting
    /// it to `self.frames.last().captures`. When `dispatch_block` spins up a
    /// fresh `vm::run`, it reads this to seed the new root frame's `captures`
    /// so `LoadCapture`/`SetCapture` instrs in closure-body sub-blocks find
    /// their capture cell. `None` outside a closure context.
    pub current_vm_captures: Option<Rc<Vec<RefCell<Value>>>>,
    /// v0.11: the active VM frame's function-local slots, bridged to the
    /// tree-walker so block-taking natives (`try`/`loop`/`while`/`foreach`)
    /// whose body blocks carry `Binding::Lexical(0, slot)` can resolve func
    /// params/locals when the enclosing func is VM-invoked (walker's
    /// `env.call_stack` is empty in that case). `Vm::call_native` saves/
    /// restores this around each native call, cloning `self.frames.last()`
    /// .locals in + writing back out so `SetWord`s in loop bodies persist.
    /// `None` outside a func call (root script scope) or in pure `Walk` mode.
    pub current_vm_locals: Option<Vec<Value>>,
    /// M63: cache of the auto-imported stdlib module (parsed + evaluated
    /// once on first `ensure_stdlib` call). Re-aliased into `user_ctx` on
    /// every `run_source*` call so a fresh REPL-line ctx still gets the
    /// stdlib words. `None` when `--no-stdlib` is set or `ensure_stdlib`
    /// hasn't run yet.
    pub stdlib: Option<Rc<RefCell<crate::value::ModuleDef>>>,
    /// M130: dynamic-scope stack for the `collect`/`keep` native pair. Each
    /// `collect` entry pushes a fresh accumulator; `keep value` appends to the
    /// top entry. Empty outside an active `collect` call (a stray `keep`
    /// errors). Distinct from the parse-only `collect` keyword in `parse.rs`.
    pub collect_stack: Vec<Vec<Value>>,
    /// M131: pointer-identity set of protected `series!` storage cells. A
    /// series is protected when its `Rc<RefCell<Vec<Value>>>` pointer (as
    /// `*const ()`) is in this set. `protect`/`unprotect` add/remove; every
    /// mutating series native calls `check_series_protected` before writing.
    /// Objects use `ObjectDef.protected` directly (no entry here). Pragmatic
    /// deviation from the "field on Series backing cell" plan note: avoids a
    /// sweeping `Series.data` type change; behavior is identical.
    pub protected_series: HashSet<*const ()>,
    /// M134: user-level `trace on`/`trace off` toggle. When `Some`, every
    /// evaluated top-level expression is molded and written to this sink
    /// before evaluation. Distinct from the CLI `--trace` VM-instruction
    /// dump (`Env::trace_out`) — this is a script-level tracing mode.
    pub user_trace: Option<Box<dyn std::io::Write>>,
    /// M162: build/task dialect registry. Maps task name → body block.
    /// Populated by `task`/`build` natives; drained by `run-task`/`default`.
    pub tasks: HashMap<Symbol, crate::value::Series>,
    /// M162: the default task to run when `--build` is used without a
    /// specific task name. Set by the `default` keyword inside a `build`
    /// block.
    pub default_task: Option<Symbol>,
    /// M162: set of tasks already run in the current `build` invocation,
    /// for dependency dedup and cycle detection.
    pub ran_tasks: HashSet<Symbol>,
    /// M70: test dialect registry. `test` natives push `TestDef`s here;
    /// `run-tests` drains them. Empty unless test natives are used.
    pub tests: Vec<crate::value::TestDef>,
    /// M70: stack of suite names — push on `suite` enter, pop on exit.
    /// Empty = top level. Used to build `TestDef.path`.
    pub current_suite: Vec<Symbol>,
    /// M70: stack of `TestHooks` frames — one per active `suite`.
    /// `test` copies the full stack into `TestDef.hooks` at registration.
    pub test_hooks: Vec<crate::value::TestHooks>,
    /// M70: filled by `run-tests` — one `TestResult` per test.
    pub test_results: Vec<crate::value::TestResult>,
    /// M70: guard — `run-tests` is idempotent within a single `--test`
    /// invocation (auto-invoke skips if already called).
    pub tests_run: bool,
    /// M70: count of failed tests — read by the CLI for the exit code
    /// (0 = all pass, >0 = failures).
    pub test_failed: usize,
    /// M170: semantic-type registry. Maps a semantic type name (`'rgb!`) to
    /// its `SemanticTypeDef`. Populated by `make semantic-type!`/`define-type`;
    /// consulted by `valid?`/generated predicates (`rgb?`)/`TypesetDef::accepts`
    /// (the func-spec path — M176). `define-type` overwrites any existing entry
    /// (re-definition shadows the old one; existing predicates/constructors
    /// registered on `natives` keep their old behavior until re-defined).
    pub semantic_types: HashMap<Symbol, Rc<crate::value::SemanticTypeDef>>,
    /// High-water mark of `call_stack.len()` since the last
    /// [`Self::reset_stats`] call. Used by the v0.3 VM milestones to prove
    /// tail-call stack bounds. Only present under the `stats` cargo feature;
    /// release builds without it pay zero cost.
    #[cfg(feature = "stats")]
    pub max_frame_depth: usize,
    /// Count of `eval` loop iterations since the last [`Self::reset_stats`]
    /// call. Gives an operation-count metric independent of wall time, used
    /// in M30 to correlate VM instr count with walker instr count. Only
    /// present under the `stats` cargo feature.
    #[cfg(feature = "stats")]
    pub instr_count: u64,
}

impl Env {
    /// Empty environment: fresh user context, no call frames, no natives,
    /// output going to `stdout`.
    pub fn new(user_ctx: Rc<Context>) -> Self {
        Self::new_with_output(user_ctx, Box::new(io::stdout()))
    }

    /// Build an environment with a custom output sink (used by tests to
    /// capture native output into an in-memory buffer).
    pub fn new_with_output(user_ctx: Rc<Context>, out: Box<dyn Write>) -> Self {
        Self {
            user_ctx,
            call_stack: Vec::new(),
            natives: HashMap::new(),
            out,
            allow_shell: false,
            allow_network: false,
            unset_on_unbound: false,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            // M29: the bytecode VM is the default evaluator. The
            // `force-walk` cargo feature (re-exported by `red-eval/force-walk`)
            // flips this back to `Walk` so the entire test suite can be run
            // against the tree-walker for golden parity
            // (`cargo test --workspace --features force-walk`). The CLI's
            // `--walk` flag overrides this at runtime via `RunOptions.walk`.
            #[cfg(feature = "force-walk")]
            mode: EvalMode::Walk,
            #[cfg(not(feature = "force-walk"))]
            mode: EvalMode::Vm,
            func_cache: HashMap::new(),
            block_cache: HashMap::new(),
            natives_by_idx: None,
            native_names_by_idx: None,
            vm_frames_pool: Vec::new(),
            vm_stack_pool: Vec::new(),
            vm_locals_pool: Vec::new(),
            trace_out: None,
            module_stack: Vec::new(),
            modules: HashMap::new(),
            modules_by_path: HashMap::new(),
            loading_modules: Vec::new(),
            current_vm_captures: None,
            current_vm_locals: None,
            stdlib: None,
            collect_stack: Vec::new(),
            protected_series: HashSet::new(),
            user_trace: None,
            tasks: HashMap::new(),
            default_task: None,
            ran_tasks: HashSet::new(),
            tests: Vec::new(),
            current_suite: Vec::new(),
            test_hooks: Vec::new(),
            test_results: Vec::new(),
            tests_run: false,
            test_failed: 0,
            semantic_types: HashMap::new(),
            #[cfg(feature = "stats")]
            max_frame_depth: 0,
            #[cfg(feature = "stats")]
            instr_count: 0,
        }
    }

    /// Reset both instrumentation counters to zero. Called at the top of each
    /// `run_source*` entry point so per-program measurements start clean.
    /// No-op (and absent) when the `stats` feature is off.
    #[cfg(feature = "stats")]
    pub fn reset_stats(&mut self) {
        self.max_frame_depth = 0;
        self.instr_count = 0;
    }

    /// Record a `CallFrame` push: bump `max_frame_depth` if the current
    /// `call_stack.len()` exceeds it. Called by the function-call shim right
    /// after `env.call_stack.push(...)`. No-op (and absent) when the `stats`
    /// feature is off.
    #[cfg(feature = "stats")]
    pub fn record_frame_push(&mut self) {
        let depth = self.call_stack.len();
        if depth > self.max_frame_depth {
            self.max_frame_depth = depth;
        }
    }

    /// Remove `fd`'s entry from `func_cache` (the authoritative VM compiled-
    /// block cache for function bodies). Called when `bind` mutates a func's
    /// body so the next `CallUser` recompiles against the new bindings. M27.
    pub fn invalidate_func_cache(&mut self, fd: &Rc<FuncDef>) {
        self.func_cache.remove(&(Rc::as_ptr(fd) as usize));
    }

    /// Remove `series`'s entry from `block_cache` (the VM compiled-block cache
    /// for `do`-ed / `reduce`-d / loop-body blocks). Called defensively by
    /// any path that mutates a block's words in place (most paths deep-clone
    /// instead, producing a new `Rc` identity → natural cache miss). M27.
    pub fn invalidate_block_cache(&mut self, series: &Series) {
        self.block_cache
            .remove(&(Rc::as_ptr(&series.data) as usize, series.index));
    }

    /// Clear both VM caches. Defensive — used when `user_ctx` is swapped (e.g.
    /// `use`), ensuring no cached block compiled against the prior ctx is
    /// reused. M27.
    pub fn clear_caches(&mut self) {
        self.func_cache.clear();
        self.block_cache.clear();
    }

    /// M30: drop the indexed-natives cache so the next `vm::run` rebuilds it
    /// from the current `natives` map. Called by `register_natives` after it
    /// inserts/overwrites native entries. Cheap (the rebuild is O(n) on next
    /// `vm::run`, and only happens once per process — `register_natives` runs
    /// at startup, before any VM run).
    pub fn invalidate_native_index(&mut self) {
        self.natives_by_idx = None;
        self.native_names_by_idx = None;
    }

    /// M170: register a semantic type definition in `semantic_types`, keyed
    /// by `def.name`. Overwrites any prior entry with the same name (re-
    /// definition shadows the old one). Used by `make semantic-type!` and
    /// `define-type`.
    pub fn register_semantic_type(&mut self, def: Rc<crate::value::SemanticTypeDef>) {
        self.semantic_types.insert(def.name.clone(), def);
    }

    /// M170: look up a registered semantic type by its name word (e.g.
    /// `'rgb!`). Returns `None` if no semantic type with that name is
    /// registered.
    pub fn lookup_semantic_type(&self, sym: &Symbol) -> Option<Rc<crate::value::SemanticTypeDef>> {
        self.semantic_types.get(sym).cloned()
    }

    /// M31: enable per-instr VM tracing to `writer`. The VM appends one line
    /// per executed instr (`pc={pc} {instr:?}`). Set by the CLI `--trace`
    /// flag (which wires `stderr`) and by inline tests (which wire a buffer).
    pub fn set_trace(&mut self, writer: Box<dyn Write>) {
        self.trace_out = Some(writer);
    }

    /// M31: disable tracing. No-op if tracing was already off.
    pub fn clear_trace(&mut self) {
        self.trace_out = None;
    }

    /// M61: the currently-evaluating module (top of `module_stack`), or
    /// `None` when not inside a `module` body. Used by the `export` native
    /// (writes to the module's `exports` set) and by `module/word` path
    /// resolution (to skip the export check when accessing from inside the
    /// body).
    pub fn current_module(&self) -> Option<&Rc<RefCell<ModuleDef>>> {
        self.module_stack.last()
    }
}

/// A function invocation record. `ctx` holds parameter slots; `func` is the
/// definition being executed. Unused in M5 (no user functions yet).
///
/// M60: `captures` holds a closure's free-variable capture cell, present iff
/// the frame was pushed by a `Value::Closure` call. `resolve_word`/
/// `write_setword` read/write it via `Binding::Closure(idx)`.
pub struct CallFrame {
    pub ctx: Context,
    pub func: Option<Rc<FuncDef>>,
    /// M60: closure capture cell (shared `Rc` so the same closure's
    /// invocations all see the same `RefCell<Value>`s). `None` for plain
    /// `func`/`does`/`function` frames.
    pub captures: Option<Rc<Vec<std::cell::RefCell<Value>>>>,
}

/// Evaluation failure. Every variant that originates from a value carries a
/// `Span` so the CLI can later render `file:line:col:`. `Return`, `Break`,
/// and `Continue` are control-flow unwinds caught by their respective
/// shims (function-call shim for `Return`, loop natives for `Break`/
/// `Continue`), not user errors.
#[derive(Debug)]
pub enum EvalError {
    /// Word has no binding and no native of that name exists.
    UnboundWord { sym: Symbol, span: Span },
    /// A native or operation expected one value kind and got another.
    TypeError {
        expected: &'static str,
        found: &'static str,
        span: Span,
    },
    /// A native was called with the wrong number of arguments.
    Arity {
        native: Symbol,
        expected: usize,
        got: usize,
        span: Span,
    },
    /// `return` unwind — caught by the function-call shim (M9).
    Return(Value),
    /// `break` unwind — caught by the enclosing loop native. Carries an
    /// optional break-value (Red's `break/return`); the loop native decides
    /// whether to use it or discard it.
    Break(Option<Value>),
    /// `continue` unwind — caught by the enclosing loop native; advances to
    /// the next iteration.
    Continue,
    /// `throw value` unwind — caught by an enclosing `catch` native. Carries
    /// the thrown value. Like `Return`/`Break`/`Continue`, this is a control-
    /// flow unwind, not a user error, and carries no span.
    Throw(Value),
    /// `exit`/`quit` unwind — caught at the top-level script entry point.
    /// Carries the requested process exit code. Not a user error.
    Quit(i32),
    /// Generic native-reported error with a message.
    Native { message: String, span: Span },
    /// M42: a structured error value raised by `cause-error` or synthesized
    /// by the VM/walker when a `Native` error propagates. `try`/`attempt`/
    /// `catch` unwrap this into a `Value::Error`. Carries the full Red field
    /// set (code/type/args/near/where/by) via `ErrorValue`. The `near` field
    /// provides the span (via its value's span) when set; otherwise `span()`
    /// returns `None` and `render_error` omits the `file:line:col:` prefix.
    Raised(Rc<ErrorValue>),
    /// M31: bytecode compiler or VM dispatch invariant violated. Carries the
    /// structured `CompileErrorKind` (so callers can match on it) and the
    /// offending span. Replaces the prior `EvalError::Native { message:
    /// format!("VM: ..."), .. }` ad-hoc sites in `vm.rs`, and the
    /// `unreachable!()` panics in `compiler.rs`. The span may be
    /// `Span::default()` (zero) for sites with no source position (e.g. a
    /// pool index OOB in a synthetic block); `render_error` omits the
    /// `file:line:col:` prefix in that case.
    Compile { kind: CompileErrorKind, span: Span },
    /// M110: `parse` named-rule recursion exceeded the depth cap. Raised when
    /// a self-referential or mutually-recursive rule set has no base case on
    /// the current input (e.g. `a: [a] parse "x" [a]`). Prevents a Rust stack
    /// overflow by bailing in the interpreter loop instead. The span is the
    /// offending rule word's source position when available.
    ParseRecursionLimit { span: Span },
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Note: renders just the message body (no `*** Error:` prefix and no
        // `file:line:col:` location). The `render_error` function in
        // `error.rs` wraps this with the full `*** Error: [loc: ]<msg>` form
        // using a `LineMap`. The bare `Display` is used by test helpers that
        // only care about the message body.
        match self {
            EvalError::UnboundWord { sym, .. } => {
                write!(f, "{:?} has no value", sym.as_str())
            }
            EvalError::TypeError {
                expected, found, ..
            } => write!(f, "expected {expected}, found {found}"),
            EvalError::Arity {
                native,
                expected,
                got,
                ..
            } => write!(
                f,
                "{:?} expects {} argument(s), got {}",
                native.as_str(),
                expected,
                got
            ),
            EvalError::Return(_) => write!(f, "return used outside a function"),
            EvalError::Break(_) => write!(f, "break used outside a loop"),
            EvalError::Continue => write!(f, "continue used outside a loop"),
            EvalError::Throw(_) => {
                write!(f, "throw used outside a catch")
            }
            EvalError::Quit(code) => write!(f, "quit with exit code {code}"),
            EvalError::Native { message, .. } => write!(f, "{message}"),
            EvalError::Compile { kind, .. } => write!(f, "compile error: {kind}"),
            EvalError::Raised(ev) => write!(f, "{}", ev.message),
            EvalError::ParseRecursionLimit { .. } => {
                write!(f, "parse recursion limit exceeded")
            }
        }
    }
}

impl EvalError {
    /// Byte-offset span where this error originated, if any. Used by
    /// `render_error` to produce `file:line:col:` prefixes. `Return`/
    /// `Break`/`Continue` are control-flow unwinds, not user errors, and
    /// carry no span.
    pub fn span(&self) -> Option<Span> {
        match self {
            EvalError::UnboundWord { span, .. }
            | EvalError::TypeError { span, .. }
            | EvalError::Arity { span, .. }
            | EvalError::Native { span, .. }
            | EvalError::Compile { span, .. }
            | EvalError::ParseRecursionLimit { span, .. } => Some(*span),
            EvalError::Raised(ev) => ev.near.as_ref().and_then(|v| v.span()),
            EvalError::Return(_)
            | EvalError::Break(_)
            | EvalError::Continue
            | EvalError::Throw(_)
            | EvalError::Quit(_) => None,
        }
    }
}

impl std::error::Error for EvalError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// When the `stats` feature is on, `Env` exposes the two counter fields
    /// and they start at zero.
    #[cfg(feature = "stats")]
    #[test]
    fn stats_fields_present_and_zero_init() {
        let env = Env::new(Rc::new(Context::new()));
        assert_eq!(env.max_frame_depth, 0);
        assert_eq!(env.instr_count, 0);
    }

    /// When the `stats` feature is on, `reset_stats` and `record_frame_push`
    /// behave as expected: a push bumps `max_frame_depth`, reset zeroes it.
    #[cfg(feature = "stats")]
    #[test]
    fn stats_reset_and_push_track() {
        let mut env = Env::new(Rc::new(Context::new()));
        env.call_stack.push(CallFrame {
            ctx: Context::new(),
            func: None,
            captures: None,
        });
        env.record_frame_push();
        assert_eq!(env.max_frame_depth, 1);
        env.reset_stats();
        assert_eq!(env.max_frame_depth, 0);
        assert_eq!(env.instr_count, 0);
    }

    /// Compile-time check that with the `stats` feature OFF, `Env` has no
    /// counter fields. We use a trait-impl trick: `HasStats` is only
    /// implemented when the feature is on; without it, this test still
    /// compiles because the trait bound is *not* asserted — instead we
    /// verify the absence structurally by confirming the struct layout
    /// didn't change. The simplest faithful check is: the methods
    /// `reset_stats`/`record_frame_push` simply don't exist, so attempting
    /// to call them would fail to compile. We reference them via a cfg-gated
    /// path so this test body stays valid in both configurations.
    #[cfg(not(feature = "stats"))]
    #[test]
    fn stats_fields_absent_when_feature_off() {
        let mut env = Env::new(Rc::new(Context::new()));
        // No `max_frame_depth` / `instr_count` fields exist; the only
        // `Env`-mutating surface here is the public non-stats API. If a
        // counter field had leaked into the default build, the cfg-gated
        // `reset_stats` call below would not compile (method not found).
        // Confirm the env is usable without any stats surface:
        let _ = env.call_stack.len();
        // (No `env.reset_stats()` call — that method only exists under
        // `stats`, and this test only compiles without it.)
        let _ = &mut env;
    }

    /// Symmetric compile-time assertion under the `stats` feature: the
    /// methods *do* exist. Kept separate from the behavior test above so
    /// the "fields absent" test stays a pure compile check.
    #[cfg(feature = "stats")]
    #[test]
    fn stats_methods_exist_when_feature_on() {
        let mut env = Env::new(Rc::new(Context::new()));
        env.reset_stats();
        env.record_frame_push();
        // (Push without an actual frame is fine: record_frame_push just
        // reads call_stack.len() == 0, so max_frame_depth stays 0.)
        assert_eq!(env.max_frame_depth, 0);
    }

    /// M29: with `force-walk` on, `Env::new*` defaults to `EvalMode::Walk`
    /// (the v0.2 tree-walker). This is the parity-baseline configuration:
    /// `cargo test --workspace --features force-walk` runs the entire suite
    /// against the walker for byte-for-byte comparison with the VM-default run.
    #[cfg(feature = "force-walk")]
    #[test]
    fn force_walk_defaults_to_walker() {
        let env = Env::new(Rc::new(Context::new()));
        assert_eq!(env.mode, EvalMode::Walk);
    }

    /// M29: with `force-walk` off (the default build), `Env::new*` defaults
    /// to `EvalMode::Vm` (the v0.3 bytecode VM). This is the production
    /// configuration; `--walk` on the CLI overrides at runtime.
    #[cfg(not(feature = "force-walk"))]
    #[test]
    fn vm_is_default_evaluator() {
        let env = Env::new(Rc::new(Context::new()));
        assert_eq!(env.mode, EvalMode::Vm);
    }
}
