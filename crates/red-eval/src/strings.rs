//! String manipulation natives (Milestone 15).
//!
//! These natives operate on `Value::String` (stored as `Rc<str>`, immutable).
//! Each returns a fresh `Value::String` with zero span (synthetic, per the
//! POC convention for runtime-produced strings); in-place mutation is not
//! possible because `Rc<str>` is not a `Series` (no cursor, no `RefCell`).
//!
//! Natives:
//!   - `rejoin block` — reduce block, concatenate molded results to a string
//!   - `reform block`  — reduce block, concatenate formed results to a string
//!   - `join a b`      — form both operands, concatenate
//!   - `split str [delim]` / `split/with str delim` — split into a block of
//!     substrings (default delimiter: any whitespace run; `/with` uses the
//!     given string delimiter)
//!   - `trim str` + `/auto` `/with` `/lines` `/all` — strip whitespace
//!   - `replace str search repl` + `/all`
//!   - `uppercase`/`lowercase str` + `/part n`
//!   - `suffix? str` — file extension including the dot, or `none`
//!
//! `find`/`copy` string extensions live in `series.rs` (alongside the block
//! paths); `+` over strings lives in `natives.rs::add`.

use red_core::printer::form_to_string;
use red_core::value::{FuncDef, Series, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};
use std::rc::Rc;

use crate::interp::eval_expression;
use crate::natives::{expect_block, type_name};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the string contents of a `Value::String`, or raise a TypeError.
fn expect_string(v: &Value) -> Result<Rc<str>, EvalError> {
    match v {
        Value::String { s, .. } => Ok(s.clone()),
        other => Err(EvalError::TypeError {
            expected: "string!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// Build a `Value::String` from an owned `String`.
fn mk_string(s: String) -> Value {
    Value::string(Rc::from(s.as_str()))
}

fn arity_err(args: &[Value], native: &str, expected: usize, got: usize) -> EvalError {
    EvalError::Arity {
        native: Symbol::new(native),
        expected,
        got,
        span: args
            .first()
            .map(|v| v.span_or_default())
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// rejoin / reform
// ---------------------------------------------------------------------------

/// `rejoin block` — evaluate each expression in the block, form each result
/// (no quotes around strings), concatenate with no separator into a single
/// `string!`. Matches Red's `rejoin` semantics: `rejoin ["a" 1 "b"]` →
/// `"a1b"`. (Plan2.md described this as "molded results", but the inline
/// test expectation `"a1b"` requires `form` — strings mold with quotes,
/// which would yield `"a"1"b"` instead. Form is Red's actual behavior.)
fn rejoin(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "rejoin")?;
    let series = match &body {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        _ => unreachable!("expect_block guarantees Block"),
    };
    let data = series.data.borrow();
    let mut out = String::new();
    let mut i = series.index;
    while i < data.len() {
        let v = eval_expression(&data, &mut i, env)?;
        out.push_str(&form_to_string(&v));
    }
    Ok(mk_string(out))
}

/// `reform block` — reduce the block, then `form` the resulting block (which
/// space-joins the values, no brackets). `reform ["a" "b"]` → `"a b"`.
fn reform(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let body = expect_block(args, 0, "reform")?;
    let series = match &body {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        _ => unreachable!("expect_block guarantees Block"),
    };
    let data = series.data.borrow();
    let mut results = Vec::new();
    let mut i = series.index;
    while i < data.len() {
        results.push(eval_expression(&data, &mut i, env)?);
    }
    drop(data);
    let result_block = Value::block(Series::new(results));
    Ok(mk_string(form_to_string(&result_block)))
}

// ---------------------------------------------------------------------------
// join
// ---------------------------------------------------------------------------

/// `join a b` — form both operands and concatenate. `join 1 "b"` → `"1b"`;
/// `join "a" "b"` → `"ab"`; `join [1] 2` → `"[1] 2"` (form of a block is
/// space-joined, no brackets).
fn join(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "join", 2, args.len()));
    }
    let mut out = String::new();
    out.push_str(&form_to_string(&args[0]));
    out.push_str(&form_to_string(&args[1]));
    Ok(mk_string(out))
}

// ---------------------------------------------------------------------------
// split
// ---------------------------------------------------------------------------

/// `split str dlm` — split `str` into a block of substrings using `dlm` as
/// the delimiter. `split/with str dlm` is the same call with the `/with`
/// flag set (Red's `/with` distinguishes "treat dlm as a single delimiter"
/// from "treat dlm as a block of alternative delimiters"; for the POC's
/// string-only split, the distinction is moot, so `/with` is a no-op flag).
///
/// An empty `dlm` splits into individual characters. A `none` dlm is
/// treated the same as an empty string (split into chars).
fn split(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "split", 2, args.len()));
    }
    let src = expect_string(&args[0])?;
    // `/with` is a no-op flag (arity 0) — accepted for parity with Red.
    let _ = refs.has(&Symbol::new("with"));
    let delim = match &args[1] {
        Value::String { s, .. } => s.clone(),
        Value::None => Rc::from(""),
        other => {
            return Err(EvalError::TypeError {
                expected: "string!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let parts: Vec<String> = if delim.is_empty() {
        src.chars().map(|c| c.to_string()).collect()
    } else {
        src.split(delim.as_ref()).map(|s| s.to_string()).collect()
    };
    let values: Vec<Value> = parts.into_iter().map(mk_string).collect();
    Ok(Value::block(Series::new(values)))
}

// ---------------------------------------------------------------------------
// trim
// ---------------------------------------------------------------------------

/// `trim str` — strip leading and trailing whitespace from a string.
///
/// Refinements:
///   - `/all`   — remove all internal+external whitespace
///   - `/lines` — strip trailing newlines/CRs only
///   - `/with chars` — strip any of the given characters (lead+trail)
///   - `/auto`  — auto-detect leading ws (POC: same as default)
fn trim(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "trim", 1, 0));
    }
    let src = expect_string(&args[0])?;

    if refs.has(&Symbol::new("all")) {
        // Remove every whitespace char (internal + external).
        let trimmed: String = src.chars().filter(|c| !c.is_whitespace()).collect();
        return Ok(mk_string(trimmed));
    }

    if refs.has(&Symbol::new("lines")) {
        // Strip trailing newlines/CRs (and only those).
        let trimmed = src.trim_end_matches(['\n', '\r']);
        return Ok(mk_string(trimmed.to_string()));
    }

    if let Some(with_args) = refs.get(&Symbol::new("with")) {
        if let Some(arg) = with_args.first() {
            let strip = expect_string(arg)?;
            let trimmed = src.trim_matches(|c| strip.contains(c));
            return Ok(mk_string(trimmed.to_string()));
        }
    }

    // Default: strip leading + trailing ASCII whitespace.
    Ok(mk_string(src.trim().to_string()))
}

