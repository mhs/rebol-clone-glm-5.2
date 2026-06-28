//! Shared test helpers for `red-eval` integration tests. Mirrors the
//! `red-core` golden fixture walker and adds an owning `Write` sink so tests
//! can capture native output into a buffer that outlives the `Env`.

use std::cell::RefCell;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::rc::Rc;

/// A matched fixture pair: source `.red` and its expected stdout.
pub struct Fixture {
    pub name: String,
    pub source_path: PathBuf,
    pub expected_path: PathBuf,
}

/// Enumerate every `*.red` file under `tests/<subdir>/` whose stem has a
/// matching `<stem>.expected` sibling. Sorted by name for stable test order.
#[allow(dead_code)] // only used by some test targets (programs.rs, programs_errors.rs)
pub fn golden_fixtures(subdir: &str) -> Vec<Fixture> {
    golden_fixtures_with_ext(subdir, "expected")
}

/// M31: like [`golden_fixtures`] but pairs `.red` with
/// `<stem>.<expected_ext>` (e.g. `disasm.expected` for the disasm golden
/// suite). Used by `tests/disasm_tests.rs`.
pub fn golden_fixtures_with_ext(subdir: &str, expected_ext: &str) -> Vec<Fixture> {
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
        let expected = path.with_extension(expected_ext);
        if !expected.exists() {
            panic!(
                "golden fixture {:?} has no matching .{expected_ext} sibling",
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

pub fn read_source(f: &Fixture) -> String {
    fs::read_to_string(&f.source_path).unwrap_or_else(|e| panic!("read {:?}: {e}", f.source_path))
}

/// Read a fixture's expected output verbatim (no trimming). Program stdout
/// legitimately ends with newlines, so we compare byte-for-byte.
pub fn read_expected(f: &Fixture) -> String {
    fs::read_to_string(&f.expected_path)
        .unwrap_or_else(|e| panic!("read {:?}: {e}", f.expected_path))
}

/// Owning `Write` sink backed by `Rc<RefCell<Vec<u8>>>`. The `Rc` lets the
/// test read the captured bytes after the `Env` (which owns the boxed writer)
/// is dropped — the writer and the test's handle share the buffer.
#[allow(dead_code)] // only used by some test targets (programs.rs, programs_errors.rs)
#[derive(Clone)]
pub struct BufferWriter {
    pub buf: Rc<RefCell<Vec<u8>>>,
}

impl BufferWriter {
    #[allow(dead_code)] // only used by some test targets
    pub fn new() -> Self {
        Self {
            buf: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl Write for BufferWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.borrow_mut().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
