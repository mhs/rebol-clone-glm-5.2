//! Word binding: top-level `bind_pass` + function-body binding (M9).
//!
//! Binding attaches a `Binding` to every `Word`/`SetWord`/`GetWord` so the
//! evaluator can resolve a word to a slot without a runtime name lookup. Two
//! kinds of contexts are involved:
//!
//! - **User context** (`Env.user_ctx`): the single top-level script context.
//!   Script-level `SetWord`s, loop variables, and words referenced by
//!   `set`/`get`/`value?`/`use` get `Binding::Local(user_ctx, idx)` here.
//!
//! - **Function-local context** (`FuncDef.ctx`): a fresh `Context` per
//!   `func`/`does` value. Params and body-local `SetWord`s get
//!   `Binding::Func(idx)` here, resolved at call time via
//!   `env.call_stack.last().ctx` (the per-call clone). Body words that
//!   reference *outer* user-context names (e.g. a recursive self-reference,
//!   or a global) get `Binding::Local(user_ctx, idx)` so they still resolve
//!   while the user context is shadowed by the call frame.
//!
//! Closures (function values that capture a fresh per-call frame with their
//! definition context as parent) are implemented via `Value::Closure` (M60):
//! the `closure` native / `Instr::MakeClosure` snapshots freevar values into a
//! `ClosureDef.captures` cell at creation time. `Binding::Closure(idx)` on
//! body freevar words resolves to that cell via the active call frame's
//! `captures` Vec. `func`/`does`/`function` keep their shallow per-call clone
//! semantics (back-compat with v0.2–v0.4 golden fixtures).
//!
//! `in context 'word` is also deferred: it returns a word bound to a context
//! value, but objects (`make object!`) — the only context-producing construct
//! in Red — are out of scope for the POC. With only the user context + func
//! contexts available, `in` has nothing to bind to that `bind`/`use`/`get`/
//! `set` don't already cover.

use std::rc::Rc;

use red_core::context::Context;
use red_core::value::{Binding, FuncDef, Series, Symbol, Value};

// ---------------------------------------------------------------------------
// Top-level binding pass (script body -> user context)
// ---------------------------------------------------------------------------

/// Walk `body` and attach `Binding::Local` to every word whose name matches a
/// slot allocated for a `SetWord`, a `repeat`/`foreach`/`forall` loop variable,
/// or a word operand of `set`/`get`/`value?`/`use`. Recurses into nested
/// `Block`/`Paren` contents so that words inside data blocks are also bound
/// (matches Red semantics: `foo: 5 [foo]` later `do`ne yields `[5]`).
///
/// Returns the `Rc<Context>` shared by all attached bindings. The caller
/// installs it into `Env.user_ctx` so eval-time writes flow through the same
/// slots.
pub fn bind_pass(body: &Series, user_ctx: Context) -> Rc<Context> {
    let ctx_rc = Rc::new(user_ctx);
    bind_pass_into(body, &ctx_rc);
    ctx_rc
}

/// Like `bind_pass` but grows an *existing* shared context in place instead
/// of consuming an owned one. Used by the REPL to bind each new line's
/// SetWords/loop-vars/parse-captures against the live `Env.user_ctx` so
/// state persists across lines and functions defined earlier still see
/// later mutations to globals (the binding's `Rc<Context>` is the same
/// pointer the prior line's bindings already reference).
pub fn bind_pass_into(body: &Series, user_ctx: &Rc<Context>) {
    collect_setwords(body, user_ctx);
    collect_loop_vars(body, user_ctx);
    collect_parse_words(body, user_ctx);
    attach_local_bindings(body, user_ctx);
}

/// True if `data[i]` begins a `use [words] body` form (an unbound `use` word
/// followed by a block). Returns the index of the body block (`i + 2`) when
/// it does, so callers can skip allocating/rebinding the body's words into
/// the outer context (use locals are scoped to the child context built at
/// runtime by `use_native`).
pub(crate) fn use_body_index(data: &[Value], i: usize) -> Option<usize> {
    let Value::Word {
        sym,
        binding: Binding::Unbound,
        ..
    } = &data[i]
    else {
        return None;
    };
    if sym.as_str() == "use" && i + 2 < data.len() && matches!(&data[i + 1], Value::Block { .. }) {
        Some(i + 2)
    } else {
        None
    }
}