// ---------------------------------------------------------------------------
// replace
// ---------------------------------------------------------------------------

/// `replace str search repl` — replace the first occurrence of `search` with
/// `repl` in `str`. `replace/all` replaces every occurrence.
fn replace(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 3 {
        return Err(arity_err(args, "replace", 3, args.len()));
    }
    let src = expect_string(&args[0])?;
    let search = expect_string(&args[1])?;
    let repl = expect_string(&args[2])?;

    if search.is_empty() {
        // Nothing to search for; return the input unchanged.
        return Ok(mk_string((*src).to_string()));
    }

    let all = refs.has(&Symbol::new("all"));
    let out = if all {
        src.replace(search.as_ref(), repl.as_ref())
    } else {
        src.replacen(search.as_ref(), repl.as_ref(), 1)
    };
    Ok(mk_string(out))
}

// ---------------------------------------------------------------------------
// uppercase / lowercase
// ---------------------------------------------------------------------------

/// `uppercase str` — uppercase the whole string. `uppercase/part n` —
/// uppercase the first `n` chars, leave the rest unchanged.
fn uppercase(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    change_case(args, refs, true, "uppercase")
}

/// `lowercase str` — lowercase the whole string. `lowercase/part n` —
/// lowercase the first `n` chars, leave the rest unchanged.
fn lowercase(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    change_case(args, refs, false, "lowercase")
}

fn change_case(
    args: &[Value],
    refs: &RefineArgs,
    upper: bool,
    native: &str,
) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, native, 1, 0));
    }
    let src = expect_string(&args[0])?;

    let part = if refs.has(&Symbol::new("part")) {
        match refs.get(&Symbol::new("part")).and_then(|a| a.first()) {
            Some(Value::Integer { n, .. }) => Some(*n),
            Some(other) => {
                return Err(EvalError::TypeError {
                    expected: "integer!",
                    found: type_name(other),
                    span: other.span_or_default(),
                });
            }
            None => None,
        }
    } else {
        None
    };

    let out: String = match part {
        Some(n) if n >= 0 => {
            let n = n as usize;
            let chars: Vec<char> = src.chars().collect();
            let mut head: String = chars.iter().take(n).collect();
            let tail: String = chars.iter().skip(n).collect();
            if upper {
                head = head.to_uppercase();
            } else {
                head = head.to_lowercase();
            }
            head.push_str(&tail);
            head
        }
        // Negative `/part` is undefined in Red for case-change; clamp to 0
        // (POC: treat as no-op, return the source unchanged).
        Some(n) if n < 0 => (*src).to_string(),
        // No `/part` refinement: transform the whole string.
        _ => {
            if upper {
                src.to_uppercase()
            } else {
                src.to_lowercase()
            }
        }
    };
    Ok(mk_string(out))
}

