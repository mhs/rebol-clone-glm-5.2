//! End-to-end CLI tests via `assert_cmd`. Exercises the hello-world fixture
//! and an error path (unbound word).

use assert_cmd::Command;
use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/
        .unwrap()
        .parent() // workspace root
        .unwrap()
        .to_path_buf()
}

#[test]
fn hello_world_prints_molded_string() {
    // mold-everything: `print "Hello, World!"` outputs the string with quotes.
    let script = workspace_root().join("examples/hello.red");
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg(script)
        .assert()
        .success()
        .stdout("\"Hello, World!\"\n");
}

#[test]
fn unbound_word_exits_nonzero_with_error() {
    let dir = tempfile_dir();
    let path = dir.join("err.red");
    fs::write(&path, "Red [] foo").unwrap();
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg(&path)
        .assert()
        .failure()
        .stderr(predicates::str::contains("*** Error:"));
}

#[test]
fn version_flag() {
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout("red 0.2.0\n");
}

#[test]
fn help_flag() {
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("USAGE:"));
}

#[test]
fn missing_file_errors() {
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg("/nonexistent/path.red")
        .assert()
        .failure()
        .stderr(predicates::str::contains("*** Error:"));
}

#[test]
fn repl_reads_from_stdin() {
    // No args → REPL. Piped stdin (non-tty) is read line-by-line; each
    // line's molded result goes to stdout. `quit` ends the session.
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.write_stdin("5\nquit\n")
        .assert()
        .success()
        .stdout("5\n");
}

#[test]
fn repl_persists_state_via_stdin() {
    // State set on one line is visible on the next: `x: 10` molds to 10
    // (the assigned value), then `x` reads the persisted slot → 10.
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.write_stdin("x: 10\nx\nquit\n")
        .assert()
        .success()
        .stdout("10\n10\n");
}

#[test]
fn repl_multiline_block_via_stdin() {
    // Unclosed `[` on line 1 → continuation; line 2 closes it; line 3
    // reads the bound word. Both the assignment and the lookup mold to
    // `[1 2]`.
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.write_stdin("x: [\n1 2\n]\nx\nquit\n")
        .assert()
        .success()
        .stdout("[1 2]\n[1 2]\n");
}

#[test]
fn trailing_args_exposed_via_system_options() {
    // `red script.red a b c` → `system/options/args` is `[a b c]`.
    let dir = tempfile_dir();
    let path = dir.join("args.red");
    fs::write(&path, "Red [] print system/options/args").unwrap();
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg(&path)
        .arg("a")
        .arg("b")
        .arg("c")
        .assert()
        .success()
        .stdout("[\"a\" \"b\" \"c\"]\n");
}

#[test]
fn shell_disabled_by_default() {
    // Without --allow-shell, `call` raises.
    let dir = tempfile_dir();
    let path = dir.join("shell.red");
    fs::write(&path, "Red [] call \"true\"").unwrap();
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg(&path)
        .assert()
        .failure()
        .stderr(predicates::str::contains("shell disabled"));
}

#[test]
fn allow_shell_enables_call() {
    // With --allow-shell, `call "true"` runs and the script prints the exit code.
    let dir = tempfile_dir();
    let path = dir.join("shell.red");
    fs::write(&path, "Red [] print call \"true\"").unwrap();
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg("--allow-shell")
        .arg(&path)
        .assert()
        .success()
        .stdout("0\n");
}

/// A scratch directory for test fixtures. Reuses `std::env::temp_dir()`; each
/// test picks a unique name to avoid collisions.
fn tempfile_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "red-cli-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}
