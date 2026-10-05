//! Fuzz target: `Value::marshal_send` on arbitrary `Value` trees must
//! never panic. Errors are graceful; panics are bugs.
//!
//! Since `Value` can't be directly constructed from arbitrary bytes (it
//! requires a parser), this fuzzer feeds arbitrary source strings through
//! `load_source`, then marshals the resulting block. The property is:
//! `marshal_send` on any parseable `Value` tree must not panic.
//!
//! Run with:
//! ```sh
//! cargo +nightly fuzz run marshal
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use red_core::{load_source, Value};

fuzz_target!(|data: &[u8]| {
    let src = String::from_utf8_lossy(data);
    if let Ok(body) = load_source(&src) {
        let block = Value::Block {
            series: body,
            span: red_core::Span::default(),
        };
        // marshal_send must not panic for any Value tree.
        let _ = block.marshal_send();
    }
});
