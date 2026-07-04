//! M86: `--unset-on-unbound` runtime gate. The standard `programs/` golden
//! harness uses `RunOptions::default()` (flag off), so the gated behavior
//! gets its own test file. Run with `cargo test -p red-eval --test unset_on_unbound`.
//!
//! Verifies:
//! - With the flag OFF (default), a truly-unbound word raises
//!   `EvalError::UnboundWord` (the v0.2–v0.6 strict-binding contract).
//! - With the flag ON, a truly-unbound word evaluates to `Value::Unset`
//!   (which `unset?` recognizes, which molds to `""`, which `print` emits
//!   as a blank line).
//! - The gate is honored in BOTH the tree-walker and the bytecode VM (the
//!   `RunOptions.walk` field forces the walker; the default is the VM).

mod common;

use common::BufferWriter;
use red_eval::{run_source_with_exit_opts, RunOptions};
use std::rc::Rc;

fn run_captured(src: &str, unset_on_unbound: bool, walk: bool) -> Result<String, String> {
    let writer = BufferWriter::new();
    let buf = writer.buf.clone();
    let opts = RunOptions {
        unset_on_unbound,
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
        Err(e) => Err(e.to_string()),
    }
}

/// Default (flag off): a truly-unbound word errors in VM mode.
#[test]
fn unbound_word_errors_default_vm() {
    let res = run_captured("Red []\nxyzzy_no_such_word", false, false);
    assert!(res.is_err(), "expected error, got: {res:?}");
    let err = res.unwrap_err();
    assert!(
        err.contains("has no value") || err.contains("xyzzy_no_such_word"),
        "expected UnboundWord error, got: {err}"
    );
}

/// Default (flag off): a truly-unbound word errors in walker mode too.
#[test]
fn unbound_word_errors_default_walker() {
    let res = run_captured("Red []\nxyzzy_no_such_word", false, true);
    assert!(res.is_err(), "expected error, got: {res:?}");
    let err = res.unwrap_err();
    assert!(
        err.contains("has no value") || err.contains("xyzzy_no_such_word"),
        "expected UnboundWord error, got: {err}"
    );
}

/// Flag on (VM): an unbound word evaluates to `unset!`, no error.
#[test]
fn unset_on_unbound_vm_yields_unset() {
    let res = run_captured("Red []\nprint unset? xyzzy_no_such_word", true, false);
    assert!(res.is_ok(), "expected ok, got error: {res:?}");
    assert_eq!(res.unwrap(), "true\n");
}

/// Flag on (walker): same — the gate is honored in both eval modes.
#[test]
fn unset_on_unbound_walker_yields_unset() {
    let res = run_captured("Red []\nprint unset? xyzzy_no_such_word", true, true);
    assert!(res.is_ok(), "expected ok, got error: {res:?}");
    assert_eq!(res.unwrap(), "true\n");
}

/// Flag on: `print` of an unbound word emits just a newline (since
/// `form(Unset) == ""`).
#[test]
fn unset_on_unbound_prints_blank_line() {
    let res = run_captured("Red []\nprint xyzzy_no_such_word", true, false);
    assert_eq!(res.unwrap(), "\n");
}

/// Flag on: `mold` of an unbound word returns an empty `string!`.
#[test]
fn unset_on_unbound_mold_is_empty_string() {
    let src = "Red []\nprin \"<\"\nprin mold xyzzy_no_such_word\nprin \">\"";
    let res = run_captured(src, true, false);
    assert_eq!(res.unwrap(), "<>");
}

/// Flag on: the `unset` constant still works alongside the gate (the gate
/// only fires on truly-unbound words after the user_ctx/native fallbacks).
#[test]
fn unset_constant_still_resolves_with_gate_on() {
    let res = run_captured("Red []\nprint unset? unset", true, false);
    assert_eq!(res.unwrap(), "true\n");
}

/// Flag on: a user-set word still resolves normally (the gate is a
/// fallback, not a replacement for the user_ctx).
#[test]
fn user_set_word_still_resolves_with_gate_on() {
    let res = run_captured("Red []\nmyvar: 42\nprint myvar", true, false);
    assert_eq!(res.unwrap(), "42\n");
}

/// Flag on: a native still resolves normally (the native registry is
/// consulted before the gate).
#[test]
fn native_still_resolves_with_gate_on() {
    let res = run_captured("Red []\nprint 1 + 2", true, false);
    assert_eq!(res.unwrap(), "3\n");
}
