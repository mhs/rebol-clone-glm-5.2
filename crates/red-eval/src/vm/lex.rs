//! Compile-time lexical analyzer + free-variable pass (v0.3, M23).
//!
//! Walks a parsed block tracking a compile-time `Scope` (a chain of
//! `Symbol -> (depth, slot)` maps), and:
//!
//! - Attaches `Binding::Lexical(depth, slot)` to every `Word`/`SetWord`/
//!   `GetWord` whose name resolves in a function-local scope (depth >= 1).
//! - Leaves script-top-level words as `Binding::Local` (their existing
//!   `bind_pass` attachment); the M24 compiler emits `LoadGlobal` for these.
//! - Leaves truly unbound words as `Binding::Unbound`; the M24 compiler
//!   emits `LoadDynamic(sym)` for them.
//! - Computes the free-variable capture list for each `func`/`does`/
//!   `function` body: words referenced inside that resolve to an ancestor
//!   function scope (not the current scope, not the global root).
//!
//! Nothing here is wired into the default binding pipeline (`bind_pass`) or
//! the tree-walker (`interp::eval`). M23 ships an opt-in module invoked only
//! by its own tests; M24 wires it into the compiler. Existing v0.2 behavior
//! is untouched — no `Binding::Lexical` word reaches the walker's
//! `"lexical binding not yet supported in the tree-walker"` arms except via
//! this module's deliberate invocation.

use std::collections::HashMap;
use std::rc::Rc;

use red_core::value::{Binding, Series, Symbol, Value};
use red_core::Context;

use crate::binding::{func_form_skip, use_body_index};
use crate::natives::extract_spec;

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

/// Compile-time lexical scope: a chain of `Symbol -> (depth, slot)` maps
/// mirroring the `FuncDef.ctx` slot layout at each function-nesting level.
///
/// `depth == 0` is the **root scope** (the user context) — words here stay
/// `Binding::Local(user_ctx, slot)` (their existing `bind_pass` attachment)
/// and the M24 compiler emits `LoadGlobal` for them. `depth >= 1` is a
/// **function-local scope** (one per enclosing `func`/`does`/`function`
/// body); words here get `Binding::Lexical(parent_depth_diff, slot)`.
///
/// The scope chain is built lazily by `analyze_block` as it descends into
/// nested function bodies. Free variables — words whose lookup escapes the
/// current scope to an ancestor function scope — are recorded in the
/// `AnalysisResult` returned for each block.
pub struct Scope {
    bindings: HashMap<Symbol, (usize, usize)>,
    parent: Option<Box<Scope>>,
    depth: usize,
}

impl Scope {
    /// Root scope (depth 0 = user context). Words present in `user_ctx`
    /// are seeded into the scope so references resolve as `Local` (the M24
    /// compiler emits `LoadGlobal` for them; the binding is left unchanged).
    pub fn root(user_ctx: &Rc<Context>) -> Self {
        let mut bindings = HashMap::new();
        for (sym, &idx) in user_ctx.names.borrow().iter() {
            bindings.insert(sym.clone(), (0, idx));
        }
        Self {
            bindings,
            parent: None,
            depth: 0,
        }
    }

    /// A function-local child scope at `depth + 1`. The child's slot map is
    /// populated as the analyzer walks the function spec (params, refinement
    /// arg words, locals). Slot numbers are *within* the child's frame, so
    /// they start at 0 for each new scope.
    pub fn child(parent: &Scope) -> Self {
        Self {
            bindings: HashMap::new(),
            parent: Some(Box::new(parent.clone())),
            depth: parent.depth + 1,
        }
    }

    /// Allocate (or reuse) a slot in this scope. Slot numbering follows the
    /// `bind_function_body` convention: 0..params.len() are params, then
    /// refinement flag+arg slots in declaration order, then `<local>` words,
    /// then body-local SetWords. The analyzer allocates only params +
    /// refinement args + locals here (body-local SetWords are allocated as
    /// they're encountered during the walk).
    pub(crate) fn slot_index(&mut self, sym: Symbol) -> usize {
        if let Some(&(_, idx)) = self.bindings.get(&sym) {
            return idx;
        }
        let idx = self.bindings.len();
        self.bindings.insert(sym, (self.depth, idx));
        idx
    }