// ---------------------------------------------------------------------------
// suffix?
// ---------------------------------------------------------------------------

/// `suffix? str` — return the file extension including the leading dot, or
/// `none` if no `.` in the file's tail (no path separator in last segment).
/// Matches Red's `suffix?` semantics for string input as closely as the POC
/// can without a `file!` type: `"foo.txt"` → `".txt"`, `"foo"` → `none`,
/// `"a.b/foo"` → `none` (the dot is not in the final path segment).
fn suffix_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "suffix?", 1, 0));
    }
    let src = expect_string(&args[0])?;
    // Take the last path segment (Red uses `/` as the path separator on
    // all platforms; we honor that here too).
    let tail = match src.rfind('/') {
        Some(i) => &src[i + 1..],
        None => src.as_ref(),
    };
    match tail.rfind('.') {
        // Empty extension like "foo." → Red returns "." (an empty suffix
        // string). Honor that.
        Some(i) => Ok(mk_string(tail[i..].to_string())),
        None => Ok(Value::None),
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

/// Register the M15 string natives.
pub fn register_string_natives(env: &mut Env) {
    let reg = |env: &mut Env, name: &str, f: NF, arity: usize| {
        let params: Vec<Symbol> = (0..arity)
            .map(|i| Symbol::new(&format!("__arg{i}")))
            .collect();
        env.natives.insert(
            Symbol::new(name),
            Rc::new(FuncDef {
                params,
                native: Some(f),
                variadic: false,
                infix: false,
                                ..Default::default()
            }),
        );
    };

    let reg_refined =
        |env: &mut Env, name: &str, f: NF, arity: usize, refines: &[(&str, usize)]| {
            let params: Vec<Symbol> = (0..arity)
                .map(|i| Symbol::new(&format!("__arg{i}")))
                .collect();
            let refinements: Vec<(Symbol, Vec<Symbol>)> = refines
                .iter()
                .map(|(rname, rarity)| {
                    let rargs: Vec<Symbol> = (0..*rarity)
                        .map(|i| Symbol::new(&format!("__{rname}_arg{i}")))
                        .collect();
                    (Symbol::new(rname), rargs)
                })
                .collect();
            env.natives.insert(
                Symbol::new(name),
                Rc::new(FuncDef {
                    params,
                    refinements,
                    native: Some(f),
                    variadic: false,
                    infix: false,
                                        ..Default::default()
                }),
            );
        };

    // Arity-1 natives without refinements.
    reg(env, "rejoin", rejoin as NF, 1);
    reg(env, "reform", reform as NF, 1);
    reg(env, "join", join as NF, 2);
    reg(env, "suffix?", suffix_q as NF, 1);

    // Refinement-bearing natives.
    reg_refined(env, "split", split as NF, 2, &[("with", 0)]);
    reg_refined(
        env,
        "trim",
        trim as NF,
        1,
        &[("auto", 0), ("with", 1), ("lines", 0), ("all", 0)],
    );
    reg_refined(env, "replace", replace as NF, 3, &[("all", 0)]);
    reg_refined(env, "uppercase", uppercase as NF, 1, &[("part", 1)]);
    reg_refined(env, "lowercase", lowercase as NF, 1, &[("part", 1)]);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::bind_pass;
    use crate::natives::{install_constants, register_natives};
    use red_core::context::Context;
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    struct BufferWriter(Rc<RefCell<Vec<u8>>>);
    impl Write for BufferWriter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn run_capture_val(src: &str) -> Result<(Value, Vec<u8>), String> {
        let body = load_source(src).map_err(|e| e.to_string())?;
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        let block = Value::block(body);
        let val = eval(&block, &mut env).map_err(|e| e.to_string())?;
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    use crate::interp::eval;

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }
    fn mold_val(v: &Value) -> String {
        mold_to_string(v)
    }

    // --- rejoin ---

    #[test]
    fn rejoin_molds_and_concatenates() {
        // rejoin ["a" 1 "b"] → "a1b"  (1 molds to "1"; the whole thing is a
        // string, which molds to the quoted form).
        assert_eq!(mold_val(&val("rejoin [\"a\" 1 \"b\"]")), "\"a1b\"");
    }

    #[test]
    fn rejoin_evaluates_expressions() {
        assert_eq!(mold_val(&val("rejoin [1 + 2 \"x\"]")), "\"3x\"");
    }

    // --- reform ---

    #[test]
    fn reform_forms_and_concatenates() {
        // reform reduces the block, then forms the resulting block
        // (space-joined, no brackets).
        assert_eq!(mold_val(&val("reform [\"a\" \"b\"]")), "\"a b\"");
    }

    // --- join ---

    #[test]
    fn join_int_and_string() {
        assert_eq!(mold_val(&val("join 1 \"b\"")), "\"1b\"");
    }
    #[test]
    fn join_two_strings() {
        assert_eq!(mold_val(&val("join \"a\" \"b\"")), "\"ab\"");
    }

    // --- split ---

    #[test]
    fn split_with_string_delimiter() {
        assert_eq!(
            mold_val(&val("split \"a,b,c\" \",\"")),
            "[\"a\" \"b\" \"c\"]"
        );
    }
    #[test]
    fn split_with_refinement_flag() {
        // `/with` is a no-op flag (arity 0); delimiter is the 2nd positional.
        assert_eq!(
            mold_val(&val("split/with \"a,b,c\" \",\"")),
            "[\"a\" \"b\" \"c\"]"
        );
    }
    #[test]
    fn split_on_whitespace_with_explicit_space() {
        assert_eq!(
            mold_val(&val("split \"a b c\" \" \"")),
            "[\"a\" \"b\" \"c\"]"
        );
    }
    #[test]
    fn split_empty_delimiter_into_chars() {
        assert_eq!(mold_val(&val("split \"abc\" \"\"")), "[\"a\" \"b\" \"c\"]");
    }

    // --- trim ---

    #[test]
    fn trim_strips_lead_and_trail() {
        assert_eq!(mold_val(&val("trim \"  hi  \"")), "\"hi\"");
    }
    #[test]
    fn trim_all_removes_internal_ws() {
        assert_eq!(mold_val(&val("trim/all \"  a  b  \"")), "\"ab\"");
    }
    #[test]
    fn trim_lines_strips_trailing_newlines() {
        assert_eq!(mold_val(&val("trim/lines \"hi\\n\\n\"")), "\"hi\"");
    }
    #[test]
    fn trim_with_strips_given_chars_from_head_and_tail() {
        // trim/with strips the given chars from the head and tail only
        // (not internal occurrences), per Red's trim/with semantics.
        assert_eq!(mold_val(&val("trim/with \"xabx\" \"x\"")), "\"ab\"");
    }

    // --- replace ---

    #[test]
    fn replace_single() {
        assert_eq!(mold_val(&val("replace \"a-a\" \"a\" \"b\"")), "\"b-a\"");
    }
    #[test]
    fn replace_all() {
        assert_eq!(mold_val(&val("replace/all \"a-a\" \"a\" \"b\"")), "\"b-b\"");
    }
    #[test]
    fn replace_empty_search_noops() {
        assert_eq!(mold_val(&val("replace \"abc\" \"\" \"X\"")), "\"abc\"");
    }

    // --- uppercase / lowercase ---

    #[test]
    fn uppercase_full() {
        assert_eq!(mold_val(&val("uppercase \"abc\"")), "\"ABC\"");
    }
    #[test]
    fn lowercase_full() {
        assert_eq!(mold_val(&val("lowercase \"ABC\"")), "\"abc\"");
    }
    #[test]
    fn uppercase_part() {
        assert_eq!(mold_val(&val("uppercase/part \"abc\" 2")), "\"ABc\"");
    }
    #[test]
    fn lowercase_part() {
        assert_eq!(mold_val(&val("lowercase/part \"ABCXYZ\" 3")), "\"abcXYZ\"");
    }

    // --- suffix? ---

    #[test]
    fn suffix_present() {
        assert_eq!(mold_val(&val("suffix? \"foo.txt\"")), "\".txt\"");
    }
    #[test]
    fn suffix_none_when_no_dot() {
        assert_eq!(mold_val(&val("suffix? \"foo\"")), "none");
    }
    #[test]
    fn suffix_ignores_dot_in_earlier_segment() {
        // The only dot is in a parent path segment, not the tail.
        assert_eq!(mold_val(&val("suffix? \"a.b/foo\"")), "none");
    }

    // --- string + operator (defined in natives.rs::add) ---

    #[test]
    fn string_plus_concatenates() {
        assert_eq!(mold_val(&val("\"abc\" + \"def\"")), "\"abcdef\"");
    }

    // --- find / copy on strings (extended in series.rs) ---

    #[test]
    fn find_substring_returns_tail_from_match() {
        assert_eq!(mold_val(&val("find \"hello\" \"ll\"")), "\"llo\"");
    }
    #[test]
    fn find_substring_no_match_returns_none() {
        assert_eq!(mold_val(&val("find \"hello\" \"zz\"")), "none");
    }
    #[test]
    fn copy_string_returns_clone() {
        assert_eq!(mold_val(&val("copy \"abc\"")), "\"abc\"");
    }
    #[test]
    fn copy_part_string_limits_length() {
        assert_eq!(mold_val(&val("copy/part \"abc\" 2")), "\"ab\"");
    }
}
