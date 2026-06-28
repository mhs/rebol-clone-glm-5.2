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

const VERSION: &str = concat!("red ", env!("CARGO_PKG_VERSION"));

const HELP: &str = "\
red — a Red subset clone

USAGE:
    red [--allow-shell] [--walk] [--trace] <file.red> [args...]   Load and evaluate a Red source file
    red --disasm <file.red>                                       Compile and disassemble the script (no run)
    red --disasm-func <name> <file.red>                           Disassemble a named top-level func
    red                                                           Interactive REPL (quit with `quit`/`exit` or Ctrl-D)
    red --help                                                    Show this help message
    red --version                                                 Print version

In file mode the interpreter reads the file, evaluates it, and exits.
Trailing args after the script path are exposed to the script as
`system/options/args` (a block of strings). `--allow-shell` enables the
`call`/`shell` natives (disabled by default for test safety). `--walk`
forces the tree-walking evaluator instead of the default bytecode VM
(useful for debugging and parity comparison). `--trace` emits one line
per executed VM instr to stderr (VM mode only; no-op in `--walk` mode).
`--disasm` compiles the script and prints the bytecode disassembly to
stdout, with per-instr `file:line:col` annotations; the script is not
run. `--disasm-func <name>` disassembles the named top-level
`func`/`does`/`function` body (AST scan, no execution). Script output
(native `print`/`prin`/`probe`) goes to stdout; the final value is not
printed by the CLI (use `print` in the script). Errors print to stderr
as `*** Error: <msg>` and exit with code 1.

In REPL mode each line is evaluated against the persistent user context;
the molded result of each line is printed unless it is `none`. Multi-line
blocks are supported: an unclosed `[` or `(` prompts for continuation
lines. Ctrl-C abandons the current input; Ctrl-D exits.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Parse flags (may appear anywhere on the command line — before,
    // between, or after positional args). `--allow-shell` enables
    // `call`/`shell` natives; `--walk` forces the tree-walker (default is the
    // bytecode VM since M29); `--trace` enables per-instr VM tracing (M31).
    let mut allow_shell = false;
    let mut walk = false;
    let mut trace = false;
    let mut disasm = false;
    // `--disasm-func <name>` consumes the next arg as the func name.
    let mut disasm_func: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--allow-shell" {
            allow_shell = true;
        } else if a == "--walk" {
            walk = true;
        } else if a == "--trace" {
            trace = true;
        } else if a == "--disasm" {
            disasm = true;
        } else if a == "--disasm-func" {
            // Consume the next arg as the func name; the file path is the
            // positional after it.
            if i + 1 >= args.len() {
                eprintln!("*** Error: --disasm-func requires a name argument");
                return ExitCode::from(1);
            }
            disasm_func = Some(args[i + 1].clone());
            i += 1;
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }

    // `--disasm` / `--disasm-func` take precedence over running.
    if disasm || disasm_func.is_some() {
        let Some(path) = positional.first() else {
            eprintln!("*** Error: --disasm requires a file path");
            return ExitCode::from(1);
        };
        return disasm_file(path, disasm_func.as_deref());
    }

    match positional.as_slice() {
        [] => repl::run_repl(walk),
        [flag] if flag == "--help" || flag == "-h" => {
            let _ = io::stdout().write_all(HELP.as_bytes());
            ExitCode::SUCCESS
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("{VERSION}");
            ExitCode::SUCCESS
        }
        [path, rest @ ..] => run_file(path, rest, allow_shell, walk, trace),
    }
}

fn run_file(path: &str, args: &[String], allow_shell: bool, walk: bool, trace: bool) -> ExitCode {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("*** Error: cannot read {path:?}: {e}");
            return ExitCode::from(1);
        }
    };
    let opts = red_eval::RunOptions {
        allow_shell,
        args: args.to_vec(),
        walk,
        trace,
    };
    match red_eval::run_source_with_exit_opts(&src, Box::new(io::stdout()), &opts) {
        Ok((_, code)) => {
            if code == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(code as u8)
            }
        }
        Err(e) => {
            eprintln!("{}", red_eval::render_error(Some(path), &src, &e));
            ExitCode::from(1)
        }
    }
}

/// M31: `--disasm <file.red>` / `--disasm-func <name> <file.red>`. Reads the
/// file, compiles (without running), prints the disassembly to stdout. For
/// `--disasm-func`, scans the top-level AST for the named func and
/// disassembles its body. Exits 0 on success, 1 on compile/parse error.
fn disasm_file(path: &str, func: Option<&str>) -> ExitCode {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("*** Error: cannot read {path:?}: {e}");
            return ExitCode::from(1);
        }
    };
    match red_eval::disasm_source(&src, func, Some(path)) {
        Ok(out) => {
            let _ = io::stdout().write_all(out.as_bytes());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}", red_eval::render_error(Some(path), &src, &e));
            ExitCode::from(1)
        }
    }
}
