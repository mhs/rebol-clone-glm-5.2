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
/// script access to trailing CLI args, and `walk` forces the tree-walker
/// (`EvalMode::Walk`) instead of the default bytecode VM — set by the CLI's
/// `--walk` flag (M29) for debugging and parity comparison.
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    pub allow_shell: bool,
    pub args: Vec<String>,
    pub walk: bool,
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
    // M29: `--walk` forces the tree-walker regardless of the build default.
    // (Under `--features force-walk` the default is already `Walk`, so this
    // is a no-op; without the feature it overrides the `Vm` default.)
    if opts.walk {
        env.mode = EvalMode::Walk;
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
