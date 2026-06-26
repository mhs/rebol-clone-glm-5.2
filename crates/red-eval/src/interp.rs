//! Tree-walking evaluator: binding pass + `eval`.
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

use red_core::lexer;
use red_core::parser::parse_program;
use red_core::value::{Binding, FuncDef, Series, Span, Symbol, Value};
use red_core::{Context, Env, Error, EvalError, EvalMode, RefineArgs};

use crate::binding::{bind_pass, has_foreign_bindings};

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
    // `bind`/`use` may rebind words to a child context the VM can't lexically
    // address. Detect that and route to the walker.
    if has_foreign_bindings(&series, &env.user_ctx) {
        return eval(block, env);
    }
    // Compile-on-demand. The M23 analyzer marks `use`/object forms
    // `needs_rebind`; such blocks return a stub `[Halt]` from `compile_block`,
    // so we check the flag and fall back to the walker.
    let registry = crate::vm::compiler::NativeRegistry::from_env(env);
    let mut scope = crate::vm::lex::Scope::root(&env.user_ctx);
    match crate::vm::compiler::compile_block(&series, &mut scope, &registry) {
        Ok(compiled) if !compiled.needs_rebind => crate::vm::run(compiled, env),
        _ => eval(block, env),
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
    if env.mode == EvalMode::Walk
        || has_foreign_bindings(&series, &env.user_ctx)
    {
        return reduce_walker(&series, env);
    }
    let registry = crate::vm::compiler::NativeRegistry::from_env(env);
    let mut scope = crate::vm::lex::Scope::root(&env.user_ctx);
    match crate::vm::compiler::compile_block_reduce(&series, &mut scope, &registry) {
        Ok(compiled) if !compiled.needs_rebind => crate::vm::run_reduce(compiled, env),
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
        | Value::String8(_)
        | Value::LitWord { .. }
        | Value::Block { .. }
        | Value::Func(_)
        | Value::Refinement { .. }
        | Value::Error(_)
        | Value::File { .. }
        | Value::Url { .. }
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
    f(&args, &refs, env)
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
/// path parts. Out-of-range returns `none`. Strings defer char! support
/// (plan: stub error until char! exists) — but we return the char as an
/// integer for POC parity with `pick` on strings if `pick` does that.
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
            // POC: string char pick deferred until char! exists. Return the
            // codepoint as an integer so `s/2` yields a usable value (mirrors
            // `pick` on strings, which the series.rs implementation also
            // returns as integers for POC).
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
            Ok(Value::integer(chars[idx] as i64))
        }
        other => Err(EvalError::TypeError {
            expected: "block!, paren!, or string! for integer path index",
            found: crate::natives::type_name(other),
            span: path_span,
        }),
    }
}

/// `:foo/bar` — resolve the head and walk the tail, returning the final
/// value *without* invoking any function encountered. Same as `walk_data_path`
/// except the head is resolved as a `GetWord` (reads the slot / fetches the
/// native Func without dispatching).
pub(crate) fn eval_get_path(parts: &[Value], path_span: Span, env: &mut Env) -> Result<Value, EvalError> {
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
    write_path_slot(&current, final_part, rhs, env, path_span)
}

