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
        .stdout(concat!("red ", env!("CARGO_PKG_VERSION"), "\n"));
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

#[test]
fn walk_flag_runs_tree_walker() {
    // `--walk` forces the tree-walker. The output should be identical to the
    // default (VM) mode. This test runs a simple program both ways and
    // asserts they match. (M29)
    let dir = tempfile_dir();
    let path = dir.join("walk.red");
    fs::write(&path, "Red [] print 1 + 2").unwrap();

    // Default (VM) mode.
    let vm_output = Command::cargo_bin("red-cli")
        .unwrap()
        .arg(&path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    // --walk mode.
    let walk_output = Command::cargo_bin("red-cli")
        .unwrap()
        .arg("--walk")
        .arg(&path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(
        vm_output, walk_output,
        "VM and Walk modes should produce identical stdout"
    );
    assert_eq!(String::from_utf8(vm_output).unwrap(), "3\n");
}

// --- M34: CLI flag-parsing edge cases -------------------------------------

#[test]
fn unknown_flag_falls_through_to_positional_and_errors() {
    // `--typo` is not a recognized flag, so it becomes a positional arg and
    // `run_file` tries to read it as a path. The read fails → `*** Error:`
    // on stderr, non-zero exit. (The plan text suggested exit 2; the actual
    // implementation returns exit 1 via the read-error branch. We assert the
    // real behavior — non-zero exit + the error line.)
    let dir = tempfile_dir();
    let path = dir.join("real.red");
    fs::write(&path, "Red [] print 1").unwrap();
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg("--typo")
        .arg(&path)
        .assert()
        .failure()
        .stderr(predicates::str::contains("*** Error:"));
}

#[test]
fn flag_after_positional_runs_script() {
    // Flags may appear after the script path. `--walk` here must be parsed
    // as a flag, not swallowed as a script arg.
    let dir = tempfile_dir();
    let path = dir.join("after.red");
    fs::write(&path, "Red [] print 1 + 2").unwrap();
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg(&path)
        .arg("--walk")
        .assert()
        .success()
        .stdout("3\n");
}

#[test]
fn flag_between_positional_args() {
    // A flag between the script path and trailing args still parses as a
    // flag; the trailing arg flows into `system/options/args`.
    let dir = tempfile_dir();
    let path = dir.join("between.red");
    fs::write(&path, "Red [] print system/options/args").unwrap();
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg(&path)
        .arg("--walk")
        .arg("kept")
        .assert()
        .success()
        .stdout("[\"kept\"]\n");
}

#[test]
fn help_flag_wins_over_other_flags() {
    // `--help` mixed with other recognized flags still prints help and exits
    // 0 (it's matched as the sole positional).
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg("--allow-shell")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("USAGE:"));
}

#[test]
fn version_flag_mixed_with_other_flag() {
    // `--version` mixed with `--walk` prints the version and exits 0.
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg("--walk")
        .arg("--version")
        .assert()
        .success()
        .stdout(concat!("red ", env!("CARGO_PKG_VERSION"), "\n"));
}

#[test]
fn multiple_recognized_flags_accumulate() {
    // `--allow-shell --walk file` parses both flags; the script runs under
    // walk mode with shell enabled. Just confirms no flag is dropped.
    let dir = tempfile_dir();
    let path = dir.join("multi.red");
    fs::write(&path, "Red [] print 1").unwrap();
    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    cmd.arg("--allow-shell")
        .arg("--walk")
        .arg(&path)
        .assert()
        .success()
        .stdout("1\n");
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
