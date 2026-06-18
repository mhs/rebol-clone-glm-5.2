//! Golden fixture harness: for each `tests/golden/*.red` fixture, lex+parse
//! the source via `load_source`, mold the resulting body series, and compare
//! to the sibling `*.expected` file. Round-trip property:
//! `mold(load_source(src)) == *.expected`.

mod common;

use common::{golden_fixtures, read_expected, read_source};
use red_core::{load_source, mold, Series};

/// Mold a body `Series` as a space-joined sequence of values (no surrounding
/// brackets — the body is a top-level script, not a block literal).
fn mold_body(series: &Series) -> String {
    let data = series.data.borrow();
    let mut out = String::new();
    for (i, v) in data.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        mold(v, &mut out);
    }
    out
}

#[test]
fn golden_round_trip() {
    let fixtures = golden_fixtures("golden");
    assert!(
        fixtures.len() >= 8,
        "expected at least 8 golden fixtures, found {}",
        fixtures.len()
    );

    for f in &fixtures {
        let src = read_source(f);
        let expected = read_expected(f);

        let series = match load_source(&src) {
            Ok(s) => s,
            Err(e) => panic!(
                "golden/{}: load_source failed: {e}\n--- src ---\n{src}",
                f.name
            ),
        };

        let actual = mold_body(&series);
        assert_eq!(
            actual, expected,
            "golden/{}: mold(load_source(src)) != expected\n\
             --- src ---\n{src}\n\
             --- expected ---\n{expected}\n\
             --- actual ---\n{actual}",
            f.name,
        );
    }
}

/// Sanity check: the `Red []` header round-trips through `load_source`
/// because `load` treats `Red` as a plain word at top level (header
/// recognition is `parse_program`'s job, not `load`'s). This test pins that
/// behavior so a future refactor doesn't silently break it.
#[test]
fn load_treats_red_header_as_body() {
    let src = "Red [] print \"hi\"";
    let series = load_source(src).expect("load_source");
    let body = mold_body(&series);
    assert_eq!(body, "Red [] print \"hi\"");
}
