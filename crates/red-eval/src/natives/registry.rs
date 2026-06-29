//! Native registration: helpers (`fixed_native`/`infix_native`/
//! `variadic_native`/`reg_refined`) and the top-level `register_natives`
//! that wires every native group into `env.natives`, plus
//! `install_constants` which seeds the `none`/`true`/`false`/`newline`/
//! `system` words into a user context.
//!
//! The per-group natives live in `natives::{io, compare, control, func, eval,
//! words}`; arithmetic (`+ - * /`) lives in `crate::math` alongside the
//! prefix aliases.

use std::rc::Rc;

use red_core::context::Context;
use red_core::value::{FuncDef, Series, Symbol, Value};
use red_core::{Env, NativeFn};

use super::compare::{
    and_op, equal, greater_equal, greater_than, less_equal, less_than, not_equal, not_op, or_op,
};
use super::control::{
    all_native, any_native, attempt_native, break_native, case_native, catch_native, cause_error,
    comment_native, continue_native, default_native, either, exit_native, if_native, loop_native,
    repeat, switch_native, throw_native, try_native, until, while_native,
};
use super::eval::{do_native, load_native, reduce};
use super::func::{does_native, func_native, function_native, function_predicate, return_native};
use super::io::{prin, print, probe};
use super::words::{
    bind_native, char_predicate, get_native, register_word_predicate_natives, set_native,
    use_native, value_predicate,
};

// ---------------------------------------------------------------------------
// Native-wrapping helpers
// ---------------------------------------------------------------------------

fn fixed_native(f: NativeFn, arity: usize) -> Rc<FuncDef> {
    let params: Vec<Symbol> = (0..arity)
        .map(|i| Symbol::new(&format!("__arg{i}")))
        .collect();
    Rc::new(FuncDef {
        params,
        native: Some(f),
        variadic: false,
        infix: false,
        ..Default::default()
    })
}

fn infix_native(f: NativeFn, arity: usize) -> Rc<FuncDef> {
    let params: Vec<Symbol> = (0..arity)
        .map(|i| Symbol::new(&format!("__arg{i}")))
        .collect();
    Rc::new(FuncDef {
        params,
        native: Some(f),
        variadic: false,
        infix: true,
        ..Default::default()
    })
}

/// Build a variadic native: collects all remaining expressions up to the next
/// native word. Used by `make` (which accepts 2 or 3 args depending on form).
fn variadic_native(f: NativeFn) -> Rc<FuncDef> {
    Rc::new(FuncDef {
        params: Vec::new(),
        native: Some(f),
        variadic: true,
        infix: false,
        ..Default::default()
    })
}

/// Register a native that declares refinements. `refines` is a list of
/// `(refinement_name, refinement_arity)`; each refinement's argument words
/// are synthetic placeholders (the count drives dispatch). Mirrors the
/// `reg_refined` closures in `series.rs`/`strings.rs`; lifted here so M16's
/// `switch`/`case` can use the same pattern without re-defining it.
fn reg_refined(env: &mut Env, name: &str, f: NativeFn, arity: usize, refines: &[(&str, usize)]) {
    let params: Vec<Symbol> = (0..arity)
        .map(|i| Symbol::new(&format!("__arg{i}")))
        .collect();
    let refinements: Vec<(Symbol, Vec<Symbol>)> = refines
        .iter()
        .map(|(rname, rarity)| {
            let rargs: Vec<Symbol> = (0..*rarity)
                .map(|i| Symbol::new(&format!("__{rname}_arg{i}")))
                .collect();
            (Symbol::new(rname), rargs)
        })
        .collect();
    env.natives.insert(
        Symbol::new(name),
        Rc::new(FuncDef {
            params,
            refinements,
            native: Some(f),
            variadic: false,
            infix: false,
            ..Default::default()
        }),
    );
}

// ---------------------------------------------------------------------------
// register_natives
// ---------------------------------------------------------------------------

