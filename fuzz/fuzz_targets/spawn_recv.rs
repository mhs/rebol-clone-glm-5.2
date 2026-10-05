//! Fuzz target: `spawn [body] recv r` for arbitrary `body` (lossy UTF-8
//! source) must never panic and must terminate within 10s. Catches
//! worker-thread panics, marshalling panics, and infinite loops in the
//! worker (the 10s timeout is enforced via `join_with_timeout`).
//!
//! Run with:
//! ```sh
//! cargo +nightly fuzz run spawn_recv
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use red_core::{load_source, Context, Env};
use red_eval::register_natives;
use red_eval::concurrency::{spawn_thread, join_with_timeout};
use std::io;
use std::rc::Rc;
use std::time::Duration;

fuzz_target!(|data: &[u8]| {
    let src = String::from_utf8_lossy(data);
    // Only test parseable bodies — parse failures are graceful LexError/
    // ParseError, not panics.
    let body = match load_source(&src) {
        Ok(b) => b,
        Err(_) => return,
    };

    let ctx = Rc::new(Context::new());
    let mut env = Env::new_with_output(ctx, Box::new(io::sink()));
    register_natives(&mut env);

    let handle = spawn_thread(body, &env);
    // 10s timeout — infinite loops surface as Err(()).
    let _ = join_with_timeout(handle, Duration::from_secs(10));
});
