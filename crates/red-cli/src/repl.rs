//! Interactive REPL (Milestone 11).
//!
//! Read-eval-print loop using `rustyline` for line editing. State (the user
//! context + native registry) persists across lines: each line is parsed,
//! then bound against the *live* `Env.user_ctx` via `bind_pass_into` (which
//! grows the shared context in place), then evaluated. Multi-line input is
//! supported by accumulating lines until a parse no longer reports
//! `MissingClose`.
//!
//! The core per-line logic (`eval_repl_line`) is factored out from the
//! rustyline driver so inline tests can drive it over a plain string without
//! a tty.

use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::rc::Rc;

#[cfg(test)]
use std::cell::RefCell;

use red_eval::{
    bind_pass_into, eval, install_constants, load_source, mold_to_string, register_natives,
    render_error, Context, Env, Error, EvalError, ParseError, Value,
};

/// Build a fresh REPL environment: empty user context with constants
/// installed, all natives registered, output sent to `out`.
fn build_env(out: Box<dyn Write>) -> Env {
    let ctx = Context::new();
    install_constants(&ctx);
    let ctx_rc = Rc::new(ctx);
    let mut env = Env::new_with_output(ctx_rc, out);
    register_natives(&mut env);
    env
}

/// Outcome of feeding an accumulated line buffer through the REPL evaluator.
enum LineAction {
    /// Parsed clean and ran; result (if non-`none`) already written to
    /// `env.out`. The buffer should be cleared for the next line.
    Evaluated,
    /// The input has an unclosed `[`/`(` — keep the buffer and prompt for a
    /// continuation line.
    NeedMoreInput,
    /// A parse/lex error. Carries the rendered message for the driver to
    /// write to stderr; the buffer should be cleared.
    Failed(String),
    /// The evaluated code called `exit`/`quit` — the driver should stop the
    /// session.
    Quit,
}

/// Parse + bind + eval one accumulated line buffer against `env`. The molded
/// result of the last value is written to `env.out` (unless it's `none`).
/// `quit`/`exit` are handled by the caller (driver), not here, so that they
/// only act as REPL commands at a fresh prompt rather than mid-block.
fn eval_repl_line(buffer: &str, env: &mut Env) -> LineAction {
    match load_source(buffer) {
        Ok(body) => {
            if body.data.borrow().is_empty() {
                return LineAction::Evaluated;
            }
            bind_pass_into(&body, &env.user_ctx);
            match eval(&Value::block(body), env) {
                Err(EvalError::Return(_)) => LineAction::Evaluated,
                Err(EvalError::Quit(_)) => LineAction::Quit,
                Err(e) => LineAction::Failed(render_error(None, buffer, &Error::Eval(e))),
                Ok(Value::None) => LineAction::Evaluated,
                Ok(v) => {
                    let _ = writeln!(env.out, "{}", mold_to_string(&v));
                    LineAction::Evaluated
                }
            }
        }
        Err(Error::Parse(ParseError::MissingClose { .. })) => LineAction::NeedMoreInput,
        Err(e) => LineAction::Failed(render_error(None, buffer, &e)),
    }
}

/// Handle one physical input line against the accumulating `buffer` and
/// `env`. Returns `false` if the REPL should exit (saw `quit`/`exit` at a
/// fresh prompt, or the evaluated code called `exit`/`quit`), `true` to
/// continue. Owns quit-detection, buffer accumulation, multi-line
/// continuation, eval, and result/error printing — shared by the interactive
/// (rustyline) and piped-stdin drivers.
fn handle_line(line: &str, buffer: &mut String, env: &mut Env) -> bool {
    if buffer.is_empty() {
        let t = line.trim();
        if t.is_empty() {
            return true;
        }
        if matches!(t, "quit" | "exit") {
            return false;
        }
    }

    if !buffer.is_empty() {
        buffer.push('\n');
    }
    buffer.push_str(line);

    match eval_repl_line(buffer, env) {
        LineAction::NeedMoreInput => true,
        LineAction::Evaluated => {
            buffer.clear();
            true
        }
        LineAction::Failed(msg) => {
            eprintln!("{msg}");
            buffer.clear();
            true
        }
        LineAction::Quit => false,
    }
}

