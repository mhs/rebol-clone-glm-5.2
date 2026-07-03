//! Script entry points: `run_source*` / `run_series*` / `RunOptions`.
//!
//! Extracted from `interp_walker.rs` in M36 so the 1600-line walker file
//! separates the *eval algorithm* (in `interp_walker`) from the *entry-point
//! plumbing* (this module): lex/parse the source, build the `Env`, install
//! constants/natives, apply `RunOptions`, dispatch on `env.mode`, and catch
//! `EvalError::Quit` as a normal termination with the requested exit code.
//!
//! Public surface unchanged: `interp.rs` re-exports these from
//! `interp_runner` (previously from `interp_walker`); the external
//! `red_eval::run_source_with_exit_opts` / `RunOptions` / etc. are
//! identical.

use red_core::lexer;
use red_core::parser::parse_program;
use red_core::value::{Series, Symbol, Value};
use red_core::{Context, Env, Error, EvalError, EvalMode};

use crate::binding::bind_pass;
use crate::interp::dispatch_block;
use crate::vm::compiler::{compile_block, NativeRegistry};
use crate::vm::lex::Scope;

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
/// script access to trailing CLI args, `walk` forces the tree-walker
/// (`EvalMode::Walk`) instead of the default bytecode VM — set by the CLI's
/// `--walk` flag (M29) for debugging and parity comparison, and `trace`
/// enables per-instr VM tracing to stderr (M31, `--trace` flag).
///
/// M63 additions: `module_paths` populates `system/options/module-path` (a
/// `block!` of `file!` directories searched by `import %file`); `no_stdlib`
/// skips the auto-import of the stdlib module (so `--no-stdlib` makes stdlib
/// words unbound — useful for testing the stdlib's absence).
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    pub allow_shell: bool,
    /// M113: mirror of `Env::allow_network` — off by default per the
    /// sandbox policy (network access is gated, like `call`/`shell`). The
    /// CLI `--allow-network` flag sets this to true.
    pub allow_network: bool,
    pub args: Vec<String>,
    pub walk: bool,
    pub trace: bool,
    pub module_paths: Vec<std::path::PathBuf>,
    pub no_stdlib: bool,
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
/// `allow_shell` and populates `system/options/args`, then evaluates the
/// body via [`dispatch_block`] — which routes to the tree-walker (`Walk`)
/// or the bytecode VM (`Vm`) based on `env.mode`. Since M29 the default is
/// `Vm`; the CLI `--walk` flag (via `opts.walk`) overrides to `Walk` for
/// debugging and the golden parity baseline.
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
    env.allow_network = opts.allow_network;
    // M29: `--walk` forces the tree-walker regardless of the build default.
    // (Under `--features force-walk` the default is already `Walk`, so this
    // is a no-op; without the feature it overrides the `Vm` default.)
    if opts.walk {
        env.mode = EvalMode::Walk;
    }
    // M31: `--trace` enables per-instr VM tracing to stderr. No-op in
    // `--walk` mode (the walker doesn't read `trace_out`; only the VM does).
    if opts.trace {
        env.set_trace(Box::new(std::io::stderr()));
    }
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
    // M113: mirror `allow_shell`/`allow_network` into `system/options/` so
    // scripts can read the gate state. (`install_system` seeds both as
    // `false`; this overwrites them with the actual CLI-flag values. The
    // pre-M113 `allow-shell` slot was vestigial — never updated after init —
    // this fixes that gap as a side-fix while adding `allow-network`.)
    {
        let allow_shell = opts.allow_shell;
        let allow_network = opts.allow_network;
        if let Some(Value::Object(sys)) = env.user_ctx.get(&Symbol::new("system")) {
            if let Some(Value::Object(opts_obj)) = sys.borrow().ctx.get(&Symbol::new("options")) {
                opts_obj
                    .borrow()
                    .ctx
                    .set(Symbol::new("allow-shell"), Value::Logic(allow_shell));
                opts_obj
                    .borrow()
                    .ctx
                    .set(Symbol::new("allow-network"), Value::Logic(allow_network));
            }
        }
    }
    // M63: populate `system/options/module-path` from CLI `--module-path`
    // flags. Overwrites the default `[%./]` set by `install_system`.
    if !opts.module_paths.is_empty() {
        let mp_block = Series::new(
            opts.module_paths
                .iter()
                .map(|p| Value::file(std::rc::Rc::from(p.to_string_lossy().as_ref())))
                .collect(),
        );
        if let Some(Value::Object(sys)) = env.user_ctx.get(&Symbol::new("system")) {
            if let Some(Value::Object(opts_obj)) = sys.borrow().ctx.get(&Symbol::new("options")) {
                opts_obj
                    .borrow()
                    .ctx
                    .set(Symbol::new("module-path"), Value::block(mp_block));
            }
        }
    }
    // M63: auto-import the stdlib unless `--no-stdlib`. The stdlib is a
    // small embedded module (M64 stub surfaced here); its exported words
    // are aliased bare into `user_ctx` so a script can `print str-upper
    // "hi"` without an explicit `import`. The compiled module is cached on
    // `env.stdlib` so the REPL doesn't recompile per line.
    if !opts.no_stdlib {
        if let Err(e) = crate::stdlib::ensure_stdlib(&mut env) {
            // A stdlib failure indicates a build/setup problem (the source
            // is `include_str!`-embedded; it should never fail at runtime).
            // Propagate as a hard error so the failure is visible.
            return Err(Error::Eval(e));
        }
    }
    let block = Value::block(body);
    // Dispatch on `env.mode`: `Walk` → tree-walker (`eval`), `Vm` →
    // compile-on-demand + `vm::run`. `dispatch_block` catches
    // `EvalError::Quit` via the VM's own `run` wrapper (vm.rs catches
    // `Quit` and returns it, matching the walker's contract).
    match dispatch_block(&block, &mut env) {
        Ok(v) => Ok((v, 0)),
        Err(EvalError::Quit(code)) => Ok((Value::None, code)),
        Err(e) => Err(Error::Eval(e)),
    }
}

