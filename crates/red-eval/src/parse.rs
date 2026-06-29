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
    /// `string!` (or single `char!`) matched as a prefix substring; for
    /// block input any value kind matches element-by-element via
    /// `series_match` (so a lit-word needle `'a` matches a `word!` element
    /// `a`). `case_sensitive` controls string/char case-folding (Red's
    /// default is case-insensitive; `/case` enables case-sensitive).
    fn match_literal(&mut self, needle: &Value, case_sensitive: bool) -> bool {
        match self {
            Input::Str { src, cursor } => {
                // M46: char! needles match a single char.
                if let Value::Char { c, .. } = needle {
                    let c = *c;
                    if *cursor >= src.len() {
                        return false;
                    }
                    let rest = &src[*cursor..];
                    let next = match rest.chars().next() {
                        Some(ch) => ch,
                        None => return false,
                    };
                    let eq = if case_sensitive {
                        next == c
                    } else {
                        next.eq_ignore_ascii_case(&c)
                    };
                    if eq {
                        // Advance one UTF-8 char.
                        let adv = rest
                            .char_indices()
                            .nth(1)
                            .map(|(idx, _)| idx)
                            .unwrap_or(rest.len());
                        *cursor += adv;
                        true
                    } else {
                        false
                    }
                } else {
                    let needle_str = match needle {
                        Value::String { s, .. } => s.as_ref(),
                        _ => return false,
                    };
                    let end = *cursor + needle_str.len();
                    if end <= src.len() {
                        let hay = &src[*cursor..end];
                        let eq = if case_sensitive {
                            hay == needle_str
                        } else {
                            hay.eq_ignore_ascii_case(needle_str)
                        };
                        if eq {
                            *cursor = end;
                            return true;
                        }
                    }
                    // Fallback: case-insensitive match may have different
                    // byte length if non-ASCII folding occurs. For ASCII
                    // (the common case) the lengths match and the fast
                    // path above suffices. For non-ASCII case-insensitivity,
                    // walk char-by-char.
                    if !case_sensitive && needle_str.is_ascii() {
                        let needle_lower = needle_str.to_ascii_lowercase();
                        let hay_chars = src[*cursor..].chars();
                        let mut consumed = 0usize;
                        let mut ok = true;
                        let mut iter = hay_chars.peekable();
                        for nc in needle_lower.chars() {
                            match iter.next() {
                                Some(hc) if hc.to_ascii_lowercase() == nc => {
                                    consumed += hc.len_utf8();
                                }
                                _ => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        if ok {
                            *cursor += consumed;
                            return true;
                        }
                    }
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

    /// `to value` — advance the cursor *until* `value` is found at the
    /// cursor; cursor ends positioned *at* the match (not past it). If the
    /// value is never found, the cursor advances to the end and the rule
    /// still succeeds (Red's "soft search" semantics — `to` never fails,
    /// it just moves as far as it can).
    fn to(&mut self, needle: &Value, case_sensitive: bool) -> bool {
        match self {
            Input::Str { src, cursor } => {
                if let Value::Char { c, .. } = needle {
                    // char! `to` — advance until that char.
                    let target = *c;
                    let rest = &src[*cursor..];
                    for (idx, ch) in rest.char_indices() {
                        let eq = if case_sensitive {
                            ch == target
                        } else {
                            ch.eq_ignore_ascii_case(&target)
                        };
                        if eq {
                            *cursor += idx;
                            return true;
                        }
                    }
                    *cursor = src.len();
                    return true;
                }
                let needle_str = match needle {
                    Value::String { s, .. } => s.as_ref(),
                    _ => return false,
                };
                if case_sensitive {
                    if let Some(rel) = src[*cursor..].find(needle_str) {
                        *cursor += rel;
                    } else {
                        *cursor = src.len();
                    }
                } else {
                    // Case-insensitive search.
                    let needle_lower = needle_str.to_ascii_lowercase();
                    let rest = &src[*cursor..];
                    let mut found = false;
                    for (idx, _) in rest.char_indices() {
                        let hay = &rest[idx..];
                        if hay.len() >= needle_lower.len() {
                            let hay_prefix = &hay[..needle_lower.len()];
                            if hay_prefix.eq_ignore_ascii_case(&needle_lower) {
                                *cursor += idx;
                                found = true;
                                break;
                            }
                        }
                    }
                    if !found {
                        *cursor = src.len();
                    }
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
                drop(data);
                series.index = series.data.borrow().len();
                true
            }
        }
    }

    /// `thru value` — advance the cursor *past* `value`. Returns `false` if
    /// `value` never appears.
    fn thru(&mut self, needle: &Value, case_sensitive: bool) -> bool {
        if self.to(needle, case_sensitive) {
            self.skip_one()
        } else {
            false
        }
    }

    /// M46: match a bitset! rule against the current input. For string
    /// input, test the current char's byte for membership; on a hit advance
    /// by one char and succeed. For block input, test the current element
    /// if it's a char!/integer! (else fail).
    fn match_bitset(&mut self, bs: &std::cell::RefCell<red_core::value::BitsetDef>) -> bool {
        let bs = bs.borrow();
        match self {
            Input::Str { src, cursor } => {
                if *cursor >= src.len() {
                    return false;
                }
                let rest = &src[*cursor..];
                let ch = match rest.chars().next() {
                    Some(c) => c,
                    None => return false,
                };
                let b = (ch as u32) as usize;
                if b < 256 && bs.test(b) {
                    let adv = rest
                        .char_indices()
                        .nth(1)
                        .map(|(idx, _)| idx)
                        .unwrap_or(rest.len());
                    *cursor += adv;
                    true
                } else {
                    false
                }
            }
            Input::Series { series, .. } => {
                let data = series.data.borrow();
                let i = series.index;
                if i >= data.len() {
                    return false;
                }
                let byte = match &data[i] {
                    Value::Char { c, .. } => (*c as u32) as usize,
                    Value::Integer { n, .. } => *n as usize,
                    _ => return false,
                };
                if byte < 256 && bs.test(byte) {
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

/// `parse` native: `parse <input> <rules> [/case]`.
///
/// Input is `string!`, `block!`, or `paren!`. Rules must be a `block!`.
/// Returns `logic!`: `true` if the rules matched *and* consumed the input
/// entirely (cursor at end), else `false`.
///
/// M46: `/case` refinement enables case-sensitive matching for string/char
/// literals. The default (no `/case`) is case-insensitive (Red parity) —
/// string/char needles are compared with `eq_ignore_ascii_case`.
pub fn parse_native(args: &[Value], refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
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

    let case_sensitive = refs.has(&Symbol::new("case"));

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
    let mut collect_stack: Vec<Vec<Value>> = Vec::new();
    let matched = match rule_seq(
        &mut input,
        &rules_data,
        &mut i,
        env,
        case_sensitive,
        &mut collect_stack,
    ) {
        Ok(m) => m,
        Err(EvalError::Break(Some(Value::Logic(true)))) => {
            // M46: `break` — parse succeeds immediately.
            return Ok(Value::Logic(true));
        }
        Err(EvalError::Break(Some(Value::Logic(false)))) => {
            // M46: `reject` — parse fails immediately.
            return Ok(Value::Logic(false));
        }
        Err(EvalError::Break(_)) => {
            // Non-logic break value — treat as parse failure.
            return Ok(Value::Logic(false));
        }
        Err(e) => return Err(e),
    };

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
    case_sensitive: bool,
    collect_stack: &mut Vec<Vec<Value>>,
) -> Result<bool, EvalError> {
    let outer_saved = input.save();
    let start = *i;
    loop {
        // Try one alternative: match rules until a `|` or end-of-slice.
        let alt_saved = input.save();
        let mut ok = true;
        while *i < rules.len() && !is_word(rules[*i].clone(), "|") {
            let before = *i;
            let matched = rule_one(input, rules, i, env, case_sensitive, collect_stack)?;
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
    case_sensitive: bool,
    collect_stack: &mut Vec<Vec<Value>>,
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
                let saved = input.save();
                let ok = input.skip_one();
                if ok {
                    // Push the consumed char/element to the active collect
                    // target (if any).
                    if let Some(top) = collect_stack.last_mut() {
                        top.push(capture_match_value(input, &saved));
                    }
                }
                return Ok(ok);
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
                return Ok(input.to(&needle, case_sensitive));
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
                return Ok(input.thru(&needle, case_sensitive));
            }
            "end" => {
                *i += 1;
                return Ok(input.at_end());
            }
            "none" => {
                *i += 1;
                return Ok(true);
            }
            // M46: `fail` — always fails.
            "fail" => {
                *i += 1;
                return Ok(false);
            }
            // M46: `break` — exits the parse entirely with a true result.
            // Uses `EvalError::Break` as a control-flow sentinel caught at
            // the top of `parse_native`.
            "break" => {
                *i += 1;
                return Err(EvalError::Break(Some(Value::Logic(true))));
            }
            // M46: `reject` — fails the parse entirely (false result).
            "reject" => {
                *i += 1;
                return Err(EvalError::Break(Some(Value::Logic(false))));
            }
            // M46: `??` debug rule — prints cursor position, always succeeds.
            "??" => {
                *i += 1;
                let pos = match &*input {
                    Input::Str { src, cursor } => {
                        let line = src[..(*cursor).min(src.len())].matches('\n').count() + 1;
                        format!("parse: cursor at byte {cursor} (line {line})\n")
                    }
                    Input::Series { series, .. } => {
                        format!("parse: cursor at index {}\n", series.index)
                    }
                };
                use std::io::Write;
                let _ = env.out.write_all(pos.as_bytes());
                return Ok(true);
            }
            "any" | "some" | "opt" | "while" => {
                let kind = sym.as_str();
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                let inner_start = *i;
                let result = run_repetition(
                    input,
                    rules,
                    inner_start,
                    kind,
                    env,
                    case_sensitive,
                    collect_stack,
                )?;
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
                let ok = rule_one(input, rules, &mut j, env, case_sensitive, collect_stack)?;
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
                let ok = rule_one(input, rules, &mut j, env, case_sensitive, collect_stack)?;
                if ok {
                    let captured = input.capture_single(&start);
                    write_capture(env, &target, captured)?;
                    *i = j;
                    return Ok(true);
                } else {
                    return Ok(false);
                }
            }
            // M46: `ahead rule` — lookahead; succeed/fail without advancing.
            "ahead" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                let saved = input.save();
                let inner_start = *i;
                let mut j = *i;
                let ok = rule_one(input, rules, &mut j, env, case_sensitive, collect_stack)?;
                input.restore(&saved);
                *i = inner_start + rule_extent(rules, inner_start);
                return Ok(ok);
            }
            // M46: `behind rule` — reverse lookahead. Best-effort: snapshot
            // cursor, step back one element, run rule, restore. Matches the
            // common "did the prior char match X" use case.
            "behind" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                let saved = input.save();
                // Step back one element/char.
                match &mut *input {
                    Input::Str { src, cursor } => {
                        if *cursor == 0 {
                            *i += rule_extent(rules, *i);
                            return Ok(false);
                        }
                        // Find previous char boundary.
                        let prev = src[..*cursor]
                            .char_indices()
                            .last()
                            .map(|(idx, _)| idx)
                            .unwrap_or(0);
                        *cursor = prev;
                    }
                    Input::Series { series, .. } => {
                        if series.index == 0 {
                            *i += rule_extent(rules, *i);
                            return Ok(false);
                        }
                        series.index -= 1;
                    }
                }
                let inner_start = *i;
                let mut j = *i;
                let ok = rule_one(input, rules, &mut j, env, case_sensitive, collect_stack)?;
                input.restore(&saved);
                *i = inner_start + rule_extent(rules, inner_start);
                return Ok(ok);
            }
            // M46: `not rule` — negation; succeed iff sub-rule fails. Cursor
            // is always restored (no advance on either outcome). The rule
            // cursor advances past the inner rule's syntactic extent.
            "not" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                let saved = input.save();
                let inner_start = *i;
                let mut j = *i;
                let ok = rule_one(input, rules, &mut j, env, case_sensitive, collect_stack)?;
                input.restore(&saved);
                // Advance past the inner rule's extent regardless of
                // success/failure (the inner rule was consumed syntactically).
                *i = inner_start + rule_extent(rules, inner_start);
                return Ok(!ok);
            }
            // M46: `if (expr)` — evaluate the paren expr and succeed iff
            // truthy. No cursor advance. The operand is a paren — if a
            // bare word/value is given, fall through (no match).
            "if" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                if let Value::Paren { .. } = &rules[*i] {
                    let result = eval(&rules[*i], env)?;
                    *i += 1;
                    return Ok(!matches!(result, Value::None | Value::Logic(false)));
                }
                // Non-paren operand: treat as literal match (no-op).
                return Ok(false);
            }
            // M46: `accept value` — short-circuit: the parse succeeds with
            // `value` as the overall result. We use `EvalError::Throw` with
            // a tagged value so `parse_native` can distinguish it from a
            // user `throw`. For simplicity we just succeed the current rule
            // and let the surrounding logic continue; the value is captured
            // into the active collect target if any.
            "accept" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(true);
                }
                let val = rules[*i].clone();
                *i += 1;
                if let Some(top) = collect_stack.last_mut() {
                    top.push(val);
                }
                return Ok(true);
            }
            // M46: `collect 'word rule` — accumulate matched values into a
            // block bound to `word`. `collect into 'word rule` appends to an
            // existing block. Each iteration of the inner rule that advances
            // the cursor pushes the captured value (substring/element) into
            // the block; `keep value` also pushes explicit values.
            "collect" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                // Optional `into` keyword.
                let mut append_mode = false;
                let mut target_idx = *i;
                if is_word(rules[*i].clone(), "into") {
                    *i += 1;
                    append_mode = true;
                    target_idx = *i;
                    if *i >= rules.len() {
                        return Ok(false);
                    }
                }
                let target = rules[target_idx].clone();
                if *i >= rules.len() {
                    return Ok(false);
                }
                *i = target_idx + 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                // If append mode, read existing block from target word.
                let mut initial: Vec<Value> = Vec::new();
                if append_mode {
                    if let Some(Value::Block { series, .. }) = read_word_value(env, &target)? {
                        initial = series.data.borrow().clone();
                    }
                }
                collect_stack.push(initial);
                let start = input.save();
                let mut j = *i;
                let ok = rule_one(input, rules, &mut j, env, case_sensitive, collect_stack)?;
                let collected = collect_stack.pop().unwrap();
                if ok {
                    let block = Value::block(Series::new(collected));
                    write_capture(env, &target, block)?;
                    *i = j;
                    return Ok(true);
                } else {
                    input.restore(&start);
                    return Ok(false);
                }
            }
            // M46: `keep value` — push value into the current collect target.
            // `keep 'word` reads the word's value. `keep (expr)` evaluates.
            "keep" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                let operand = rules[*i].clone();
                *i += 1;
                let value = if let Value::Paren { .. } = &operand {
                    eval(&operand, env)?
                } else if word_sym(&operand).is_some() {
                    read_word_value(env, &operand)?.unwrap_or(Value::None)
                } else {
                    operand
                };
                if let Some(top) = collect_stack.last_mut() {
                    top.push(value);
                    return Ok(true);
                }
                // No active collect target — silently succeed.
                return Ok(true);
            }
            // M46: `match value` — like literal match but also pushes the
            // matched value into the collect target.
            "match" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                let operand = rules[*i].clone();
                *i += 1;
                let saved = input.save();
                if input.match_literal(&operand, case_sensitive) {
                    // Push the matched value to the collect target.
                    if let Some(top) = collect_stack.last_mut() {
                        top.push(capture_match_value(input, &saved));
                    }
                    return Ok(true);
                } else {
                    input.restore(&saved);
                    return Ok(false);
                }
            }
            // M46: `into 'word rule` — parse a sub-series. For block input,
            // the current element (a block) becomes the input for `rule`.
            "into" => {
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                let target = rules[*i].clone();
                *i += 1;
                if *i >= rules.len() {
                    return Ok(false);
                }
                // Extract a sub-input from the current element if it's a
                // block/string.
                let sub_value = match &*input {
                    Input::Series { series, .. } => {
                        let data = series.data.borrow();
                        if series.index >= data.len() {
                            return Ok(false);
                        }
                        data[series.index].clone()
                    }
                    Input::Str { .. } => {
                        // String-into: capture the rest as a sub-string.
                        // (Red's `into` on strings is rare; we support
                        // block-into primarily.)
                        return Ok(false);
                    }
                };
                let sub_input = match &sub_value {
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
                    Value::String { s, .. } => Input::Str {
                        src: Rc::clone(s),
                        cursor: 0,
                    },
                    _ => return Ok(false),
                };
                // Run the inner rule against the sub-input.
                let mut sub = sub_input;
                let inner_rule = rules[*i].clone();
                let inner_data = match &inner_rule {
                    Value::Block { series, .. } => series.data.borrow().clone(),
                    _ => {
                        // Single-rule form: wrap in a one-element slice.
                        vec![inner_rule.clone()]
                    }
                };
                let mut j = 0;
                let ok = rule_seq(
                    &mut sub,
                    &inner_data,
                    &mut j,
                    env,
                    case_sensitive,
                    collect_stack,
                )?;
                *i += 1;
                if ok {
                    // Capture the sub-input as the result.
                    let captured = sub.capture(&Cursor::default_for(&sub));
                    write_capture(env, &target, captured)?;
                    // Advance the outer cursor past the consumed element.
                    input.skip_one();
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
        let ok = rule_seq(input, &child, &mut j, env, case_sensitive, collect_stack)?;
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

    // M46: resolve a bound `Word` to its value (e.g. a bitset variable).
    // Keywords were handled above; a word here is either a literal match
    // (block input) or a reference to a rule value (bitset/string). Resolve
    // bound words to their stored value and dispatch on the resolved type.
    let v_resolved = if let Some(sym) = word_sym(&v) {
        if let Some(idx) = env.user_ctx.index_of(sym) {
            let val = env.user_ctx.slot_value_unchecked(idx);
            // If the resolved value is a bitset, use it directly.
            if matches!(val, Value::Bitset(_)) {
                Some(val)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // M46: bitset! rule — charset match against current input char/element.
    // The bitset may be a literal `Value::Bitset` or a word resolved to one.
    let bitset_ref: Option<std::rc::Rc<std::cell::RefCell<red_core::value::BitsetDef>>> =
        if let Value::Bitset(b) = &v {
            Some(Rc::clone(b))
        } else if let Some(Value::Bitset(b)) = &v_resolved {
            Some(Rc::clone(b))
        } else {
            None
        };
    if let Some(b) = bitset_ref {
        let saved = input.save();
        if input.match_bitset(&b) {
            *i += 1;
            // Push the matched char to the active collect target (if any).
            if let Some(top) = collect_stack.last_mut() {
                top.push(capture_match_value(input, &saved));
            }
            return Ok(true);
        } else {
            input.restore(&saved);
            return Ok(false);
        }
    }

    // Literal value — match against input.
    let saved = input.save();
    if input.match_literal(&v, case_sensitive) {
        *i += 1;
        // Push the matched value to the active collect target (if any).
        if let Some(top) = collect_stack.last_mut() {
            top.push(capture_match_value(input, &saved));
        }
        Ok(true)
    } else {
        input.restore(&saved);
        Ok(false)
    }
}

/// Capture the value matched by a single-element rule (used by `collect`
/// to push per-match). For string input, a single-char capture returns a
/// `char!`; a multi-char capture returns a `string!`. For block input, the
/// element at the start index is returned.
fn capture_match_value(input: &Input, start: &Cursor) -> Value {
    match (input, start) {
        (Input::Str { src, cursor }, Cursor::Str { cursor: s }) => {
            let slice = &src.as_ref()[*s..*cursor];
            if let Some(c) = slice.chars().next() {
                if c.len_utf8() == slice.len() {
                    return Value::Char {
                        c,
                        span: Span::default(),
                    };
                }
            }
            Value::string(slice)
        }
        (Input::Series { series, .. }, Cursor::Series { index: s }) => {
            let data = series.data.borrow();
            if *s < data.len() {
                data[*s].clone()
            } else {
                Value::None
            }
        }
        _ => Value::None,
    }
}

/// Read a word's value from the user context (returns `None` if unbound).
fn read_word_value(env: &Env, target: &Value) -> Result<Option<Value>, EvalError> {
    let sym = match target {
        Value::LitWord { sym, .. } | Value::Word { sym, .. } => sym.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "word!",
                found: type_name(other),
                span: other.span_or_default(),
            })
        }
    };
    Ok(env
        .user_ctx
        .index_of(&sym)
        .map(|idx| env.user_ctx.slot_value_unchecked(idx)))
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
    case_sensitive: bool,
    collect_stack: &mut Vec<Vec<Value>>,
) -> Result<bool, EvalError> {
    let mut count = 0usize;
    loop {
        if input.at_end() && kind != "opt" {
            break;
        }
        let saved = input.save();
        let mut j = inner_start;
        let ok = rule_one(input, rules, &mut j, env, case_sensitive, collect_stack)?;
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

    /// A zero/initial cursor for the given input (used by `into`).
    fn default_for(input: &Input) -> Cursor {
        match input {
            Input::Str { .. } => Cursor::Str { cursor: 0 },
            Input::Series { .. } => Cursor::Series { index: 0 },
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
        None => return 1, // literal / `[...]` / `(...)` / bitset — one slot
    };
    match sym {
        "skip" | "end" | "none" | "fail" | "break" | "reject" | "??" => 1,
        "to" | "thru" => {
            // `to X` / `thru X` — X is one slot (a value or `end`).
            if i + 1 < rules.len() {
                2
            } else {
                1
            }
        }
        "any" | "some" | "opt" | "while" | "ahead" | "behind" | "not" | "if" => {
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
        "collect" => {
            // `collect W R` or `collect into W R` — W is one slot, R is one
            // rule. The `into` keyword, if present, adds one slot.
            let mut slots = 1; // the keyword itself
            let mut j = i + 1;
            if j < rules.len() && is_word(rules[j].clone(), "into") {
                slots += 1;
                j += 1;
            }
            // word operand
            if j < rules.len() {
                slots += 1;
                j += 1;
            }
            // inner rule
            slots
                + if j < rules.len() {
                    rule_extent(rules, j)
                } else {
                    0
                }
        }
        "keep" | "match" | "accept" => {
            // `keep V` / `match V` / `accept V` — V is one slot.
            if i + 1 < rules.len() {
                2
            } else {
                1
            }
        }
        "into" => {
            // `into W R` — W is one slot, R is one rule.
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

    // --- M46 new rule tests ---

    #[test]
    fn parse_collect_skips() {
        // `collect w some [skip]` accumulates matched values (each skip
        // pushes the char). For string input, single-char captures produce
        // char! values (Red parity).
        let v = val(r#"parse "abc" [collect w some [skip]] w"#);
        assert_eq!(m(&v), "[#\"a\" #\"b\" #\"c\"]");
    }

    #[test]
    fn parse_collect_match_alts() {
        // `collect w some [match #"a" | match #"b" | skip]` — `match`
        // pushes the matched char; `skip` also pushes the consumed char.
        // On "a1b2": match a → #"a", skip 1 → #"1", match b → #"b", skip 2 → #"2".
        let v = val(r#"parse "a1b2" [collect w some [match #"a" | match #"b" | skip]] w"#);
        assert_eq!(m(&v), "[#\"a\" #\"1\" #\"b\" #\"2\"]");
    }

    #[test]
    fn parse_case_sensitive_default() {
        // Default is case-insensitive (Red parity): "A" matches "a".
        assert_eq!(m(&val(r#"parse "abc" ["A" "b" "c"]"#)), "true");
    }

    #[test]
    fn parse_case_refinement() {
        // `/case` enables case-sensitive matching.
        assert_eq!(m(&val(r#"parse/case "Abc" ["A" "b" "c"]"#)), "true");
        assert_eq!(m(&val(r#"parse/case "abc" ["A" "b" "c"]"#)), "false");
    }

    #[test]
    fn parse_bitset_rule() {
        // A bitset! rule matches any char in the set. The bitset must be
        // constructed before the parse (via `charset`/`make bitset!`) and
        // referenced by word in the rule block.
        let v = val(r#"letters: charset "xyz" parse "xzz" [letters letters "z"]"#);
        assert_eq!(m(&v), "true");
    }

    #[test]
    fn parse_bitset_some() {
        // `some` over a charset consumes matching chars.
        let v2 = val(r#"cs: charset "abc" parse "aaa" [some cs]"#);
        assert_eq!(m(&v2), "true");
        let v3 = val(r#"cs: charset "abc" parse "aaz" [some cs]"#);
        assert_eq!(m(&v3), "false");
    }

    #[test]
    fn parse_ahead_lookahead() {
        // `ahead "a"` succeeds without advancing; the next rule must still
        // match at the same position.
        assert_eq!(m(&val(r#"parse "abc" [ahead "a" "a" "b" "c"]"#)), "true");
        // `ahead` doesn't consume — `"b"` fails on `"a"`.
        assert_eq!(m(&val(r#"parse "abc" [ahead "a" "b" "c"]"#)), "false");
    }

    #[test]
    fn parse_not_rule() {
        // `not "z"` succeeds iff "z" doesn't match here.
        assert_eq!(m(&val(r#"parse "abc" [not "z" "a" "b" "c"]"#)), "true");
        assert_eq!(m(&val(r#"parse "zbc" [not "z" "a" "b" "c"]"#)), "false");
    }

    #[test]
    fn parse_fail_rule() {
        // `fail` always fails the current alternative.
        assert_eq!(m(&val(r#"parse "abc" [fail]"#)), "false");
        // `fail` in an alternative forces fallback to the next alt.
        assert_eq!(m(&val(r#"parse "abc" [fail | "a" "b" "c"]"#)), "true");
    }

    #[test]
    fn parse_if_rule() {
        assert_eq!(m(&val(r#"parse "abc" [if (1 < 2) "a" "b" "c"]"#)), "true");
        assert_eq!(m(&val(r#"parse "abc" [if (1 > 2) "a" "b" "c"]"#)), "false");
    }

    #[test]
    fn parse_break_rule() {
        // `break` exits the parse immediately with success.
        assert_eq!(m(&val(r#"parse "abc" ["a" break "z"]"#)), "true");
    }

    #[test]
    fn parse_keep_value() {
        // `collect w [keep "x" keep "y"]` — explicit keep pushes.
        let v = val(r#"parse "" [collect w [keep "x" keep "y"]] w"#);
        assert_eq!(m(&v), "[\"x\" \"y\"]");
    }

    #[test]
    fn parse_keep_paren() {
        // `keep (expr)` evaluates and pushes.
        let v = val(r#"parse "" [collect w [keep (1 + 2)]] w"#);
        assert_eq!(m(&v), "[3]");
    }

    #[test]
    fn parse_char_match_string() {
        // M46: char! literals now match a single char in string input.
        assert_eq!(m(&val(r#"parse "abc" [#"a" #"b" #"c"]"#)), "true");
        assert_eq!(m(&val(r#"parse "abc" [#"a" #"z"]"#)), "false");
    }

    #[test]
    fn parse_char_match_case() {
        // Case-insensitive by default; /case sensitive.
        assert_eq!(m(&val(r#"parse "ABC" [#"a" #"b" #"c"]"#)), "true");
        assert_eq!(m(&val(r#"parse/case "ABC" [#"a" #"b" #"c"]"#)), "false");
    }
}
