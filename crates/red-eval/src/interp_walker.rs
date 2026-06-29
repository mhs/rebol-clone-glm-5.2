//! Tree-walking evaluator (the v0.2 `interp`).
//!
//! M29 split the evaluator into a thin `interp.rs` dispatch shim that routes
//! `eval` calls to either this walker (`EvalMode::Walk`) or the bytecode VM
//! (`EvalMode::Vm`) based on `env.mode`. The VM is the default since M29;
//! this module is retained as the correctness fallback for `bind`/`use`/
//! `do`-on-data blocks flagged `needs_rebind`, and as the `--walk` /
//! `--features force-walk` baseline for the golden parity harness. See
//! `interp.rs` for the dispatch entry point.
//!
//! ---
//!
//! Original module-level docs (M5–M7 design notes):
//!
//! Milestone 5 scope: literals return themselves; `Block` is data (returned
//! as-is unless entered explicitly); `Paren` is walked eagerly in place;
//! `Word` resolves via its binding (or errors if unbound and not a native);
//! `SetWord` evaluates the next value and writes it into the bound slot;
//! `GetWord` reads the slot without calling. Native *calls* (collecting
//! arguments and invoking `f`) land in M6 — for now `resolve_word` may
//! produce a `Value::Func`, but nothing dispatches it.
//!
//! Milestone 7 adds *expression-based* evaluation: a single step evaluates a
//! prefix value followed by any chain of infix natives (`1 + 2 * 3` →
//! `((1 + 2) * 3)` = 9, Red's left-to-right no-precedence rule). SetWord
//! RHS and native arguments are both evaluated as expressions so that
//! `x: 1 + 2` and `print 1 + 2` work. Loop-variable names (`repeat 'i ...`
//! / `repeat i ...`) are pre-allocated by the binding pass so body
//! references resolve to the counter slot.
//!
//! Index contract: every evaluator function (`eval`, `eval_expression`,
//! `eval_prefix`, `dispatch_call`) leaves `*i` pointing at the *next*
//! unprocessed value — one past whatever it consumed.

use std::rc::Rc;

use red_core::value::{Binding, FuncDef, Series, Span, Symbol, Value};
use red_core::vm_ir::CompiledBlock;
use red_core::{Env, EvalError, EvalMode, RefineArgs};

use crate::binding::has_foreign_bindings;

/// Evaluate a block/paren value: walk its contents in order, returning the
/// last value. Non-block/paren values passed in are returned as-is (cloned).
///
/// This is the *block-walker*. Each step evaluates one *expression* (a
/// prefix value plus any trailing infix chain) via [`eval_expression`].
pub fn eval(block: &Value, env: &mut Env) -> Result<Value, EvalError> {
    let series = match block {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        _ => return Ok(block.clone()),
    };

    let mut last = Value::None;
    let data = series.data.borrow();
    let mut i = series.index;
    while i < data.len() {
        #[cfg(feature = "stats")]
        {
            env.instr_count += 1;
        }
        last = eval_expression(&data, &mut i, env)?;
    }
    Ok(last)
}

/// Dispatch a block/paren evaluation to the active evaluator (`Walk` or `Vm`),
/// used by natives that recurse into a block argument (`do`/`if`/`either`/
/// `loop`/`while`/`repeat`/`until`/`foreach`/`forall`/`switch`/`case`/`try`/
/// `attempt`/`catch`/`use`).
///
/// - `Walk` → `interp::eval` (the tree-walker).
/// - `Vm`   → compile-on-demand + `vm::run`. If the block's compiled form is
///   `needs_rebind` (set by M23 for `use`/`make object!`/`object`/`context`
///   forms, or by `bind` rebinding words to a non-`user_ctx` context), the
///   block falls back to the walker. Compilation failure also falls back
///   (defensive — the walker handles anything the compiler can't yet).
///
/// Non-block/paren values are returned as-is (cloned), mirroring `eval`.
///
/// M29 flips `Env::mode` to `Vm` by default; until then, the shim exists so
/// VM-mode tests can exercise native recursion through the VM (M26).
pub(crate) fn dispatch_block(block: &Value, env: &mut Env) -> Result<Value, EvalError> {
    if env.mode == EvalMode::Walk {
        return eval(block, env);
    }
    let series = match block {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        _ => return Ok(block.clone()),
    };
    // M27: check the Env-level block cache by Series identity before
    // recompiling. Safe without explicit invalidation because `bind`/`use`
    // deep-clone the series (new `Rc` → new identity → miss) and `user_ctx`
    // slots are append-only (cached `LoadGlobal(slot)` stays valid).
    //
    // M29 SAFETY FIX: the cache key is `Rc::as_ptr(&series.data)`. If the
    // `Rc` is dropped and the allocator reuses the address for a new
    // `Rc<RefCell<Vec<Value>>>`, the cache returns a stale entry — the root
    // cause of the M29 object-inheritance bug (two `make object!` spec blocks
    // got the same `Rc::as_ptr` after the first was dropped, so the second
    // ran the first's compiled form with wrong slot indices). The fix is to
    // also verify the cached block's `source_span` matches the series's
    // `block_source_span` — a cheap O(1) structural check that catches
    // allocator reuse (two different blocks almost always have different
    // spans). If the spans don't match, we recompile (cache miss).
    //
    // M30 PERF: the cache check happens *before* `has_foreign_bindings` so a
    // cached block skips the per-value foreign-bindings walk on every re-
    // entry. Without this ordering, a 1M-iteration `repeat` paid the O(n)
    // walk 1M times — the root cause of the v0.3.0 `sum_loop`/`sum_while`
    // regressions (VM 3x slower than walker on those fixtures). A cached
    // block is by construction non-foreign (the cache only stores blocks
    // that passed the check on first compile), so skipping the recheck is
    // sound.
    let cache_key = (Rc::as_ptr(&series.data) as usize, series.index);
    if let Some(cached) = env.block_cache.get(&cache_key).cloned() {
        let expected_span = crate::vm::compiler::block_source_span(&series);
        if cached.source_span == expected_span {
            return crate::vm::run((*cached).clone(), env);
        }
    }
    // `bind`/`use` may rebind words to a child context the VM can't lexically
    // address. Detect that and route to the walker. Only paid on a cache miss
    // (first entry of a block) — subsequent entries hit the cache above and
    // skip this O(n) walk.
    if has_foreign_bindings(&series, &env.user_ctx) {
        return eval(block, env);
    }
    // Compile-on-demand. The M23 analyzer marks `use`/object forms
    // `needs_rebind`; such blocks return a stub `[Halt]` from `compile_block`,
    // so we check the flag and fall back to the walker.
    //
    // M29: `analyze_block` (called inside `compile_block`) mutates the
    // series's bindings in-place (converting `Local` → `Lexical`). If the
    // compile fails or `needs_rebind`, we fall back to the walker — but the
    // walker can't handle `Lexical` bindings (it errors). Fix: compile a
    // deep clone so the original series's bindings stay intact for the
    // walker fallback. The clone is only taken in VM mode (the walker path
    // above doesn't reach here), so there's no cost in `Walk` mode.
    let registry = crate::vm::compiler::NativeRegistry::from_env(env);
    let mut scope = crate::vm::lex::Scope::root(&env.user_ctx);
    let compile_series = crate::binding::deep_clone_series(&series);
    match crate::vm::compiler::compile_block(&compile_series, &mut scope, &registry) {
        Ok(compiled) if !compiled.needs_rebind => {
            env.block_cache.insert(cache_key, Rc::new(compiled.clone()));
            crate::vm::run(compiled, env)
        }
        _ => eval(block, env),
    }
}