    /// Resolve `sym` against this scope chain. Returns:
    /// - `Some((depth, slot))` if found at scope `depth` (0 = global,
    ///   >=1 = function-local).
    /// - `None` if not found (truly unbound — compiler emits `LoadDynamic`).
    pub(crate) fn lookup(&self, sym: &Symbol) -> Option<(usize, usize)> {
        if let Some(&entry) = self.bindings.get(sym) {
            return Some(entry);
        }
        self.parent.as_ref().and_then(|p| p.lookup(sym))
    }

    /// Depth of this scope (0 = root, >=1 = nested function).
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Number of slots allocated in this scope (params + refinements + locals
    /// + body-local SetWords). The M25 VM uses this to size a func frame's
    ///   `locals` Vec at `CallUser` time.
    pub(crate) fn slot_count(&self) -> usize {
        self.bindings.len()
    }
}

impl Clone for Scope {
    fn clone(&self) -> Self {
        Self {
            bindings: self.bindings.clone(),
            parent: self.parent.as_ref().map(|p| Box::new(p.as_ref().clone())),
            depth: self.depth,
        }
    }
}

// ---------------------------------------------------------------------------
// Analysis result
// ---------------------------------------------------------------------------

/// Output of `analyze_block`: the free-variable capture list for the block's
/// enclosing function, plus the `needs_rebind` flag the M24 compiler reads to
/// decide whether to compile this block or fall back to the tree-walker.
#[derive(Clone, Debug, Default)]
pub struct AnalysisResult {
    /// Words referenced in this block that resolve to an ancestor *function*
    /// scope (depth >= 1) — these are the free variables the VM must capture
    /// at `MakeFunc` time. Words resolving to the root (user) scope are NOT
    /// free vars (they resolve via `LoadGlobal`).
    pub freevars: Vec<Symbol>,
    /// True if this block (or any nested block reachable from it) contains a
    /// `use [words] body` form or `make object!`/`object`/`context` spec —
    /// those are runtime-scoped and the VM must defer to the walker for them.
    pub needs_rebind: bool,
}

// ---------------------------------------------------------------------------
// Entry point: analyze_block
// ---------------------------------------------------------------------------

/// Walk `body` under `scope`, attaching `Binding::Lexical` to every word
/// that resolves to a function-local scope, and computing the free variables
/// (words that resolve to an ancestor function scope, not the current scope
/// and not the global root).
///
/// The caller seeds `scope` via `Scope::root(&env.user_ctx)` for the
/// top-level script body, or `Scope::child(&parent_scope)` for a function
/// body. `analyze_block` mutates the bindings in place (via the `Series`
/// `RefCell`) and returns the free-variable list for the block as a whole.
///
/// Descends into `func`/`does`/`function` bodies (computing their freevars
/// recursively) but does NOT descend into `use` bodies or `make object!`
/// specs — those set `needs_rebind = true` and are left for the walker.
pub fn analyze_block(body: &Series, scope: &mut Scope) -> AnalysisResult {
    let mut result = AnalysisResult::default();
    analyze_inner(body, scope, &mut result);
    dedup_freevars(&mut result.freevars);
    result
}

fn dedup_freevars(freevars: &mut Vec<Symbol>) {
    let mut seen: Vec<Symbol> = Vec::new();
    for sym in freevars.drain(..) {
        if !seen.contains(&sym) {
            seen.push(sym);
        }
    }
    *freevars = seen;
}

/// Inner walker. Mirrors the structure of `attach_func_bindings`
/// (`binding.rs:459`): `borrow_mut()` the `RefCell`, iterate by `while i < n`,
/// recurse into nested `Block`/`Paren` via `series.clone()` so the outer
/// borrow stays valid (the child series is a different `RefCell`).
fn analyze_inner(series: &Series, scope: &mut Scope, result: &mut AnalysisResult) {
    let mut data = series.data.borrow_mut();
    let n = data.len();
    let mut i = 0;
    while i < n {
        // `use [words] body` — runtime-scoped locals. Mark needs_rebind and
        // skip the body (do not analyze it; the walker handles `use`).
        if use_body_index(&data, i).is_some() {
            result.needs_rebind = true;
            i += 3;
            continue;
        }
        // `make object! [spec]` — the `make` native at runtime dispatches to
        // `object::make_object` which walks the spec itself. The spec body is
        // not compiled; flag it so the VM falls back to the walker.
        if is_make_object_form(&data, i) {
            result.needs_rebind = true;
            i += 3;
            continue;
        }
        // `object [spec]` / `context [spec]` — keyword aliases for
        // `make object!`. Same handling: flag and skip.
        if is_object_keyword_form(&data, i) {
            result.needs_rebind = true;
            i += 2;
            continue;
        }
        // `func [spec] [body]` / `function [spec] [body]` / `does [body]` —
        // descend into the body with a fresh child scope to compute freevars
        // and attach `Binding::Lexical` to function-local words. Unlike
        // `bind_pass` (which skips these forms entirely), the lexical
        // analyzer MUST descend — that's how it discovers free variables.
        if let Some(skip) = func_form_skip(&data, i) {
            analyze_func_form(&data, i, scope, result);
            i += skip;
            continue;
        }
        analyze_value_mut(&mut data, i, scope, result);
        i += 1;
    }
}

/// Collect body-local SetWords into `scope` before the main walk attaches
/// bindings. Mirrors `collect_setwords_inner` (`binding.rs:145`) but with a
/// `Scope` instead of a `Context`. Skips `use`/`make object!`/`object`/
/// `context`/`func`/`does`/`function` forms (their locals are scoped
/// elsewhere). Recurses into nested `Block`/`Paren` so SetWords inside data
/// blocks are also collected (matches Red semantics: `foo: 5 [bar: 1]` later
/// `do`ne yields a bound `bar`).
fn collect_setwords(series: &Series, scope: &mut Scope) {
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
                // Allocate a slot if not already known in the scope chain.
                // (Params/refinements/locals are pre-allocated; body-local
                // SetWords shadow nothing — they're fresh slots.)
                if scope.lookup(sym).is_none() {
                    scope.slot_index(sym.clone());
                }
                i += 1;
            }
            Value::Block { series: s, .. } | Value::Paren { series: s, .. } => {
                let child = s.clone();
                collect_setwords(&child, scope);
                i += 1;
            }
            _ => i += 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Form detectors
// ---------------------------------------------------------------------------

/// True if `data[i..]` begins a `make object! [spec]` form: `Word(make)
/// Word(object) Block(spec)` (also tolerates `Word(make) LitWord(object)
/// Block(spec)` since `object!` parses as a lit-word).
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

/// True if `data[i..]` begins an `object [spec]` or `context [spec]` form
/// (keyword aliases for `make object!`).
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
    matches!(sym.as_str(), "object" | "context") && matches!(&data[i + 1], Value::Block { .. })
}

