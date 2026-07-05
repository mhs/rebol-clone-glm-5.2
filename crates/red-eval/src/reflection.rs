//! M134: eval reflection natives. v0.10 ships `dump` and `errors`; the
//! user-level `trace` toggle is deferred to v0.11 (it requires per-expression
//! hooks in both the walker and VM eval loops, which breaks the v0.10
//! "additive native only" non-goal — documented in `project-brief.md`).

use std::rc::Rc;

use red_core::value::{Series, Span, Symbol, Value};
use red_core::{Env, EvalError, NativeFn, RefineArgs};

use crate::natives::arity_err;

/// `dump value` — prints `name: <mold>` for debugging. Takes the word
/// unevaluated (registered in `uneval_first`) so the source name is shown.
/// For a non-word value, prints just the mold.
fn dump_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "dump", 1, args.len()));
    }
    let (label, val) = match &args[0] {
        Value::Word { sym, .. }
        | Value::LitWord { sym, .. }
        | Value::GetWord { sym, .. }
        | Value::SetWord { sym, .. } => {
            let v = env
                .user_ctx
                .get(sym)
                .or_else(|| env.natives.get(sym).map(|fd| Value::Func(Rc::clone(fd))))
                .unwrap_or(Value::None);
            (sym.as_str().to_string(), v)
        }
        other => (red_core::mold_to_string(other), other.clone()),
    };
    let molded = red_core::mold_to_string(&val);
    let _ = writeln!(env.out, "{label}: {molded}");
    Ok(val)
}

/// `errors` — returns a `block!` of `lit-word!`s enumerating the known error
/// categories (the `kind` field of `ErrorValue`). Mirrors the type words
/// `make error!` accepts.
fn errors_catalog(_args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    let kinds = [
        "script", "math", "io", "user", "syntax", "type", "access", "memory", "internal",
    ];
    let items: Vec<Value> = kinds
        .iter()
        .map(|k| Value::LitWord {
            sym: Symbol::new(k),
            span: Span::new(0, 0),
        })
        .collect();
    Ok(Value::Block {
        series: Series::new(items),
        span: Span::new(0, 0),
    })
}

pub fn register_reflection_natives(env: &mut Env) {
    use std::rc::Rc as Rc2;
    let reg = |env: &mut Env, name: &str, f: NativeFn, arity: usize| {
        let params: Vec<Symbol> = (0..arity)
            .map(|i| Symbol::new(&format!("__arg{i}")))
            .collect();
        env.natives.insert(
            Symbol::new(name),
            Rc2::new(red_core::value::FuncDef {
                params,
                native: Some(f),
                ..Default::default()
            }),
        );
    };
    reg(env, "dump", dump_native as NativeFn, 1);
    reg(env, "errors", errors_catalog as NativeFn, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_returns_block() {
        let mut env = Env::new(std::rc::Rc::new(red_core::Context::new()));
        let r = errors_catalog(&[], &RefineArgs::default(), &mut env).unwrap();
        match r {
            Value::Block { series, .. } => assert!(series.data.borrow().len() >= 6),
            _ => panic!("expected block!"),
        }
    }
}
