//! Stdlib module auto-import (M63, surfaced ahead of M64).
//!
//! The stdlib is a small Red module file (`crates/red-eval/stdlib/stdlib.red`)
//! compiled into the binary via `include_str!` — no filesystem dependency at
//! runtime. `ensure_stdlib` parses + evaluates the source as a module and
//! aliases its exported words into `env.user_ctx` so a script can reference
//! them bare (`print str-upper "hi"`). The compiled module is cached on
//! `Env::stdlib` so the REPL doesn't recompile per line — the first
//! `ensure_stdlib` call evaluates the source once and stores the resulting
//! `ModuleDef`; subsequent calls (one per REPL line) re-alias the cached
//! module's exports into the current `user_ctx`.
//!
//! Auto-import runs from `run_series_inner_opts` (after `register_natives`,
//! before `dispatch_block`) unless `RunOptions::no_stdlib` is true. The CLI
//! `--no-stdlib` flag propagates that opt-out.

use std::rc::Rc;

use red_core::value::{ModuleDef, Value};
use red_core::{Env, EvalError};

/// The stdlib source, embedded at compile time.
pub const STDLIB_SRC: &str = include_str!("../stdlib/stdlib.red");

/// Idempotent: parse + eval the stdlib source once (cached on
/// `env.stdlib`), then alias its exports into `env.user_ctx`. Safe to
/// call repeatedly — subsequent calls re-alias the cached module's exports
/// (the REPL calls this per line so a fresh `user_ctx` still gets the
/// stdlib words).
pub fn ensure_stdlib(env: &mut Env) -> Result<(), EvalError> {
    if env.stdlib.is_none() {
        let module_rc = load_stdlib_module(env)?;
        env.stdlib = Some(Rc::clone(&module_rc));
    }
    // Re-alias on every call: the REPL swaps user_ctx per line (a fresh
    // ctx for each `run_source*` call), so even a cached stdlib needs its
    // exports copied into the current ctx.
    let module_opt = env.stdlib.clone();
    if let Some(module_rc) = module_opt {
        alias_stdlib_exports(&module_rc, env);
    }
    Ok(())
}

/// Parse + evaluate `STDLIB_SRC` as a module body, returning the resulting
/// `ModuleDef`. Mirrors `module::import_file`'s `eval_body_for_module` path:
/// the source is a bare `module 'stdlib [...]` form, so evaluating it yields
/// a `Value::Module` directly. Caches the result on `env.modules['stdlib]`
/// so `import 'stdlib` later finds it (matches the named-module cache
/// invariant).
fn load_stdlib_module(env: &mut Env) -> Result<Rc<std::cell::RefCell<ModuleDef>>, EvalError> {
    let body = red_core::parser::load_source(STDLIB_SRC).map_err(|e| EvalError::Native {
        message: format!("stdlib: parse error: {e}"),
        span: red_core::value::Span::default(),
    })?;
    // `eval_body_for_module` runs the body in a throwaway ctx and returns
    // the resulting Module if the body's final value is a `module` form.
    let module_rc = match crate::module::eval_body_for_module_pub(&body, env)? {
        Some(m) => m,
        None => {
            return Err(EvalError::Native {
                message: "stdlib: source did not yield a module value".to_string(),
                span: red_core::value::Span::default(),
            });
        }
    };
    // Register under the name 'stdlib' so `import 'stdlib` works too
    // (matches the named-module caching invariant from M61).
    let name_opt = module_rc.borrow().name.clone();
    if let Some(name) = name_opt {
        env.modules.insert(name, Rc::clone(&module_rc));
    }
    Ok(module_rc)
}

/// Copy each exported word of the stdlib module into `env.user_ctx` under
/// the same name (mirrors `module::alias_exports_into_user_ctx`, which is
/// private — duplicated here to keep the stdlib module self-contained).
fn alias_stdlib_exports(m: &Rc<std::cell::RefCell<ModuleDef>>, env: &mut Env) {
    let borrow = m.borrow();
    let exports = borrow.exports.borrow();
    for sym in borrow.ctx.words() {
        if exports.contains(&sym) {
            let val = borrow.ctx.get(&sym).unwrap_or(Value::None);
            env.user_ctx.set(sym, val);
        }
    }
}