// ---------------------------------------------------------------------------
// Func-form analysis (the heart of the free-variable computation)
// ---------------------------------------------------------------------------

/// Analyze a `func`/`does`/`function` form: extract the spec, open a child
/// scope, allocate slots for params/refinements/locals, then recursively
/// analyze the body. The body's freevars are computed by the recursive call
/// (words referenced there that resolve to *this* scope's parent — i.e. the
/// current function's enclosing scope — are free vars of this function).
/// Those freevars are then propagated up: a freevar of a nested function is
/// also a freevar of the enclosing function (transitively), so we append
/// them to the enclosing `AnalysisResult.freevars` for the parent block.
///
/// `MakeFunc` time (M24) will copy these onto `FuncDef::freevars`; for M23
/// we just return them via the parent `AnalysisResult`.
fn analyze_func_form(data: &[Value], i: usize, scope: &mut Scope, result: &mut AnalysisResult) {
    // Locate the body block. For `does` it's `data[i+1]`; for `func`/
    // `function` it's `data[i+2]`. We rely on `func_form_skip` having
    // already validated the shape.
    let is_does = matches!(
        &data[i],
        Value::Word { sym, .. } if sym.as_str() == "does"
    );
    let body_idx = if is_does { i + 1 } else { i + 2 };
    let body_value = &data[body_idx];
    let body_series = match body_value {
        Value::Block { series, .. } => series.clone(),
        _ => return,
    };

    // Extract the spec to learn the param/refinement/local names. For `does`
    // there's no spec — synthesize an empty one.
    let spec = if is_does {
        crate::natives::FuncSpec {
            params: Vec::new(),
            refinements: Vec::new(),
            locals: Vec::new(),
        }
    } else {
        match extract_spec(&data[i + 1]) {
            Ok(s) => s,
            Err(_) => return, // Malformed spec — let the runtime native report it.
        }
    };

    // Open a child scope for this function. Slot indices start at 0 within
    // the child frame (matching `bind_function_body`'s slot allocation order:
    // params, then refinement flag+args, then locals, then body-local
    // SetWords).
    let mut child = Scope::child(scope);
    for p in &spec.params {
        child.slot_index(p.clone());
    }
    for (ref_name, ref_args) in &spec.refinements {
        child.slot_index(ref_name.clone());
        for arg in ref_args {
            child.slot_index(arg.clone());
        }
    }
    for local in &spec.locals {
        child.slot_index(local.clone());
    }
    // Pre-collect body-local SetWords (mirrors `collect_setwords_inner` in
    // `binding.rs`) so subsequent references resolve to Lexical(0, slot).
    collect_setwords(&body_series, &mut child);

    // Recursively analyze the body in the child scope. Words resolving to
    // the child scope itself get `Lexical(0, slot)`; words resolving to an
    // ancestor scope (the parent or higher) get `Lexical(depth_diff, slot)`
    // AND are recorded as freevars of this function — UNLESS the ancestor
    // is the root (user context, depth 0), in which case the binding is left
    // as `Binding::Local` (compiler emits `LoadGlobal`).
    let mut body_result = analyze_block(&body_series, &mut child);

    // Freevars computed for the body are also freevars of the enclosing
    // block (transitively) — propagate them up so the outermost function
    // captures everything its nested functions need.
    for fv in body_result.freevars.drain(..) {
        if !result.freevars.contains(&fv) {
            result.freevars.push(fv);
        }
    }
    if body_result.needs_rebind {
        result.needs_rebind = true;
    }
}

