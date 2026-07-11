//! Native (Rust-implemented) operations.
//!
//! This module groups the native function implementations by concern:
//!   - [`io`] — `print`, `prin`, `probe`.
//!   - [`compare`] — `= <> < > <= >=` and `and`/`or`/`not`, plus the shared
//!     `values_equal` used by series equality.
//!   - [`control`] — `if`/`either`/loops/`break`/`continue`/`switch`/`case`/
//!     `default`/`all`/`any`/`try`/`attempt`/`catch`/`throw`/`cause-error`/
//!     `comment`/`exit`/`quit`.
//!   - [`func`] — `func`/`does`/`function`/`function?`/`return` + the shared
//!     `extract_spec`/`FuncSpec` used by the VM compiler.
//!   - [`eval`] — `do`/`load`/`reduce`.
//!   - [`words`] — `get`/`set`/`value?`/`use`/`bind`.
//!   - [`registry`] — the `register_natives`/`install_constants` entry
//!     points that wire every native group into `env.natives`.
//!
//! Arithmetic (`+ - * /` and prefix aliases) lives in `crate::math`;
//! series, parse, strings, math, object, path, and I/O file/shell natives
//! live in their own top-level modules (`series.rs`, `parse.rs`, …) and are
//! registered from `registry::register_natives`.
//!
//! Shared helpers used across the per-concern sub-modules (`truthy`,
//! `arity_err`, `type_name`, `expect_block`) live in this file and are
//! `pub(crate)` so the rest of the crate (`math.rs`, `parse.rs`, `vm::*`,
//! …) can keep importing them as `crate::natives::{truthy, …}`.

use std::rc::Rc;

use red_core::value::{Symbol, Value};
use red_core::EvalError;

mod compare;
mod control;
mod eval;
mod func;
mod io;
mod registry;
mod test;
mod words;

pub(crate) use compare::{num_cmp, values_equal};
pub(crate) use control::parse_error_block_public;
pub(crate) use func::{extract_spec, func_native, FuncSpec};
pub(crate) use registry::reg_refined;
pub use registry::{install_constants, register_natives};
pub(crate) use test::register_test_natives;
pub(crate) use test::run_tests_native;
pub(crate) use words::value_predicate;
// ---------------------------------------------------------------------------
// M42: structured error enrichment
// ---------------------------------------------------------------------------

