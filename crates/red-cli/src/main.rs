//! `red` — CLI entry point for the POC Red clone.
//!
//! Milestone 6 scope: run a single `.red` file via `run_source`. `--help`/
//! `--version` print and exit. No-args / REPL is M11. Errors render via the
//! `Error` `Display` (which already prefixes `*** Error:`) and exit 1.

use std::io::{self, Write};
use std::process::ExitCode;

const VERSION: &str = "red 0.0.1";

const HELP: &str = "\
red — a POC Red clone

USAGE:
    red <file.red>      Load and evaluate a Red source file
    red --help          Show this help message
    red --version       Print version

The interpreter reads the file, evaluates it, and exits. Script output
(native `print`/`prin`/`probe`) goes to stdout; the final value is not
printed by the CLI (use `print` in the script). Errors print to stderr
as `*** Error: <msg>` and exit with code 1.

REPL mode is not implemented in this milestone.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.as_slice() {
        [] => {
            // No-args: print usage to stderr, exit 1 (REPL is M11).
            let _ = io::stderr().write_all(HELP.as_bytes());
            ExitCode::from(1)
        }
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
    match red_eval::run_source(&src) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            // `Error`/`EvalError` `Display` already prefixes `*** Error:`.
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