/// M31: compile a source script (without running it) and return its
/// disassembly. Used by the CLI `--disasm <file.red>` flag.
///
/// `func`: when `None`, disassembles the top-level script body. When
/// `Some(name)`, scans the parsed body's top-level values for a
/// `name: func [spec] [body]` / `name: does [body]` / `name: function [spec]
/// [body]` form and disassembles the matched func's body (compiled with a
/// child scope seeded from the spec, mirroring `ensure_compiled`). The scan
/// is AST-only — no execution — so side-effecting top-level forms (`print`,
/// file I/O) don't run. Returns an error if the named func isn't found or
/// isn't a literal `func`/`does`/`function` form at the top level.
///
/// The output includes per-instr `file:line:col` annotations (from
/// `disasm_with_spans`) when `file` is `Some`. The `src` is used to build a
/// `LineMap` translating byte-offset spans to positions.
pub fn disasm_source(src: &str, func: Option<&str>, file: Option<&str>) -> Result<String, Error> {
    let tokens = lexer::lex(src)?;
    let body = if tokens.is_empty() {
        Series::empty()
    } else {
        let (_header, body) = parse_program(&tokens)?;
        body
    };
    let ctx = Context::new();
    crate::natives::install_constants(&ctx);
    let ctx_rc = bind_pass(&body, ctx);
    let mut env = Env::new_with_output(ctx_rc, Box::new(std::io::sink()));
    crate::natives::register_natives(&mut env);
    let registry = NativeRegistry::from_env(&env);
    let (compiled, src_for_disasm) = match func {
        None => {
            let mut scope = Scope::root(&env.user_ctx);
            let c = compile_block(&body, &mut scope, &registry).map_err(|e| {
                Error::Eval(EvalError::Compile {
                    kind: e.kind,
                    span: e.span,
                })
            })?;
            (c, src)
        }
        Some(name) => {
            let name_sym = Symbol::new(name);
            let body_block = find_top_level_func_body(&body, &name_sym).ok_or_else(|| {
                Error::Eval(EvalError::Native {
                    message: format!(
                        "disasm: no top-level `func`/`does`/`function` form named {:?}",
                        name
                    ),
                    span: red_core::value::Span::default(),
                })
            })?;
            // Compile the func body with a child scope seeded from the spec
            // (mirrors `ensure_compiled`'s lazy-compile path). The spec is
            // parsed via `extract_spec` to get params/refinements/locals.
            let spec_val = body_block.spec.clone();
            let body_series = body_block.body.clone();
            let spec = crate::natives::extract_spec(&spec_val).map_err(|e| {
                Error::Eval(EvalError::Native {
                    message: e.to_string(),
                    span: spec_val.span_or_default(),
                })
            })?;
            let parent = Scope::root(&env.user_ctx);
            let mut child = Scope::child(&parent);
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
            // Pre-collect body SetWords (mirrors `compile_make_func`).
            crate::vm::compiler::collect_setwords_inline_pub(&body_series, &mut child);
            // M31: use `compile_block_for_func_body` (not `compile_block`)
            // so the func's own slot is pre-recorded for recursive
            // `CallUser`/`CallUserGlobal` emission. Without this, a
            // recursive `fib n - 1` inside the body would degrade to
            // `LoadGlobal` (value load) instead of `CallUser` (call),
            // producing wrong disasm output. The slot is looked up from
            // `user_ctx` (the binding pass allocated it for the SetWord).
            let self_slot = env
                .user_ctx
                .names
                .borrow()
                .get(&name_sym)
                .copied()
                .unwrap_or(0);
            let c = crate::vm::compiler::compile_block_for_func_body_pub(
                &body_series,
                &mut child,
                &registry,
                (self_slot as u32, spec.params.len()),
                spec.params.len(),
            )
            .map_err(|e| {
                Error::Eval(EvalError::Compile {
                    kind: e.kind,
                    span: e.span,
                })
            })?;
            (c, src)
        }
    };
    Ok(red_core::disasm_with_spans(
        &compiled,
        Some(src_for_disasm),
        file,
    ))
}

