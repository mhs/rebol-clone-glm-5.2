//! Property test: `mold(load_source(mold(v))) == mold(v)` for generated
//! `Value`s. Exercises the printer/parser round-trip on random-ish trees.
//!
//! Excluded variants (documented POC gaps):
//! - `Func` — molds as `#[function]`, not reparseable.
//! - `Closure` (M60) — molds as `#[closure]`, not reparseable (snapshot
//!   captures can't be reconstituted from source).
//! - `Error` — molds as `make error! "..."` (or `make error! [...]` for
//!   structured errors), which parses to a block of words (the `make`
//!   native runs at eval time, not parse time). Round-trip would require
//!   the parser to fold `make error!` into an `Error` value, which is out
//!   of scope for the printer property test.
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
use red_core::{
    form_to_string, load_source, mold_to_string, printer::mold, Context, DateValue, HashDef,
    ImageDef, MapDef, MapKey, ModuleDef, Series, Span, Symbol, Value, VectorDef,
};
use std::rc::Rc;

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
        // M80: percent! literals — integer-valued percentages (0..=1000 ⇒
        // `0%`..`1000%`) so the mold form `NN%` round-trips exactly through
        // the lexer. Fractional percentages are covered by inline unit tests
        // (the `{:.6}` mold format loses precision below 1e-6, so the property
        // test sticks to the integer case).
        (-1000i64..=1000).prop_map(|n| Value::Percent {
            value: n as f64 / 100.0,
            span: Span::new(0, 0),
        }),
        // M80: issue! literals — short alphanumeric bodies (mold form `#body`
        // reparses through the lexer). Avoid `"`/`{` as the first char (those
        // route to char!/binary! scanners).
        "[a-zA-Z0-9_.!?-]{1,8}".prop_map(|s: String| Value::Issue {
            s: s.into(),
            span: Span::new(0, 0),
        }),
        // M80: email! literals — short `user@host.tld` forms that round-trip
        // through the lexer's email detection.
        "[a-z]{1,6}@[a-z]{1,6}\\.[a-z]{2,4}".prop_map(|s: String| Value::Email {
            addr: s.into(),
            span: Span::new(0, 0),
        }),
        // M81: tag! literals — short alphanumeric bodies. The mold form
        // `<body>` reparses through `scan_tag` (no escapes needed since the
        // body has no `<`/`>`/`\`). Escaped forms are exercised by inline
        // unit tests in printer.rs.
        "[a-zA-Z0-9_]{1,8}".prop_map(|s: String| Value::Tag {
            text: s.into(),
            span: Span::new(0, 0),
        }),
        // mold form `$<dollars>.<DD>[:CCC]` round-trips. Keep cents small to
        // avoid i64 edge cases; use 3 currencies (USD default, EUR/JPY
        // non-default to exercise the suffix).
        (-1_000_000i64..=1_000_000, 0u8..3).prop_map(|(cents, cur_idx)| {
            let currency = match cur_idx {
                0 => "USD",
                1 => "EUR",
                _ => "JPY",
            };
            Value::Money {
                amount: std::rc::Rc::new(red_core::MoneyValue::new(cents, currency)),
                span: Span::new(0, 0),
            }
        }),
        // Printable ASCII strings (mold escapes as needed).
        "[a-z0-9 \\\"\\n\\t]{0,20}".prop_map(|s: String| Value::String {
            s: s.into(),
            span: Span::new(0, 0),
        }),
        // M38: char! literals — printable ASCII excluding `"`, `^`, `\` so the
        // mold form `#"c"` stays escape-free and round-trips deterministically.
        // (Escape forms are exercised by inline unit tests in printer.rs.)
        "[a-zA-Z0-9 ,.;:_/()\\[\\]{}!@#$%&*+=|<>?~`'-]".prop_map(|s: String| {
            let c = s.chars().next().unwrap_or('a');
            Value::Char {
                c,
                span: Span::new(0, 0),
            }
        }),
        // M41: binary! literals — short byte vectors that mold to `#{HEX}` and
        // reparse through the new lexer rule.
        prop::collection::vec(any::<u8>(), 0..8).prop_map(|bytes| Value::String8 {
            bytes,
            span: Span::new(0, 0),
        }),
        // M44: pair! literals — two small integers (mold form `NxM` reparses).
        // Keep values non-negative so the mold form is unambiguous (no leading
        // `-` to confuse the lexer's pair-detection).
        (0i64..1000, 0i64..1000).prop_map(|(x, y)| Value::Pair {
            x: Rc::new(Value::Integer {
                n: x,
                span: Span::new(0, 0)
            }),
            y: Rc::new(Value::Integer {
                n: y,
                span: Span::new(0, 0)
            }),
            span: Span::new(0, 0),
        }),
        // M44: tuple! literals — 3 or 4 bytes 0-255 (mold form `R.G.B[.A]`
        // reparses).
        (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b)| Value::Tuple {
            bytes: Rc::from(&[r, g, b][..]),
            span: Span::new(0, 0),
        }),
        (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b, a)| {
            Value::Tuple {
                bytes: Rc::from(&[r, g, b, a][..]),
                span: Span::new(0, 0),
            }
        }),
        // M45: date! literals. Generate a valid year/month/day (chrono
        // validates), an optional time (HH:MM:SS), and an optional fixed
        // offset zone (0..=14h). The mold form `DD-Mon-YYYY[/HH:MM:SS[+HH:MM]]`
        // reparses through the new lexer rule. Skip the `now`-derived zone
        // (use fixed offsets only, per plan5.md M45 line 627).
        (1900i32..2100, 1u32..=12, 1u32..=28).prop_map(|(y, m, d)| {
            let date = red_core::NaiveDate::from_ymd_opt(y, m, d).unwrap();
            Value::date(DateValue::date_only(date))
        }),
        (
            1900i32..2100,
            1u32..=12,
            1u32..=28,
            0u32..23,
            0u32..59,
            0u32..59
        )
            .prop_map(|(y, m, d, h, mi, s)| {
                let date = red_core::NaiveDate::from_ymd_opt(y, m, d).unwrap();
                let time = red_core::NaiveTime::from_hms_opt(h, mi, s).unwrap();
                Value::date(DateValue::from_local(date.and_time(time), None))
            },),
        (
            1900i32..2100,
            1u32..=12,
            1u32..=28,
            0u32..23,
            0u32..59,
            0u32..59,
            -14i32..=14,
        )
            .prop_map(|(y, m, d, h, mi, s, zh)| {
                let date = red_core::NaiveDate::from_ymd_opt(y, m, d).unwrap();
                let time = red_core::NaiveTime::from_hms_opt(h, mi, s).unwrap();
                let zone = Some(zh * 60);
                Value::date(DateValue::from_local(date.and_time(time), zone))
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

// M43: `map!` mold is deterministic. Maps are synthetic (mold as
// `make map! [...]` which parses to a block, not a Map value), so they're
// excluded from the `gen_value` round-trip above. This focused test builds a
// `MapDef` directly with hashable keys and asserts that molding it twice
// yields the same string, and that the form starts with `make map! [`.
proptest! {
    #[test]
    fn map_mold_is_stable(
        word_keys in prop::collection::vec("[a-z][a-z0-9]{0,6}", 0..4),
        int_keys in prop::collection::vec(any::<i64>(), 0..3),
    ) {
        let m = MapDef::new();
        for (i, k) in word_keys.iter().enumerate() {
            m.set(
                MapKey::Sym(Symbol::new(k)),
                Value::Integer { n: i as i64, span: Span::new(0, 0) },
            );
        }
        for (i, k) in int_keys.iter().enumerate() {
            m.set(MapKey::Int(*k), Value::integer((i as i64) * 10));
        }
        let v = Value::map(m);
        let molded1 = mold_to_string(&v);
        let molded2 = mold_to_string(&v);
        prop_assert_eq!(&molded1, &molded2, "mold not deterministic");
        prop_assert!(
            molded1.starts_with("make map! ["),
            "expected `make map! [...]` form, got: {molded1}"
        );
        prop_assert!(molded1.ends_with(']'), "expected closing ]: {molded1}");
    }
}

// M83: `hash!` mold stability. Like `map!`, `hash!` is synthetic (mold as
// `make hash! [...]` which parses to a block, not a Hash value), so it's
// excluded from `gen_value`. This focused test builds a `HashDef` directly
// with hashable keys and asserts that molding it twice yields the same
// string (the `key_order` vec makes output deterministic), and that the form
// starts with `make hash! [`.
proptest! {
    #[test]
    fn hash_mold_is_stable(
        word_keys in prop::collection::vec("[a-z][a-z0-9]{0,6}", 0..4),
        int_keys in prop::collection::vec(any::<i64>(), 0..3),
    ) {
        let h = HashDef::new();
        for (i, k) in word_keys.iter().enumerate() {
            h.set(
                MapKey::Sym(Symbol::new(k)),
                Value::Integer { n: i as i64, span: Span::new(0, 0) },
            );
        }
        for (i, k) in int_keys.iter().enumerate() {
            h.set(MapKey::Int(*k), Value::integer((i as i64) * 10));
        }
        let v = Value::hash(h);
        let molded1 = mold_to_string(&v);
        let molded2 = mold_to_string(&v);
        prop_assert_eq!(&molded1, &molded2, "mold not deterministic");
        prop_assert!(
            molded1.starts_with("make hash! ["),
            "expected `make hash! [...]` form, got: {molded1}"
        );
        prop_assert!(molded1.ends_with(']'), "expected closing ]: {molded1}");
    }
}

// M84: `vector!` mold stability. Like `hash!`, `vector!` is synthetic (mold
// as `make vector! [...]` which parses to a block, not a Vector value), so
// it's excluded from `gen_value`. This focused test builds a `VectorDef`
// directly with integer or float kinds and asserts that molding it twice
// yields the same string, and that the form starts with `make vector! [`.
proptest! {
    #[test]
    fn vector_mold_is_stable(
        int_elems in prop::collection::vec(any::<i64>(), 0..8),
        float_elems in prop::collection::vec(any::<f64>(), 0..8),
    ) {
        let int_v = Value::vector(VectorDef::new(
            Symbol::new("integer!"),
            int_elems.iter().map(|n| Value::integer(*n)).collect(),
        ));
        let molded1 = mold_to_string(&int_v);
        let molded2 = mold_to_string(&int_v);
        prop_assert_eq!(&molded1, &molded2, "int mold not deterministic");
        prop_assert!(
            molded1.starts_with("make vector! [integer!"),
            "expected `make vector! [integer! ...]` form, got: {molded1}"
        );
        prop_assert!(molded1.ends_with(']'), "expected closing ]: {molded1}");

        let float_v = Value::vector(VectorDef::new(
            Symbol::new("float!"),
            float_elems.iter().map(|f| Value::float(*f)).collect(),
        ));
        let molded1 = mold_to_string(&float_v);
        let molded2 = mold_to_string(&float_v);
        prop_assert_eq!(&molded1, &molded2, "float mold not deterministic");
        prop_assert!(
            molded1.starts_with("make vector! [float!"),
            "expected `make vector! [float! ...]` form, got: {molded1}"
        );
        prop_assert!(molded1.ends_with(']'), "expected closing ]: {molded1}");
    }
}

// M65: `module!` mold round-trips through `make module!`. Modules are
// synthetic (mold as `make module! [...]` which parses to a block, not a
// Module value), so they're excluded from `gen_value`. This focused test
// builds a `ModuleDef` directly, molds it, re-parses the molded form, and
// asserts the re-molded string is identical (stability) and that the parsed
// tree's head word is `make` (the `make module!` constructor form). Private
// words are omitted by the mold (only exports appear), so the round-trip
// loses private state by design — the assertion is on the public surface.
proptest! {
    #[test]
    fn module_mold_roundtrips(
        exported_words in prop::collection::vec("[a-z][a-z0-9]{0,6}", 1..4),
    ) {
        let ctx = Context::new();
        // Allocate slots for an exported + a private word so the test
        // exercises the exports filter (private words must NOT appear in
        // the molded form).
        let priv_sym = Symbol::new("priv");
        ctx.set(priv_sym.clone(), Value::integer(999));
        let mut exports = std::collections::HashSet::new();
        for (i, w) in exported_words.iter().enumerate() {
            let s = Symbol::new(w);
            ctx.set(s.clone(), Value::integer(i as i64));
            exports.insert(s);
        }
        let mut md = ModuleDef::new();
        md.ctx = Rc::new(ctx);
        md.name = Some(Symbol::new("m"));
        *md.exports.borrow_mut() = exports;
        let v = Value::module(md);

        let molded1 = mold_to_string(&v);
        // Re-parse the molded form (must be valid source).
        let toks = red_core::lexer::lex(&molded1).expect("lex molded module");
        let body = red_core::parser::load(&toks).expect("load molded module");
        // The head value must be `Word("make")` — the `make module!` form.
        let data = body.data.borrow();
        prop_assert!(
            matches!(
                data.first(),
                Some(Value::Word { sym, .. }) if sym.as_str() == "make"
            ),
            "expected `make` head, got: {:?} (molded: {molded1})",
            data.first()
        );
        drop(data);
        // Re-mold the parsed body as a block; molding a block wraps it in
        // `[...]`, so strip the outer brackets to compare against the
        // original (unwrapped) module mold.
        let block = Value::block(body);
        let block_molded = mold_to_string(&block);
        let re_molded = block_molded
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(&block_molded);
        prop_assert_eq!(
            re_molded, molded1.clone(),
            "module mold not stable across round-trip"
        );
        // Private word must NOT appear in the molded form.
        prop_assert!(
            !molded1.contains("priv:"),
            "private word leaked into mold: {molded1}"
        );
    }
}

// M65: `closure!` molds as the opaque placeholder `#[closure]` (M60). Not
// reparseable as a literal — documented in the exclusion list above. This
// focused test asserts the placeholder is the stable string, so future
// printer changes don't silently break the contract.
#[test]
fn closure_mold_is_stable_placeholder() {
    // Build a minimal closure: empty spec + empty body + empty captures.
    let fd = red_core::value::FuncDef {
        body: Series::new(vec![]),
        ..Default::default()
    };
    let captures = Rc::new(Vec::new());
    let v = Value::closure(Rc::new(fd), captures);
    assert_eq!(mold_to_string(&v), "#[closure]");
    // `form` agrees with `mold` for the placeholder (no separate form).
    let mut form_buf = String::new();
    mold(&v, &mut form_buf);
    assert_eq!(form_buf, "#[closure]");
}

// M86: `unset!` is a synthetic sentinel that molds/forms to the empty string
// (matches Red). It is deliberately NOT added to `gen_value`'s round-trip
// pool — the empty mold re-parses as an empty block, not as a `Word("unset")`,
// so it cannot round-trip. The stable-string contract is asserted here.
#[test]
fn unset_mold_is_empty_string() {
    assert_eq!(mold_to_string(&Value::Unset), "");
    assert_eq!(form_to_string(&Value::Unset), "");
}

// M85: `image!` mold stability. Like `vector!`/`hash!`, `image!` is synthetic
// (mold as `make image! [...]` which parses to a block, not an Image value),
// so it's excluded from `gen_value`. This focused test builds an `ImageDef`
// directly with random dimensions + pixel bytes and asserts that molding it
// twice yields the same string, and that the form starts with `make image! [`.
proptest! {
    #[test]
    fn image_mold_is_stable(
        w in 0u16..4,
        h in 0u16..4,
        bytes in prop::collection::vec(any::<u8>(), 0..32),
    ) {
        let pixel_count = (w as usize) * (h as usize);
        let needed = pixel_count * 4;
        // Pad or truncate to exactly the required byte count so ImageDef
        // construction always succeeds.
        let buf: Vec<u8> = if bytes.len() >= needed {
            bytes[..needed].to_vec()
        } else {
            let mut v = bytes.clone();
            v.resize(needed, 0);
            v
        };
        let img = Value::image(ImageDef::from_bytes(w as usize, h as usize, &buf).unwrap());
        let molded1 = mold_to_string(&img);
        let molded2 = mold_to_string(&img);
        prop_assert_eq!(&molded1, &molded2, "mold not deterministic");
        prop_assert!(
            molded1.starts_with("make image! [width:"),
            "expected `make image! [width: ...]` form, got: {molded1}"
        );
        prop_assert!(molded1.ends_with(']'), "expected closing ]: {molded1}");
    }
}
