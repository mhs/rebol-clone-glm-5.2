//! Error-fixture harness: for each `tests/programs_errors/*.red` fixture,
//! run the source through `run_source_with_output` and assert it fails with
//! an error whose rendered form *contains* the substring in the sibling
//! `*.expected` file. Drop a new `<name>.red` + `<name>.expected` pair in
//! `tests/programs_errors/` to get a new error-case test for free.
//!
//! The `.expected` file holds a substring of the error message body (e.g.
//! `has no value`, `expected integer!, found string!`) — not the full
//! `file:line:col:` line — so fixtures stay robust to span/formatting tweaks.

mod common;

use common::{golden_fixtures, read_expected, read_source, BufferWriter};
use red_eval::{render_error, run_source_with_output};

#[test]
fn golden_program_errors() {
    let fixtures = golden_fixtures("programs_errors");
    assert!(
        fixtures.len() >= 6,
        "expected at least 6 error fixtures, found {}",
        fixtures.len()
    );

    for f in &fixtures {
        let src = read_source(f);
        let expected = read_expected(f);
        // The expected file is a substring — trim any trailing newline.
        let needle = expected.trim();

        let writer = BufferWriter::new();
        match run_source_with_output(&src, Box::new(writer)) {
            Ok(_) => panic!(
                "programs_errors/{}: expected an error but the program succeeded\n--- src ---\n{src}",
                f.name
            ),
            Err(e) => {
                let rendered = render_error(None, &src, &e);
                assert!(
                    rendered.contains(needle),
                    "programs_errors/{}: rendered error does not contain expected substring\n\
                     --- src ---\n{src}\n\
                     --- expected substring ---\n{needle:?}\n\
                     --- rendered ---\n{rendered}",
                    f.name,
                );
            }
        }
    }
}