/// M30.2.E: Resolve a block to its `CompiledBlock` for VM execution, using
/// the same cache as `dispatch_block` but without running it. Returns
/// `None` if the block should fall back to the walker (foreign bindings,
/// `needs_rebind`, or `EvalMode::Walk`). The loop natives
/// (`repeat`/`while`/`foreach`/`forall`/`loop`/`until`) call this once to
/// get the compiled form, then call `vm::run` in a tight loop — eliminating
/// the per-iteration `dispatch_block` overhead (HashMap lookup + Rc bumps +
/// `CompiledBlock` clone + pool drain/restore) that caused the v0.3.0 loop
/// regressions.
///
/// On the first call (cache miss), this compiles the block, inserts it
/// into `env.block_cache`, and returns it. Subsequent calls hit the cache
/// (the cache key is `(Rc::as_ptr(&series.data), series.index)`, stable
/// across `Rc` clones of the same series).
pub(crate) fn resolve_compiled_block(block: &Value, env: &mut Env) -> Option<Rc<CompiledBlock>> {
    if env.mode == EvalMode::Walk {
        return None;
    }
    let series = match block {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        _ => return None,
    };
    // Check the cache (same logic as `dispatch_block`).
    let cache_key = (Rc::as_ptr(&series.data) as usize, series.index);
    if let Some(cached) = env.block_cache.get(&cache_key).cloned() {
        let expected_span = crate::vm::compiler::block_source_span(&series);
        if cached.source_span == expected_span {
            return Some(cached);
        }
    }
    // Cache miss: check foreign bindings + compile.
    if has_foreign_bindings(&series, &env.user_ctx) {
        return None;
    }
    let registry = crate::vm::compiler::NativeRegistry::from_env(env);
    let mut scope = crate::vm::lex::Scope::root(&env.user_ctx);
    let compile_series = crate::binding::deep_clone_series(&series);
    match crate::vm::compiler::compile_block(&compile_series, &mut scope, &registry) {
        Ok(compiled) if !compiled.needs_rebind => {
            let rc = Rc::new(compiled);
            env.block_cache.insert(cache_key, Rc::clone(&rc));
            Some(rc)
        }
        _ => None,
    }
}

/// Like `dispatch_block` but for the `reduce` native: in VM mode, compiles the
/// block with `compile_block_reduce` (no `Pop` between expressions) and runs
/// `vm::run_reduce`, which collects every expression's result into a
/// `Value::Block`. In `Walk` mode, uses the walker's per-expression
/// `eval_expression` loop (mirroring `reduce`'s existing behavior). Falls
/// back to the walker for `needs_rebind`/foreign-bound blocks. (M26)
pub(crate) fn dispatch_block_reduce(block: &Value, env: &mut Env) -> Result<Value, EvalError> {
    let series = match block {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        _ => return Ok(block.clone()),
    };
    if env.mode == EvalMode::Walk || has_foreign_bindings(&series, &env.user_ctx) {
        return reduce_walker(&series, env);
    }
    // M27: Env-level block cache (same identity key + M29 safety fix as
    // `dispatch_block` — verify `source_span` to catch allocator-reuse).
    let cache_key = (Rc::as_ptr(&series.data) as usize, series.index);
    if let Some(cached) = env.block_cache.get(&cache_key).cloned() {
        let expected_span = crate::vm::compiler::block_source_span(&series);
        if cached.source_span == expected_span {
            return crate::vm::run_reduce((*cached).clone(), env);
        }
    }
    let registry = crate::vm::compiler::NativeRegistry::from_env(env);
    let mut scope = crate::vm::lex::Scope::root(&env.user_ctx);
    // M29: compile a deep clone (same rationale as `dispatch_block`).
    let compile_series = crate::binding::deep_clone_series(&series);
    match crate::vm::compiler::compile_block_reduce(&compile_series, &mut scope, &registry) {
        Ok(compiled) if !compiled.needs_rebind => {
            env.block_cache.insert(cache_key, Rc::new(compiled.clone()));
            crate::vm::run_reduce(compiled, env)
        }
        _ => reduce_walker(&series, env),
    }
}

/// Walker-side `reduce`: eval each expression, collect results into a block.
/// (Factored out of `reduce` for reuse by `dispatch_block_reduce`.)
fn reduce_walker(series: &Series, env: &mut Env) -> Result<Value, EvalError> {
    let data = series.data.borrow();
    let mut results = Vec::new();
    let mut i = series.index;
    while i < data.len() {
        results.push(eval_expression(&data, &mut i, env)?);
    }
    drop(data);
    Ok(Value::block(Series::new(results)))
}

