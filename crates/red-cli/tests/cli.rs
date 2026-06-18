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
        .stdout("red 0.0.1\n");
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
