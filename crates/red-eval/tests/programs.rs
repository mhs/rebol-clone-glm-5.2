//! Golden program harness: for each `tests/programs/*.red` fixture, run the
//! source through `run_source_with_output` with an in-memory buffer, then
//! compare captured stdout to the sibling `*.expected` file. Drop a new
//! `<name>.red` + `<name>.expected` pair in `tests/programs/` to get a new
//! test for free.

mod common;

use common::{golden_fixtures, read_expected, read_source, BufferWriter};
use red_eval::run_source_with_output;

#[test]
fn golden_programs() {
    let fixtures = golden_fixtures("programs");
    assert!(
        fixtures.len() >= 8,
        "expected at least 8 program fixtures, found {}",
        fixtures.len()
    );

    for f in &fixtures {
        let src = read_source(f);
        let expected = read_expected(f);

        let writer = BufferWriter::new();
        let buf_handle = writer.buf.clone();
        match run_source_with_output(&src, Box::new(writer)) {
            Ok(_) => {}
            Err(e) => panic!(
                "programs/{}: run_source failed: {e}\n--- src ---\n{src}",
                f.name
            ),
        }
        let actual = String::from_utf8_lossy(&buf_handle.borrow()).into_owned();

        assert_eq!(
            actual, expected,
            "programs/{}: stdout != expected\n\
             --- src ---\n{src}\n\
             --- expected ---\n{expected}\n\
             --- actual ---\n{actual}",
            f.name,
        );
    }
}
