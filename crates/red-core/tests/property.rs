//! Property test: `mold(load_source(mold(v))) == mold(v)` for generated
//! `Value`s. Exercises the printer/parser round-trip on random-ish trees.
//!
//! Excluded variants (documented POC gaps):
//! - `Func` — molds as `#[function]`, not reparseable.
//! - `String8` — molds as `#{hex}`, not reparseable.
//! - `Error` — molds as `make error! "..."`, not reparseable (the `make`
//!   native runs at eval time, not parse time).
//! - `NaN`/`inf` floats — no lexer literal for them.
//! - Series with `index != 0` — mold renders from the cursor, so a positioned
//!   series doesn't round-trip to its head form.
//!
//! `None`/`Logic` round-trip at the *mold* level: `mold(Value::None)` is
//! `none`, which re-parses as `Word("none")` whose mold is also `none`. The
//! span difference is invisible to `mold`.
//!
//! `Path` and `Refinement` (M13) are source-origin and round-trip. Paths
//! are generated with word-only parts so they reparse via adjacency folding.

use proptest::prelude::*;
use red_core::{load_source, mold_to_string, printer::mold, Series, Span, Value};

/// Generate a `Value` tree of bounded depth using only reparseable variants.
fn gen_value(_depth: u32) -> BoxedStrategy<Value> {
    prop_oneof![
        // Integer literals (small range keeps the test fast and avoids
        // i64 edge cases the lexer might reject).
        any::<i64>().prop_map(|n| Value::Integer {
            n,
            span: Span::new(0, 0),
        }),
        // Finite floats only — NaN/inf are a documented gap.
        (-1_000_000.0f64..1_000_000.0f64).prop_map(|f| Value::Float {
            f,
            span: Span::new(0, 0),
        }),
        // Printable ASCII strings (mold escapes as needed).
        "[a-z0-9 \\\"\\n\\t]{0,20}".prop_map(|s: String| Value::String {
            s: s.into(),
            span: Span::new(0, 0),
        }),
        // Word family.
        "[a-z][a-z0-9]{0,8}".prop_map(|s: String| Value::Word {
            sym: red_core::Symbol::new(&s),
            binding: red_core::Binding::Unbound,
            span: Span::new(0, 0),
        }),
        "[a-z][a-z0-9]{0,8}".prop_map(|s: String| Value::SetWord {
            sym: red_core::Symbol::new(&s),
            binding: red_core::Binding::Unbound,
            span: Span::new(0, 0),
        }),
        "[a-z][a-z0-9]{0,8}".prop_map(|s: String| Value::GetWord {
            sym: red_core::Symbol::new(&s),
            binding: red_core::Binding::Unbound,
            span: Span::new(0, 0),
        }),
        "[a-z][a-z0-9]{0,8}".prop_map(|s: String| Value::LitWord {
            sym: red_core::Symbol::new(&s),
            span: Span::new(0, 0),
        }),
        // `none` / `true` / `false` constants round-trip at mold level.
        Just(Value::None),
        any::<bool>().prop_map(Value::Logic),
        // Refinement word (standalone): `/foo` re-parses to a Refinement.
        "[a-z][a-z0-9]{0,8}".prop_map(|s: String| Value::Refinement {
            sym: red_core::Symbol::new(&s),
            span: Span::new(0, 0),
        }),
        // File! literal: bare path form (no delimiters) round-trips.
        "[a-z][a-z0-9/._-]{0,12}".prop_map(|s: String| Value::File {
            path: s.into(),
            span: Span::new(0, 0),
        }),
        // Url! literal: `scheme://...` round-trips.
        "[a-z]{1,5}://[a-z0-9./_-]{0,12}".prop_map(|s: String| Value::Url {
            url: s.into(),
            span: Span::new(0, 0),
        }),
    ]
    .prop_recursive(
        3,  // max depth
        16, // max total items
        4,  // max items per collection
        |inner| {
            prop_oneof![
                // Block of generated values (index 0 — positioned series
                // don't round-trip).
                inner.clone().prop_map(|v| {
                    let series = Series::new(vec![v]);
                    Value::Block {
                        series,
                        span: Span::new(0, 0),
                    }
                }),
                // Multi-element block.
                prop::collection::vec(inner.clone(), 0..4).prop_map(|vs| {
                    let series = Series::new(vs);
                    Value::Block {
                        series,
                        span: Span::new(0, 0),
                    }
                }),
                // Paren.
                prop::collection::vec(inner.clone(), 0..4).prop_map(|vs| {
                    let series = Series::new(vs);
                    Value::Paren {
                        series,
                        span: Span::new(0, 0),
                    }
                }),
                // Path of 2..4 word parts. Word-only parts reparse via the
                // parser's adjacency folding into a single Path value.
                prop::collection::vec(
                    "[a-z][a-z0-9]{0,8}".prop_map(|s: String| Value::Word {
                        sym: red_core::Symbol::new(&s),
                        binding: red_core::Binding::Unbound,
                        span: Span::new(0, 0),
                    }),
                    2..4,
                )
                .prop_map(|parts| Value::Path {
                    parts,
                    span: Span::new(0, 0),
                }),
                // Get-path: `:a/b` re-parses as a GetPath. The mold emits
                // `:a/b` (prefix on the whole path, head demoted to Word).
                prop::collection::vec(
                    "[a-z][a-z0-9]{0,8}".prop_map(|s: String| Value::Word {
                        sym: red_core::Symbol::new(&s),
                        binding: red_core::Binding::Unbound,
                        span: Span::new(0, 0),
                    }),
                    2..4,
                )
                .prop_map(|parts| Value::GetPath {
                    parts,
                    span: Span::new(0, 0),
                }),
                // Lit-path: `'a/b` re-parses as a LitPath.
                prop::collection::vec(
                    "[a-z][a-z0-9]{0,8}".prop_map(|s: String| Value::Word {
                        sym: red_core::Symbol::new(&s),
                        binding: red_core::Binding::Unbound,
                        span: Span::new(0, 0),
                    }),
                    2..4,
                )
                .prop_map(|parts| Value::LitPath {
                    parts,
                    span: Span::new(0, 0),
                }),
            ]
        },
    )
    .boxed()
}

proptest! {
    #[test]
    fn mold_parse_round_trips(v in gen_value(3)) {
        let molded = mold_to_string(&v);
        let reparsed = match load_source(&molded) {
            Ok(series) => series,
            Err(e) => {
                prop_assert!(
                    false,
                    "load_source failed on molded form {} (original {:?}): {}",
                    molded, v, e
                );
                return Err(TestCaseError::Fail("load_source failed".into()));
            }
        };
        // Mold the reparsed body series as a space-joined sequence.
        let data = reparsed.data.borrow();
        let mut re_molded = String::new();
        for (i, item) in data.iter().enumerate() {
            if i > 0 {
                re_molded.push(' ');
            }
            mold(item, &mut re_molded);
        }
        drop(data);
        prop_assert_eq!(
            re_molded.clone(), molded.clone(),
            "round-trip mismatch\n--- original value ---\n{:?}\n--- molded ---\n{}\n--- re-molded ---\n{}",
            v, molded, re_molded
        );
    }
}