/// A located func body for `disasm_source`: the spec value, the body series,
/// and the calling-word kind (`func`/`does`/`function`). Returned by
/// `find_top_level_func_body`.
struct LocatedFunc {
    spec: Value,
    body: Series,
}

/// M31: scan a parsed body's top-level values for
/// `name: <func|does|function> [spec] [body]` (or `name: does [body]`) and
/// return the spec value + body series for the named func. AST-only — no
/// execution. Returns `None` if not found or the form isn't a literal
/// func/does/function at the top level.
fn find_top_level_func_body(body: &Series, name: &Symbol) -> Option<LocatedFunc> {
    let data = body.data.borrow();
    let n = data.len();
    let mut i = body.index;
    while i < n {
        // Look for `SetWord(name)` followed by a `func`/`does`/`function` word.
        if let Value::SetWord { sym, .. } = &data[i] {
            if sym == name && i + 1 < n {
                let next = &data[i + 1];
                if let Value::Word {
                    sym: kw, binding, ..
                } = next
                {
                    if matches!(binding, red_core::value::Binding::Unbound) {
                        match kw.as_str() {
                            "func" | "function" => {
                                if i + 3 < n
                                    && matches!(&data[i + 2], Value::Block { .. })
                                    && matches!(&data[i + 3], Value::Block { .. })
                                {
                                    let spec = data[i + 2].clone();
                                    let body_series = match &data[i + 3] {
                                        Value::Block { series, .. } => series.clone(),
                                        _ => return None,
                                    };
                                    return Some(LocatedFunc {
                                        spec,
                                        body: body_series,
                                    });
                                }
                            }
                            "does" => {
                                if i + 2 < n && matches!(&data[i + 2], Value::Block { .. }) {
                                    let spec = Value::block(Series::empty());
                                    let body_series = match &data[i + 2] {
                                        Value::Block { series, .. } => series.clone(),
                                        _ => return None,
                                    };
                                    return Some(LocatedFunc {
                                        spec,
                                        body: body_series,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        i += 1;
    }
    None
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
    fn string_path_integer_returns_char() {
        // M38: string char pick returns a `char!` (molded as `#"b"`).
        assert_eq!(mold_to_string(&run("s: \"abc\" s/2")), "#\"b\"");
    }

    #[test]
    fn char_literal_round_trips() {
        assert_eq!(mold_to_string(&run("#\"a\"")), "#\"a\"");
        assert_eq!(mold_to_string(&run("#\"^A\"")), "#\"^A\"");
        assert_eq!(mold_to_string(&run("#\"^(41)\"")), "#\"A\"");
    }

    #[test]
    fn char_pick_negative_index() {
        // `s: "abc" s/-1` — negative index picks from the tail.
        assert_eq!(mold_to_string(&run("s: \"abc\" s/-1")), "#\"c\"");
    }

    #[test]
    fn char_predicate() {
        assert_eq!(mold_to_string(&run("char? #\"a\"")), "true");
        assert_eq!(mold_to_string(&run("char? 5")), "false");
        assert_eq!(mold_to_string(&run("char? \"a\"")), "false");
    }

    #[test]
    fn char_arith_add_int() {
        // `char + int → char`, `char + char → int`.
        assert_eq!(mold_to_string(&run("#\"a\" + 1")), "#\"b\"");
        assert_eq!(mold_to_string(&run("#\"b\" - #\"a\"")), "1");
        assert_eq!(mold_to_string(&run("1 + #\"a\"")), "#\"b\"");
    }

    #[test]
    fn char_comparison() {
        assert_eq!(mold_to_string(&run("#\"a\" < #\"b\"")), "true");
        assert_eq!(mold_to_string(&run("#\"a\" = #\"a\"")), "true");
        assert_eq!(mold_to_string(&run("#\"a\" = #\"b\"")), "false");
    }

    #[test]
    fn char_to_integer_and_back() {
        assert_eq!(mold_to_string(&run("to-integer #\"A\"")), "65");
        assert_eq!(mold_to_string(&run("to-char 66")), "#\"B\"");
        assert_eq!(mold_to_string(&run("make char! 67")), "#\"C\"");
    }

    #[test]
    fn char_min_max() {
        assert_eq!(mold_to_string(&run("min #\"a\" #\"b\"")), "#\"a\"");
        assert_eq!(mold_to_string(&run("max #\"a\" #\"b\"")), "#\"b\"");
    }

    #[test]
    fn string_char_poke_via_set_path() {
        // M38 follow-up: `s/2: #"X"` now works (integer SetPath lexes).
        assert_eq!(mold_to_string(&run("s: \"abc\" s/2: #\"X\" s")), "\"aXc\"");
        assert_eq!(mold_to_string(&run("s: \"abc\" s/-1: #\"Z\" s")), "\"abZ\"");
    }

    #[test]
    fn block_integer_set_path() {
        // M38 follow-up: `b/2: 99` now works (integer SetPath lexes).
        assert_eq!(mold_to_string(&run("b: [1 2 3] b/2: 99 b")), "[1 99 3]");
    }

    #[test]
    fn append_string_with_string() {
        assert_eq!(mold_to_string(&run("append \"foo\" \"bar\"")), "\"foobar\"");
    }

    #[test]
    fn append_string_with_char() {
        assert_eq!(mold_to_string(&run("append \"foo\" #\"s\"")), "\"foos\"");
    }

    #[test]
    fn append_string_with_block_splice() {
        assert_eq!(
            mold_to_string(&run("append \"foo\" [#\"a\" #\"b\"]")),
            "\"fooab\""
        );
    }

    #[test]
    fn insert_string_with_string() {
        assert_eq!(mold_to_string(&run("insert \"foo\" \"bar\"")), "\"barfoo\"");
    }

    #[test]
    fn insert_string_with_char() {
        assert_eq!(mold_to_string(&run("insert \"foo\" #\"X\"")), "\"Xfoo\"");
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

    // --- M31: disasm_source -----------------------------------------------

    #[test]
    fn disasm_source_top_level_contains_instrs_and_positions() {
        let out = disasm_source("1 + 2", None, Some("test.red")).expect("disasm");
        assert!(out.contains("ConstInt(1)"), "got:\n{out}");
        assert!(out.contains("ConstInt(2)"), "got:\n{out}");
        assert!(out.contains("Call("), "got:\n{out}");
        assert!(out.contains("Return"), "got:\n{out}");
        assert!(
            out.contains("test.red:1:1"),
            "expected position prefix; got:\n{out}"
        );
    }

    #[test]
    fn disasm_source_named_func_compiles_body() {
        let src = "fib: func [n][either n < 2 [n][(fib n - 1) + fib n - 2]]";
        let out = disasm_source(src, Some("fib"), None).expect("disasm");
        // The func body's disasm should reference the recursive call
        // (CallUser/CallUserGlobal) and the `<` comparison + `either` jump.
        assert!(
            out.contains("CallUser") || out.contains("CallUserGlobal"),
            "expected CallUser in fib body; got:\n{out}"
        );
        assert!(out.contains("JumpIfFalse"), "got:\n{out}");
    }

    #[test]
    fn disasm_source_named_does_func() {
        let src = "noop: does [42]";
        let out = disasm_source(src, Some("noop"), None).expect("disasm");
        assert!(out.contains("ConstInt(42)"), "got:\n{out}");
        assert!(out.contains("Return"), "got:\n{out}");
    }

    #[test]
    fn disasm_source_named_func_not_found_errors() {
        let out = disasm_source("x: 5", Some("nope"), None);
        assert!(out.is_err(), "expected error for missing func");
    }

    #[test]
    fn disasm_source_no_side_effects() {
        // `print` is a native; disasm_source must NOT run it. The disasm
        // should contain a `Call(print_idx, ...)` instr, not execute it.
        let out = disasm_source("print 1", None, None).expect("disasm");
        assert!(out.contains("Call("), "got:\n{out}");
        // No stdout was captured (we used `io::sink()`), so correctness is
        // "didn't panic / didn't error" — the print native wasn't invoked.
    }

    // --- M63: CLI flags + system/options extension -------------------------

    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

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

    fn run_opts_capture(src: &str, opts: &RunOptions) -> (Result<Value, Error>, Vec<u8>) {
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let result = run_source_with_exit_opts(src, Box::new(BufferWriter(Rc::clone(&buf))), opts);
        let out = buf.borrow().clone();
        match result {
            Ok((v, _code)) => (Ok(v), out),
            Err(e) => (Err(e), out),
        }
    }

    fn out_opts(src: &str, opts: &RunOptions) -> String {
        String::from_utf8(run_opts_capture(src, opts).1).unwrap()
    }

    #[test]
    fn stdlib_auto_import_makes_bare_word_resolvable() {
        // Default RunOptions auto-imports the stdlib; `str-upper` is an
        // exported stdlib word, so bare use resolves.
        assert_eq!(
            out_opts("print str-upper \"hi\"", &RunOptions::default()).trim(),
            "HI"
        );
    }

    #[test]
    fn no_stdlib_makes_stdlib_words_unbound() {
        // `--no-stdlib` skips auto-import; `str-upper` stays unbound.
        let opts = RunOptions {
            no_stdlib: true,
            ..Default::default()
        };
        let (res, _) = run_opts_capture("print str-upper \"hi\"", &opts);
        match res {
            Err(Error::Eval(EvalError::UnboundWord { sym, .. })) => {
                assert_eq!(sym.as_str(), "str-upper");
            }
            other => panic!("expected UnboundWord, got {other:?}"),
        }
    }

    #[test]
    fn module_path_flag_populates_system_options() {
        // `--module-path /tmp` overwrites the default `[%./]` block. PathBuf
        // normalization strips the trailing slash, so the mold is `[%/tmp]`.
        let mut opts = RunOptions::default();
        opts.module_paths.push(std::path::PathBuf::from("/tmp"));
        let (v, _) = run_opts_capture("system/options/module-path", &opts);
        let v = v.expect("module-path probe should succeed");
        assert_eq!(mold_to_string(&v), "[%/tmp]");
    }

    #[test]
    fn module_path_flag_repeatable_appends() {
        let mut opts = RunOptions::default();
        opts.module_paths.push(std::path::PathBuf::from("/tmp"));
        opts.module_paths.push(std::path::PathBuf::from("/usr/lib"));
        let (v, _) = run_opts_capture("system/options/module-path", &opts);
        let v = v.expect("module-path probe should succeed");
        assert_eq!(mold_to_string(&v), "[%/tmp %/usr/lib]");
    }

    #[test]
    fn module_path_default_is_cwd_block() {
        // With no `--module-path`, the default `[%./]` set by install_system
        // is preserved.
        let (v, _) = run_opts_capture("system/options/module-path", &RunOptions::default());
        let v = v.expect("module-path probe should succeed");
        assert_eq!(mold_to_string(&v), "[%./]");
    }

    #[test]
    fn import_file_searches_module_path_when_cwd_misses() {
        // Write a temp module into a scratch dir, then `import %name.red`
        // without referencing the dir — `--module-path` should find it.
        let dir = tempfile::tempdir().expect("tempdir");
        let mod_path = dir.path().join("m63_search.red");
        std::fs::write(&mod_path, "module [x: 42 export 'x]").expect("write");
        let src = "import %m63_search.red print x";
        let mut opts = RunOptions::default();
        opts.module_paths.push(dir.path().to_path_buf());
        assert_eq!(out_opts(src, &opts).trim(), "42");
    }

    #[test]
    fn import_file_without_module_path_still_errors_when_missing() {
        // Without `--module-path`, a cwd-missing import errors (search
        // didn't kick in).
        let src = "import %definitely_not_here_m63.red print x";
        let (res, _) = run_opts_capture(src, &RunOptions::default());
        let err = res.expect_err("expected import error");
        let msg = match err {
            Error::Eval(EvalError::Native { message, .. }) => message,
            Error::Eval(EvalError::Raised(ev)) => ev.message.clone(),
            other => panic!("expected Native/Raised error, got {other:?}"),
        };
        assert!(msg.contains("cannot read"), "{msg}");
    }

    #[test]
    fn stdlib_cached_across_repeated_calls() {
        // Calling ensure_stdlib twice shouldn't panic or recompile; the
        // second call re-aliases the cached module.
        let src = "print str-upper \"a\" print str-upper \"b\"";
        assert_eq!(out_opts(src, &RunOptions::default()).trim(), "A\nB");
    }

    #[test]
    fn stdlib_does_not_shadow_user_definitions() {
        // A user-defined `gcd` should shadow the stdlib's `gcd` (Red's
        // import-shadows-locals behavior — but here the user defines the
        // word AFTER auto-import, so the SetWord overwrites the aliased
        // slot).
        let src = "gcd: func [a b][a + b] print gcd 3 4";
        assert_eq!(out_opts(src, &RunOptions::default()).trim(), "7");
    }
}
