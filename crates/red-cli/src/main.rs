//! `red` — CLI entry point for the POC Red clone.
//!
//! `red <file.red>` loads and evaluates a single source file. `red` (no
//! args) drops into an interactive REPL (Milestone 11) using `rustyline`;
//! state persists across lines and `quit`/`exit` or Ctrl-D exits.
//! `--help`/`--version` print and exit. Errors render via the `Error`
//! `Display` (which already prefixes `*** Error:`) and, in file mode, exit 1.

mod repl;

use std::io::{self, Write};
use std::process::ExitCode;

const VERSION: &str = "red 0.1.0";

const HELP: &str = "\
red — a POC Red clone

USAGE:
    red <file.red>      Load and evaluate a Red source file
    red                 Interactive REPL (quit with `quit`/`exit` or Ctrl-D)
    red --help          Show this help message
    red --version       Print version

In file mode the interpreter reads the file, evaluates it, and exits.
Script output (native `print`/`prin`/`probe`) goes to stdout; the final
value is not printed by the CLI (use `print` in the script). Errors print
to stderr as `*** Error: <msg>` and exit with code 1.

In REPL mode each line is evaluated against the persistent user context;
the molded result of each line is printed unless it is `none`. Multi-line
blocks are supported: an unclosed `[` or `(` prompts for continuation
lines. Ctrl-C abandons the current input; Ctrl-D exits.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.as_slice() {
        [] => repl::run_repl(),
        [flag] if flag == "--help" || flag == "-h" => {
            let _ = io::stdout().write_all(HELP.as_bytes());
            ExitCode::SUCCESS
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("{VERSION}");
            ExitCode::SUCCESS
        }
        [path] => run_file(path),
        _ => {
            let _ = io::stderr().write_all(HELP.as_bytes());
            ExitCode::from(1)
        }
    }
}

fn run_file(path: &str) -> ExitCode {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("*** Error: cannot read {path:?}: {e}");
            return ExitCode::from(1);
        }
    };
    match red_eval::run_source_with_exit(&src) {
        Ok((_, code)) => {
            if code == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(code as u8)
            }
        }
        Err(e) => {
            // Render with `file:line:col:` location using the source we
            // already hold + the error's byte-offset span.
            eprintln!("{}", red_eval::render_error(Some(path), &src, &e));
            ExitCode::from(1)
        }
    }
}
