//! Fuzz target: `run_source(arbitrary_bytes)` must not panic.
//!
//! Feeds arbitrary bytes to `red_eval::run_source` (default VM mode). The
//! property: the VM must distinguish panics (bugs — overflow, index OOB,
//! unwrap on None, etc.) from `EvalError`s (graceful, expected for
//! malformed input). A panic here is a VM bug; an `Err` is fine.
//!
//! Run with:
//! ```sh
//! cargo +nightly fuzz run run_source
//! ```
//! (Requires `cargo-fuzz` installed and a nightly toolchain registered with
//! `rustup`. The fuzz crate is excluded from the default workspace test run —
//! it's a separate `cargo fuzz` invocation.)

#![no_main]

use libfuzzer_sys::fuzz_target;
use red_eval::run_source;

fuzz_target!(|data: &[u8]| {
    // Arbitrary bytes → UTF-8 string (lossy; invalid UTF-8 becomes U+FFFD).
    // The lexer will reject malformed input gracefully (LexError), which is
    // the expected non-panic path.
    let src = String::from_utf8_lossy(data);
    // Run in the default (VM) mode. We discard the result — the property is
    // "no panic", not "specific output". An `Err` (lex/parse/eval error) is
    // a graceful failure, not a bug.
    let _ = run_source(&src);
});