/// Evaluate a single expression starting at `data[*i]`: a prefix value
/// followed by zero or more infix native applications (Red's left-to-right,
/// no-precedence rule). Advances `*i` past the entire expression.
///
/// Examples (within a block):
///   - `5`                → `Integer(5)`        (no trailing infix)
///   - `1 + 2`            → `Integer(3)`        (one infix op)
///   - `1 + 2 * 3`        → `Integer(9)`        (two infix ops, L-to-R)
///   - `foo`              → slot value          (word resolves, no infix)
///   - `print "hi"`       → `None`              (prefix native call)
///   - `x: 1 + 2`         → `Integer(3)`        (SetWord RHS is an expression)
pub(crate) fn eval_expression(
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let mut value = eval_prefix(data, i, env)?;

    // Chain infix natives left-to-right. Each infix native uses `value` as
    // its left operand and consumes one prefix value as its right.
    while *i < data.len() {
        let infix = match infix_native_at(&data[*i], env) {
            Some(fd) => fd,
            None => break,
        };
        let infix_span = data[*i].span_or_default();
        *i += 1; // consume the infix word itself; now *i points at the right operand
        let arity = infix.params.len();
        debug_assert!(
            arity >= 1,
            "infix native must take at least one operand (the left value)"
        );
        let mut args = Vec::with_capacity(arity);
        args.push(value);
        // The first operand (left value) is already evaluated; the remaining
        // `arity - 1` operands are consumed as prefix values from the block.
        for _ in 1..arity {
            if *i >= data.len() {
                return Err(EvalError::Arity {
                    native: Symbol::new("<infix>"),
                    expected: arity,
                    got: args.len(),
                    span: infix_span,
                });
            }
            args.push(eval_prefix(data, i, env)?);
        }
        let f = infix.native.unwrap();
        value = f(&args, &RefineArgs::empty(), env)?;
    }
    Ok(value)
}

/// If `v` is an unbound `Word` naming an infix native, return its `FuncDef`.
/// Infix natives are never invoked through the normal `dispatch_call` path —
/// they're consumed by [`eval_expression`] before the prefix evaluator ever
/// sees them.
fn infix_native_at(v: &Value, env: &Env) -> Option<Rc<FuncDef>> {
    let sym = match v {
        Value::Word { sym, binding, .. } | Value::GetWord { sym, binding, .. } => {
            if !matches!(binding, Binding::Unbound) {
                return None;
            }
            sym
        }
        _ => return None,
    };
    env.natives.get(sym).filter(|fd| fd.infix).cloned()
}

/// Evaluate a single prefix value (no infix chaining): literals are cloned,
/// `Paren` is walked eagerly, `Word` resolves and dispatches a native call,
/// `SetWord` consumes one trailing expression as its RHS, `GetWord` reads
/// the slot without invoking, `Block` is returned as data. Advances `*i`
/// past the consumed value (and any native args / SetWord RHS).
fn eval_prefix(
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let span = data[*i].span_or_default();
    let cur = data[*i].clone();
    *i += 1; // consume the prefix value itself
    match &cur {
        // Data / literals: returned as-is.
        Value::None
        | Value::Logic(_)
        | Value::Integer { .. }
        | Value::Float { .. }
        | Value::String { .. }
        | Value::String8 { .. }
        | Value::LitWord { .. }
        | Value::Block { .. }
        | Value::Func(_)
        | Value::Refinement { .. }
        | Value::Error(_)
        | Value::File { .. }
        | Value::Url { .. }
        | Value::Char { .. }
        | Value::Object(_) => Ok(cur),

        // Path: a function-headed path is a refined call (`copy/part`,
        // `find/case`); anything else is a data-path select (`block/2`,
        // `obj/field`) which lands in M19. Resolve the head; if it's a Func,
        // dispatch a refined call with the path tail as leading refinement
        // flags. Otherwise dispatch as a data-path select.
        Value::Path {
            parts,
            span: path_span,
        } => eval_path_call(parts, *path_span, data, i, env),

        // GetPath `:foo/bar` — like `GetWord`, walks the path returning the
        // value at the final field WITHOUT invoking it (so `:obj/method`
        // yields the function value). M19.
        Value::GetPath {
            parts,
            span: path_span,
        } => eval_get_path(parts, *path_span, env),

        // LitPath `'foo/bar` — returns the path as data (mirrors `LitWord`).
        // The head is *not* resolved; the value carries the full parts.
        Value::LitPath { .. } => Ok(cur),

        // SetPath `obj/field: value` — evaluate the next expression as the
        // RHS, then write it into the final field/slot identified by walking
        // the path. M19.
        Value::SetPath {
            parts,
            span: path_span,
        } => {
            if *i >= data.len() {
                return Err(EvalError::Arity {
                    native: Symbol::new("<set-path>"),
                    expected: 1,
                    got: 0,
                    span: *path_span,
                });
            }
            let rhs = eval_expression(data, i, env)?;
            set_path_value(parts, rhs.clone(), env, *path_span)?;
            Ok(rhs)
        }

        // Paren: walked eagerly in place. The recursion borrows the child
        // series's `RefCell`, distinct from the outer borrow.
        Value::Paren { series: p, .. } => {
            let p = p.clone();
            eval(&Value::Paren { series: p, span }, env)
        }

        Value::Word { sym, binding, .. } => {
            let resolved = resolve_word(sym, binding, env, span)?;
            // `dispatch_call` collects args starting at the new `*i`.
            dispatch_call(resolved, sym, data, i, env, span)
        }

        Value::SetWord { sym, binding, .. } => {
            // Evaluate the next *expression* as the RHS (so `x: 1 + 2` works).
            if *i >= data.len() {
                return Err(EvalError::Arity {
                    native: sym.clone(),
                    expected: 1,
                    got: 0,
                    span,
                });
            }
            let rhs = eval_expression(data, i, env)?;
            write_setword(sym, binding, rhs.clone(), env, span)?;
            // `foo: 5` evaluates to the written value (Red semantics).
            Ok(rhs)
        }

        Value::GetWord { sym, binding, .. } => {
            // GetWord returns the slot value (or native Func) without
            // invoking it — no argument collection, no dispatch.
            resolve_word(sym, binding, env, span)
        }
    }
}

