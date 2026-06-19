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
//! definition context as parent) are explicitly out of scope for the POC —
//! `func` uses shallow per-call clones of `FuncDef.ctx` only.
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
fn use_body_index(data: &[Value], i: usize) -> Option<usize> {
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

/// Phase 1: allocate a slot in `ctx` for every `SetWord` encountered anywhere
/// in the tree — *except* inside `use` bodies, whose locals are scoped to the
/// child context built at runtime by `use_native`. The slots are populated
/// during eval, not here.
pub(crate) fn collect_setwords(series: &Series, ctx: &Context) {
    let data = series.data.borrow();
    let n = data.len();
    let mut i = 0;
    while i < n {
        // `use [words] body` — skip the whole form (use + words block + body
        // block) so use-locals don't get allocated into the user context.
        if use_body_index(&data, i).is_some() {
            i += 3;
            continue;
        }
        match &data[i] {
            Value::SetWord { sym, .. } => {
                ctx.slot_index(sym.clone());
                i += 1;
            }
            Value::Block { series: s, .. } | Value::Paren { series: s, .. } => {
                let child = s.clone();
                collect_setwords(&child, ctx);
                i += 1;
            }
            _ => i += 1,
        }
    }
}

/// Phase 1b: allocate a slot for every word introduced as a loop variable by
/// `repeat`, `foreach`, or `forall`. Each is recognized in either of two
/// forms:
/// - `repeat 'i <count> <body>`  (lit-word counter, Red canonical form)
/// - `repeat i <count> <body>`   (bare-word counter, accepted by the POC)
/// - `foreach 'word <series> <body>` / `forall 'word <series> <body>`
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
        match &data[i] {
            Value::Word {
                sym,
                binding: Binding::Unbound,
                ..
            } if matches!(sym.as_str(), "repeat" | "foreach" | "forall") => {
                if i + 1 < n {
                    let name = loop_word_name(&data[i + 1]);
                    if let Some(sym) = name {
                        ctx.slot_index(sym);
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
            if matches!(sym.as_str(), "copy" | "set") && i + 1 < n {
                if let Some(name) = loop_word_name(&data[i + 1]) {
                    ctx.slot_index(name);
                }
                // Skip the operand; the following rule (1+ values) is walked
                // normally below — its sub-blocks may contain nested
                // copy/set forms we still want to find.
                i += 2;
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
    // 2. Body-local SetWords + loop vars become function-local slots.
    collect_setwords(&fd.body, &fd.ctx);
    collect_loop_vars(&fd.body, &fd.ctx);
    // 3. Attach bindings: function-local first, then outer user-ctx refs.
    attach_func_bindings(&fd.body, &fd.ctx, user_ctx);
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
            _ => {}
        }
    }
}

/// Like `attach_local_bindings` but distinguishes function-local words
/// (`Binding::Func(idx)` via `func_ctx`) from outer user-context words
/// (`Binding::Local(user_ctx, idx)`). Function-local names shadow outer ones.
fn attach_func_bindings(series: &Series, func_ctx: &Context, user_ctx: &Rc<Context>) {
    let mut data = series.data.borrow_mut();
    for i in 0..data.len() {
        match &mut data[i] {
            Value::Block { series, .. } | Value::Paren { series, .. } => {
                let child = series.clone();
                attach_func_bindings(&child, func_ctx, user_ctx);
            }
            Value::Word { sym, binding, .. }
            | Value::SetWord { sym, binding, .. }
            | Value::GetWord { sym, binding, .. } => {
                if let Some(idx) = func_ctx.index_of(sym) {
                    *binding = Binding::Func(idx);
                } else if let Some(idx) = user_ctx.index_of(sym) {
                    *binding = Binding::Local(Rc::clone(user_ctx), idx);
                }
            }
            _ => {}
        }
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
pub(crate) fn deep_clone_series(series: &Series) -> Series {
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
}
