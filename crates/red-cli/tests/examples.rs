//! Examples harness (M35): every `examples/*.red` runs through the CLI and
//! must exit 0. Makes `examples/` a regression surface rather than dead
//! weight.
//!
//! Design notes:
//! - `current_dir` is set to the workspace root because several examples
//!   (`dir.red`, `file-io.red`, `read-lines.red`, `save-load.red`) write to
//!   relative `%examples/_tmp_*` paths and clean up after themselves.
//! - No example currently invokes the `call`/`shell` natives (the only grep
//!   hits for those words are in comments). If a shell-using example is ever
//!   added, gate it here with `.arg("--allow-shell")`.
//! - Forward-compatible `.expected` support: if `examples/<stem>.expected`
//!   exists, stdout is compared byte-for-byte; otherwise only exit 0 is
//!   asserted. Environment-dependent examples (`dir.red`, `env.red`) stay
//!   exit-0-only.

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
fn examples_all_run_clean() {
    let root = workspace_root();
    let examples_dir = root.join("examples");

    let mut entries: Vec<PathBuf> = fs::read_dir(&examples_dir)
        .unwrap_or_else(|e| panic!("read {examples_dir:?}: {e}"))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("red"))
        .collect();
    entries.sort();

    assert!(
        !entries.is_empty(),
        "no examples/*.red found — harness misconfigured"
    );

    let mut failures: Vec<String> = Vec::new();

    for path in &entries {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("<non-utf8>")
            .to_string();

        let mut cmd = Command::cargo_bin("red-cli").unwrap();
        // Run from the workspace root so `%examples/_tmp_*` relative paths
        // in the file-IO examples resolve and self-clean.
        cmd.current_dir(&root).arg(path);

        let expected_path = path.with_extension("expected");
        let has_expected = expected_path.exists();

        let assert = cmd.assert();
        let output = assert.get_output();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            failures.push(format!(
                "{name}: exit {:?}\n  stderr: {}\n  stdout: {}",
                output.status.code(),
                stderr.trim(),
                stdout.trim(),
            ));
            continue;
        }

        if has_expected {
            let expected = fs::read_to_string(&expected_path)
                .unwrap_or_else(|e| panic!("read {expected_path:?}: {e}"));
            let actual = String::from_utf8(output.stdout.clone()).unwrap();
            if actual != expected {
                failures.push(format!(
                    "{name}: stdout mismatch\n  expected: {:?}\n  actual:   {:?}",
                    expected, actual,
                ));
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} example(s) failed:\n{}",
            failures.len(),
            failures.join("\n---\n")
        );
    }
}

/// M64: targeted test for `examples/modules/main.red` — the end-to-end
/// integration demo that imports sibling module files (`mathutils.red`,
/// `stringutils.red`, `tree.red`, `counter.red`) and exercises their
/// exports. The top-level `examples_all_run_clean` harness is
/// non-recursive, so it doesn't pick up `examples/modules/*.red`;
/// furthermore the module files are pure module definitions (not
/// standalone scripts), so they shouldn't be run individually. This
/// test runs only `main.red` with `current_dir` set to
/// `examples/modules/` so the relative `import %name.red` paths resolve.
#[test]
fn examples_modules_main_runs_clean() {
    let root = workspace_root();
    let modules_dir = root.join("examples/modules");
    let main_red = modules_dir.join("main.red");

    assert!(
        main_red.exists(),
        "examples/modules/main.red not found — harness misconfigured"
    );

    let mut cmd = Command::cargo_bin("red-cli").unwrap();
    // Run from `examples/modules/` so `import %mathutils.red` etc.
    // resolve relative to cwd.
    cmd.current_dir(&modules_dir).arg(&main_red);
    cmd.assert().success();
}