/// If `resolved` is a `Func`, collect arguments from `data` (advancing `i`)
/// and invoke it. Native funcs (M6+) call their `NativeFn` directly; user
/// funcs (M9: created via `func`/`does`/`make function!`) push a `CallFrame`
/// with a per-call context clone, evaluate the body, and catch
/// `EvalError::Return` as the return value. Otherwise return `resolved`
/// as-is. `sym` is the calling word's symbol, used for arity-error messages.
///
/// Pre: `*i` points at the first potential argument (the calling word has
/// already been consumed by [`eval_prefix`]). Each argument is evaluated as a
/// full *expression* (so `print 1 + 2` passes `3`), not just a single prefix
/// value. Post: `*i` points past the last consumed argument.
fn dispatch_call(
    resolved: Value,
    sym: &Symbol,
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
    span: Span,
) -> Result<Value, EvalError> {
    dispatch_call_with_refs(resolved, sym, &[], data, i, env, span)
}

/// Like [`dispatch_call`] but the caller has already consumed some refinement
/// flags from a path (`copy/part` → head `copy`, leading ref `["part"]`).
/// `leading_refs` are refinement names already activated; they're merged with
/// any inline `/ref` tokens discovered at the call site during spec-order
/// collection.
fn dispatch_call_with_refs(
    resolved: Value,
    sym: &Symbol,
    leading_refs: &[Symbol],
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
    span: Span,
) -> Result<Value, EvalError> {
    let fd = match &resolved {
        Value::Func(fd) if fd.native.is_some() => fd.clone(),
        Value::Func(fd) => {
            let (args, refs) = collect_call_args(sym, fd, leading_refs, data, i, env, span)?;
            return call_user_func(fd, args, &refs, env);
        }
        _ => return Ok(resolved),
    };
    let (args, refs) = collect_call_args(sym, &fd, leading_refs, data, i, env, span)?;
    let f = fd.native.unwrap();
    // M42: enrich `Native`/`TypeError`/etc. errors into `Raised` with the
    // native name (`where`) and call-site span (`near`). Centralizes
    // structured-error construction so `try`/`catch` see a uniform payload.
    match f(&args, &refs, env) {
        Ok(v) => Ok(v),
        Err(e) => Err(crate::natives::enrich_error(e, Some(sym.clone()), span)),
    }
}

/// A function-headed path (`copy/part [1 2 3] 2`) dispatches as a refined
/// call: resolve the head word; if it's a Func, treat the path tail as
/// leading refinement flags and delegate to [`dispatch_call_with_refs`].
/// Otherwise dispatch as a data-path select: object field, block/string
/// integer pick, with paren parts evaluated in place (M19).
fn eval_path_call(
    parts: &[Value],
    path_span: Span,
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if parts.is_empty() {
        return Err(EvalError::Native {
            message: "empty path".into(),
            span: path_span,
        });
    }
    // Resolve the head word.
    let (head_sym, head_binding) = match &parts[0] {
        Value::Word { sym, binding, .. } => (sym.clone(), binding.clone()),
        Value::GetWord { sym, binding, .. } => (sym.clone(), binding.clone()),
        Value::LitWord { sym, .. } => (sym.clone(), Binding::Unbound),
        other => {
            return Err(EvalError::Native {
                message: format!(
                    "path head must be a word, found {}",
                    crate::natives::type_name(other)
                ),
                span: path_span,
            });
        }
    };
    // For function-headed paths (`copy/part`), the path tail is treated as
    // leading refinement flags — only Word parts are extracted; non-word
    // parts (integer/paren) aren't valid refinement flags so they're dropped
    // from the refinement list (a malformed call surfaces as an arity error
    // during arg collection).
    let leading_refs: Vec<Symbol> = parts[1..]
        .iter()
        .filter_map(|p| match p {
            Value::Word { sym, .. } => Some(sym.clone()),
            _ => None,
        })
        .collect();
    let resolved = resolve_word(&head_sym, &head_binding, env, path_span)?;
    match resolved {
        Value::Func(_) => {
            dispatch_call_with_refs(resolved, &head_sym, &leading_refs, data, i, env, path_span)
        }
        Value::Object(obj) => select_object_path(obj, &parts[1..], data, i, env, path_span),
        // Non-function, non-object data path: walk the tail selecting by
        // integer index (block/string) or word field (object encountered
        // mid-walk). Paren parts are evaluated in place. M19.
        other => {
            let result = walk_data_path(other, &parts[1..], env, path_span)?;
            // If the final value is a Func, it's a method call on whatever
            // object/context produced it — but only the Object arm above
            // knows the owning ctx. For block/string-headed paths reaching a
            // Func, return the Func value as-is (callers can use `:path` to
            // fetch it deliberately).
            Ok(result)
        }
    }
}

/// Walk a path's tail parts starting from `current`, returning the final
/// selected value. Each step:
/// - `Word` → field lookup on the current value (object field, or error
///   for non-objects). For object intermediates, dispatches a method call
///   if the field is a Func and there are more block args available — but
///   only when reached via `eval_path_call` (which has the block cursor);
///   this helper alone returns the Func value.
/// - `Integer` → 1-based index pick (negative from tail) on a block or
///   string. Out-of-range → `none`.
/// - `Paren` → evaluate in place, replace `current` with the result.
/// - other → TypeError.
///
/// This is the shared core of `eval_path_call` (data-path select),
/// `eval_get_path`, and `set_path_value` (which uses the second-to-last
/// value as the assign target).
fn walk_data_path(
    mut current: Value,
    tail: &[Value],
    env: &mut Env,
    path_span: Span,
) -> Result<Value, EvalError> {
    for part in tail {
        current = step_path(&current, part, env, path_span)?;
    }
    Ok(current)
}