/// Register all native words (M6 I/O + M7 arithmetic/comparison/logic/
/// control-flow/loops/eval) into `env.natives`.
pub fn register_natives(env: &mut Env) {
    // I/O (M6)
    env.natives
        .insert(Symbol::new("print"), fixed_native(print as NativeFn, 1));
    env.natives
        .insert(Symbol::new("prin"), fixed_native(prin as NativeFn, 1));
    env.natives
        .insert(Symbol::new("probe"), fixed_native(probe as NativeFn, 1));

    // Arithmetic (M7, infix). The infix `+ - * /` implementations live in
    // `crate::math` alongside their prefix aliases (`add`/`subtract`/…).
    env.natives.insert(
        Symbol::new("+"),
        infix_native(crate::math::add as NativeFn, 2),
    );
    env.natives.insert(
        Symbol::new("-"),
        infix_native(crate::math::subtract as NativeFn, 2),
    );
    env.natives.insert(
        Symbol::new("*"),
        infix_native(crate::math::multiply as NativeFn, 2),
    );
    env.natives.insert(
        Symbol::new("/"),
        infix_native(crate::math::divide as NativeFn, 2),
    );
    env.natives.insert(
        Symbol::new("//"),
        infix_native(crate::math::modulo as NativeFn, 2),
    );

    // Comparison (M7, infix)
    env.natives
        .insert(Symbol::new("="), infix_native(equal as NativeFn, 2));
    env.natives
        .insert(Symbol::new("<>"), infix_native(not_equal as NativeFn, 2));
    env.natives
        .insert(Symbol::new("<"), infix_native(less_than as NativeFn, 2));
    env.natives
        .insert(Symbol::new(">"), infix_native(greater_than as NativeFn, 2));
    env.natives
        .insert(Symbol::new("<="), infix_native(less_equal as NativeFn, 2));
    env.natives.insert(
        Symbol::new(">="),
        infix_native(greater_equal as NativeFn, 2),
    );

    // Logic (M7)
    env.natives
        .insert(Symbol::new("and"), infix_native(and_op as NativeFn, 2));
    env.natives
        .insert(Symbol::new("or"), infix_native(or_op as NativeFn, 2));
    env.natives
        .insert(Symbol::new("not"), fixed_native(not_op as NativeFn, 1));

    // Conditionals (M7)
    env.natives
        .insert(Symbol::new("if"), fixed_native(if_native as NativeFn, 2));
    env.natives
        .insert(Symbol::new("either"), fixed_native(either as NativeFn, 3));

    // Loops (M7)
    env.natives.insert(
        Symbol::new("loop"),
        fixed_native(loop_native as NativeFn, 1),
    );
    env.natives
        .insert(Symbol::new("repeat"), fixed_native(repeat as NativeFn, 3));
    env.natives
        .insert(Symbol::new("until"), fixed_native(until as NativeFn, 1));
    env.natives.insert(
        Symbol::new("while"),
        fixed_native(while_native as NativeFn, 2),
    );

    // Control flow (M7)
    env.natives.insert(
        Symbol::new("break"),
        fixed_native(break_native as NativeFn, 0),
    );
    env.natives.insert(
        Symbol::new("continue"),
        fixed_native(continue_native as NativeFn, 0),
    );

    // Control flow expansion (M16)
    reg_refined(
        env,
        "switch",
        switch_native as NativeFn,
        2,
        &[("default", 1), ("case", 0)],
    );
    reg_refined(
        env,
        "case",
        case_native as NativeFn,
        1,
        &[("default", 1), ("all", 0)],
    );
    env.natives.insert(
        Symbol::new("default"),
        fixed_native(default_native as NativeFn, 2),
    );
    env.natives
        .insert(Symbol::new("all"), fixed_native(all_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("any"), fixed_native(any_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("try"), fixed_native(try_native as NativeFn, 1));
    env.natives.insert(
        Symbol::new("attempt"),
        fixed_native(attempt_native as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("catch"),
        fixed_native(catch_native as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("throw"),
        fixed_native(throw_native as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("cause-error"),
        variadic_native(cause_error as NativeFn),
    );
    env.natives.insert(
        Symbol::new("comment"),
        fixed_native(comment_native as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("exit"),
        variadic_native(exit_native as NativeFn),
    );
    env.natives.insert(
        Symbol::new("quit"),
        variadic_native(exit_native as NativeFn),
    );

    // Eval (M7 + M16.1)
    env.natives
        .insert(Symbol::new("do"), fixed_native(do_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("reduce"), fixed_native(reduce as NativeFn, 1));
    env.natives.insert(
        Symbol::new("load"),
        fixed_native(load_native as NativeFn, 1),
    );

    // Functions (M9)
    env.natives.insert(
        Symbol::new("func"),
        fixed_native(func_native as NativeFn, 2),
    );
    env.natives.insert(
        Symbol::new("does"),
        fixed_native(does_native as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("function"),
        fixed_native(function_native as NativeFn, 2),
    );
    env.natives.insert(
        Symbol::new("function?"),
        fixed_native(function_predicate as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("return"),
        variadic_native(return_native as NativeFn),
    );

    // Binding (M9)
    env.natives
        .insert(Symbol::new("get"), fixed_native(get_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("set"), fixed_native(set_native as NativeFn, 2));
    env.natives.insert(
        Symbol::new("value?"),
        fixed_native(value_predicate as NativeFn, 1),
    );
    env.natives.insert(
        Symbol::new("char?"),
        fixed_native(char_predicate as NativeFn, 1),
    );
    env.natives
        .insert(Symbol::new("use"), fixed_native(use_native as NativeFn, 2));
    env.natives.insert(
        Symbol::new("bind"),
        fixed_native(bind_native as NativeFn, 2),
    );

    // M39: type predicates + type?/types-of.
    register_word_predicate_natives(env);

    // Series (M8)
    crate::series::register_series_natives(env);

    // Parse dialect (M10)
    env.natives.insert(
        Symbol::new("parse"),
        fixed_native(crate::parse::parse_native as NativeFn, 2),
    );

    // Type conversions + make/to/form (M14)
    crate::convert::register_convert_natives(env);

    // String manipulation natives (M15)
    crate::strings::register_string_natives(env);

    // Math + bitwise natives (M17)
    crate::math::register_math_natives(env);

    // Objects & contexts (M18)
    crate::object::register_object_natives(env);

    // Path natives (M19)
    crate::path::register_path_natives(env);

    // File & shell I/O (M20)
    crate::io::register_io_natives(env);

    // M30: invalidate the VM's indexed-natives cache so the next `vm::run`
    // rebuilds it from the now-complete `natives` map. (Cheap: the rebuild
    // is O(n) on the first `vm::run`, then cached for the rest of the
    // process. Without this, the VM would index a partial native set on its
    // first call if `register_natives` were interleaved with VM runs — which
    // it isn't in practice, but the defensive invalidate keeps the invariant
    // "natives_by_idx is consistent with env.natives" true at all times.)
    env.invalidate_native_index();
}

/// Install the predefined constant words (`none`, `true`, `false`, `newline`,
/// `system`) into a user context. Must be called before `bind_pass` so
/// references to these words get `Local` bindings to the constant slots.
///
/// `system` is an object with an `options` field (also an object) carrying
/// `args` (a block of CLI arg strings, empty by default), `allow-shell`
/// (logic, false by default), and `path` (the current working directory as a
/// file!). The CLI mutates these slots after `install_constants` to reflect
/// its flags; `change-dir` updates `path` at runtime.
pub fn install_constants(ctx: &Context) {
    ctx.set(Symbol::new("none"), Value::None);
    ctx.set(Symbol::new("true"), Value::Logic(true));
    ctx.set(Symbol::new("false"), Value::Logic(false));
    ctx.set(
        Symbol::new("newline"),
        Value::string(std::rc::Rc::from("\n")),
    );
    install_system(ctx);
}

/// Build and install the `system` object. The object mirrors `Env`'s
/// `allow_shell`/`cwd` fields for script-readable access via
/// `system/options/args` etc. The CLI populates `args`/`allow-shell` after
/// `install_constants` by writing into the slots directly.
fn install_system(ctx: &Context) {
    use red_core::value::ObjectDef;
    use std::rc::Rc as StdRc;
    // options object: args, allow-shell, path.
    let opts = ObjectDef::new();
    opts.ctx
        .set(Symbol::new("args"), Value::block(Series::empty()));
    opts.ctx
        .set(Symbol::new("allow-shell"), Value::Logic(false));
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    opts.ctx.set(
        Symbol::new("path"),
        Value::file(StdRc::from(cwd.to_string_lossy().as_ref())),
    );

    // system object: options.
    let sys = ObjectDef::new();
    sys.ctx.set(Symbol::new("options"), Value::object(opts));
    ctx.set(Symbol::new("system"), Value::object(sys));
}
