//! Source-position helpers: `LineMap` translates byte offsets (as carried by
//! `Span`) into 1-based `line:col` positions for error rendering.
//!
//! `LineMap` precomputes the byte offset of the start of each line so the
//! lookup is a binary search — cheap to build once per source string and
//! cheap to query per error. The lexer remains a free function (no source
//! handle retained); the CLI/REPL builds a `LineMap` from the source text it
//! already holds and threads it into error rendering.

/// Precomputed line-start offsets for a source string, supporting
/// `byte_offset -> (line, col)` lookup in `O(log(lines))`.
///
/// Lines are 1-based; columns are 1-based byte offsets from the line start
/// (not char offsets — sufficient for the POC's ASCII-centric fixture set;
/// multi-byte chars would report byte columns, which is still a usable
/// pointer for diagnostics).
pub struct LineMap {
    /// Byte offset of the start of each line. Line 1 starts at offset 0;
    /// line `n` starts at `line_starts[n-1]`. Always non-empty (a `LineMap`
    /// for the empty string holds a single entry, `[0]`).
    line_starts: Vec<usize>,
}

impl LineMap {
    /// Build a `LineMap` by scanning `src` for `\n`. The map indexes into
    /// `src`'s byte offsets; the source string itself is not retained.
    pub fn new(src: &str) -> Self {
        let mut starts = vec![0usize];
        for (i, b) in src.as_bytes().iter().enumerate() {
            if *b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self {
            line_starts: starts,
        }
    }

    /// `(line, col)` for `offset` (1-based both). Clamps offsets past EOF to
    /// the last line's tail column. Returns `(1, 1)` for offset 0.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        // Binary search for the last line_start <= offset.
        let idx = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let line = idx + 1;
        let col = offset.saturating_sub(self.line_starts[idx]) + 1;
        (line, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line_offsets() {
        let m = LineMap::new("hello");
        assert_eq!(m.line_col(0), (1, 1));
        assert_eq!(m.line_col(2), (1, 3));
        assert_eq!(m.line_col(5), (1, 6));
    }

    #[test]
    fn multi_line_offsets() {
        // "ab\ncde\nf" — line 1 = "ab", line 2 = "cde", line 3 = "f".
        let m = LineMap::new("ab\ncde\nf");
        assert_eq!(m.line_col(0), (1, 1)); // 'a'
        assert_eq!(m.line_col(2), (1, 3)); // '\n'
        assert_eq!(m.line_col(3), (2, 1)); // 'c'
        assert_eq!(m.line_col(5), (2, 3)); // 'e'
        assert_eq!(m.line_col(6), (2, 4)); // '\n'
        assert_eq!(m.line_col(7), (3, 1)); // 'f'
    }

    #[test]
    fn empty_string() {
        let m = LineMap::new("");
        assert_eq!(m.line_col(0), (1, 1));
    }

    #[test]
    fn trailing_newline_yields_extra_line() {
        // "x\n" — line 1 = "x", line 2 = "" (empty trailing line).
        let m = LineMap::new("x\n");
        assert_eq!(m.line_col(0), (1, 1));
        assert_eq!(m.line_col(1), (1, 2)); // '\n'
        assert_eq!(m.line_col(2), (2, 1)); // past newline → line 2 col 1
    }

    #[test]
    fn offset_past_eof_clamps() {
        let m = LineMap::new("ab\ncd");
        // Offset 100 is way past EOF; clamps to the last line.
        let (line, _col) = m.line_col(100);
        assert_eq!(line, 2);
    }
}