/// Wrap a catchable `EvalError` into an `EvalError::Raised` carrying a
/// structured `ErrorValue`. Used by the VM `Call` arm and the walker's
/// native-call path so that `try`/`catch` see a uniform `Raised` payload.
///
/// - `EvalError::Raised` passes through unchanged (already structured — may
///   carry richer fields from `cause-error` or math-specific classification).
/// - `EvalError::Native { message, span }` → synthesized with
///   `type: 'script`, `where: native_name`, `near: span-block`.
/// - `EvalError::TypeError`/`Arity`/`UnboundWord`/`Compile` → synthesized
///   with `type: 'script`, `where: native_name`, `near: span-block`, and
///   the rendered message body.
/// - Control-flow unwinds (`Return`/`Break`/`Continue`/`Throw`/`Quit`)
///   pass through unchanged.
pub(crate) fn enrich_error(
    e: EvalError,
    native_name: Option<Symbol>,
    span: red_core::value::Span,
) -> EvalError {
    match e {
        // Already structured — pass through.
        raised @ EvalError::Raised(_) => raised,
        // Control flow — pass through.
        flow @ (EvalError::Return(_)
        | EvalError::Break(_)
        | EvalError::Continue
        | EvalError::Throw(_)
        | EvalError::Quit(_)) => flow,
        // Specific error variants — pass through so callers/tests can match
        // on them. `try` synthesizes these into a structured `ErrorValue`
        // with `type: 'script` via its fallback arm.
        specific @ (EvalError::UnboundWord { .. }
        | EvalError::TypeError { .. }
        | EvalError::Arity { .. }
        | EvalError::Compile { .. }
        | EvalError::ParseRecursionLimit { .. }) => specific,
        // Generic `Native` errors — synthesize a structured `ErrorValue` with
        // `type: 'script`, `where: native_name`, `near: span-block`.
        EvalError::Native { message, .. } => {
            use red_core::value::ErrorValue;
            let near = if span.is_default() {
                None
            } else {
                Some(Value::Block {
                    series: red_core::value::Series::new(Vec::new()),
                    span,
                })
            };
            EvalError::Raised(Rc::new(ErrorValue::new_structed(
                message,
                None,
                Some(Symbol::new("script")),
                Vec::new(),
                near,
                native_name,
                None,
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers (used by every sub-module + across the crate)
// ---------------------------------------------------------------------------

/// Truthiness rule: only `false` and `none` are falsy; everything else is
/// truthy.
pub(crate) fn truthy(v: &Value) -> bool {
    !matches!(v, Value::None | Value::Logic(false))
}

/// Build an `Arity` error for `native` with the given expected/got counts.
/// The span falls back to the first argument's source position (if any) so
/// the user gets a `file:line:col:` pointer to the call site even though
/// natives don't receive the calling word's span directly.
pub(crate) fn arity_err(args: &[Value], native: &str, expected: usize, got: usize) -> EvalError {
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

pub(crate) fn type_name(v: &Value) -> &'static str {
    match v {
        Value::None => "none!",
        Value::Unset => "unset!",
        Value::Logic(_) => "logic!",
        Value::Integer { .. } => "integer!",
        Value::Float { .. } => "float!",
        Value::Decimal { .. } => "decimal!",
        Value::Percent { .. } => "percent!",
        Value::Money { .. } => "money!",
        Value::Issue { .. } => "issue!",
        Value::Email { .. } => "email!",
        Value::Tag { .. } => "tag!",
        Value::String { .. } => "string!",
        Value::Char { .. } => "char!",
        Value::Pair { .. } => "pair!",
        Value::Tuple { .. } => "tuple!",
        Value::String8 { .. } => "binary!",
        Value::Word { .. } => "word!",
        Value::SetWord { .. } => "set-word!",
        Value::GetWord { .. } => "get-word!",
        Value::LitWord { .. } => "lit-word!",
        Value::Block { .. } => "block!",
        Value::Paren { .. } => "paren!",
        // M87: `native!`/`op!`/`function!` split. Red distinguishes infix
        // operators (`op!`) from built-ins (`native!`) from user-defined
        // functions (`function!`). We keep the `FuncDef` flags (no `Value`
        // enum split — see plan8 M87 decision) but surface the distinction
        // here so `type?`/`types-of`/error messages report the right word.
        // Order matters: an infix native (e.g. `+`) is an `op!`, NOT a
        // `native!` (Red parity — `op?` and `native?` are disjoint).
        Value::Func(fd) => {
            if fd.infix {
                "op!"
            } else if fd.native.is_some() {
                "native!"
            } else {
                "function!"
            }
        }
        Value::Closure(_) => "closure!",
        Value::Path { .. } => "path!",
        Value::GetPath { .. } => "get-path!",
        Value::LitPath { .. } => "lit-path!",
        Value::SetPath { .. } => "set-path!",
        Value::Refinement { .. } => "refinement!",
        Value::File { .. } => "file!",
        Value::Url { .. } => "url!",
        Value::Error(_) => "error!",
        Value::Object(_) => "object!",
        Value::Module(_) => "module!",
        Value::Map(_) => "map!",
        Value::Hash(_) => "hash!",
        Value::Vector(_) => "vector!",
        Value::Image(_) => "image!",
        Value::Date { .. } => "date!",
        Value::Duration { .. } => "duration!",
        Value::Bitset(_) => "bitset!",
        Value::Port(_) => "port!",
        Value::Typeset(_) => "typeset!",
    }
}

/// Extract a `Block` value from `args[idx]`, or raise a TypeError. The error
/// span is taken from the offending argument (its source position when
/// available).
pub(crate) fn expect_block(args: &[Value], idx: usize, native: &str) -> Result<Value, EvalError> {
    match args.get(idx) {
        Some(v @ Value::Block { .. }) => Ok(v.clone()),
        Some(other) => Err(EvalError::TypeError {
            expected: "block!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
        None => Err(EvalError::Arity {
            native: Symbol::new(native),
            expected: idx + 1,
            got: args.len(),
            // No argument to read a span from; fall back to the calling
            // native's first-arg span if present, else zero.
            span: args
                .first()
                .map(|v| v.span_or_default())
                .unwrap_or_default(),
        }),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::interp::eval;
    use red_core::context::Context;
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use red_core::Env;
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    /// In-memory `Write` sink that records bytes into a shared `Rc<RefCell<Vec<u8>>>`.
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

    /// Run `src` with a fresh env (constants + natives) and capture stdout.
    fn run_capture(src: &str) -> Result<Vec<u8>, String> {
        run_capture_val(src).map(|(_, out)| out)
    }

    fn run_capture_val(src: &str) -> Result<(Value, Vec<u8>), String> {
        use crate::binding::bind_pass;
        let body = load_source(src).map_err(|e| e.to_string())?;
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        let block = Value::block(body);
        // Catch `Quit` (from `exit`/`quit`) as a normal termination so tests
        // can assert on the output captured before the exit. Other errors
        // propagate as strings.
        let val = match eval(&block, &mut env) {
            Ok(v) => v,
            Err(EvalError::Quit(_)) => Value::None,
            Err(e) => return Err(e.to_string()),
        };
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    // --- M6 I/O tests (preserved) ---

    #[test]
    fn print_integer() {
        assert_eq!(s(&run_capture("print 5").unwrap()), "5\n");
    }

    #[test]
    fn prin_concat() {
        // form-based: strings render without quotes, so `prin "a" prin "b"`
        // yields `ab` (no trailing newline — `prin` doesn't add one).
        assert_eq!(s(&run_capture("prin \"a\" prin \"b\"").unwrap()), "ab");
    }

    #[test]
    fn print_block() {
        // `print` of a block forms each element (space-joined, no brackets).
        assert_eq!(s(&run_capture("print [1 2 3]").unwrap()), "1 2 3\n");
    }

    #[test]
    fn print_string_formed() {
        // `print` forms (not molds) — strings render without quotes.
        assert_eq!(
            s(&run_capture("print \"Hello, World!\"").unwrap()),
            "Hello, World!\n"
        );
    }

    #[test]
    fn probe_value() {
        assert_eq!(s(&run_capture("probe 42").unwrap()), "== 42\n");
    }

    #[test]
    fn print_returns_none() {
        let (v, _) = run_capture_val("print 5").unwrap();
        assert_eq!(mold_to_string(&v), "none");
    }

    // --- M7 arithmetic ---

    #[test]
    fn add_integers() {
        assert_eq!(mold_to_string(&val("1 + 2")), "3");
    }

    #[test]
    fn subtract_integers() {
        assert_eq!(mold_to_string(&val("10 - 4")), "6");
    }

    #[test]
    fn multiply_integers() {
        assert_eq!(mold_to_string(&val("3 * 4")), "12");
    }

    #[test]
    fn divide_integers() {
        assert_eq!(mold_to_string(&val("10 / 3")), "3");
    }

    #[test]
    fn division_by_zero_errors() {
        let err = run_capture("10 / 0").unwrap_err();
        assert!(err.contains("division by zero"));
    }

    #[test]
    fn mixed_int_float_promotes_to_float() {
        assert_eq!(mold_to_string(&val("1 + 2.0")), "3.0");
    }

    #[test]
    fn left_to_right_no_precedence() {
        // `1 + 2 * 3` = `(1 + 2) * 3` = 9
        assert_eq!(mold_to_string(&val("1 + 2 * 3")), "9");
    }

    // --- M7 comparison ---

    #[test]
    fn equal_returns_logic() {
        assert_eq!(mold_to_string(&val("3 = 3")), "true");
        assert_eq!(mold_to_string(&val("3 = 4")), "false");
    }

    #[test]
    fn not_equal_returns_logic() {
        assert_eq!(mold_to_string(&val("3 <> 4")), "true");
    }

    #[test]
    fn less_than() {
        assert_eq!(mold_to_string(&val("1 < 2")), "true");
        assert_eq!(mold_to_string(&val("2 < 1")), "false");
    }

    #[test]
    fn greater_than() {
        assert_eq!(mold_to_string(&val("2 > 1")), "true");
    }

    #[test]
    fn less_equal() {
        assert_eq!(mold_to_string(&val("2 <= 2")), "true");
    }

    #[test]
    fn greater_equal() {
        assert_eq!(mold_to_string(&val("3 >= 2")), "true");
    }

    #[test]
    fn one_plus_two_equals_three() {
        // The milestone test: `1 + 2 = 3` evaluates left-to-right to `true`.
        assert_eq!(mold_to_string(&val("1 + 2 = 3")), "true");
    }

    // --- M7 logic ---

    #[test]
    fn and_or_not() {
        assert_eq!(mold_to_string(&val("true and false")), "false");
        assert_eq!(mold_to_string(&val("true or false")), "true");
        assert_eq!(mold_to_string(&val("not true")), "false");
        assert_eq!(mold_to_string(&val("not false")), "true");
    }

    #[test]
    fn none_is_falsy() {
        assert_eq!(mold_to_string(&val("not none")), "true");
    }

    // --- M7 conditionals ---

    #[test]
    fn if_true_evaluates_block() {
        assert_eq!(mold_to_string(&val("if true [42]")), "42");
    }

    #[test]
    fn if_false_returns_none() {
        assert_eq!(mold_to_string(&val("if false [42]")), "none");
    }

    // --- M120: unless ---

    #[test]
    fn unless_false_evaluates_block() {
        assert_eq!(mold_to_string(&val("unless false [1]")), "1");
    }

    #[test]
    fn unless_true_returns_none() {
        assert_eq!(mold_to_string(&val("unless true [1]")), "none");
    }

    #[test]
    fn unless_truthy_condition_prints_nothing() {
        let out = run_capture("unless (1 = 1) [print \"no\"]").unwrap();
        assert_eq!(s(&out), "");
    }

    #[test]
    fn either_true_branch() {
        assert_eq!(mold_to_string(&val("either 1 > 0 [\"y\"][\"n\"]")), "\"y\"");
    }

    #[test]
    fn either_false_branch() {
        assert_eq!(mold_to_string(&val("either 1 < 0 [\"y\"][\"n\"]")), "\"n\"");
    }

    // --- M7 loops ---

    #[test]
    fn repeat_prints_counter() {
        let out = run_capture("repeat i 3 [print i]").unwrap();
        assert_eq!(s(&out), "1\n2\n3\n");
    }

    #[test]
    fn repeat_litword_form() {
        let out = run_capture("repeat 'i 3 [print i]").unwrap();
        assert_eq!(s(&out), "1\n2\n3\n");
    }

    #[test]
    fn until_terminates() {
        // `i: 0 until [i: i + 1 i > 3]` → true, i == 4
        let v = val("i: 0 until [i: i + 1 i > 3]");
        assert_eq!(mold_to_string(&v), "true");
        // Verify i ended at 4.
        assert_eq!(mold_to_string(&val("i: 0 until [i: i + 1 i > 3] i")), "4");
    }

    #[test]
    fn while_terminates() {
        // `a: 0 while [a < 3][a: a + 1]` → terminates; a == 3
        let v = val("a: 0 while [a < 3][a: a + 1]");
        assert_eq!(mold_to_string(&v), "none");
        assert_eq!(mold_to_string(&val("a: 0 while [a < 3][a: a + 1] a")), "3");
    }

    #[test]
    fn loop_with_break() {
        // `i: 0 loop [i: i + 1 if i > 3 [break]] i` → i == 4
        let v = val("i: 0 loop [i: i + 1 if i > 3 [break]] i");
        assert_eq!(mold_to_string(&v), "4");
    }

    #[test]
    fn loop_break_returns_none() {
        assert_eq!(mold_to_string(&val("loop [break]")), "none");
    }

    #[test]
    fn loop_count_form() {
        // `loop count block` — evaluate block `count` times.
        let v = val("i: 0 loop 3 [i: i + 1] i");
        assert_eq!(mold_to_string(&v), "3");
    }

    #[test]
    fn loop_count_zero_is_noop() {
        let v = val("i: 0 loop 0 [i: 99] i");
        assert_eq!(mold_to_string(&v), "0");
    }

    #[test]
    fn loop_count_with_break() {
        let v = val("i: 0 loop 10 [i: i + 1 if i > 2 [break]] i");
        assert_eq!(mold_to_string(&v), "3");
    }

    // --- M121: forever ---

    #[test]
    fn forever_breaks_with_value() {
        let v = val("i: 0 forever [i: i + 1 if i = 5 [break]] i");
        assert_eq!(mold_to_string(&v), "5");
    }

    #[test]
    fn forever_breaks_cleanly_single_iteration() {
        let v = val("forever [break]");
        assert_eq!(mold_to_string(&v), "none");
    }

    // --- M121: for ---

    #[test]
    fn for_ascending_sum() {
        let v = val("total: 0 for i 1 5 1 [total: total + i] total");
        assert_eq!(mold_to_string(&v), "15");
    }

    #[test]
    fn for_descending_prints_reverse() {
        let out = run_capture("for i 5 1 -1 [prin i]").unwrap();
        assert_eq!(s(&out), "54321");
    }

    #[test]
    fn for_single_iteration() {
        let out = run_capture("for i 1 1 1 [prin \"x\"]").unwrap();
        assert_eq!(s(&out), "x");
    }

    #[test]
    fn for_empty_range_runs_nothing() {
        let out = run_capture("for i 1 0 1 [prin \"x\"]").unwrap();
        assert_eq!(s(&out), "");
    }

    #[test]
    fn for_break_exits_cleanly() {
        let v = val("total: 0 for i 1 100 1 [if i > 3 [break] total: total + i] total");
        assert_eq!(mold_to_string(&v), "6");
    }

    #[test]
    fn for_float_step() {
        let out = run_capture("for i 1.0 2.0 0.5 [prin i prin \" \"]").unwrap();
        assert_eq!(s(&out), "1.0 1.5 2.0 ");
    }

    #[test]
    fn for_char_ascending() {
        let out = run_capture("for c #\"a\" #\"c\" 1 [prin c]").unwrap();
        assert_eq!(s(&out), "abc");
    }

    #[test]
    fn continue_skips_rest() {
        // Sum 1..5 skipping 3: i: 0 sum: 0 repeat 5 [if i = 2 [continue] sum: sum + i] sum
        // Actually with continue, the `sum: sum + i` after `continue` won't run.
        // i goes 1..5. When i=2, continue skips the rest. sum = 0+1+3+4+5 = 13.
        // Wait, i=2 is skipped but the repeat counter is the loop var...
        // Let me use a clearer test: repeat 5 [if i = 3 [continue] print i]
        // → prints 1, 2, 4, 5 (skips 3)
        let out = run_capture("repeat i 5 [if i = 3 [continue] print i]").unwrap();
        assert_eq!(s(&out), "1\n2\n4\n5\n");
    }

    // --- M7 eval ---

    #[test]
    fn do_evaluates_block() {
        assert_eq!(mold_to_string(&val("do [1 + 2]")), "3");
    }

    #[test]
    fn reduce_collects_results() {
        assert_eq!(mold_to_string(&val("reduce [1 + 1 2 + 2]")), "[2 4]");
    }

    #[test]
    fn reduce_empty_block() {
        assert_eq!(mold_to_string(&val("reduce []")), "[]");
    }

    // --- M16.1: load + do-with-string ---

    #[test]
    fn load_returns_block_from_string() {
        let v = val("load \"1 + 2\"");
        match v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                assert_eq!(data.len(), 3, "expected 3 values [1 + 2], got {data:?}");
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn do_evaluates_string() {
        assert_eq!(mold_to_string(&val("do \"1 + 2\"")), "3");
    }

    #[test]
    fn do_load_calculator_pattern() {
        // The canonical string→code→eval pattern now works.
        let v = val("calc: function [expr][do load expr] calc \"1 + 2 * 3\"");
        assert_eq!(mold_to_string(&v), "9");
    }

    #[test]
    fn do_string_sets_existing_global() {
        // `do "x: 5"` writes to the pre-allocated `x` slot in the user ctx.
        assert_eq!(mold_to_string(&val("x: 0 do \"x: 5\" x")), "5");
    }

    #[test]
    fn do_string_errors_on_eval_failure() {
        // A syntactically-valid but semantically-broken string propagates
        // the eval error (here: `1 +` is missing its right operand).
        let err = run_capture("do \"1 +\"").unwrap_err();
        assert!(err.contains("expects") || err.contains("argument"));
    }

    #[test]
    fn do_string_errors_on_lex_failure() {
        // A lex error in the string propagates as a native error.
        let err = run_capture("do {\"unterminated}").unwrap_err();
        assert!(err.contains("unterminated") || err.contains("string"));
    }

    #[test]
    fn do_rejects_non_block_non_string() {
        let err = run_capture("do 5").unwrap_err();
        assert!(err.contains("expected") && err.contains("block"));
    }

    // --- M7 truthiness edge cases ---

    #[test]
    fn if_with_integer_condition() {
        // Non-false, non-none values are truthy.
        assert_eq!(mold_to_string(&val("if 5 [42]")), "42");
    }

    #[test]
    fn if_with_zero_is_truthy() {
        // In Red, 0 is truthy (only false and none are falsy).
        assert_eq!(mold_to_string(&val("if 0 [42]")), "42");
    }

    #[test]
    fn if_with_none_is_falsy() {
        assert_eq!(mold_to_string(&val("if none [42]")), "none");
    }

    // --- M13: user-function refinements ---

    #[test]
    fn func_with_only_refinement_callable_with_and_without() {
        // `func [x /only][...]` — callable both ways. The body reads `only`
        // as a logic flag (true when `/only` supplied, false otherwise).
        let src = r#"
            f: func [x /only][
                either only [x * 10][x]
            ]
            print f 5
            print f/only 5
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "5\n50\n");
    }

    #[test]
    fn func_refinement_with_argument() {
        // `func [x /with y][...]` — `/with` takes one arg `y`. The inactive
        // branch must not reference `y` (it's `none` when `/with` is unused).
        let src = r#"
            f: func [x /with y][
                if with [return x + y]
                x
            ]
            print f 5
            print f/with 5 7
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "5\n12\n");
    }

    #[test]
    fn func_refinement_inline_spaced_form() {
        // The spaced form `f 5 /with 7` (refinement as a standalone token
        // after the positional args) also works — spec-order dispatch
        // consumes positional args first, then the refinement flag + its
        // args. (Refinements may not skip required positionals.)
        let src = r#"
            f: func [x /with y][
                if with [return x + y]
                x
            ]
            print f 5 /with 7
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "12\n");
    }

    #[test]
    fn func_refinement_arg_defaults_to_none_when_inactive() {
        // When `/with` isn't supplied, `y` is `none` in the body. The body
        // must guard against using `y` in the inactive path.
        let src = r#"
            f: func [x /with y][
                if with [return y]
                x
            ]
            print f 5
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "5\n");
    }

    #[test]
    fn func_multiple_refinements() {
        // Two refinements, both usable independently and together.
        let src = r#"
            f: func [x /double /add n][
                if double [x: x * 2]
                if add [x: x + n]
                x
            ]
            print f 5
            print f/double 5
            print f/add 5 3
            print f/double/add 5 3
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "5\n10\n8\n13\n");
    }

    // --- M16 control flow expansion ---

    #[test]
    fn switch_matches_value() {
        assert_eq!(
            mold_to_string(&val("switch 2 [1 [\"a\"] 2 [\"b\"]]")),
            "\"b\""
        );
    }

    #[test]
    fn switch_no_match_returns_none() {
        assert_eq!(
            mold_to_string(&val("switch 3 [1 [\"a\"] 2 [\"b\"]]")),
            "none"
        );
    }

    #[test]
    fn switch_default_runs_when_no_match() {
        assert_eq!(
            mold_to_string(&val("switch/default 3 [1 [\"a\"]] [\"d\"]")),
            "\"d\""
        );
    }

    #[test]
    fn switch_case_refinement_accepted() {
        // `/case` is accepted (POC: string equality is already case-sensitive,
        // so the flag is a no-op — but it must parse without error).
        assert_eq!(
            mold_to_string(&val(
                "switch/case \"A\" [\"a\" [\"lower\"] \"A\" [\"upper\"]]"
            )),
            "\"upper\""
        );
    }

    #[test]
    fn case_returns_first_matching_branch() {
        assert_eq!(
            mold_to_string(&val("case [1 > 2 [\"a\"] 2 > 1 [\"b\"]]")),
            "\"b\""
        );
    }

    #[test]
    fn case_no_match_returns_none() {
        assert_eq!(
            mold_to_string(&val("case [1 > 2 [\"a\"] 2 > 3 [\"b\"]]")),
            "none"
        );
    }

    #[test]
    fn case_default_runs_when_no_match() {
        assert_eq!(
            mold_to_string(&val("case/default [1 > 2 [\"a\"]] [\"d\"]")),
            "\"d\""
        );
    }

    #[test]
    fn case_all_evaluates_every_match() {
        // `/all` runs every matching branch; returns the last.
        let out = run_capture("case/all [true [print 1] true [print 2]]").unwrap();
        assert_eq!(s(&out), "1\n2\n");
    }

    #[test]
    fn all_short_circuits_on_false() {
        assert_eq!(mold_to_string(&val("all [true 1 2]")), "2");
        assert_eq!(mold_to_string(&val("all [true false]")), "none");
        // Short-circuit: the failing expression after `false` would error if
        // evaluated.
        assert_eq!(mold_to_string(&val("all [false 1 + \"a\"]")), "none");
    }

    #[test]
    fn any_returns_first_truthy() {
        assert_eq!(mold_to_string(&val("any [false 5 6]")), "5");
        assert_eq!(mold_to_string(&val("any [false false]")), "none");
        // Short-circuit: the expression after the truthy value isn't eval'd.
        assert_eq!(mold_to_string(&val("any [5 1 + \"a\"]")), "5");
    }

    #[test]
    fn default_sets_when_none() {
        // `x: none default 'x 10 x` → x becomes 10.
        assert_eq!(mold_to_string(&val("x: none default 'x 10 x")), "10");
    }

    #[test]
    fn default_keeps_existing_value() {
        // `x: 5 default 'x 10 x` → x stays 5.
        assert_eq!(mold_to_string(&val("x: 5 default 'x 10 x")), "5");
    }

    #[test]
    fn try_returns_error_value_on_failure() {
        // `try [1 + "a"]` catches the type error → an error value (molds as
        // `make error! "..."`).
        let v = val("try [1 + \"a\"]");
        match v {
            Value::Error(ev) => {
                assert!(
                    ev.message.contains("expected") || ev.message.contains("integer"),
                    "unexpected error message: {}",
                    ev.message
                );
            }
            other => panic!("expected Value::Error, got {:?}", other),
        }
    }

    #[test]
    fn try_returns_value_on_success() {
        assert_eq!(mold_to_string(&val("try [1 + 2]")), "3");
    }

    #[test]
    fn attempt_returns_none_on_error() {
        assert_eq!(mold_to_string(&val("attempt [1 + \"a\"]")), "none");
    }

    #[test]
    fn attempt_returns_value_on_success() {
        assert_eq!(mold_to_string(&val("attempt [1 + 2]")), "3");
    }

    #[test]
    fn try_does_not_catch_throw() {
        // `throw` is control-flow; `try` must let it propagate to `catch`.
        let err = run_capture("try [throw 42]").unwrap_err();
        assert!(err.contains("throw"));
    }

    #[test]
    fn catch_catches_throw_value() {
        assert_eq!(mold_to_string(&val("catch [throw 42]")), "42");
    }

    #[test]
    fn catch_returns_block_value_when_no_throw() {
        assert_eq!(mold_to_string(&val("catch [1 + 2]")), "3");
    }

    #[test]
    fn catch_lets_errors_propagate() {
        let err = run_capture("catch [1 + \"a\"]").unwrap_err();
        assert!(err.contains("expected") || err.contains("integer"));
    }

    #[test]
    fn cause_error_raises_native_error() {
        let err = run_capture("cause-error \"bad-thing\"").unwrap_err();
        assert!(err.contains("bad-thing"));
    }

    #[test]
    fn comment_returns_none_and_discards_arg() {
        assert_eq!(mold_to_string(&val("comment [this is ignored] 42")), "42");
        assert_eq!(mold_to_string(&val("comment \"ignored\" 7")), "7");
    }

    #[test]
    fn function_auto_locals_with_local_marker() {
        // `function [x <local> y][y: x + 1 y]` — `y` is declared as a local
        // via `<local>`; the body assigns it. Returns the local's value.
        assert_eq!(
            mold_to_string(&val("f: function [x <local> y][y: x + 1 y] f 5")),
            "6"
        );
    }

    #[test]
    fn function_local_referenced_before_assignment() {
        // Without `<local>`, referencing `y` before assignment would be an
        // unbound-word error. With `<local>`, `y` starts as `none`.
        assert_eq!(
            mold_to_string(&val("f: function [x <local> y][y] f 5")),
            "none"
        );
    }

    #[test]
    fn function_with_params_and_locals() {
        // Combined params + locals + a refinement.
        let src = r#"
            f: function [a b <local> sum][
                sum: a + b
                sum
            ]
            print f 3 4
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "7\n");
    }

    #[test]
    fn function_body_setword_does_not_leak_to_global() {
        // `function [x][local: 5 ...]` — `local` is a function-local word,
        // NOT a global. After the call, `value? 'local` must be false.
        let src = r#"
            f: function [x][local: 5 x + local]
            print f 10
            print value? 'local
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "15\nfalse\n");
    }

    #[test]
    fn func_body_setword_does_not_leak_to_global() {
        // Same isolation applies to `func` (bind_pass skips func bodies).
        let src = r#"
            f: func [x][local: 5 x + local]
            print f 10
            print value? 'local
        "#;
        let out = run_capture(src).unwrap();
        assert_eq!(s(&out), "15\nfalse\n");
    }

    #[test]
    fn exit_halts_script_with_value_preserved() {
        // `exit` stops eval; `print` after exit doesn't run. The captured
        // stdout only has the pre-exit output.
        let out = run_capture("print 1 exit print 2").unwrap();
        assert_eq!(s(&out), "1\n");
    }

    #[test]
    fn quit_alias_works() {
        let out = run_capture("print 1 quit print 2").unwrap();
        assert_eq!(s(&out), "1\n");
    }

    #[test]
    fn exit_with_code_propagates() {
        // The exit-code-aware runner returns the requested code.
        let (val, code) =
            crate::interp::run_source_with_exit("print 1 exit 3").expect("run failed");
        assert_eq!(code, 3);
        assert_eq!(mold_to_string(&val), "none");
    }

    // --- M39 type predicates + type?/types-of ---

    #[test]
    fn integer_predicate_matches() {
        assert_eq!(mold_to_string(&val("integer? 5")), "true");
        assert_eq!(mold_to_string(&val("integer? 5.0")), "false");
    }

    #[test]
    fn float_predicate_matches() {
        assert_eq!(mold_to_string(&val("float? 5.0")), "true");
        assert_eq!(mold_to_string(&val("float? 5")), "false");
    }

    #[test]
    fn number_predicate_matches_int_and_float() {
        assert_eq!(mold_to_string(&val("number? 5")), "true");
        assert_eq!(mold_to_string(&val("number? 5.0")), "true");
        assert_eq!(mold_to_string(&val("number? \"a\"")), "false");
    }

    #[test]
    fn string_predicate_matches() {
        assert_eq!(mold_to_string(&val("string? \"hi\"")), "true");
        assert_eq!(mold_to_string(&val("string? 5")), "false");
    }

    #[test]
    fn logic_predicate_matches() {
        assert_eq!(mold_to_string(&val("logic? true")), "true");
        assert_eq!(mold_to_string(&val("logic? false")), "true");
        assert_eq!(mold_to_string(&val("logic? 5")), "false");
    }

    #[test]
    fn none_predicate_matches() {
        assert_eq!(mold_to_string(&val("none? none")), "true");
        assert_eq!(mold_to_string(&val("none? 0")), "false");
    }

    #[test]
    fn error_predicate_matches() {
        assert_eq!(mold_to_string(&val("error? try [1 + \"a\"]")), "true");
        assert_eq!(mold_to_string(&val("error? 5")), "false");
    }

    #[test]
    fn word_predicates() {
        // `'foo` is a `lit-word!`; its evaluation yields the word `foo`.
        // To get a bare `word!` value, extract `first` of a block of words.
        assert_eq!(mold_to_string(&val("word? first [foo]")), "true");
        assert_eq!(mold_to_string(&val("set-word? first [foo:]")), "true");
        assert_eq!(mold_to_string(&val("get-word? first [:foo]")), "true");
        assert_eq!(mold_to_string(&val("lit-word? 'foo")), "true");
        assert_eq!(mold_to_string(&val("refinement? first [/foo]")), "true");
    }

    #[test]
    fn any_word_predicate_matches_all_word_kinds() {
        assert_eq!(mold_to_string(&val("any-word? first [foo]")), "true");
        assert_eq!(mold_to_string(&val("any-word? first [foo:]")), "true");
        assert_eq!(mold_to_string(&val("any-word? first [:foo]")), "true");
        assert_eq!(mold_to_string(&val("any-word? 'foo")), "true");
        assert_eq!(mold_to_string(&val("any-word? 5")), "false");
    }

    #[test]
    fn any_path_predicate_matches_path_kinds() {
        // `foo/bar` with unbound `foo` resolves via path machinery; the path
        // value itself is what the predicate inspects.
        assert_eq!(mold_to_string(&val("any-path? 'foo/bar")), "true");
        assert_eq!(mold_to_string(&val("any-path? 5")), "false");
    }

    #[test]
    fn any_object_predicate_matches() {
        assert_eq!(mold_to_string(&val("any-object? make object! []")), "true");
        assert_eq!(mold_to_string(&val("any-object? 5")), "false");
    }

    #[test]
    fn type_q_returns_type_word() {
        assert_eq!(mold_to_string(&val("type? 5")), "integer!");
        assert_eq!(mold_to_string(&val("type? 5.0")), "float!");
        assert_eq!(mold_to_string(&val("type? \"hi\"")), "string!");
        assert_eq!(mold_to_string(&val(r#"type? #"a""#)), "char!");
        assert_eq!(mold_to_string(&val("type? true")), "logic!");
        assert_eq!(mold_to_string(&val("type? none")), "none!");
        assert_eq!(mold_to_string(&val("type? first [foo]")), "word!");
        assert_eq!(mold_to_string(&val("type? 'foo")), "lit-word!");
        assert_eq!(mold_to_string(&val("type? [1 2]")), "block!");
    }

    #[test]
    fn types_of_returns_specific_and_umbrella() {
        // Integer matches `integer!` + `number!`.
        assert_eq!(mold_to_string(&val("types-of 5")), "[integer! number!]");
        // Float matches `float!` + `number!`.
        assert_eq!(mold_to_string(&val("types-of 5.0")), "[float! number!]");
        // String matches `string!` + `any-string!` + `series!`.
        assert_eq!(
            mold_to_string(&val("types-of \"hi\"")),
            "[string! any-string! series!]"
        );
        // Word matches `word!` + `any-word!`.
        assert_eq!(
            mold_to_string(&val("types-of first [foo]")),
            "[word! any-word!]"
        );
        // Block matches `block!` + `any-block!` + `series!`.
        assert_eq!(
            mold_to_string(&val("types-of [1 2]")),
            "[block! any-block! series!]"
        );
        // None matches only `none!` (no umbrella).
        assert_eq!(mold_to_string(&val("types-of none")), "[none!]");
    }

    // --- M87 native!/op!/function! split ---

    #[test]
    fn m87_type_of_plus_is_op() {
        // `+` is registered with `infix: true` AND `native: Some(...)`; the
        // infix check wins → `op!` (Red parity: an op is NOT a native).
        assert_eq!(mold_to_string(&val("type? :+")), "op!");
    }

    #[test]
    fn m87_type_of_print_is_native() {
        assert_eq!(mold_to_string(&val("type? :print")), "native!");
    }

    #[test]
    fn m87_type_of_user_func_is_function() {
        assert_eq!(mold_to_string(&val("type? func [x][x]")), "function!");
    }

    #[test]
    fn m87_type_of_closure_is_closure() {
        assert_eq!(mold_to_string(&val("type? closure [] []")), "closure!");
    }

    #[test]
    fn m87_native_predicate() {
        // `print` is a non-infix native → true.
        assert_eq!(mold_to_string(&val("native? :print")), "true");
        // `+` is infix → NOT a native (disjoint from `op?`).
        assert_eq!(mold_to_string(&val("native? :+")), "false");
        // User-defined func → false.
        assert_eq!(mold_to_string(&val("native? func [] []")), "false");
        // Closure → false (use `closure?`/`any-function?`).
        assert_eq!(mold_to_string(&val("native? closure [] []")), "false");
        // Non-function → false.
        assert_eq!(mold_to_string(&val("native? 5")), "false");
    }

    #[test]
    fn m87_op_predicate() {
        assert_eq!(mold_to_string(&val("op? :+")), "true");
        assert_eq!(mold_to_string(&val("op? :print")), "false");
        assert_eq!(mold_to_string(&val("op? func [] []")), "false");
        assert_eq!(mold_to_string(&val("op? 5")), "false");
    }

    #[test]
    fn m87_any_function_predicate() {
        // Umbrella: true on all function-kinds.
        assert_eq!(mold_to_string(&val("any-function? :+")), "true");
        assert_eq!(mold_to_string(&val("any-function? :print")), "true");
        assert_eq!(mold_to_string(&val("any-function? func [] []")), "true");
        assert_eq!(mold_to_string(&val("any-function? closure [] []")), "true");
        // Non-function → false.
        assert_eq!(mold_to_string(&val("any-function? 5")), "false");
    }

    #[test]
    fn m87_function_predicate_unchanged_broad() {
        // `function?` stays the broad umbrella (back-compat): true on
        // native!/op!/function!/closure! alike.
        assert_eq!(mold_to_string(&val("function? :+")), "true");
        assert_eq!(mold_to_string(&val("function? :print")), "true");
        assert_eq!(mold_to_string(&val("function? func [] []")), "true");
        assert_eq!(mold_to_string(&val("function? closure [] []")), "true");
        assert_eq!(mold_to_string(&val("function? 5")), "false");
    }

    #[test]
    fn m87_types_of_func_includes_any_function() {
        // A native's `types-of` includes `native!` (primary) + `any-function!`.
        assert_eq!(
            mold_to_string(&val("types-of :print")),
            "[native! any-function!]"
        );
        // An op's `types-of` includes `op!` (primary) + `any-function!`.
        assert_eq!(mold_to_string(&val("types-of :+")), "[op! any-function!]");
        // A user func: `function!` + `any-function!`.
        assert_eq!(
            mold_to_string(&val("types-of func [] []")),
            "[function! any-function!]"
        );
        // A closure: `closure!` + `any-function!`.
        assert_eq!(
            mold_to_string(&val("types-of closure [] []")),
            "[closure! any-function!]"
        );
    }

    // --- M42 error value tests ---

    #[test]
    fn m42_make_error_string_molds_back() {
        assert_eq!(
            mold_to_string(&val("make error! \"boom\"")),
            "make error! \"boom\""
        );
    }

    #[test]
    fn m42_make_error_structured_molds_all_fields() {
        let out = mold_to_string(&val("make error! [code: 42 type: 'math message: \"x\"]"));
        assert!(out.contains("code: 42"), "got: {out}");
        assert!(out.contains("type: 'math"), "got: {out}");
        assert!(out.contains("message: \"x\""), "got: {out}");
    }

    #[test]
    fn m42_try_div_zero_returns_math_error() {
        let v = val("try [1 / 0]");
        assert!(matches!(v, Value::Error(_)));
        let ev = match v {
            Value::Error(ev) => ev,
            _ => unreachable!(),
        };
        assert_eq!(ev.kind.as_ref().map(|s| s.as_str()), Some("math"));
    }

    #[test]
    fn m42_try_type_error_returns_script_error() {
        let v = val("try [1 + \"a\"]");
        assert!(matches!(v, Value::Error(_)));
        let ev = match v {
            Value::Error(ev) => ev,
            _ => unreachable!(),
        };
        assert_eq!(ev.kind.as_ref().map(|s| s.as_str()), Some("script"));
    }

    #[test]
    fn m42_cause_error_with_type() {
        let v = val("try [cause-error 'user \"boom\"]");
        let ev = match v {
            Value::Error(ev) => ev,
            _ => unreachable!(),
        };
        assert_eq!(ev.kind.as_ref().map(|s| s.as_str()), Some("user"));
        assert_eq!(ev.message, "boom");
    }

    #[test]
    fn m42_error_predicate_true_for_try_error() {
        assert_eq!(mold_to_string(&val("error? try [1 / 0]")), "true");
        assert_eq!(mold_to_string(&val("error? 5")), "false");
    }

    #[test]
    fn m42_error_code_numeric_for_div_zero() {
        let v = val("error-code (try [1 / 0])");
        assert!(matches!(v, Value::Integer { .. }));
    }

    #[test]
    fn m42_structured_equality() {
        // Two structured errors with same fields should be equal?.
        let src = r#"
            a: make error! [code: 42 type: 'math message: "x"]
            b: make error! [code: 42 type: 'math message: "x"]
            a = b
        "#;
        assert_eq!(mold_to_string(&val(src)), "true");
    }

    // --- M44 pair! / tuple! tests ---

    #[test]
    fn m44_pair_predicate() {
        assert_eq!(mold_to_string(&val("pair? 1x2")), "true");
        assert_eq!(mold_to_string(&val("pair? 5")), "false");
    }

    #[test]
    fn m44_tuple_predicate() {
        assert_eq!(mold_to_string(&val("tuple? 1.2.3")), "true");
        assert_eq!(mold_to_string(&val("tuple? 5")), "false");
    }

    #[test]
    fn m44_type_of_pair_tuple() {
        assert_eq!(mold_to_string(&val("type? 1x2")), "pair!");
        assert_eq!(mold_to_string(&val("type? 1.2.3")), "tuple!");
    }

    #[test]
    fn m44_tuple_path_access() {
        assert_eq!(mold_to_string(&val("255.0.0/r")), "255");
        assert_eq!(mold_to_string(&val("255.0.0/g")), "0");
        assert_eq!(mold_to_string(&val("255.0.0/1")), "255");
        assert_eq!(mold_to_string(&val("255.0.0/2")), "0");
    }

    #[test]
    fn m44_pair_path_access() {
        assert_eq!(mold_to_string(&val("100x200/x")), "100");
        assert_eq!(mold_to_string(&val("100x200/y")), "200");
        assert_eq!(mold_to_string(&val("100x200/1")), "100");
        assert_eq!(mold_to_string(&val("100x200/2")), "200");
    }

    #[test]
    fn m44_pair_tuple_equality() {
        assert_eq!(mold_to_string(&val("1x2 = 1x2")), "true");
        assert_eq!(mold_to_string(&val("1x2 <> 2x1")), "true");
        assert_eq!(mold_to_string(&val("255.0.0 = 255.0.0")), "true");
        assert_eq!(mold_to_string(&val("255.0.0 <> 0.255.0")), "true");
    }

    // --- M86 unset! tests ---

    #[test]
    fn m86_unset_predicate() {
        // `unset` constant (installed by `install_constants`) evaluates to
        // `Value::Unset`; `unset?` is true on it, false on `none`/other types.
        assert_eq!(mold_to_string(&val("unset? unset")), "true");
        assert_eq!(mold_to_string(&val("unset? none")), "false");
        assert_eq!(mold_to_string(&val("unset? 5")), "false");
    }

    #[test]
    fn m86_unset_distinct_from_none() {
        assert_eq!(mold_to_string(&val("unset = unset")), "true");
        // M86: `unset = none` is false (distinct from `none!`).
        assert_eq!(mold_to_string(&val("unset = none")), "false");
        assert_eq!(mold_to_string(&val("unset <> none")), "true");
    }

    #[test]
    fn m86_unset_molds_to_empty() {
        // Direct: `mold_to_string(&Value::Unset)` is the empty string. The
        // script-level `mold unset` native returns `Value::string("")` (an
        // empty string value), whose own mold is `""` — so the contract is
        // verified at the printer level here.
        assert_eq!(mold_to_string(&Value::Unset), "");
        // The native `mold unset` returns an empty `string!` value.
        let v = val("mold unset");
        match v {
            Value::String { s, .. } => assert!(s.is_empty(), "expected empty string, got {s:?}"),
            other => panic!("expected string!, got {other:?}"),
        }
    }

    #[test]
    fn m86_unset_prints_nothing() {
        // `print unset` emits just a newline (the `writeln!`).
        assert_eq!(s(&run_capture("print unset").unwrap()), "\n");
    }

    #[test]
    fn m86_unset_type_name() {
        assert_eq!(type_name(&Value::Unset), "unset!");
        assert_eq!(mold_to_string(&val("type? unset")), "unset!");
    }

    #[test]
    fn m86_unset_on_unbound_gate_default_off() {
        // Default: `unset_on_unbound` is off, so a truly-unbound word errors.
        let err = run_capture("xyzzy_unbound_word").err().unwrap_or_else(|| {
            panic!(
                "expected unbound-word error, got: {:?}",
                run_capture("xyzzy_unbound_word")
            )
        });
        assert!(
            err.contains("has no value") || err.contains("xyzzy_unbound_word"),
            "got: {err}"
        );
    }

    #[test]
    fn m86_unset_on_unbound_gate_on_yields_unset() {
        // With `env.unset_on_unbound = true`, a truly-unbound word resolves to
        // `Value::Unset` (walker path — `eval` is the dispatch shim's walker
        // route when no VM is involved for this small block). Both the walker
        // and the VM consult the gate.
        use crate::binding::bind_pass;
        let body = load_source("xyzzy_unbound_word").unwrap();
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        env.unset_on_unbound = true;
        let block = Value::block(body);
        let v = eval(&block, &mut env).expect("expected Unset, got error");
        assert!(
            matches!(v, Value::Unset),
            "expected Value::Unset, got {:?}",
            v
        );
        // `unset?` on the result is true.
        assert_eq!(type_name(&v), "unset!");
    }
}