/// Write `rhs` into the slot identified by `part` on `current`. The final
/// write target:
/// - Object + Word → set object slot.
/// - Block + Integer → `poke`-style 1-based (negative from tail) write into
///   shared storage.
/// - String + Integer → poke char (deferred; error).
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
                message: "string char poke deferred until char! type exists".into(),
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
///   `[param_0 .. param_{n-1}] [ref_0_flag] [ref_0_arg_0 ..] [ref_1_flag] ...`
fn call_user_func(
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
    env.call_stack.push(crate::context::CallFrame {
        ctx: call_ctx,
        func: Some(Rc::clone(fd)),
    });
    #[cfg(feature = "stats")]
    {
        env.record_frame_push();
    }
    let body_block = Value::Block {
        series: fd.body.clone(),
        span: Span::new(0, 0),
    };
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

/// End-to-end: lex → parse → bind → eval. Handles both bare bodies and
/// `Red [...] <body>` programs (the header is discarded for the POC).
pub fn run_source(src: &str) -> Result<Value, Error> {
    run_source_with_output(src, Box::new(std::io::stdout()))
}

/// Like `run_source` but with a custom output sink. Used by golden program
/// tests to capture native output into an in-memory buffer.
pub fn run_source_with_output(src: &str, out: Box<dyn std::io::Write>) -> Result<Value, Error> {
    let tokens = lexer::lex(src)?;
    let body = if tokens.is_empty() {
        Series::empty()
    } else {
        let (_header, body) = parse_program(&tokens)?;
        body
    };
    run_series_with_output(body, out)
}

/// End-to-end run that also returns the requested exit code (from `exit`/
/// `quit`). Mirrors `run_source` but yields `(last_value, exit_code)`. Used
/// by the CLI to propagate the script's exit status to the process.
pub fn run_source_with_exit(src: &str) -> Result<(Value, i32), Error> {
    run_source_with_exit_output(src, Box::new(std::io::stdout()))
}

/// Like `run_source_with_exit` but with a custom output sink.
pub fn run_source_with_exit_output(
    src: &str,
    out: Box<dyn std::io::Write>,
) -> Result<(Value, i32), Error> {
    run_source_with_exit_opts(src, out, &RunOptions::default())
}

/// CLI run options: `allow_shell` mirrors `Env::allow_shell` (off by default
/// per the M20 sandbox policy), `args` populates `system/options/args` for
/// script access to trailing CLI args.
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    pub allow_shell: bool,
    pub args: Vec<String>,
}

/// Like `run_source_with_exit_output` but applies CLI `RunOptions` (allow-shell
/// flag + trailing args exposed as `system/options/args`) before eval.
pub fn run_source_with_exit_opts(
    src: &str,
    out: Box<dyn std::io::Write>,
    opts: &RunOptions,
) -> Result<(Value, i32), Error> {
    let tokens = lexer::lex(src)?;
    let body = if tokens.is_empty() {
        Series::empty()
    } else {
        let (_header, body) = parse_program(&tokens)?;
        body
    };
    run_series_with_exit_opts(body, out, opts)
}

/// Like `run_series_with_exit_output` but applies CLI `RunOptions`.
pub fn run_series_with_exit_opts(
    body: Series,
    out: Box<dyn std::io::Write>,
    opts: &RunOptions,
) -> Result<(Value, i32), Error> {
    run_series_inner_opts(body, out, opts)
}

/// Evaluate an already-parsed body series with a fresh environment.
/// Constants (`none`/`true`/`false`/`newline`) are installed into the user
/// context before the binding pass, and natives (`print`/`prin`/`probe`) are
/// registered before eval.
pub fn run_series(body: Series) -> Result<Value, Error> {
    run_series_with_output(body, Box::new(std::io::stdout()))
}

/// Like `run_series` but with a custom output sink.
pub fn run_series_with_output(body: Series, out: Box<dyn std::io::Write>) -> Result<Value, Error> {
    Ok(run_series_inner(body, out)?.0)
}

/// Like `run_series` but returns the exit code from `exit`/`quit`. The CLI
/// uses this to set the process exit status.
pub fn run_series_with_exit_output(
    body: Series,
    out: Box<dyn std::io::Write>,
) -> Result<(Value, i32), Error> {
    run_series_inner(body, out)
}

/// Shared core: runs the body, catching `EvalError::Quit(code)` as a normal
/// termination with the given exit code. Other errors propagate as `Error`.
fn run_series_inner(body: Series, out: Box<dyn std::io::Write>) -> Result<(Value, i32), Error> {
    run_series_inner_opts(body, out, &RunOptions::default())
}