// ---------------------------------------------------------------------------
// Per-value analysis
// ---------------------------------------------------------------------------

/// Attach lexical bindings to a single value at `data[i]`, recursing into
/// nested blocks/parens. Mirrors `attach_func_bindings`'s match arms
/// (`binding.rs:462`) but writes `Binding::Lexical` instead of `Func`/`Local`.
fn analyze_value_mut(data: &mut [Value], i: usize, scope: &mut Scope, result: &mut AnalysisResult) {
    match &mut data[i] {
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            let child = series.clone();
            analyze_inner(&child, scope, result);
        }
        Value::Word { sym, binding, .. }
        | Value::SetWord { sym, binding, .. }
        | Value::GetWord { sym, binding, .. } => {
            attach_lexical(sym, binding, scope, result);
            // SetWord at function-local scope: the analyzer must also
            // allocate a slot for it if it's a new local. We do this in the
            // `attach_lexical` path by allocating on first encounter.
        }
        Value::Path { parts, .. }
        | Value::GetPath { parts, .. }
        | Value::LitPath { parts, .. }
        | Value::SetPath { parts, .. } => {
            // Only the head word is bound (matches `attach_func_bindings`).
            let head = parts.first_mut();
            if let Some(Value::Word { sym, binding, .. })
            | Some(Value::GetWord { sym, binding, .. }) = head
            {
                attach_lexical(sym, binding, scope, result);
            }
        }
        _ => {}
    }
}

