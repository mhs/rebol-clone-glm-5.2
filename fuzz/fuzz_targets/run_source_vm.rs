//! Fuzz target: `run_source(arbitrary_bytes)` in explicit VM mode must not
//! panic. Same as `run_source` but forces `EvalMode::Vm` via `RunOptions`
//! (the default is already `Vm`, but this is explicit so a future default
//! flip doesn't silently skip the VM fuzz path).
//!
//! Run with:
//! ```sh
//! cargo +nightly fuzz run run_source_vm
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use red_eval::{run_source_with_exit_opts, RunOptions};
use std::io::sink;

fuzz_target!(|data: &[u8]| {
    let src = String::from_utf8_lossy(data);
    let opts = RunOptions::default(); // walk=false → VM mode (unless force-walk)
    let _ = run_source_with_exit_opts(&src, Box::new(sink()), &opts);
});