/// If `data[i]` begins a `func`/`function`/`does`/`closure`/`module` form,
/// return the number of values the form consumes (so callers can skip the
/// body block and avoid collecting its SetWords into the outer context).
/// The body's locals are scoped to the function-local or module-local context
/// built at runtime by `func_native`/`function_native`/`does_native`/
/// `closure_native`/`module_native` via `bind_function_body`/
/// `bind_pass_into`.
///
/// Forms:
/// - `func [spec] [body]` / `function [spec] [body]` / `closure [spec] [body]`
///   → consumes 3 values.
/// - `does [body]` → consumes 2 values.
/// - `module [body]` → consumes 2 values. (M62)
/// - `module 'name [body]` → consumes 3 values. (M62)
///
/// `make function! [...]` / `make object! [...]` are handled at eval time
/// (not bare word forms) and are not skipped here — their body SetWords are
/// collected, but since the body is deep-cloned and bound at `make` time
/// this is harmless. (M62 note: `make object!` body SetWords do leak into
/// `user_ctx` as `none`-valued slots; this is pre-existing M18 behavior and
/// not changed by M62. The `module` native does not have this issue because
/// `func_form_skip` skips its body here.)
pub(crate) fn func_form_skip(data: &[Value], i: usize) -> Option<usize> {
    let Value::Word {
        sym,
        binding: Binding::Unbound,
        ..
    } = &data[i]
    else {
        return None;
    };
    match sym.as_str() {
        "func" | "function" | "closure" => {
            // `func [spec] [body]` — need at least 3 values (word + spec + body).
            if i + 2 < data.len()
                && matches!(&data[i + 1], Value::Block { .. })
                && matches!(&data[i + 2], Value::Block { .. })
            {
                Some(3)
            } else {
                None
            }
        }
        "does" => {
            // `does [body]` — need at least 2 values.
            if i + 1 < data.len() && matches!(&data[i + 1], Value::Block { .. }) {
                Some(2)
            } else {
                None
            }
        }
        "module" => {
            // M62: `module [body]` (2 values) or `module 'name [body]` (3
            // values). The name is a word-family literal (Word/GetWord/
            // LitWord/SetWord). Skipping the body here prevents its
            // SetWords from being pre-allocated in the outer user_ctx —
            // they belong to the module's own ctx (allocated at eval time
            // by `build_module` → `bind_pass_into`). Without this skip,
            // `resolve_word`'s M62 `user_ctx` fallback would resolve bare
            // references to private module words as `none` (the leaked
            // pre-allocated slot value) instead of erroring `UnboundWord`.
            if i + 1 < data.len() && matches!(&data[i + 1], Value::Block { .. }) {
                Some(2)
            } else if i + 2 < data.len()
                && matches!(
                    &data[i + 1],
                    Value::Word { .. }
                        | Value::GetWord { .. }
                        | Value::LitWord { .. }
                        | Value::SetWord { .. }
                )
                && matches!(&data[i + 2], Value::Block { .. })
            {
                Some(3)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Phase 1: allocate a slot in `ctx` for every `SetWord` encountered anywhere
/// in the tree — *except* inside `use` bodies, whose locals are scoped to the
/// child context built at runtime by `use_native`. The slots are populated
/// during eval, not here.
pub(crate) fn collect_setwords(series: &Series, ctx: &Context) {
    collect_setwords_inner(series, ctx, None);
}

/// Like `collect_setwords` but skips SetWords whose name is already in
/// `shadow` (the user/definition context). Used by `bind_function_body` so
/// that body SetWords naming outer words (e.g. object fields) are NOT
/// allocated as function-locals — they resolve to the outer context instead.
fn collect_setwords_inner(series: &Series, ctx: &Context, shadow: Option<&Context>) {
    let data = series.data.borrow();
    let n = data.len();
    let mut i = 0;
    while i < n {
        if use_body_index(&data, i).is_some() {
            i += 3;
            continue;
        }
        if let Some(skip) = func_form_skip(&data, i) {
            i += skip;
            continue;
        }
        match &data[i] {
            Value::SetWord { sym, .. } => {
                let in_shadow = shadow.map(|s| s.has(sym)).unwrap_or(false);
                if !in_shadow {
                    ctx.slot_index(sym.clone());
                }
                i += 1;
            }
            Value::Block { series: s, .. } | Value::Paren { series: s, .. } => {
                let child = s.clone();
                collect_setwords_inner(&child, ctx, shadow);
                i += 1;
            }
            _ => i += 1,
        }
    }
}

/// Phase 1b: allocate a slot for every word introduced as a loop variable by
/// `repeat`, `foreach`, `forall`, `for`, or `forskip`. Each is recognized in
/// either of two forms:
/// - `repeat 'i <count> <body>`  (lit-word counter, Red canonical form)
/// - `repeat i <count> <body>`   (bare-word counter, accepted by the POC)
/// - `foreach 'word <series> <body>` / `forall 'word <series> <body>`
/// - `for word <start> <end> <bump> <body>` (word at i+1)
/// - `forskip 'word <series> <size> <body>` (word at i+1)
///
/// The lit-word/bare-word value itself is *not* a SetWord, so without this
/// pass the loop name would never get a slot and body references would
/// resolve as unbound. Recurses into nested `Block`/`Paren`, skipping `use`
/// bodies (loop vars inside a use are use-locals).
pub(crate) fn collect_loop_vars(series: &Series, ctx: &Context) {
    let data = series.data.borrow();
    let n = data.len();
    let mut i = 0;
    while i < n {
        if use_body_index(&data, i).is_some() {
            i += 3;
            continue;
        }
        if let Some(skip) = func_form_skip(&data, i) {
            i += skip;
            continue;
        }
        match &data[i] {
            Value::Word {
                sym,
                binding: Binding::Unbound,
                ..
            } if matches!(
                sym.as_str(),
                "repeat" | "foreach" | "forall" | "for" | "forskip"
            ) =>
            {
                if i + 1 < n {
                    // `foreach [k v] series body` — block word-list form:
                    // allocate a slot for each word in the block.
                    if let Value::Block { series: ws, .. } | Value::Paren { series: ws, .. } =
                        &data[i + 1]
                    {
                        let wd = ws.data.borrow();
                        for w in wd.iter().skip(ws.index) {
                            if let Some(sym) = loop_word_name(w) {
                                ctx.slot_index(sym);
                            }
                        }
                    } else {
                        // Single-word form: `foreach 'word series body`.
                        let name = loop_word_name(&data[i + 1]);
                        if let Some(sym) = name {
                            ctx.slot_index(sym);
                        }
                    }
                }
                i += 1;
            }
            Value::Block { series: s, .. } | Value::Paren { series: s, .. } => {
                let child = s.clone();
                collect_loop_vars(&child, ctx);
                i += 1;
            }
            _ => i += 1,
        }
    }
}

/// Phase 1c: allocate a slot for every word introduced as a capture target by
/// the `parse` dialect's `copy 'word rule` / `set 'word rule` forms. The
/// `parse` native runs at the script level and writes captures into the
/// **user context** (per the project brief), so each `copy`/`set` operand
/// must already have a slot when the native runs — `env.user_ctx` is a shared
/// `Rc<Context>` and can't allocate at runtime.
///
/// Recognizes `parse <input> <rules-block>` (unbound `parse` word followed by
/// any input value and a block of rules), then walks the rules block looking
/// for `copy <word> <rule>` / `set <word> <rule>` patterns. The operand may
/// be a `LitWord` (`'w`) or a bare unbound `Word` (`w`). Sub-rule blocks
/// (`[...]` groups inside the rules) are recursed into; `(...)` side-effects
/// are *not* (they are Red code, not parse rules). `use` bodies are skipped
/// (matches `collect_loop_vars`/`collect_setwords` scoping).
pub(crate) fn collect_parse_words(series: &Series, ctx: &Context) {
    let data = series.data.borrow();
    let n = data.len();
    let mut i = 0;
    while i < n {
        if use_body_index(&data, i).is_some() {
            i += 3;
            continue;
        }
        if let Some(skip) = func_form_skip(&data, i) {
            i += skip;
            continue;
        }
        // Detect `parse <input> <rules-block>`: the unbound `parse` word is
        // followed by any input value and a rules block. Walk the rules
        // block for `copy`/`set` capture operands.
        if let Value::Word {
            sym,
            binding: Binding::Unbound,
            ..
        } = &data[i]
        {
            if sym.as_str() == "parse" && i + 2 < n {
                if let Value::Block { series: rules, .. } = &data[i + 2] {
                    let rules_clone = rules.clone();
                    collect_parse_capture_words(&rules_clone, ctx);
                    i += 3;
                    continue;
                }
            }
        }
        // Recurse into nested blocks/parens so `parse` forms nested inside
        // other blocks/parens are also found.
        match &data[i] {
            Value::Block { series: s, .. } | Value::Paren { series: s, .. } => {
                let child = s.clone();
                collect_parse_words(&child, ctx);
            }
            _ => {}
        }
        i += 1;
    }
}

/// Inner walker for `collect_parse_words`: scans a parse rules block and
/// allocates a slot for each `copy`/`set` operand. Recurses into `[...]`
/// sub-rule groups but not into `(...)` Red side-effects.
fn collect_parse_capture_words(series: &Series, ctx: &Context) {
    let data = series.data.borrow();
    let n = data.len();
    let mut i = 0;
    while i < n {
        if let Value::Word {
            sym,
            binding: Binding::Unbound,
            ..
        } = &data[i]
        {
            if matches!(sym.as_str(), "copy" | "set" | "collect" | "into") && i + 1 < n {
                if let Some(name) = loop_word_name(&data[i + 1]) {
                    ctx.slot_index(name);
                }
                // Skip the operand; the following rule (1+ values) is walked
                // normally below — its sub-blocks may contain nested
                // copy/set/collect/into forms we still want to find.
                // For `collect into 'word rule`, also skip the `into` word.
                if sym.as_str() == "collect"
                    && i + 2 < n
                    && matches!(
                        &data[i + 1],
                        Value::Word {
                            sym,
                            binding: Binding::Unbound,
                            ..
                        } if sym.as_str() == "into"
                    )
                {
                    // `collect into 'word rule` — skip `into`, 'word.
                    i += 3;
                } else {
                    i += 2;
                }
                continue;
            }
        }
        // Recurse into `[...]` sub-rule groups. Parens are Red code — skip.
        if let Value::Block { series: s, .. } = &data[i] {
            let child = s.clone();
            collect_parse_capture_words(&child, ctx);
        }
        i += 1;
    }
}

/// Phase 2: for every `Word`/`SetWord`/`GetWord` whose name is now in `ctx`,
/// replace its `binding` with `Binding::Local(Rc::clone(ctx), idx)`. Words
/// with no matching slot stay `Unbound` (function locals / natives resolved
/// at eval time).
fn attach_local_bindings(series: &Series, ctx: &Rc<Context>) {
    let mut data = series.data.borrow_mut();
    for i in 0..data.len() {
        match &mut data[i] {
            Value::Block { series, .. } | Value::Paren { series, .. } => {
                let child = series.clone();
                // Recurse into the child series — a different `RefCell`, so
                // the outer `borrow_mut` above stays valid.
                attach_local_bindings(&child, ctx);
            }
            Value::Word { sym, binding, .. }
            | Value::SetWord { sym, binding, .. }
            | Value::GetWord { sym, binding, .. } => {
                if let Some(idx) = ctx.index_of(sym) {
                    *binding = Binding::Local(Rc::clone(ctx), idx);
                }
            }
            // Path family: bind only the head word (the function or object
            // being navigated). Tail parts are refinement/field names looked
            // up by symbol at eval time, not variables.
            Value::Path { parts, .. }
            | Value::GetPath { parts, .. }
            | Value::LitPath { parts, .. }
            | Value::SetPath { parts, .. } => {
                if let Some(Value::Word { sym, binding, .. })
                | Some(Value::GetWord { sym, binding, .. }) = parts.first_mut()
                {
                    if let Some(idx) = ctx.index_of(sym) {
                        *binding = Binding::Local(Rc::clone(ctx), idx);
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Function-body binding pass (func/does -> FuncDef.ctx + outer user_ctx)
// ---------------------------------------------------------------------------

/// Bind a freshly-constructed `FuncDef`'s body. Allocates param slots (in
/// order, so param 0 = slot 0), collects body-local `SetWord`s and loop vars
/// into `fd.ctx`, then attaches bindings:
/// - params + body locals → `Binding::Func(idx)` (resolved via the call frame
///   at call time).
/// - outer user-context references (e.g. the function's own name for
///   recursion, or globals) → `Binding::Local(user_ctx, idx)`.
/// - everything else → `Unbound` (native lookup at call time).
///
/// `user_ctx` is the context active at the point of `func`/`does` invocation
/// — used so the body can see outer words (recursion, globals).
pub fn bind_function_body(fd: &mut FuncDef, user_ctx: &Rc<Context>) {
    // 1. Param slots, in order. The call shim fills slot `i` with arg `i`.
    for p in fd.params.iter() {
        fd.ctx.slot_index(p.clone());
    }
    // 2. Refinement slots, in spec order: for each refinement a flag slot
    //    (named by the refinement word, holds a `logic!` at call time) then
    //    one slot per refinement argument word. The call shim
    //    (`call_user_func`) fills these in the same order. Body references
    //    to the refinement word or its arg words bind to these `Func` slots.
    for (ref_name, ref_args) in &fd.refinements {
        fd.ctx.slot_index(ref_name.clone());
        for arg in ref_args {
            fd.ctx.slot_index(arg.clone());
        }
    }
    // 3. Explicit `<local>` words (M16 `function`): slots after params +
    //    refinements so `call_user_func` (which fills params then refinement
    //    slots in order) leaves them as `none` defaults.
    for local in &fd.locals {
        fd.ctx.slot_index(local.clone());
    }
    // 4. Body-local SetWords + loop vars become function-local slots.
    //    Pass `user_ctx` as shadow so SetWords naming outer words (e.g. object
    //    fields when the func is a method) are NOT allocated as function-locals
    //    — they resolve to the outer context via `attach_func_bindings`.
    collect_setwords_inner(&fd.body, &fd.ctx, Some(user_ctx));
    collect_loop_vars(&fd.body, &fd.ctx);
    // 5. Attach bindings: function-local first, then outer user-ctx refs.
    attach_func_bindings(&fd.body, &fd.ctx, user_ctx);
    // 6. M27: defensively clear the construction-time compiled hint. The body
    //    bindings just changed, so any previously-compiled form is stale. In
    //    the common case this runs at func-creation time (before any VM cache
    //    entry exists), so it's a no-op; it exists for correctness against
    //    future re-bind paths. Callers with `&mut Env` should also call
    //    `Env::invalidate_func_cache` to clear the authoritative cache entry.
    fd.invalidate_compiled();
}

/// Bind a `use` body's words to the child context: words whose names are in
/// `child_ctx` (the listed locals + body SetWords + loop vars collected by
/// `use_native`) get `Binding::Local(child_ctx, idx)`; remaining words that
/// match the outer `user_ctx` get `Binding::Local(user_ctx, idx)`; the rest
/// stay `Unbound`. Operates on a *deep-cloned* series so the original source
/// tree isn't mutated. Mirrors `attach_func_bindings` but uses `Local`
/// (not `Func`) for the local context — `use` swaps `env.user_ctx` to the
/// child rather than pushing a call frame.
pub(crate) fn attach_use_bindings(
    series: &Series,
    child_ctx: &Rc<Context>,
    user_ctx: &Rc<Context>,
) {
    let mut data = series.data.borrow_mut();
    attach_use_inner(&mut data, child_ctx, user_ctx);
}

fn attach_use_inner(data: &mut [Value], child_ctx: &Rc<Context>, user_ctx: &Rc<Context>) {
    for v in data.iter_mut() {
        match v {
            Value::Block { series, .. } | Value::Paren { series, .. } => {
                let mut child_data = series.data.borrow_mut();
                attach_use_inner(&mut child_data, child_ctx, user_ctx);
            }
            Value::Word { sym, binding, .. }
            | Value::SetWord { sym, binding, .. }
            | Value::GetWord { sym, binding, .. } => {
                if let Some(idx) = child_ctx.index_of(sym) {
                    *binding = Binding::Local(Rc::clone(child_ctx), idx);
                } else if let Some(idx) = user_ctx.index_of(sym) {
                    *binding = Binding::Local(Rc::clone(user_ctx), idx);
                }
            }
            Value::Path { parts, .. }
            | Value::GetPath { parts, .. }
            | Value::LitPath { parts, .. }
            | Value::SetPath { parts, .. } => {
                if let Some(Value::Word { sym, binding, .. })
                | Some(Value::GetWord { sym, binding, .. }) = parts.first_mut()
                {
                    if let Some(idx) = child_ctx.index_of(sym) {
                        *binding = Binding::Local(Rc::clone(child_ctx), idx);
                    } else if let Some(idx) = user_ctx.index_of(sym) {
                        *binding = Binding::Local(Rc::clone(user_ctx), idx);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Like `attach_local_bindings` but distinguishes function-local words
/// (`Binding::Func(idx)` via `func_ctx`) from outer user-context words
/// (`Binding::Local(user_ctx, idx)`). Function-local names shadow outer ones.
///
/// M60: words already carrying `Binding::Closure(idx)` (set by the VM's
/// lexical analyzer for `closure` freevars) are preserved — `bind_function_body`
/// runs at `MakeFunc`/`MakeClosure` time, AFTER the analyzer has attached
/// capture bindings, so we must not clobber them. (For the walker path,
/// `closure_native` overlays `Binding::Closure` AFTER `bind_function_body`,
/// so this guard is a no-op there.)
///
/// M60: skips `func`/`does`/`function`/`closure` body blocks (via
/// `func_form_skip`) — those have their own scopes and are bound by their
/// own `bind_function_body`/`closure_native` calls. Without this skip, an
/// enclosing func's binding pass would clobber closure-body freevar words
/// with `Binding::Func` (pointing to the enclosing func's params), breaking
/// the capture.
fn attach_func_bindings(series: &Series, func_ctx: &Context, user_ctx: &Rc<Context>) {
    let mut data = series.data.borrow_mut();
    let n = data.len();
    let mut i = 0;
    while i < n {
        // Skip `func`/`does`/`function`/`closure` bodies — their words belong
        // to their own scopes, not this func's. (M60: without this skip, an
        // enclosing func's binding pass would clobber closure-body freevar
        // words with `Binding::Func`, breaking the capture.)
        // Note: `use [words] body` is NOT skipped — the use body's references
        // to the enclosing func's params are valid and must be bound here
        // (use_native's `attach_use_bindings` only checks child_ctx + user_ctx,
        // not the func ctx).
        if let Some(skip) = func_form_skip(&data, i) {
            i += skip;
            continue;
        }
        match &mut data[i] {
            Value::Block { series, .. } | Value::Paren { series, .. } => {
                let child = series.clone();
                attach_func_bindings(&child, func_ctx, user_ctx);
            }
            Value::Word { sym, binding, .. }
            | Value::SetWord { sym, binding, .. }
            | Value::GetWord { sym, binding, .. } => {
                // M60: don't clobber capture bindings set by the analyzer.
                if matches!(binding, Binding::Closure(_)) {
                    i += 1;
                    continue;
                }
                if let Some(idx) = func_ctx.index_of(sym) {
                    *binding = Binding::Func(idx);
                } else if let Some(idx) = user_ctx.index_of(sym) {
                    *binding = Binding::Local(Rc::clone(user_ctx), idx);
                }
            }
            Value::Path { parts, .. }
            | Value::GetPath { parts, .. }
            | Value::LitPath { parts, .. }
            | Value::SetPath { parts, .. } => {
                if let Some(Value::Word { sym, binding, .. })
                | Some(Value::GetWord { sym, binding, .. }) = parts.first_mut()
                {
                    // M60: don't clobber capture bindings set by the analyzer.
                    if matches!(binding, Binding::Closure(_)) {
                        i += 1;
                        continue;
                    }
                    if let Some(idx) = func_ctx.index_of(sym) {
                        *binding = Binding::Func(idx);
                    } else if let Some(idx) = user_ctx.index_of(sym) {
                        *binding = Binding::Local(Rc::clone(user_ctx), idx);
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Extract a `Symbol` from a lit-word or bare unbound word value — the form
/// used by `repeat 'i`, `foreach x`, `set 'foo`, `get word`, etc. Returns
/// `None` for any other shape.
pub(crate) fn loop_word_name(v: &Value) -> Option<Symbol> {
    match v {
        Value::LitWord { sym, .. } => Some(sym.clone()),
        Value::Word {
            sym,
            binding: Binding::Unbound,
            ..
        } => Some(sym.clone()),
        _ => None,
    }
}

/// Rebind words in `series` whose names appear in `names` to
/// `Binding::Local(Rc::clone(ctx), idx)`, where `idx` is the slot index in
/// `ctx`. Words not in `names` are left untouched (they keep their existing
/// binding). Recurses into nested `Block`/`Paren`. Used by the `bind` and
/// `use` natives on a *deep copy* of the block (callers clone the `Vec`
/// before calling so shared source data isn't mutated).
pub(crate) fn rebind_to_context(series: &Series, ctx: &Rc<Context>, names: &[Symbol]) {
    // Build a name -> idx lookup once for O(1) per-word checks.
    let lookup: std::collections::HashMap<&Symbol, usize> = names
        .iter()
        .filter_map(|s| ctx.index_of(s).map(|idx| (s, idx)))
        .collect();
    let mut data = series.data.borrow_mut();
    rebind_inner(&mut data, ctx, &lookup);
}

/// Produce a true deep copy of a `Series`: a fresh `Rc<RefCell<Vec<Value>>>`
/// with every nested `Block`/`Paren` also deep-copied. The default
/// `Series::clone` only bumps the outer `Rc`, sharing nested storage —
/// unsuitable when `bind`/`use` need to rebind words without corrupting the
/// original source tree.
pub fn deep_clone_series(series: &Series) -> Series {
    let data = series.data.borrow();
    let cloned: Vec<Value> = data.iter().map(deep_clone_value).collect();
    Series::new(cloned)
}

/// Deep-clone a `Value`: literals/words clone as-is; `Block`/`Paren` are
/// rebuilt with deep-cloned nested series.
pub(crate) fn deep_clone_value(v: &Value) -> Value {
    match v {
        Value::Block { series, span } => Value::Block {
            series: deep_clone_series(series),
            span: *span,
        },
        Value::Paren { series, span } => Value::Paren {
            series: deep_clone_series(series),
            span: *span,
        },
        other => other.clone(),
    }
}

fn rebind_inner(
    data: &mut [Value],
    ctx: &Rc<Context>,
    lookup: &std::collections::HashMap<&Symbol, usize>,
) {
    for v in data.iter_mut() {
        match v {
            Value::Block { series, .. } | Value::Paren { series, .. } => {
                // After deep_clone, each nested series is unique (not shared),
                // so borrowing its RefCell here is safe and mutation won't
                // leak to other trees.
                let mut child_data = series.data.borrow_mut();
                rebind_inner(&mut child_data, ctx, lookup);
            }
            Value::Word { sym, binding, .. }
            | Value::SetWord { sym, binding, .. }
            | Value::GetWord { sym, binding, .. } => {
                if let Some(&idx) = lookup.get(sym) {
                    *binding = Binding::Local(Rc::clone(ctx), idx);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Closure capture binding (M60)
// ---------------------------------------------------------------------------

/// Overlay `Binding::Closure(idx)` on every `Word`/`SetWord`/`GetWord` (and
/// path head) in `series` whose name matches an entry in `captures_map`.
/// `captures_map` maps freevar names → capture-cell index. Used by the
/// walker's `closure_native` AFTER `bind_function_body` has run (so params/
/// locals already have `Binding::Func`, and globals have `Binding::Local`).
/// This overwrites those with `Binding::Closure(idx)` for captured words.
///
/// Recurses into nested `Block`/`Paren` (freevar references may appear in
/// nested blocks that are `do`ne at call time). Does NOT recurse into
/// `func`/`does`/`function`/`closure` body blocks — those have their own
/// scopes (a nested func's body references ITS params, not the outer
/// closure's captures).
pub(crate) fn set_closure_bindings(
    series: &Series,
    captures_map: &std::collections::HashMap<Symbol, usize>,
) {
    let mut data = series.data.borrow_mut();
    set_closure_bindings_inner(&mut data, captures_map);
}

fn set_closure_bindings_inner(
    data: &mut [Value],
    captures_map: &std::collections::HashMap<Symbol, usize>,
) {
    let mut i = 0;
    while i < data.len() {
        // Skip nested func/does/function/closure bodies — their words belong
        // to their own scope, not the closure's captures.
        if use_body_index(data, i).is_some() {
            i += 3;
            continue;
        }
        if let Some(skip) = func_form_skip(data, i) {
            i += skip;
            continue;
        }
        match &mut data[i] {
            Value::Block { series, .. } | Value::Paren { series, .. } => {
                let child = series.clone();
                set_closure_bindings_inner(
                    // Borrow the child's RefCell (different from the outer).
                    &mut child.data.borrow_mut(),
                    captures_map,
                );
            }
            Value::Word { sym, binding, .. }
            | Value::SetWord { sym, binding, .. }
            | Value::GetWord { sym, binding, .. } => {
                if let Some(&idx) = captures_map.get(sym) {
                    *binding = Binding::Closure(idx);
                }
            }
            Value::Path { parts, .. }
            | Value::GetPath { parts, .. }
            | Value::LitPath { parts, .. }
            | Value::SetPath { parts, .. } => {
                if let Some(Value::Word { sym, binding, .. })
                | Some(Value::GetWord { sym, binding, .. }) = parts.first_mut()
                {
                    if let Some(&idx) = captures_map.get(sym) {
                        *binding = Binding::Closure(idx);
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------
// VM dispatch helpers (M26)
// ---------------------------------------------------------------------------

/// True if any `Word`/`SetWord`/`GetWord` in `series` (recursing into nested
/// `Block`/`Paren`) carries a binding the VM can't lexically address:
/// - `Binding::Local(ctx, _)` whose `ctx` is not `user_ctx` — a foreign
///   context installed by `bind`/`use` against a child/object context.
/// - `Binding::Func(_)` — function-local slots set by `bind_function_body`,
///   resolved via `env.call_stack` (the walker's frame stack). The VM uses
///   its own `vm.frames`, so it can't see the walker's call frames. When a
///   walker native (`if`/`either`/`loop`/`foreach`/`do`/etc.) calls
///   `dispatch_block` on a block containing `Func` bindings (e.g. a func
///   body's branch block), the VM must fall back to the walker. (M29 fix —
///   was the root cause of the user-func refinement test failures.)
///   `Binding::Lexical`/`Unbound` are VM-safe.
///
/// Used by `interp::dispatch_block` to pick walker vs. VM for plain `Block`
/// values passed to `do`/`reduce`/loop natives (which carry no cached
/// `CompiledBlock` — M27 adds the Env-level cache).
pub(crate) fn has_foreign_bindings(series: &Series, user_ctx: &Rc<Context>) -> bool {
    let data = series.data.borrow();
    data.iter().any(|v| has_foreign_binding_value(v, user_ctx))
}

fn has_foreign_binding_value(v: &Value, user_ctx: &Rc<Context>) -> bool {
    match v {
        Value::Word { binding, .. }
        | Value::SetWord { binding, .. }
        | Value::GetWord { binding, .. } => match binding {
            Binding::Local(ctx, _) => !Rc::ptr_eq(ctx, user_ctx),
            Binding::Func(_) => true, // M29: VM can't resolve via env.call_stack
            // M60: Closure bindings resolve via the frame's captures cell,
            // not a foreign context — VM-safe.
            Binding::Closure(_) => false,
            _ => false,
        },
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            has_foreign_bindings(series, user_ctx)
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::natives::{install_constants, register_natives};
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use red_core::{Env, Error};
    use std::cell::RefCell;
    use std::io::Write;

    /// In-memory `Write` sink that records bytes into a shared `Rc<RefCell<Vec<u8>>>`.
    struct BufferWriter(Rc<RefCell<Vec<u8>>>);

    impl Write for BufferWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Run `src` with a fresh env (constants + natives) and capture stdout.
    fn run_capture_val(src: &str) -> Result<(Value, Vec<u8>), String> {
        let body = load_source(src).map_err(|e| e.to_string())?;
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        let block = Value::block(body);
        let val = eval(&block, &mut env).map_err(|e| e.to_string())?;
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    fn run_err(src: &str) -> Error {
        let body = load_source(src).expect("parse failed");
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        let block = Value::block(body);
        let err = eval(&block, &mut env).expect_err("expected error");
        Error::Eval(err)
    }

    use crate::interp::eval;
    use red_core::EvalError;

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    fn out(src: &str) -> String {
        s(&run_capture_val(src).unwrap().1)
    }

    // --- func / does ---

    #[test]
    fn func_square() {
        assert_eq!(
            mold_to_string(&val("square: func [x][x * x] square 5")),
            "25"
        );
    }

    #[test]
    fn func_two_params() {
        assert_eq!(mold_to_string(&val("add: func [a b][a + b] add 3 4")), "7");
    }

    #[test]
    fn does_zero_arg() {
        assert_eq!(mold_to_string(&val("greet: does [42] greet")), "42");
    }

    #[test]
    fn func_returns_last_value() {
        assert_eq!(mold_to_string(&val("f: func [x][x x + 1] f 10")), "11");
    }

    // --- return ---

    #[test]
    fn return_exits_early() {
        // `return` unwinds via EvalError::Return, caught by the call shim.
        assert_eq!(
            mold_to_string(&val("f: func [x][return 99 x + 1] f 5")),
            "99"
        );
    }

    #[test]
    fn return_none_default() {
        // `return` with no value returns none.
        assert_eq!(
            mold_to_string(&val("f: func [x][if x > 0 [return x] return] f 0")),
            "none"
        );
    }

    #[test]
    fn return_outside_function_errors() {
        let err = run_err("return 5");
        assert!(matches!(err, Error::Eval(EvalError::Return(_))));
    }

    // --- recursion ---

    #[test]
    fn recursive_factorial() {
        let src = "fact: func [n][either n <= 1 [1][n * fact n - 1]] fact 5";
        assert_eq!(mold_to_string(&val(src)), "120");
    }

    #[test]
    fn recursive_fibonacci() {
        let src = "fib: func [n][either n < 2 [n][(fib n - 1) + fib n - 2]] fib 6";
        assert_eq!(mold_to_string(&val(src)), "8");
    }

    #[test]
    fn recursion_does_not_clobber_outer_locals() {
        // Each call gets its own frame ctx clone, so a nested call's param
        // write must not corrupt the caller's param.
        let src = "f: func [n][if n > 0 [f n - 1] n] f 3";
        assert_eq!(mold_to_string(&val(src)), "3");
    }

    // --- function? ---

    #[test]
    fn function_predicate() {
        assert_eq!(mold_to_string(&val("function? func [x][x]")), "true");
        assert_eq!(mold_to_string(&val("function? 5")), "false");
        assert_eq!(mold_to_string(&val("function? [1 2 3]")), "false");
    }

    // --- make function! ---

    #[test]
    fn make_function_packed_block() {
        assert_eq!(
            mold_to_string(&val("f: make function! [[x][x * x]] f 6")),
            "36"
        );
    }

    #[test]
    fn make_function_no_params() {
        assert_eq!(mold_to_string(&val("f: make function! [[][42]] f")), "42");
    }

    // --- use ---

    #[test]
    fn use_block_creates_locals() {
        // `use [x][x: 5 x]` → 5; x is unbound outside the use block.
        assert_eq!(mold_to_string(&val("use [x][x: 5 x]")), "5");
    }

    #[test]
    fn use_locals_do_not_leak() {
        // After `use`, the local word is shadowed back to unbound (no global).
        let err = run_err("use [x][x: 5] x");
        assert!(matches!(err, Error::Eval(EvalError::UnboundWord { .. })));
    }

    // --- get / set / value? ---

    #[test]
    fn valueq_before_and_after() {
        // `value? 'foo` before assignment → false.
        assert_eq!(mold_to_string(&val("value? 'foo")), "false");
        // After `foo: 5`, `value? 'foo` → true.
        assert_eq!(mold_to_string(&val("foo: 5 value? 'foo")), "true");
    }

    #[test]
    fn get_word_value() {
        assert_eq!(mold_to_string(&val("foo: 7 get 'foo")), "7");
    }

    #[test]
    fn set_word_value() {
        // `set` writes to an existing slot (declared via `bar:` first); the
        // POC `user_ctx` name map is frozen at bind time, so the word must
        // already have a slot allocated.
        assert_eq!(mold_to_string(&val("bar: 0 set 'bar 8 bar")), "8");
        // `set` returns the set value.
        assert_eq!(mold_to_string(&val("bar: 0 set 'bar 8")), "8");
    }

    #[test]
    fn get_unbound_errors() {
        let err = run_err("get 'nope");
        assert!(matches!(err, Error::Eval(EvalError::UnboundWord { .. })));
    }

    // --- bind ---

    #[test]
    fn bind_rebinds_words_to_context() {
        // `bind block ctx` rebinds words in `block` to the user context so
        // they resolve to the script-level slots. Here `x` is set to 5 at
        // top level; `bind [x] user-ctx` makes the block's `x` resolve to 5
        // when the block is `do`ne.
        assert_eq!(mold_to_string(&val("x: 5 do bind [x] 'x")), "5");
    }

    #[test]
    fn bind_with_set_word() {
        // `bind` also rebinds set-words in the block.
        assert_eq!(mold_to_string(&val("y: 0 bind [y: 99] 'y do [y]")), "0");
        // After binding + doing the set-word block, y should be 99.
        assert_eq!(mold_to_string(&val("y: 0 do bind [y: 99] 'y y")), "99");
    }

    // --- golden-style behavior: function calling print ---

    #[test]
    fn func_calls_print() {
        assert_eq!(out("f: func [x][print x] f 42"), "42\n");
    }

    #[test]
    fn func_calling_native_in_body() {
        // `if n > 0 [n]` returns `n` when truthy; the trailing `0` is the
        // body's last value, so the call evaluates to 0.
        assert_eq!(mold_to_string(&val("f: func [n][if n > 0 [n] 0] f 5")), "0");
    }

    // -----------------------------------------------------------------------
    // M26: `has_foreign_bindings` — drives `dispatch_block`'s walker fallback
    // -----------------------------------------------------------------------

    /// A block whose words are `Unbound` has no foreign bindings — the VM
    /// (lexical addressing) can attempt it; `LoadDynamic` resolves unbound
    /// words at runtime.
    #[test]
    fn foreign_bindings_unbound_is_not_foreign() {
        let body = load_source("[x y z]").expect("parse");
        let user_ctx = Rc::new(Context::new());
        assert!(!has_foreign_bindings(&body, &user_ctx));
    }

    /// A block whose words are bound to `user_ctx` (via `bind_pass`) is NOT
    /// foreign — the VM addresses them as globals (`LoadGlobal`/`SetGlobal`).
    #[test]
    fn foreign_bindings_user_ctx_is_not_foreign() {
        let body = load_source("[x]").expect("parse");
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        // `x` was allocated a slot in `ctx_rc` and bound as
        // `Binding::Local(ctx_rc, 0)` — same context, so not foreign.
        assert!(!has_foreign_bindings(&body, &ctx_rc));
    }

    /// A block rebound by `bind` to a DIFFERENT context (simulating `use`'s
    /// child context) IS foreign — `dispatch_block` must route it to the
    /// walker because the VM's lexical addressing can't resolve
    /// `Binding::Local(child_ctx, _)`.
    #[test]
    fn foreign_bindings_other_ctx_is_foreign() {
        let body = load_source("[x]").expect("parse");
        let user_ctx = Rc::new(Context::new());
        // Bind `x` to a *different* context (simulating `use`'s child).
        let child_ctx = Rc::new(Context::new());
        child_ctx.slot_index(Symbol::new("x"));
        rebind_to_context(&body, &child_ctx, &[Symbol::new("x")]);
        assert!(has_foreign_bindings(&body, &user_ctx));
        // And symmetrically: checking against the child ctx says not foreign.
        assert!(!has_foreign_bindings(&body, &child_ctx));
    }

    /// Foreign bindings recurse into nested blocks: `[x [y]]` where `y` is
    /// foreign (but `x` is on `user_ctx`) still reports foreign.
    #[test]
    fn foreign_bindings_recurse_into_nested_blocks() {
        let body = load_source("[x [y]]").expect("parse");
        let user_ctx = Rc::new(Context::new());
        let child_ctx = Rc::new(Context::new());
        child_ctx.slot_index(Symbol::new("y"));
        rebind_to_context(&body, &child_ctx, &[Symbol::new("y")]);
        assert!(has_foreign_bindings(&body, &user_ctx));
    }
}
