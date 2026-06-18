//! Shared test helpers: walk a directory of `*.red` + `*.expected` fixture
//! pairs. Each crate's integration tests can call `golden_fixtures(subdir)`
//! pointing at its own `tests/<subdir>/` folder.

use std::fs;
use std::path::PathBuf;

/// A matched fixture pair: source `.red` and its expected mold output.
pub struct Fixture {
    pub name: String,
    pub source_path: PathBuf,
    pub expected_path: PathBuf,
}

/// Enumerate every `*.red` file under `tests/<subdir>/` whose stem has a
/// matching `<stem>.expected` sibling. Sorted by name for stable test order.
pub fn golden_fixtures(subdir: &str) -> Vec<Fixture> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(subdir);
    let mut out: Vec<Fixture> = Vec::new();

    let entries = fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}"));
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("red") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("utf8 file stem")
            .to_string();
        let expected = path.with_extension("expected");
        if !expected.exists() {
            panic!(
                "golden fixture {:?} has no matching .expected sibling",
                path
            );
        }
        out.push(Fixture {
            name: stem,
            source_path: path,
            expected_path: expected,
        });
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Read a fixture's source text.
pub fn read_source(f: &Fixture) -> String {
    fs::read_to_string(&f.source_path).unwrap_or_else(|e| panic!("read {:?}: {e}", f.source_path))
}

/// Read a fixture's expected output. Trailing newline is trimmed so fixture
/// files can end with a conventional newline without affecting comparison.
pub fn read_expected(f: &Fixture) -> String {
    let raw = fs::read_to_string(&f.expected_path)
        .unwrap_or_else(|e| panic!("read {:?}: {e}", f.expected_path));
    raw.trim_end_matches('\n').to_string()
}