/// Shared core with CLI options: installs constants/natives, applies
/// `allow_shell` and populates `system/options/args`, then evaluates.
fn run_series_inner_opts(
    body: Series,
    out: Box<dyn std::io::Write>,
    opts: &RunOptions,
) -> Result<(Value, i32), Error> {
    let ctx = Context::new();
    crate::natives::install_constants(&ctx);
    let ctx_rc = bind_pass(&body, ctx);
    let mut env = Env::new_with_output(ctx_rc, out);
    crate::natives::register_natives(&mut env);
    env.allow_shell = opts.allow_shell;
    #[cfg(feature = "stats")]
    {
        env.reset_stats();
    }
    // Populate system/options/args from CLI args.
    if !opts.args.is_empty() {
        let args_block = Series::new(opts.args.iter().map(|a| Value::string(a.clone())).collect());
        if let Some(Value::Object(sys)) = env.user_ctx.get(&Symbol::new("system")) {
            if let Some(Value::Object(opts_obj)) = sys.borrow().ctx.get(&Symbol::new("options")) {
                opts_obj
                    .borrow()
                    .ctx
                    .set(Symbol::new("args"), Value::block(args_block));
            }
        }
    }
    let block = Value::Block {
        series: body,
        span: Span::new(0, 0),
    };
    match eval(&block, &mut env) {
        Ok(v) => Ok((v, 0)),
        Err(EvalError::Quit(code)) => Ok((Value::None, code)),
        Err(e) => Err(Error::Eval(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use red_core::printer::mold_to_string;

    fn run(src: &str) -> Value {
        run_source(src).expect("run_source failed")
    }

    fn run_err(src: &str) -> Error {
        run_source(src).expect_err("expected error")
    }

    #[test]
    fn integer_literal() {
        assert_eq!(mold_to_string(&run("5")), "5");
    }

    #[test]
    fn setword_then_word() {
        assert_eq!(mold_to_string(&run("foo: 5 foo")), "5");
    }

    #[test]
    fn unbound_word_errors() {
        let err = run_err("foo");
        assert!(matches!(err, Error::Eval(EvalError::UnboundWord { .. })));
    }

    #[test]
    fn paren_eager() {
        // Paren walks eagerly; last value is the result.
        assert_eq!(mold_to_string(&run("(1 2 3)")), "3");
    }

    #[test]
    fn block_returns_as_data() {
        assert_eq!(mold_to_string(&run("[1 2 3]")), "[1 2 3]");
    }

    #[test]
    fn setword_returns_written_value() {
        // `foo: 5` itself evaluates to 5 (Red semantics).
        assert_eq!(mold_to_string(&run("foo: 5")), "5");
    }

    #[test]
    fn nested_block_data_preserved() {
        // Blocks inside the body are data, not walked.
        assert_eq!(mold_to_string(&run("[a [b c] d]")), "[a [b c] d]");
    }

    #[test]
    fn setword_then_word_in_nested_block_data() {
        // The inner `[foo]` is data here; the outer eval doesn't enter it.
        // `foo: 5 [foo]` returns the block `[foo]` (last value of the body).
        assert_eq!(mold_to_string(&run("foo: 5 [foo]")), "[foo]");
    }

    #[test]
    fn word_in_paren_resolves() {
        // Paren is walked eagerly, so `foo` inside resolves to 5.
        assert_eq!(mold_to_string(&run("foo: 5 (foo)")), "5");
    }

    #[test]
    fn multiple_assignments() {
        assert_eq!(mold_to_string(&run("a: 1 b: 2 a")), "1");
        assert_eq!(mold_to_string(&run("a: 1 b: 2 b")), "2");
    }

    #[test]
    fn getword_reads_slot() {
        assert_eq!(mold_to_string(&run("foo: 7 :foo")), "7");
    }

    #[test]
    fn litword_returns_as_data() {
        assert_eq!(mold_to_string(&run("'foo")), "'foo");
    }

    #[test]
    fn empty_source_returns_none() {
        assert_eq!(mold_to_string(&run("")), "none");
    }

    #[test]
    fn header_program_evaluates_body() {
        assert_eq!(mold_to_string(&run("Red [] foo: 42 foo")), "42");
    }

    #[test]
    fn setword_at_eof_errors() {
        let err = run_err("foo:");
        assert!(matches!(
            err,
            Error::Eval(EvalError::Arity { native, expected: 1, got: 0, .. })
            if native.as_str() == "foo"
        ));
    }

    // --- Milestone 7: expression / infix evaluation ---

    #[test]
    fn infix_addition() {
        assert_eq!(mold_to_string(&run("1 + 2")), "3");
    }

    #[test]
    fn infix_left_to_right_no_precedence() {
        // Red: `1 + 2 * 3` = `(1 + 2) * 3` = 9.
        assert_eq!(mold_to_string(&run("1 + 2 * 3")), "9");
    }

    #[test]
    fn setword_rhs_is_full_expression() {
        assert_eq!(mold_to_string(&run("x: 1 + 2 x")), "3");
    }

    #[test]
    fn native_arg_is_full_expression() {
        // `print 1 + 2` should pass 3 to print. The block's last value is
        // print's return: none.
        assert_eq!(mold_to_string(&run("print 1 + 2")), "none");
    }

    // --- M19: real paths ---

    #[test]
    fn block_path_integer_select() {
        assert_eq!(mold_to_string(&run("b: [10 20 30] b/2")), "20");
    }

    #[test]
    fn block_path_negative_index() {
        assert_eq!(mold_to_string(&run("b: [10 20 30] b/-1")), "30");
    }

    #[test]
    fn block_path_out_of_range_returns_none() {
        assert_eq!(mold_to_string(&run("b: [10 20 30] b/9")), "none");
    }

    #[test]
    fn object_path_field_access() {
        assert_eq!(mold_to_string(&run("o: make object! [a: 1] o/a")), "1");
    }

    #[test]
    fn object_set_path_writes_field() {
        assert_eq!(
            mold_to_string(&run("o: make object! [a: 1] o/a: 5 o/a")),
            "5"
        );
    }

    #[test]
    fn object_set_path_returns_rhs() {
        assert_eq!(mold_to_string(&run("o: make object! [a: 1] o/a: 5")), "5");
    }

    #[test]
    fn nested_object_path_through_graph() {
        let src = "o: make object! [inner: make object! [x: 42]] o/inner/x";
        assert_eq!(mold_to_string(&run(src)), "42");
    }

    #[test]
    fn nested_object_set_path() {
        let src = "o: make object! [inner: make object! [x: 0]] o/inner/x: 99 o/inner/x";
        assert_eq!(mold_to_string(&run(src)), "99");
    }

    #[test]
    fn block_set_path_writes_slot() {
        // Block-integer set-paths (`b/2: 99`) require lexer support for
        // `2:` (a number followed by a colon), which is not in this POC.
        // Object-field set-paths work (see `object_set_path_writes_field`).
        // Use `poke` for block slot writes instead.
        assert_eq!(mold_to_string(&run("b: [1 2 3] poke b 2 99 b/2")), "99");
    }

    #[test]
    fn get_path_returns_value_without_calling() {
        // `:obj/method` returns the function value, not the result of calling it.
        let src = "o: make object! [f: does [42]] :o/f";
        let v = run(src);
        assert!(matches!(v, Value::Func(_)));
    }

    #[test]
    fn lit_path_returns_as_data() {
        let v = run("'foo/bar");
        match v {
            Value::LitPath { parts, .. } => {
                assert_eq!(parts.len(), 2);
            }
            other => panic!("expected LitPath, got {other:?}"),
        }
    }

    #[test]
    fn path_with_paren_part_evaluates_paren() {
        // `foo/(2)/bar` — the paren evaluates to 2, then... we need a block
        // at `foo` to index. Use a block-typed word.
        assert_eq!(
            mold_to_string(&run("b: [[100 200] [300 400]] b/(1 + 1)/2")),
            "400"
        );
    }

    #[test]
    fn path_paren_evaluated_for_index() {
        // `b/(1 + 1)` evaluates the paren to 2, then picks index 2.
        assert_eq!(mold_to_string(&run("b: [10 20 30] b/(1 + 1)")), "20");
    }

    #[test]
    fn string_path_integer_returns_codepoint() {
        // POC: string char pick returns the codepoint as an integer (char!
        // deferred).
        assert_eq!(mold_to_string(&run("s: \"abc\" s/2")), "98");
    }

    #[test]
    fn object_path_with_block_field_then_index() {
        // `obj/items/2` — object field is a block, then integer index.
        let src = "o: make object! [items: [10 20 30]] o/items/2";
        assert_eq!(mold_to_string(&run(src)), "20");
    }

    #[test]
    fn object_method_call_with_args_via_path() {
        let src = "o: make object! [add: func [x y][x + y]] o/add 3 4";
        assert_eq!(mold_to_string(&run(src)), "7");
    }
}