/// One step of data-path traversal: select `part` from `current`.
/// `_path_span` is retained in the signature for caller symmetry with
/// `walk_data_path`/`set_path_value`; per-step errors localize to the
/// part's own span instead.
fn step_path(
    current: &Value,
    part: &Value,
    env: &mut Env,
    _path_span: Span,
) -> Result<Value, EvalError> {
    // Localize per-step errors to the offending part's own span (not the
    // whole path's) so a multi-segment path can pinpoint the bad segment.
    let part_span = part.span_or_default();
    match part {
        Value::Word { sym, .. } => select_field(current, sym, part_span),
        Value::GetWord { sym, .. } => select_field(current, sym, part_span),
        Value::LitWord { sym, .. } => select_field(current, sym, part_span),
        Value::Integer { n, .. } => pick_path_index(current, *n, part_span),
        Value::Paren { series, .. } => {
            let p = series.clone();
            let v = eval(
                &Value::Paren {
                    series: p,
                    span: part_span,
                },
                env,
            )?;
            // The paren's result is the *selector* for this step — recurse
            // with the evaluated value as the part. So `b/(1 + 1)` evaluates
            // the paren to `2`, then picks index 2 from `b`.
            step_path(current, &v, env, part_span)
        }
        other => Err(EvalError::TypeError {
            expected: "word!, integer!, or paren! in path",
            found: crate::natives::type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// Select a named field from `current`. Objects look up by slot; other types
/// error. (Map support deferred per plan.)
fn select_field(current: &Value, sym: &Symbol, path_span: Span) -> Result<Value, EvalError> {
    match current {
        Value::Object(obj) => obj.borrow().ctx.get(sym).ok_or_else(|| EvalError::Native {
            message: format!("object has no field {}", sym.as_str()),
            span: path_span,
        }),
        other => Err(EvalError::Native {
            message: format!(
                "cannot select field {} from {}",
                sym.as_str(),
                crate::natives::type_name(other)
            ),
            span: path_span,
        }),
    }
}

/// `pick`-style 1-based index selection (negative from tail) for block/string
/// path parts. Out-of-range returns `none`. Strings return a `char!` (the
/// codepoint at the 1-based index).
fn pick_path_index(current: &Value, n: i64, path_span: Span) -> Result<Value, EvalError> {
    match current {
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            let data = series.data.borrow();
            let len = data.len() as i64;
            let idx = if n > 0 {
                (n - 1) as usize
            } else if n < 0 {
                match (len + n).try_into() {
                    Ok(v) => v,
                    Err(_) => return Ok(Value::None),
                }
            } else {
                return Ok(Value::None);
            };
            if idx >= data.len() {
                return Ok(Value::None);
            }
            Ok(data[idx].clone())
        }
        Value::String { s, .. } => {
            // M38: string char pick returns a `char!` value (the codepoint at
            // the 1-based index; negative counts from the tail).
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let idx = if n > 0 {
                (n - 1) as usize
            } else if n < 0 {
                match (len + n).try_into() {
                    Ok(v) => v,
                    Err(_) => return Ok(Value::None),
                }
            } else {
                return Ok(Value::None);
            };
            if idx >= chars.len() {
                return Ok(Value::None);
            }
            Ok(Value::char(chars[idx]))
        }
        other => Err(EvalError::TypeError {
            expected: "block!, paren!, or string! for integer path index",
            found: crate::natives::type_name(other),
            span: path_span,
        }),
    }
}

/// M38: rebuild a string with the char at 1-based index `n` replaced by the
/// codepoint of `rhs` (a `char!` or single-char `string!`/`integer!`). Returns
/// the new string value. `Rc<str>` is immutable, so the caller must write the
/// result back to the head word's binding.
fn poke_string_char(s: &Rc<str>, n: i64, rhs: &Value, path_span: Span) -> Result<Value, EvalError> {
    let new_c = match rhs {
        Value::Char { c, .. } => *c,
        Value::Integer { n: k, .. } => {
            char::from_u32(*k as u32).ok_or_else(|| EvalError::Native {
                message: format!("poke: integer {k} is not a valid char codepoint"),
                span: path_span,
            })?
        }
        Value::String { s: ss, .. } => {
            let mut chars = ss.chars();
            let c = chars.next().ok_or_else(|| EvalError::Native {
                message: "poke: empty string cannot be poked as a char".into(),
                span: path_span,
            })?;
            if chars.next().is_some() {
                return Err(EvalError::Native {
                    message: "poke: multi-char string is not a char".into(),
                    span: path_span,
                });
            }
            c
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "char!, integer!, or single-char string!",
                found: crate::natives::type_name(other),
                span: path_span,
            });
        }
    };
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let idx = if n > 0 {
        (n - 1) as usize
    } else if n < 0 {
        match (len + n).try_into() {
            Ok(v) => v,
            Err(_) => {
                return Err(EvalError::Native {
                    message: format!("poke index {n} out of range"),
                    span: path_span,
                });
            }
        }
    } else {
        return Err(EvalError::Native {
            message: "poke index 0 is invalid (1-based)".into(),
            span: path_span,
        });
    };
    if idx >= chars.len() {
        return Err(EvalError::Native {
            message: format!("poke index {n} out of range"),
            span: path_span,
        });
    }
    let mut rebuilt = chars.clone();
    rebuilt[idx] = new_c;
    let new_str: String = rebuilt.into_iter().collect();
    Ok(Value::string(Rc::from(new_str.as_str())))
}

/// `:foo/bar` — resolve the head and walk the tail, returning the final
/// value *without* invoking any function encountered. Same as `walk_data_path`
/// except the head is resolved as a `GetWord` (reads the slot / fetches the
/// native Func without dispatching).
pub(crate) fn eval_get_path(
    parts: &[Value],
    path_span: Span,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if parts.is_empty() {
        return Err(EvalError::Native {
            message: "empty get-path".into(),
            span: path_span,
        });
    }
    let head = match &parts[0] {
        Value::Word { sym, binding, .. } | Value::GetWord { sym, binding, .. } => {
            resolve_word(sym, binding, env, path_span)?
        }
        Value::LitWord { sym: _, .. } => {
            // A lit-path head with lit-word head is itself data; return the
            // first field as a lit-word. (Unusual; not really expected.)
            return Ok(parts[0].clone());
        }
        other => {
            return Err(EvalError::Native {
                message: format!(
                    "get-path head must be a word, found {}",
                    crate::natives::type_name(other)
                ),
                span: path_span,
            });
        }
    };
    walk_data_path(head, &parts[1..], env, path_span)
}

/// `obj/field: value` — walk the path to its second-to-last element, then
/// write `rhs` into the final field/index. The head is resolved (so the
/// path is bound to a real container), then each intermediate step is
/// walked via `step_path`, and finally the last part selects the target
/// container and slot to write into.
pub(crate) fn set_path_value(
    parts: &[Value],
    rhs: Value,
    env: &mut Env,
    path_span: Span,
) -> Result<(), EvalError> {
    if parts.len() < 2 {
        return Err(EvalError::Native {
            message: "set-path requires at least two parts".into(),
            span: path_span,
        });
    }
    // Resolve head.
    let (head_sym, head_binding) = match &parts[0] {
        Value::Word { sym, binding, .. } | Value::GetWord { sym, binding, .. } => {
            (sym.clone(), binding.clone())
        }
        other => {
            return Err(EvalError::Native {
                message: format!(
                    "set-path head must be a word, found {}",
                    crate::natives::type_name(other)
                ),
                span: path_span,
            });
        }
    };
    let mut current = resolve_word(&head_sym, &head_binding, env, path_span)?;
    // Walk intermediate parts (all but the last two: head and final part).
    // For a 2-part path `obj/field:`, that's just `parts[0]` (head) → current
    // is the obj, and `parts[1]` is the final field write.
    let intermediates = &parts[1..parts.len() - 1];
    for part in intermediates {
        current = step_path(&current, part, env, path_span)?;
    }
    // Final write.
    let final_part = &parts[parts.len() - 1];
    // M38: string char poke. `Rc<str>` is immutable, so we rebuild the string
    // and write the new value back to the head word's binding (mirrors how
    // `s/2: #"X"` works in Red — strings are value types for path poke).
    if let Value::String { s, .. } = &current {
        if let Value::Integer { n, .. } = final_part {
            let new_str = poke_string_char(s, *n, &rhs, path_span)?;
            return write_setword(&head_sym, &head_binding, new_str, env, path_span);
        }
    }
    write_path_slot(&current, final_part, rhs, env, path_span)
}

/// Write `rhs` into the slot identified by `part` on `current`. The final
/// write target:
/// - Object + Word → set object slot.
/// - Block + Integer → `poke`-style 1-based (negative from tail) write into
///   shared storage.
/// - String + Integer → handled in `set_path_value` (rebuild + write back to
///   head binding); this arm is only reachable via nested paren paths and
///   surfaces as an error (immutable `Rc<str>`).
/// - Paren → evaluate, then recurse on the result (so `obj/(word)/field:` works).
fn write_path_slot(
    current: &Value,
    part: &Value,
    rhs: Value,
    env: &mut Env,
    path_span: Span,
) -> Result<(), EvalError> {
    match part {
        Value::Word { sym, .. } | Value::GetWord { sym, .. } | Value::LitWord { sym, .. } => {
            match current {
                Value::Object(obj) => {
                    let obj_ctx = Rc::clone(&obj.borrow().ctx);
                    if let Some(idx) = obj_ctx.index_of(sym) {
                        obj_ctx.set_slot(idx, rhs);
                        Ok(())
                    } else {
                        // Allocate a new slot for a previously-unknown field
                        // (objects support field addition via set-path).
                        obj_ctx.set(sym.clone(), rhs);
                        Ok(())
                    }
                }
                other => Err(EvalError::Native {
                    message: format!(
                        "cannot set field {} on {}",
                        sym.as_str(),
                        crate::natives::type_name(other)
                    ),
                    span: path_span,
                }),
            }
        }
        Value::Integer { n, .. } => match current {
            Value::Block { series, .. } | Value::Paren { series, .. } => {
                let mut data = series.data.borrow_mut();
                let len = data.len() as i64;
                let idx = if *n > 0 {
                    (*n - 1) as usize
                } else if *n < 0 {
                    match (len + n).try_into() {
                        Ok(v) => v,
                        Err(_) => {
                            return Err(EvalError::Native {
                                message: format!("poke index {n} out of range"),
                                span: path_span,
                            });
                        }
                    }
                } else {
                    return Err(EvalError::Native {
                        message: "poke index 0 is invalid (1-based)".into(),
                        span: path_span,
                    });
                };
                if idx >= data.len() {
                    return Err(EvalError::Native {
                        message: format!("poke index {n} out of range"),
                        span: path_span,
                    });
                }
                data[idx] = rhs;
                Ok(())
            }
            Value::String { .. } => Err(EvalError::Native {
                message: "string char poke must target a word head (immutable Rc<str>)".into(),
                span: path_span,
            }),
            other => Err(EvalError::TypeError {
                expected: "block! or paren! for integer set-path index",
                found: crate::natives::type_name(other),
                span: path_span,
            }),
        },
        Value::Paren { series, .. } => {
            let p = series.clone();
            let v = eval(
                &Value::Paren {
                    series: p,
                    span: path_span,
                },
                env,
            )?;
            write_path_slot(&v, part, rhs, env, path_span)
        }
        other => Err(EvalError::TypeError {
            expected: "word!, integer!, or paren! in set-path",
            found: crate::natives::type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// Resolve `obj/field` or `obj/method` (with optional method args collected
/// from the enclosing block). Walks the field chain through nested objects;
/// when a field value is a `Func` and it's the last part, dispatches as a
/// method call with `env.user_ctx` temporarily swapped to the owning
/// object's ctx. Non-Word parts (integer/paren) at intermediate positions
/// dispatch to `walk_data_path` on the field value.
fn select_object_path(
    obj: std::rc::Rc<std::cell::RefCell<red_core::value::ObjectDef>>,
    parts: &[Value],
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
    path_span: Span,
) -> Result<Value, EvalError> {
    if parts.is_empty() {
        return Err(EvalError::Native {
            message: "object path requires at least one field".into(),
            span: path_span,
        });
    }
    let mut current_obj = obj;
    let mut idx = 0;
    loop {
        let part = &parts[idx];
        // Only Word-family parts select object fields; integer/paren parts
        // switch to general data-path traversal on the current field value.
        let field_sym = match part {
            Value::Word { sym, .. } | Value::GetWord { sym, .. } | Value::LitWord { sym, .. } => {
                sym.clone()
            }
            other => {
                // Non-word part on an object head: fetch the field-named-by-
                // prior part's value, then dispatch to walk_data_path for
                // this and subsequent parts. (This branch is only hit when
                // an object path mixes word and non-word parts, e.g.
                // `obj/items/2`.)
                let _ = other;
                let head_val = Value::Object(Rc::clone(&current_obj));
                let mut current = head_val;
                while idx < parts.len() {
                    current = step_path(&current, &parts[idx], env, path_span)?;
                    idx += 1;
                }
                return Ok(current);
            }
        };
        let field_val =
            current_obj
                .borrow()
                .ctx
                .get(&field_sym)
                .ok_or_else(|| EvalError::Native {
                    message: format!("object has no field {}", field_sym.as_str()),
                    span: path_span,
                })?;
        // If this is the last part, return it (or call it if a Func).
        if idx == parts.len() - 1 {
            return match field_val {
                Value::Func(_) => {
                    // Method call: swap user_ctx to the object's ctx so the
                    // method body (bound during spec eval) resolves fields.
                    let obj_ctx = Rc::clone(&current_obj.borrow().ctx);
                    let saved = std::mem::replace(&mut env.user_ctx, obj_ctx);
                    let result = dispatch_call(field_val, &field_sym, data, i, env, path_span);
                    env.user_ctx = saved;
                    result
                }
                other => Ok(other),
            };
        }
        // Intermediate part: descend through the field value.
        match field_val {
            Value::Object(inner) => {
                current_obj = inner;
                idx += 1;
            }
            other => {
                // Switch to general traversal for the remaining parts.
                let mut current = other;
                idx += 1;
                while idx < parts.len() {
                    current = step_path(&current, &parts[idx], env, path_span)?;
                    idx += 1;
                }
                return Ok(current);
            }
        }
    }
}

/// Collect positional args + refinement args in spec order (Red's
/// refinement semantics). Walks `fd.params` then `fd.refinements` in
/// declaration order; for each refinement, if it's in `leading_refs` (path
/// tail) or the next inline token is a matching `Value::Refinement`, the
/// refinement is active and its `arity` expressions are collected. Returns
/// the positional `args` and a `RefineArgs` of active refinements + their
/// collected arg values.
///
/// The special-case natives that take their first argument unevaluated
/// (`repeat`/`foreach`/`forall`/`make`/`to` — a word/name, not a value) are
/// honored only for the *positional* params; refinements on those natives
/// aren't supported (none declare any).
fn collect_call_args(
    sym: &Symbol,
    fd: &Rc<FuncDef>,
    leading_refs: &[Symbol],
    data: &std::cell::Ref<Vec<Value>>,
    i: &mut usize,
    env: &mut Env,
    span: Span,
) -> Result<(Vec<Value>, RefineArgs), EvalError> {
    // Variadic natives (e.g. `return`, `print`) collect all remaining
    // expressions up to the next native word. They don't take refinements.
    if fd.variadic {
        let mut args = Vec::new();
        while *i < data.len() && !is_native_word(&data[*i], env) {
            args.push(eval_expression(data, i, env)?);
        }
        return Ok((args, RefineArgs::default()));
    }

    let arity = fd.params.len();
    let mut args: Vec<Value> = Vec::with_capacity(arity);
    let uneval_first = matches!(
        sym.as_str(),
        "repeat" | "foreach" | "forall" | "make" | "to" | "default"
    );

    // Positional params.
    for n in 0..arity {
        if *i >= data.len() {
            return Err(EvalError::Arity {
                native: sym.clone(),
                expected: arity,
                got: args.len(),
                span,
            });
        }
        if n == 0 && uneval_first {
            args.push(data[*i].clone());
            *i += 1;
        } else {
            args.push(eval_expression(data, i, env)?);
        }
    }

    // Refinements in spec order.
    let mut ref_pairs: Vec<(Symbol, Vec<Value>)> = Vec::new();
    for (ref_name, ref_args_spec) in &fd.refinements {
        let already_leading = leading_refs.iter().any(|r| r == ref_name);
        let mut active = already_leading;
        // Inline refinement flag? Peek the next value; if it's a matching
        // Refinement token, consume it and activate.
        if !active {
            if let Some(Value::Refinement { sym: rname, .. }) = data.get(*i) {
                if rname == ref_name {
                    *i += 1;
                    active = true;
                }
            }
        }
        if active {
            let mut collected = Vec::with_capacity(ref_args_spec.len());
            for _ in 0..ref_args_spec.len() {
                if *i >= data.len() {
                    // Refinement-arg exhaustion: name the refinement so the
                    // user knows *which* `/ref` is missing its argument,
                    // rather than a generic positional-arity message.
                    return Err(EvalError::Native {
                        message: format!(
                            "{}: refinement /{} expects {} argument(s), got {}",
                            sym.as_str(),
                            ref_name.as_str(),
                            ref_args_spec.len(),
                            collected.len(),
                        ),
                        span,
                    });
                }
                collected.push(eval_expression(data, i, env)?);
            }
            ref_pairs.push((ref_name.clone(), collected));
        }
    }

    Ok((args, RefineArgs::from_pairs(ref_pairs)))
}

/// Invoke a user-defined function: clone its `FuncDef.ctx` (fresh slot
/// storage per call so recursion is safe), fill param slots in order, fill
/// refinement flag + arg slots, push a `CallFrame`, evaluate the body, then
/// pop the frame. `EvalError::Return(v)` is caught and converted to
/// `Ok(v)` — that's how the `return` native exits a function. Any other
/// error propagates.
///
/// Slot layout (established by `bind_function_body`):
///   `[param_0 ... param_{n-1}] [ref_0_flag] [ref_0_arg_0 ..] [ref_1_flag] ...`
pub(crate) fn call_user_func(
    fd: &Rc<FuncDef>,
    args: Vec<Value>,
    refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let call_ctx = fd.ctx.clone();
    let mut slot = 0;
    for arg in args.iter() {
        call_ctx.set_slot(slot, arg.clone());
        slot += 1;
    }
    // Refinement slots follow param slots in spec order: for each declared
    // refinement, a logic flag slot then its arg-word slots.
    for (ref_name, ref_args_spec) in &fd.refinements {
        let active = refs.has(ref_name);
        call_ctx.set_slot(slot, Value::Logic(active));
        slot += 1;
        if active {
            if let Some(collected) = refs.get(ref_name) {
                for v in collected {
                    call_ctx.set_slot(slot, v.clone());
                    slot += 1;
                }
            }
        } else {
            // Inactive refinement: arg slots default to `none`.
            for _ in 0..ref_args_spec.len() {
                call_ctx.set_slot(slot, Value::None);
                slot += 1;
            }
        }
    }
    env.call_stack.push(crate::CallFrame {
        ctx: call_ctx,
        func: Some(Rc::clone(fd)),
    });
    #[cfg(feature = "stats")]
    {
        env.record_frame_push();
    }
    let body_block = Value::block(fd.body.clone());
    // M29: call the walker's own `eval` directly (NOT `interp::eval` which
    // dispatches on `env.mode`). In VM mode, the top-level body may fall
    // back to the walker (compile error or `needs_rebind`). When the walker
    // evaluates a `func` call, `call_user_func` pushes a `CallFrame` to
    // `env.call_stack` (the walker's frame stack) and evaluates the body.
    // If we dispatched to the VM here, the VM would create its OWN root
    // frame (`vm.frames`) and couldn't see the walker's `CallFrame` — the
    // func body's `Binding::Func(slot)` bindings would resolve to empty
    // VM locals instead of the walker's call context. So function bodies
    // invoked from the walker always run on the walker. In pure VM mode
    // (top-level body compiled successfully), the VM's `CallUser` handler
    // is the path that invokes functions — it uses `vm.frames`, not
    // `env.call_stack`.
    let result = eval(&body_block, env);
    env.call_stack.pop();
    match result {
        Ok(v) => Ok(v),
        Err(EvalError::Return(v)) => Ok(v),
        Err(e) => Err(e),
    }
}

/// True if `v` is an unbound `Word`/`GetWord` whose name is a registered
/// native. Used to stop variadic argument collection at the next native call.
fn is_native_word(v: &Value, env: &Env) -> bool {
    let sym = match v {
        Value::Word { sym, binding, .. } | Value::GetWord { sym, binding, .. } => {
            if !matches!(binding, Binding::Unbound) {
                return false;
            }
            sym
        }
        _ => return false,
    };
    env.natives.contains_key(sym)
}

/// Resolve a `Word`/`GetWord` to its value via the binding, or via the native
/// registry if unbound. `GetWord` shares this path in M5 since no natives are
/// registered yet; M6+ may diverge if `GetWord` should return a function
/// value without invoking it (it does here — invocation is the caller's job).
fn resolve_word(
    sym: &Symbol,
    binding: &Binding,
    env: &mut Env,
    span: Span,
) -> Result<Value, EvalError> {
    match binding {
        Binding::Local(ctx, idx) => Ok(ctx.slot_value(*idx)),
        Binding::Func(idx) => {
            // Function-local slot: read from the current call frame's
            // per-call context clone. `call_stack` is non-empty whenever
            // a function body is being evaluated.
            let frame = env
                .call_stack
                .last()
                .ok_or_else(|| EvalError::UnboundWord {
                    sym: sym.clone(),
                    span,
                })?;
            Ok(frame.ctx.slot_value(*idx))
        }
        Binding::Unbound => {
            if let Some(fd) = env.natives.get(sym) {
                Ok(Value::Func(Rc::clone(fd)))
            } else {
                Err(EvalError::UnboundWord {
                    sym: sym.clone(),
                    span,
                })
            }
        }
        // Lexical bindings are set by the v0.3 compiler (M23) and resolved
        // by the VM (M25); the tree-walker never sees them. If one reaches
        // here it indicates a `bind`/`use`/`do`-on-data block that should
        // have been routed to the walker — surface as a clear runtime error
        // rather than silently misresolving.
        Binding::Lexical(_, _) => Err(EvalError::Native {
            message: format!(
                "lexical binding for {:?} not yet supported in the tree-walker",
                sym.as_str()
            ),
            span,
        }),
    }
}

/// Write `val` into the slot bound to a `SetWord`. For M5 all top-level
/// `SetWord`s are bound by `bind_pass`, so the `Unbound` arm only fires for
/// malformed trees and surfaces as an error (runtime slot allocation on a
/// shared `Rc<Context>` would require a `RefCell` name map, deferred).
fn write_setword(
    sym: &Symbol,
    binding: &Binding,
    val: Value,
    env: &mut Env,
    span: Span,
) -> Result<(), EvalError> {
    match binding {
        Binding::Local(ctx, idx) => {
            ctx.set_slot(*idx, val);
            Ok(())
        }
        Binding::Func(idx) => {
            // Write to the current call frame's function-local slot.
            let frame = env
                .call_stack
                .last()
                .ok_or_else(|| EvalError::UnboundWord {
                    sym: sym.clone(),
                    span,
                })?;
            frame.ctx.set_slot(*idx, val);
            Ok(())
        }
        Binding::Unbound => Err(EvalError::UnboundWord {
            sym: sym.clone(),
            span,
        }),
        // See `resolve_word`: lexical bindings are VM-only; the walker never
        // writes through one. Surface as an error if reached.
        Binding::Lexical(_, _) => Err(EvalError::Native {
            message: format!(
                "lexical binding for {:?} not yet supported in the tree-walker",
                sym.as_str()
            ),
            span,
        }),
    }
}
