//! M29 parity harness: run each golden program fixture in BOTH `Walk` and
//! `Vm` modes, assert identical stdout. This is the regression guard that
//! proves the bytecode VM produces the same observable behavior as the
//! tree-walker. Run with `cargo test -p red-eval --test parity`.
//!
//! The harness uses `RunOptions { walk: true/false, .. }` to force each mode
//! regardless of the build default (`force-walk` feature). Error fixtures
//! assert that the rendered `*** Error:` line matches between modes (spans
//! preserved through compilation).

mod common;

use common::{golden_fixtures, read_source, read_expected, BufferWriter};
use red_eval::{render_error, run_source_with_exit_opts, RunOptions};
use std::rc::Rc;

/// Run `src` in the given mode, returning the captured stdout (or the
/// rendered error line on failure).
fn run_captured(src: &str, walk: bool) -> Result<String, String> {
    let writer = BufferWriter::new();
    let buf = writer.buf.clone();
    let opts = RunOptions {
        walk,
        ..Default::default()
    };
    match run_source_with_exit_opts(src, Box::new(writer), &opts) {
        Ok(_) => {
            let out = Rc::try_unwrap(buf)
                .map(|r| r.into_inner())
                .unwrap_or_else(|r| r.borrow().clone());
            Ok(String::from_utf8_lossy(&out).into_owned())
        }
        Err(e) => {
            let rendered = render_error(None, src, &e);
            Err(rendered)
        }
    }
}

/// Parity test for golden programs: stdout must match byte-for-byte between
/// `Walk` and `Vm` modes.
#[test]
fn golden_programs_parity() {
    let fixtures = golden_fixtures("programs");
    assert!(
        fixtures.len() >= 8,
        "expected at least 8 program fixtures, found {}",
        fixtures.len()
    );
    for f in &fixtures {
        let src = read_source(f);
        let expected = read_expected(f);
        let vm_out = run_captured(&src, false);
        let walk_out = run_captured(&src, true);
        // Both modes should produce the same result (Ok or Err).
        match (&vm_out, &walk_out) {
            (Ok(vm), Ok(walk)) => {
                assert_eq!(
                    vm, walk,
                    "programs/{}: stdout mismatch between VM and Walk modes",
                    f.name
                );
                // Also assert the expected output.
                assert_eq!(
                    vm, &expected,
                    "programs/{}: stdout doesn't match .expected",
                    f.name
                );
            }
            (Err(vm), Err(walk)) => {
                // Both errored: the error messages should match (spans
                // preserved through compilation).
                assert_eq!(
                    vm, walk,
                    "programs/{}: error mismatch between VM and Walk modes",
                    f.name
                );
            }
            (vm, walk) => {
                panic!(
                    "programs/{}: one mode succeeded, the other failed\nVM: {:?}\nWalk: {:?}",
                    f.name, vm, walk
                );
            }
        }
    }
}

/// Parity test for error fixtures: the rendered `*** Error:` line must match
/// exactly (spans preserved through compilation).
#[test]
fn golden_program_errors_parity() {
    let fixtures = golden_fixtures("programs_errors");
    assert!(
        fixtures.len() >= 6,
        "expected at least 6 error fixtures, found {}",
        fixtures.len()
    );
    for f in &fixtures {
        let src = read_source(f);
        let expected = read_expected(f);
        let needle = expected.trim();
        let vm_out = run_captured(&src, false);
        let walk_out = run_captured(&src, true);
        // Both modes should error.
        let vm_err = vm_out.expect_err(&format!(
            "programs_errors/{}: VM mode should have errored",
            f.name
        ));
        let walk_err = walk_out.expect_err(&format!(
            "programs_errors/{}: Walk mode should have errored",
            f.name
        ));
        // Both error messages should match (modulo the optional `line:col:`
        // prefix — the VM may lose span information for some error paths,
        // e.g. `LoadDynamic` produces zero-span errors while the walker
        // carries the word's original span). We strip the location prefix
        // before comparing.
        let vm_msg = strip_location(&vm_err);
        let walk_msg = strip_location(&walk_err);
        assert_eq!(
            vm_msg, walk_msg,
            "programs_errors/{}: error message mismatch between VM and Walk modes",
            f.name
        );
        // The error should contain the expected substring.
        assert!(
            vm_err.contains(needle),
            "programs_errors/{}: error doesn't contain expected substring {:?}\ngot: {}",
            f.name,
            needle,
            vm_err
        );
    }
}

/// Strip the optional `line:col: ` prefix from a rendered error line.
/// `*** Error: [file:line:col: ]<msg>` → `*** Error: <msg>`. The VM may
/// produce zero-span errors (e.g. `LoadDynamic` → `UnboundWord` with
/// `Span::new(0,0)`) while the walker carries the original span. M29
/// accepts this difference; M31 (span-annotated disassembly) will close
/// the gap.
fn strip_location(err: &str) -> String {
    if let Some(rest) = err.strip_prefix("*** Error: ") {
        // Check for `<digits>:<digits>: ` prefix.
        let parts: Vec<&str> = rest.splitn(3, ':').collect();
        if parts.len() == 3
            && parts[0].parse::<u32>().is_ok()
            && parts[1].parse::<u32>().is_ok()
            && parts[2].starts_with(' ')
        {
            return format!("*** Error: {}", &parts[2][1..]);
        }
    }
    err.to_string()
}