/// Entry point for `red` invoked with no file argument.
pub fn run_repl() -> ExitCode {
    let mut env = build_env(Box::new(io::stdout()));
    let mut buffer = String::new();

    if io::stdin().is_terminal() {
        // Interactive: line editing via rustyline.
        let mut rl = match rustyline::DefaultEditor::new() {
            Ok(ed) => ed,
            Err(e) => {
                eprintln!("*** Error: failed to start REPL: {e}");
                return ExitCode::from(1);
            }
        };
        loop {
            let prompt = if buffer.is_empty() { "red> " } else { "...   " };
            let line = match rl.readline(prompt) {
                Ok(l) => l,
                Err(rustyline::error::ReadlineError::Eof) => break,
                Err(rustyline::error::ReadlineError::Interrupted) => {
                    // Ctrl-C: discard any partial multi-line input and
                    // re-prompt on a fresh line.
                    if buffer.is_empty() {
                        println!();
                    } else {
                        buffer.clear();
                    }
                    continue;
                }
                Err(e) => {
                    eprintln!("*** Error: {e}");
                    break;
                }
            };
            let _ = rl.add_history_entry(&line);
            if !handle_line(&line, &mut buffer, &mut env) {
                break;
            }
        }
    } else {
        // Non-interactive (piped stdin / `cat file | red`): read plain lines
        // without rustyline so behavior is deterministic without a tty.
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if !handle_line(&l, &mut buffer, &mut env) {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("*** Error: {e}");
                    break;
                }
            }
        }
    }

    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------
// Testable in-memory driver (no rustyline, no tty)
// ---------------------------------------------------------------------------

/// A `Write` sink backed by a shared `Rc<RefCell<Vec<u8>>>` so the test can
/// read back everything the REPL wrote (the molded result of each line, plus
/// any `print`/`prin` output from natives).
#[cfg(test)]
#[derive(Clone)]
struct BufferSink(Rc<RefCell<Vec<u8>>>);

#[cfg(test)]
impl Write for BufferSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Drive the REPL over `input` (lines separated by `\n`), returning
/// everything written to the environment's output sink. `quit`/`exit` on a
/// fresh prompt stop the session. Used by inline tests; mirrors `run_repl`
/// but without rustyline.
#[cfg(test)]
fn repl_session(input: &str) -> String {
    let sink = Rc::new(RefCell::new(Vec::<u8>::new()));
    let mut env = build_env(Box::new(BufferSink(Rc::clone(&sink))));
    let mut buffer = String::new();

    for raw in input.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if !handle_line(line, &mut buffer, &mut env) {
            break;
        }
    }

    let bytes = sink.borrow().clone();
    String::from_utf8(bytes).expect("REPL output was not UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repl_evaluates_integer() {
        assert_eq!(repl_session("5\n"), "5\n");
    }

    #[test]
    fn repl_persists_assignment() {
        // `x: 10` evaluates to 10 (the assigned value, Red semantics) and is
        // molded; `x` on the next line reads the persisted slot → also 10.
        assert_eq!(repl_session("x: 10\nx\n"), "10\n10\n");
    }

    #[test]
    fn repl_multiline_block() {
        // Unclosed `[` on line 1 → continuation; line 2 closes it; line 3
        // references the bound word. `x: [1 2]` molds to `[1 2]` (the
        // assigned value), then `x` → `[1 2]` again.
        assert_eq!(repl_session("x: [\n1 2\n]\nx\n"), "[1 2]\n[1 2]\n");
    }

    #[test]
    fn repl_none_suppressed() {
        // `none` evaluates to None → not printed.
        assert_eq!(repl_session("none\n"), "");
    }

    #[test]
    fn repl_error_continues() {
        // First line errors (unbound word), second line still evaluates.
        let out = repl_session("foo\n5\n");
        assert!(out.contains("5\n"));
    }

    #[test]
    fn repl_func_sees_global_mutation() {
        // Validates the interior-mutability approach: a function defined at
        // the REPL closes over the *live* user context, so mutating a global
        // after definition is visible when the function runs. Each line's
        // result is printed: `g: 1`→1, `f: func…`→`#[function]`, `g: 2`→2,
        // `f`→2 (the mutation is visible inside the function body).
        assert_eq!(
            repl_session("g: 1\nf: func [][g]\ng: 2\nf\n"),
            "1\n#[function]\n2\n2\n"
        );
    }

    #[test]
    fn repl_quit_stops_session() {
        // `quit` terminates; the line after is never evaluated.
        let out = repl_session("5\nquit\n10\n");
        assert_eq!(out, "5\n");
    }

    #[test]
    fn repl_blank_line_re_prompts() {
        // A blank line does nothing and doesn't error.
        let out = repl_session("\n\n5\n");
        assert_eq!(out, "5\n");
    }
}