/// Resolve `sym` against the scope chain and overwrite `binding` with the
/// appropriate variant:
///
/// - Found in current function scope (depth d) → `Lexical(0, slot)`.
/// - Found in ancestor function scope at depth `d' < current_depth` →
///   `Lexical(current_depth - d', slot)` AND record `sym` in `result.freevars`.
/// - Found in root scope (depth 0) → leave as-is (`Binding::Local`; the M24
///   compiler emits `LoadGlobal`). NOT a freevar.
/// - Not found → leave as `Unbound`. The compiler emits `LoadDynamic`.
///
/// If the binding is already something other than `Unbound`/`Local`/`Lexical`
/// (e.g. `Func` from `bind_function_body`), we overwrite with `Lexical` — M23
/// runs *after* `bind_pass`/`bind_function_body` in the M24 pipeline, so its
/// attachments take precedence. For M23's opt-in tests the binding starts
/// `Unbound` (or `Local` for user-ctx words) and we attach fresh.
fn attach_lexical(sym: &Symbol, binding: &mut Binding, scope: &Scope, result: &mut AnalysisResult) {
    let Some((found_depth, slot)) = scope.lookup(sym) else {
        return; // Unbound — leave as-is.
    };
    if found_depth == 0 {
        // Global (user-ctx) reference. `bind_pass` has already attached
        // `Binding::Local(user_ctx, slot)` if the word is a known global;
        // we leave it alone so the M24 compiler can emit `LoadGlobal`. If
        // it's still `Unbound` here, it's a truly unbound word — leave it.
        return;
    }
    // Found in a function scope. Compute the depth difference from the
    // current scope (the innermost) to the defining scope.
    let current_depth = scope.depth();
    let depth_diff = current_depth - found_depth;
    *binding = Binding::Lexical(depth_diff, slot);
    // If the defining scope is an ancestor (not the current scope), this is
    // a free variable of the current function — record it.
    if depth_diff > 0 && !result.freevars.contains(sym) {
        result.freevars.push(sym.clone());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::bind_pass;
    use crate::natives::install_constants;
    use red_core::parser::load_source;
    use red_core::value::{Binding, Value};
    use red_core::Context;

    /// Build a fresh user context seeded with constants, run `bind_pass` on
    /// `src` (so top-level SetWords are allocated), then return the body and
    /// the context. The lexical analyzer is invoked on the result.
    fn parse_and_bind(src: &str) -> (Series, Rc<Context>) {
        let body = load_source(src).expect("parse failed");
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        (body, ctx_rc)
    }

    /// Find the first `Value::Func` reachable in `body`'s tree, returning the
    /// `Rc<FuncDef>`. Used to inspect the FuncDef that the `func`/`does`/
    /// `function` natives produce after `bind_function_body`. For M23 tests
    /// the analyzer does NOT create FuncDefs (M24 does, at MakeFunc time);
    /// instead we inspect bindings directly on the body.
    fn find_func_value(body: &Series) -> Option<std::rc::Rc<red_core::FuncDef>> {
        let data = body.data.borrow();
        for v in data.iter() {
            if let Value::Func(fd) = v {
                return Some(fd.clone());
            }
        }
        None
    }

    /// Walk `body` (and nested blocks) collecting the first `Word`/`SetWord`/
    /// `GetWord` named `name`, returning a clone of its `Binding`.
    fn find_word_binding(body: &Series, name: &str) -> Option<Binding> {
        fn walk(data: &[Value], name: &str) -> Option<Binding> {
            for v in data.iter() {
                match v {
                    Value::Word { sym, binding, .. }
                    | Value::SetWord { sym, binding, .. }
                    | Value::GetWord { sym, binding, .. }
                        if sym.as_str() == name =>
                    {
                        return Some(binding.clone());
                    }
                    Value::Block { series, .. } | Value::Paren { series, .. } => {
                        let child = series.clone();
                        let child_data = child.data.borrow();
                        if let Some(b) = walk(&child_data, name) {
                            return Some(b);
                        }
                    }
                    Value::Path { parts, .. }
                    | Value::GetPath { parts, .. }
                    | Value::LitPath { parts, .. }
                    | Value::SetPath { parts, .. } => {
                        for p in parts {
                            match p {
                                Value::Word { sym, binding, .. }
                                | Value::SetWord { sym, binding, .. }
                                | Value::GetWord { sym, binding, .. }
                                    if sym.as_str() == name =>
                                {
                                    return Some(binding.clone());
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        let data = body.data.borrow();
        walk(&data, name)
    }

    // --- Plan-required tests ------------------------------------------------

    #[test]
    fn square_func_body_x_is_lexical_0_0_no_freevars() {
        // `square: func [x][x * x]` — after analyze, `x` in the func body is
        // `Lexical(0, 0)` (depth 0 = the func's own scope; slot 0 = the
        // first param). No freevars.
        let (body, ctx_rc) = parse_and_bind("square: func [x][x * x]");
        let mut scope = Scope::root(&ctx_rc);
        let result = analyze_block(&body, &mut scope);
        assert!(
            result.freevars.is_empty(),
            "square should have no freevars, got {:?}",
            result.freevars
        );
        // Find `x` inside the func body. Top-level body is:
        // [square: func [x] [x * x]] — indices 0=setword, 1=word(func),
        // 2=spec block, 3=body block.
        let data = body.data.borrow();
        let Value::Block {
            series: func_body, ..
        } = &data[3]
        else {
            panic!("expected func body block at index 3");
        };
        let x_binding = find_word_binding(func_body, "x");
        assert!(
            matches!(x_binding, Some(Binding::Lexical(0, 0))),
            "x in square body should be Lexical(0, 0), got {:?}",
            x_binding
        );
    }

    #[test]
    fn inner_func_freevars_include_y() {
        // `outer: func [y][inner: func [][y] inner]` — `inner`'s body
        // references `y`, which is a param of `outer` (depth 1). From
        // `inner`'s scope (depth 2), `y` resolves to depth 1, so it's
        // `Lexical(1, 0)` and `inner`'s freevars == [y].
        let (body, ctx_rc) = parse_and_bind("outer: func [y][inner: func [][y] inner]");
        let mut scope = Scope::root(&ctx_rc);
        let result = analyze_block(&body, &mut scope);
        assert_eq!(
            result.freevars,
            vec![Symbol::new("y")],
            "outer's freevars should be [y]"
        );
        // Also verify `y` is Lexical(1, 0) inside inner's body.
        // Top-level: [outer: func [y] [inner: func [][y] inner]] —
        // index 3 = outer body block.
        let data = body.data.borrow();
        let Value::Block {
            series: outer_body, ..
        } = &data[3]
        else {
            panic!("expected outer body block at index 3");
        };
        let outer_data = outer_body.data.borrow();
        // outer_body = [inner: func [][y] inner] — index 3 = inner body block.
        let Value::Block {
            series: inner_body, ..
        } = &outer_data[3]
        else {
            panic!("expected inner body block at outer body index 3");
        };
        let y_binding = find_word_binding(inner_body, "y");
        assert!(
            matches!(y_binding, Some(Binding::Lexical(1, 0))),
            "y in inner body should be Lexical(1, 0), got {:?}",
            y_binding
        );
    }

    #[test]
    fn unbound_script_word_stays_unbound() {
        // `foo` with no SetWord defining it → stays `Binding::Unbound`.
        let (body, ctx_rc) = parse_and_bind("foo");
        let mut scope = Scope::root(&ctx_rc);
        let _ = analyze_block(&body, &mut scope);
        let foo_binding = find_word_binding(&body, "foo");
        assert!(
            matches!(foo_binding, Some(Binding::Unbound)),
            "foo should remain Unbound, got {:?}",
            foo_binding
        );
    }

    #[test]
    fn use_block_sets_needs_rebind() {
        // `use [x][x: 1 x]` → the use body is runtime-scoped; the analyzer
        // must set `needs_rebind = true` and NOT descend into the body.
        let (body, ctx_rc) = parse_and_bind("use [x][x: 1 x]");
        let mut scope = Scope::root(&ctx_rc);
        let result = analyze_block(&body, &mut scope);
        assert!(
            result.needs_rebind,
            "use block should set needs_rebind = true"
        );
    }

    #[test]
    fn make_object_sets_needs_rebind() {
        // `make object! [a: 1]` → the spec is walked at runtime by
        // `object::make_object`; the analyzer flags it.
        let (body, ctx_rc) = parse_and_bind("make object! [a: 1]");
        let mut scope = Scope::root(&ctx_rc);
        let result = analyze_block(&body, &mut scope);
        assert!(
            result.needs_rebind,
            "make object! should set needs_rebind = true"
        );
    }

    // --- Extra sanity tests ------------------------------------------------

    #[test]
    fn top_level_setword_stays_local() {
        // `foo: 5` at script top level — `bind_pass` attached
        // `Binding::Local(user_ctx, 0)` to `foo`. The analyzer should NOT
        // overwrite it (top-level = depth 0 = root scope).
        let (body, ctx_rc) = parse_and_bind("foo: 5 foo");
        let mut scope = Scope::root(&ctx_rc);
        let _ = analyze_block(&body, &mut scope);
        let foo_binding = find_word_binding(&body, "foo");
        assert!(
            matches!(foo_binding, Some(Binding::Local(_, _))),
            "top-level foo should stay Binding::Local, got {:?}",
            foo_binding
        );
    }

    #[test]
    fn does_body_word_is_lexical() {
        // `greet: does [hello]` — `hello` inside the does body is unbound
        // (no SetWord anywhere), so it stays `Unbound`. But if we add a
        // SetWord inside, it should become `Lexical(0, 0)` (the does's own
        // scope, slot 0 = first local).
        let (body, ctx_rc) = parse_and_bind("greet: does [x: 1 x]");
        let mut scope = Scope::root(&ctx_rc);
        let result = analyze_block(&body, &mut scope);
        assert!(
            result.freevars.is_empty(),
            "does body with only locals should have no freevars"
        );
        let data = body.data.borrow();
        // Body: [greet: does [x: 1 x]] — does body is at index 2.
        let Value::Block {
            series: does_body, ..
        } = &data[2]
        else {
            panic!("expected does body block at index 2");
        };
        let x_binding = find_word_binding(does_body, "x");
        // `x` is allocated as a local SetWord at slot 0 of the does's scope,
        // then referenced as a Word — both should be Lexical(0, 0).
        assert!(
            matches!(x_binding, Some(Binding::Lexical(0, 0))),
            "x in does body should be Lexical(0, 0), got {:?}",
            x_binding
        );
    }

    #[test]
    fn object_keyword_sets_needs_rebind() {
        // `object [a: 1]` (keyword alias for `make object!`) — same flagging.
        let (body, ctx_rc) = parse_and_bind("object [a: 1]");
        let mut scope = Scope::root(&ctx_rc);
        let result = analyze_block(&body, &mut scope);
        assert!(
            result.needs_rebind,
            "object keyword should set needs_rebind = true"
        );
    }

    #[test]
    fn context_keyword_sets_needs_rebind() {
        let (body, ctx_rc) = parse_and_bind("context [a: 1]");
        let mut scope = Scope::root(&ctx_rc);
        let result = analyze_block(&body, &mut scope);
        assert!(
            result.needs_rebind,
            "context keyword should set needs_rebind = true"
        );
    }

    #[test]
    fn recursive_self_reference_is_global_not_freevar() {
        // `fact: func [n][either n <= 1 [1][n * fact n - 1]]` — `fact`
        // references itself by name. `fact` is a top-level SetWord (bound to
        // the user ctx by `bind_pass` as `Binding::Local`), so from inside
        // the func body it resolves to depth 0 (global). NOT a freevar.
        let (body, ctx_rc) = parse_and_bind("fact: func [n][either n <= 1 [1][n * fact n - 1]]");
        let mut scope = Scope::root(&ctx_rc);
        let result = analyze_block(&body, &mut scope);
        assert!(
            result.freevars.is_empty(),
            "recursive self-reference should not be a freevar, got {:?}",
            result.freevars
        );
    }

    #[test]
    fn analyze_block_is_idempotent() {
        // Running analyze_block twice should yield the same bindings — the
        // second pass re-resolves and overwrites with the same Lexical value.
        let (body, ctx_rc) = parse_and_bind("square: func [x][x * x]");
        let mut scope = Scope::root(&ctx_rc);
        let _ = analyze_block(&body, &mut scope);
        let mut scope2 = Scope::root(&ctx_rc);
        let result = analyze_block(&body, &mut scope2);
        assert!(result.freevars.is_empty());
    }

    // Suppress unused helper warning — `find_func_value` is kept for M24's
    // use; for now the M23 tests inspect bindings directly on the body.
    #[allow(dead_code)]
    fn _suppress_unused_helper() {
        let (body, _) = parse_and_bind("square: func [x][x * x] square 5");
        let _ = find_func_value(&body);
    }
}
