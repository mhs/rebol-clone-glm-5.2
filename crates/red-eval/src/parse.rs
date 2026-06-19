//! `parse` dialect (Milestone 10): a matcher mini-DSL over `string!` or
//! `block!` input.
//!
//! Implements the POC rule subset described in `project-brief.md`:
//! - Literal values (string/integer/word/lit-word) match against input.
//! - `skip`, `to value`, `thru value`, `end`, `none`.
//! - `any rule`, `some rule`, `opt rule`, `while rule`.
//! - `|` (alternative).
//! - `copy 'word rule` (capture sub-match), `set 'word rule` (single value).
//! - `[...]` grouping (sub-rules).
//! - `(...)` Red code side-effect, evaluated via `eval`.
//! - Returns `logic!` (matched entirely / failed).
//!
//! Backtracking: the input cursor is saved before each alternative branch and
//! each repetition iteration; on failure the cursor is restored. Captures
//! made by a branch that ultimately fails are *not* rolled back (matches
//! Red's documented behavior — once a `copy`/`set` writes a value, the
//! write stands even if a later rule fails the overall match).

use std::rc::Rc;

use red_core::value::{Series, Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::interp::eval;
use crate::natives::type_name;
use crate::series::{series_match, word_sym};

// ---------------------------------------------------------------------------
// Input cursor
// ---------------------------------------------------------------------------

/// Cursor over either a `string!` or `block!`/`paren!` input. The string
/// cursor is a byte index guarded by `str::is_char_boundary` so UTF-8 stays
/// intact. The series cursor reuses `Series.index` (Red series semantics).
enum Input {
    Str {
        src: Rc<str>,
        cursor: usize,
    },
    Series {
        series: Series,
        is_paren: bool,
        span: Span,
    },
}

impl Input {
    /// Snapshot of the cursor position for backtracking. Cheap: clones the
    /// `Rc<str>` (string mode) or bumps the outer `Rc` of the series storage
    /// (series mode — the underlying `Vec` is shared, not copied).
    fn save(&self) -> Cursor {
        match self {
            Input::Str { cursor, .. } => Cursor::Str { cursor: *cursor },
            Input::Series { series, .. } => Cursor::Series {
                index: series.index,
            },
        }
    }

    fn restore(&mut self, c: &Cursor) {
        match (self, c) {
            (Input::Str { cursor, .. }, Cursor::Str { cursor: saved }) => *cursor = *saved,
            (Input::Series { series, .. }, Cursor::Series { index: saved }) => {
                series.index = *saved
            }
            _ => unreachable!("cursor/input mode mismatch"),
        }
    }

    fn at_end(&self) -> bool {
        match self {
            Input::Str { src, cursor } => *cursor >= src.len(),
            Input::Series { series, .. } => series.index >= series.data.borrow().len(),
        }
    }

    /// `to end` / `thru end` — advance cursor to end of input.
    fn seek_end(&mut self) -> bool {
        match self {
            Input::Str { src, cursor } => {
                *cursor = src.len();
                true
            }
            Input::Series { series, .. } => {
                let len = series.data.borrow().len();
                series.index = len;
                true
            }
        }
    }

    /// Match a single literal value at the cursor; on success advance past
    /// it. Returns `true` on match. For string input the needle must be a
    /// `string!` matched as a prefix substring; for block input any value
    /// kind matches element-by-element via `series_match` (so a lit-word
    /// needle `'a` matches a `word!` element `a`).
    fn match_literal(&mut self, needle: &Value) -> bool {
        match self {
            Input::Str { src, cursor } => {
                let needle_str = match needle {
                    Value::String { s, .. } => s.as_ref(),
                    _ => return false,
                };
                let end = *cursor + needle_str.len();
                if end <= src.len() && &src[*cursor..end] == needle_str {
                    *cursor = end;
                    true
                } else {
                    false
                }
            }
            Input::Series { series, .. } => {
                let data = series.data.borrow_mut();
                let i = series.index;
                if i < data.len() && series_match(needle, &data[i]) {
                    series.index = i + 1;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// `skip` — advance one element/char. Returns `false` if at end.
    fn skip_one(&mut self) -> bool {
        match self {
            Input::Str { src, cursor } => {
                if *cursor >= src.len() {
                    false
                } else {
                    // Advance one UTF-8 char.
                    let next = src[*cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(idx, _)| *cursor + idx)
                        .unwrap_or(src.len());
                    *cursor = next;
                    true
                }
            }
            Input::Series { series, .. } => {
                let len = series.data.borrow().len();
                if series.index < len {
                    series.index += 1;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// `to value` — advance the cursor *until* `value` is found at the
    /// cursor; cursor ends positioned *at* the match (not past it). If the
    /// value is never found, the cursor advances to the end and the rule
    /// still succeeds (Red's "soft search" semantics — `to` never fails,
    /// it just moves as far as it can).
    fn to(&mut self, needle: &Value) -> bool {
        match self {
            Input::Str { src, cursor } => {
                let needle_str = match needle {
                    Value::String { s, .. } => s.as_ref(),
                    _ => return false,
                };
                if let Some(rel) = src[*cursor..].find(needle_str) {
                    *cursor += rel;
                } else {
                    // Not found — advance to end (soft search succeeds).
                    *cursor = src.len();
                }
                true
            }
            Input::Series { series, .. } => {
                let data = series.data.borrow();
                let mut i = series.index;
                while i < data.len() {
                    if series_match(needle, &data[i]) {
                        drop(data);
                        series.index = i;
                        return true;
                    }
                    i += 1;
                }
                // Not found — advance to end (soft search succeeds).
                drop(data);
                series.index = series.data.borrow().len();
                true
            }
        }
    }

    /// `thru value` — advance the cursor *past* `value`. Returns `false` if
    /// `value` never appears.
    fn thru(&mut self, needle: &Value) -> bool {
        if self.to(needle) {
            self.skip_one()
        } else {
            false
        }
    }

    /// Capture the sub-input between `start` and the current cursor.
    /// String mode returns a `string!`; series mode returns a positioned
    /// `block!`/`paren!` whose cursor is `start` (Red's `copy` semantics
    /// for block input return a sub-series, not a fresh block).
    fn capture(&self, start: &Cursor) -> Value {
        match (self, start) {
            (Input::Str { src, cursor }, Cursor::Str { cursor: start }) => {
                let s: &str = &src[*start..*cursor];
                Value::string(s)
            }
            (
                Input::Series {
                    series,
                    is_paren,
                    span,
                    ..
                },
                Cursor::Series { index: start },
            ) => {
                let mut sub = series.clone();
                sub.index = *start;
                if *is_paren {
                    Value::Paren {
                        series: sub,
                        span: *span,
                    }
                } else {
                    Value::Block {
                        series: sub,
                        span: *span,
                    }
                }
            }
            _ => unreachable!("cursor/input mode mismatch"),
        }
    }

    /// Capture the single value at the `start` cursor position. Used by
    /// `set 'word rule` which binds to one element (block mode) or the
    /// consumed substring (string mode — same as `capture` for a single-
    /// char match).
    fn capture_single(&self, start: &Cursor) -> Value {
        match (self, start) {
            (Input::Str { src, cursor }, Cursor::Str { cursor: start }) => {
                let s: &str = &src[*start..*cursor];
                Value::string(s)
            }
            (Input::Series { series, .. }, Cursor::Series { index: start }) => {
                let data = series.data.borrow();
                if *start < data.len() {
                    data[*start].clone()
                } else {
                    Value::None
                }
            }
            _ => unreachable!("cursor/input mode mismatch"),
        }
    }
}

enum Cursor {
    Str { cursor: usize },
    Series { index: usize },
}

// ---------------------------------------------------------------------------
// Rule interpreter
// ---------------------------------------------------------------------------

/// `parse` native: `parse <input> <rules>`.
///
/// Input is `string!`, `block!`, or `paren!`. Rules must be a `block!`.
/// Returns `logic!`: `true` if the rules matched *and* consumed the input
/// entirely (cursor at end), else `false`.
pub fn parse_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(EvalError::Arity {
            native: Symbol::new("parse"),
            expected: 2,
            got: args.len(),
            span: args
                .first()
                .map(|v| v.span_or_default())
                .unwrap_or_default(),
        });
    }

    let mut input = match &args[0] {
        Value::String { s, .. } => Input::Str {
            src: Rc::clone(s),
            cursor: 0,
        },
        Value::Block { series, span } => Input::Series {
            series: series.clone(),
            is_paren: false,
            span: *span,
        },
        Value::Paren { series, span } => Input::Series {
            series: series.clone(),
            is_paren: true,
            span: *span,
        },
        other => {
            return Err(EvalError::TypeError {
                expected: "string! or block!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };

    let (rules_series, _rules_span) = match &args[1] {
        Value::Block { series, span } => (series.clone(), *span),
        Value::Paren { series, span } => (series.clone(), *span),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };

    // Walk the rules block as data. `rule_seq` returns true if the entire
    // sequence of rules matched; on failure the cursor is left wherever the
    // failure occurred (the caller doesn't care — the top-level result is
    // `matched && input.at_end()`).
    let rules_data = rules_series.data.borrow().clone();
    let mut i = 0;
    let matched = rule_seq(&mut input, &rules_data, &mut i, env)?;

    Ok(Value::Logic(matched && input.at_end()))
}

/// Match a sequence of rules starting at `*i` in `rules`, stopping at the
/// end of the slice. Handles `|` alternatives: rules separated by `|` are
/// tried in order; the first fully-matching alternative wins and the rest
/// are skipped. On failure of all alternatives the cursor is restored to
/// its position before `rule_seq` was entered and `false` is returned.
fn rule_seq(
    input: &mut Input,
    rules: &[Value],
    i: &mut usize,
    env: &mut Env,
) -> Result<bool, EvalError> {
    let outer_saved = input.save();
    let start = *i;
    loop {
        // Try one alternative: match rules until a `|` or end-of-slice.
        let alt_saved = input.save();
        let mut ok = true;
        while *i < rules.len() && !is_word(rules[*i].clone(), "|") {
            let before = *i;
            let matched = rule_one(input, rules, i, env)?;
            if !matched {
                input.restore(&alt_saved);
                *i = before;
                ok = false;
                break;
            }
            if *i == before {
                // No progress — avoid infinite loop on a no-op rule.
                *i += 1;
            }
        }
        if ok {
            // This alternative matched. Skip any remaining `|`-separated
            // alternatives (they're ignored once one succeeds) and stop at
            // the end of the slice.
            *i = rules.len();
            return Ok(true);
        }
        // This alternative failed — scan forward to the next `|` and try
        // the alternative after it.
        while *i < rules.len() && !is_word(rules[*i].clone(), "|") {
            *i += 1;
        }
        if *i >= rules.len() {
            // No more `|` — all alternatives exhausted.
            input.restore(&outer_saved);
            *i = start;
            return Ok(false);
        }
        // Skip the `|` and loop to try the next alternative.
        *i += 1;
    }
}

/// Match a single rule starting at `*i`. Returns `false` (cursor restored)
/// if the rule fails. Advances `*i` past the rule on success.
fn rule_one(
    input: &mut Input,
    rules: &[Value],
    i: &mut usize,
    env: &mut Env,
) -> Result<bool, EvalError> {
    if *i >= rules.len() {
        return Ok(false);
    }
    let v = rules[*i].clone();

    // Keyword rules — recognized by an unbound `Word`.
    if let Some(sym) = word_sym(&v) {
        match sym.as_str() {
            "skip" => {
                *i += 1;
                return Ok(input.skip_one());
            }
            "to" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                // `to end` — advance to end of input.
                if is_word(rules[*i].clone(), "end") {
                    *i += 1;
                    return Ok(input.seek_end());
                }
                let needle = rules[*i].clone();
                *i += 1;
                return Ok(input.to(&needle));
            }
            "thru" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                // `thru end` — advance to end of input (past end is end).
                if is_word(rules[*i].clone(), "end") {
                    *i += 1;
                    input.seek_end();
                    return Ok(true);
                }
                let needle = rules[*i].clone();
                *i += 1;
                return Ok(input.thru(&needle));
            }
            "end" => {
                *i += 1;
                return Ok(input.at_end());
            }
            "none" => {
                *i += 1;
                return Ok(true);
            }
            "any" | "some" | "opt" | "while" => {
                let kind = sym.as_str();
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                let inner_start = *i;
                let result = run_repetition(input, rules, inner_start, kind, env)?;
                // Advance the rule cursor past the inner rule's syntactic
                // extent (without evaluating it — avoids double-evaluating
                // side-effects like `(...)`).
                *i = inner_start + rule_extent(rules, inner_start);
                return Ok(result);
            }
            "copy" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                let target = rules[*i].clone();
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                let start = input.save();
                let mut j = *i;
                let ok = rule_one(input, rules, &mut j, env)?;
                if ok {
                    let captured = input.capture(&start);
                    write_capture(env, &target, captured)?;
                    *i = j;
                    return Ok(true);
                } else {
                    return Ok(false);
                }
            }
            "set" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                let target = rules[*i].clone();
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                // `set 'w rule`: bind `w` to the single value matched by
                // `rule`. For block input that's the element at the cursor
                // when the rule started; for string input it's the
                // substring the rule consumed (a one-char string for
                // `skip`, etc.).
                let start = input.save();
                let mut j = *i;
                let ok = rule_one(input, rules, &mut j, env)?;
                if ok {
                    let captured = input.capture_single(&start);
                    write_capture(env, &target, captured)?;
                    *i = j;
                    return Ok(true);
                } else {
                    return Ok(false);
                }
            }
            _ => {}
        }
    }

    // `|` alternative separator — handled by `rule_seq`; a stray occurrence
    // at `rule_one` level (shouldn't happen) is a no-op match.
    if is_word(v.clone(), "|") {
        return Ok(true);
    }

    // `[...]` sub-rule group — recurse as a sub-sequence.
    if let Value::Block { series, .. } = &v {
        let child = series.data.borrow().clone();
        let saved = input.save();
        let mut j = 0;
        let ok = rule_seq(input, &child, &mut j, env)?;
        *i += 1;
        if !ok {
            input.restore(&saved);
            return Ok(false);
        }
        return Ok(true);
    }

    // `(...)` Red side-effect — evaluate via `eval`, succeed iff result is
    // truthy (only `false`/`none` fail).
    if let Value::Paren { .. } = &v {
        let result = eval(&v, env)?;
        *i += 1;
        return Ok(!matches!(result, Value::None | Value::Logic(false)));
    }

    // Literal value — match against input.
    let saved = input.save();
    if input.match_literal(&v) {
        *i += 1;
        Ok(true)
    } else {
        input.restore(&saved);
        Ok(false)
    }
}

/// True if `v` is an unbound `Word` with the given name.
fn is_word(v: Value, name: &str) -> bool {
    matches!(&v, Value::Word { sym, binding: red_core::value::Binding::Unbound, .. } if sym.as_str() == name)
}

/// Write a captured value into the user-context slot for `target` (a
/// `LitWord` or `Word`). Mirrors the `repeat`/`foreach`/`set` pattern: the
/// slot was pre-allocated by `collect_parse_words` in the binding pass.
fn write_capture(env: &mut Env, target: &Value, value: Value) -> Result<(), EvalError> {
    let sym = match target {
        Value::LitWord { sym, .. } => sym.clone(),
        Value::Word { sym, .. } => sym.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    let idx = env
        .user_ctx
        .index_of(&sym)
        .ok_or_else(|| EvalError::UnboundWord {
            sym: sym.clone(),
            span: target.span_or_default(),
        })?;
    env.user_ctx.set_slot(idx, value);
    Ok(())
}

/// Repetition runner for `any`/`some`/`opt`/`while`. Runs the inner rule
/// (located at `inner_start` in `rules`) repeatedly against `input`,
/// backtracking on the first failed iteration. Returns success per the
/// rule kind:
/// - `any`:  zero or more matches.
/// - `some`: one or more matches.
/// - `opt`:  zero or one match.
/// - `while`: like `any` but stops as soon as the rule matches without
///   advancing the cursor (avoids infinite loops on no-progress rules).
fn run_repetition(
    input: &mut Input,
    rules: &[Value],
    inner_start: usize,
    kind: &str,
    env: &mut Env,
) -> Result<bool, EvalError> {
    let mut count = 0usize;
    loop {
        if input.at_end() && kind != "opt" {
            break;
        }
        let saved = input.save();
        let mut j = inner_start;
        let ok = rule_one(input, rules, &mut j, env)?;
        if !ok {
            input.restore(&saved);
            break;
        }
        // Progress guard: if the rule matched without advancing the input,
        // stop to avoid an infinite loop (matches Red's `any`/`while`
        // progress check). Count the no-progress iteration once for `some`.
        if input.save().equals(&saved) {
            count += 1;
            break;
        }
        count += 1;
        if kind == "opt" {
            break;
        }
    }
    let success = match kind {
        "some" => count >= 1,
        "opt" => count <= 1,
        _ => true,
    };
    Ok(success)
}

impl Cursor {
    fn equals(&self, other: &Cursor) -> bool {
        match (self, other) {
            (Cursor::Str { cursor: a }, Cursor::Str { cursor: b }) => a == b,
            (Cursor::Series { index: a }, Cursor::Series { index: b }) => a == b,
            _ => false,
        }
    }
}

/// Number of rule slots consumed by the rule starting at `rules[i]`,
/// computed syntactically (no evaluation, no side-effects). Used by
/// `any`/`some`/`opt`/`while` to advance the rule cursor past their inner
/// rule without re-running it.
fn rule_extent(rules: &[Value], i: usize) -> usize {
    if i >= rules.len() {
        return 0;
    }
    let v = &rules[i];
    let sym = match word_sym(v) {
        Some(s) => s.as_str(),
        None => return 1, // literal / `[...]` / `(...)` — one slot
    };
    match sym {
        "skip" | "end" | "none" => 1,
        "to" | "thru" => {
            // `to X` / `thru X` — X is one slot (a value or `end`).
            if i + 1 < rules.len() {
                2
            } else {
                1
            }
        }
        "any" | "some" | "opt" | "while" => {
            1 + if i + 1 < rules.len() {
                rule_extent(rules, i + 1)
            } else {
                0
            }
        }
        "copy" | "set" => {
            // `copy W R` / `set W R` — W is one slot, R is one rule.
            2 + if i + 2 < rules.len() {
                rule_extent(rules, i + 2)
            } else {
                0
            }
        }
        _ => 1, // a word used as a literal match — one slot
    }
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

    struct BufferWriter(Rc<RefCell<Vec<u8>>>);

    impl Write for BufferWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
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

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    fn m(v: &Value) -> String {
        mold_to_string(v)
    }

    #[test]
    fn parse_string_match() {
        assert_eq!(m(&val(r#"parse "abc" ["a" "b" "c"]"#)), "true");
    }

    #[test]
    fn parse_string_fail() {
        assert_eq!(m(&val(r#"parse "abc" ["a" "z"]"#)), "false");
    }

    #[test]
    fn parse_block_match() {
        assert_eq!(m(&val("parse [1 2 3] [1 2 3]")), "true");
    }

    #[test]
    fn parse_copy_to_end() {
        // `copy w to end` captures the whole input into `w`; `w`'s slot is
        // pre-allocated by `collect_parse_words` in the binding pass, so no
        // explicit `w: none` pre-declaration is needed.
        let v = val(r#"parse "hello" [copy w to end] w"#);
        assert_eq!(m(&v), "\"hello\"");
        // The parse itself returned true (w is the last expr, but we can
        // confirm the match succeeded by checking w was actually written).
        let v2 = val(r#"parse "hello" [copy w to end]"#);
        assert_eq!(m(&v2), "true");
    }

    #[test]
    fn parse_some_skip_to() {
        assert_eq!(m(&val(r#"parse "a;b;c" [some [skip to ";"]]"#)), "true");
    }

    #[test]
    fn parse_side_effect() {
        let out = run_capture_val(r#"n: 0 parse "ab" ["a" (n: n + 1) "b" (n: n + 1)] print n"#)
            .unwrap()
            .1;
        assert_eq!(String::from_utf8(out).unwrap(), "2\n");
    }

    #[test]
    fn parse_side_effect_runs() {
        // Explicit `(...)` side-effect executes Red code; matching continues
        // after the paren result is checked for truthiness.
        let out = run_capture_val(r#"s: [] parse "x" [(append s "x")] probe s"#)
            .unwrap()
            .1;
        assert_eq!(String::from_utf8(out).unwrap(), "== [\"x\"]\n");
    }

    #[test]
    fn parse_alternative() {
        // `|` tries the left branch first, then the right.
        assert_eq!(m(&val(r#"parse "b" ["a" | "b" | "c"]"#)), "true");
        assert_eq!(m(&val(r#"parse "z" ["a" | "b" | "c"]"#)), "false");
    }
}
