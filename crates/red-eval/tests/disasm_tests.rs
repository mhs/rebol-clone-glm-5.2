//! M31: golden disassembly suite. For each `*.red` + `*.disasm.expected`
//! pair under `tests/disasm/`, compile the script via `disasm_source` and
//! assert each non-empty expected line appears as a substring of the disasm
//! output. Substring matching (not exact) so native-index churn and pool
//! value formatting tweaks don't break fixtures — only the instr mnemonics
//! and symbol names (the semantically meaningful parts) are asserted.

mod common;

use common::golden_fixtures_with_ext;
use red_eval::disasm_source;

#[test]
fn golden_disasm() {
    let fixtures = golden_fixtures_with_ext("disasm", "disasm.expected");
    assert!(
        fixtures.len() >= 4,
        "expected at least 4 disasm fixtures; got {}",
        fixtures.len()
    );
    for f in &fixtures {
        let src = common::read_source(f);
        let expected = common::read_expected(f);
        // File path used for `file:line:col` annotation — pass the fixture's
        // file name so the position prefix is stable across machines.
        let path_str = f.source_path.to_string_lossy();
        let out = disasm_source(&src, None, Some(&path_str)).unwrap_or_else(|e| {
            panic!(
                "disasm_source failed for {:?}: {}",
                f.source_path,
                red_eval::render_error(Some(&path_str), &src, &e)
            )
        });
        // Each non-empty expected line must appear in the disasm output.
        for line in expected.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            assert!(
                out.contains(line),
                "fixture {:?}: disasm output missing expected substring {:?}\n--- disasm:\n{out}",
                f.source_path,
                line
            );
        }
    }
}
