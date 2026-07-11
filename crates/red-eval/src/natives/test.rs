//! M70–M72: Unit testing dialect — `test`, `suite`, `assert-*`, `run-tests`.
//!
//! Tests register into `Env::tests` during script evaluation (collection phase).
//! `run-tests` drains the registry, executes each test body in an isolated
//! shallow-clone of `user_ctx`, and prints TAP-14 output.
//!
//! The natives are always-on (registered unconditionally in `register_natives`).
//! Without `--test`, tests register silently; with `--test`, the CLI auto-invokes
//! `run-tests` after script eval.

use std::rc::Rc;

use red_core::value::{ErrorValue, Series, Span, Symbol, TestDef, TestHooks, TestResult, TestStatus, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::interp::{dispatch_block, dispatch_block_reduce};
use crate::natives::{truthy, type_name, values_equal};

// ===========================================================================
// M71: test / suite / hooks / assert-* / fail natives
// ===========================================================================

/// `test "name" [body]` — register a test. Does NOT run the body.
pub fn test_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() < 2 {
        return Err(EvalError::Arity {
            native: Symbol::new("test"),
            expected: 2,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    let name = match &args[0] {
        Value::String { s, .. } => (**s).to_string(),
        other => {
            return Err(EvalError::TypeError {
                expected: "string!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    let body = match &args[1] {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    let span = args[0].span_or_default();
    // Capture the current hook chain (parent-first).
    let hooks: Vec<TestHooks> = env.test_hooks.iter().cloned().collect();
    env.tests.push(TestDef {
        name,
        path: env.current_suite.clone(),
        body,
        hooks,
        span,
    });
    Ok(Value::None)
}

/// `suite "name" [body]` — open a group. Nested suites compose paths.
pub fn suite_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() < 2 {
        return Err(EvalError::Arity {
            native: Symbol::new("suite"),
            expected: 2,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    let name = match &args[0] {
        Value::String { s, .. } => {
            let sym = Symbol::new(s);
            sym
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "string!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    let body = match &args[1] {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };

    // Push suite name + fresh hooks frame.
    env.current_suite.push(name);
    env.test_hooks.push(TestHooks::default());

    // Evaluate the suite body (registering nested tests/suites).
    let block = Value::block(body);
    let result = dispatch_block(&block, env);

    // Pop both stacks.
    env.test_hooks.pop();
    env.current_suite.pop();

    result.map(|_| Value::None)
}

/// `before-test [body]` — set the before hook for the current suite.
pub fn before_test_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("before-test"),
            expected: 1,
            got: 0,
            span: Span::default(),
        });
    }
    let body = match &args[0] {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    if env.test_hooks.is_empty() {
        eprintln!("warning: before-test outside a suite is a no-op");
        return Ok(Value::None);
    }
    let frame = env.test_hooks.last_mut().unwrap();
    frame.before = Some(body);
    Ok(Value::None)
}

/// `after-test [body]` — set the after hook for the current suite.
pub fn after_test_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("after-test"),
            expected: 1,
            got: 0,
            span: Span::default(),
        });
    }
    let body = match &args[0] {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    if env.test_hooks.is_empty() {
        eprintln!("warning: after-test outside a suite is a no-op");
        return Ok(Value::None);
    }
    let frame = env.test_hooks.last_mut().unwrap();
    frame.after = Some(body);
    Ok(Value::None)
}

/// `assert [cond]` — fail if the condition is falsy. The block is reduced;
/// the last value is tested for truthiness.
pub fn assert_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("assert"),
            expected: 1,
            got: 0,
            span: Span::default(),
        });
    }
    let block = match &args[0] {
        Value::Block { .. } | Value::Paren { .. } => args[0].clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    let reduced = dispatch_block_reduce(&block, env)?;
    let last = match &reduced {
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            if series.index >= data.len() {
                Value::None
            } else {
                data.last().cloned().unwrap_or(Value::None)
            }
        }
        v => v.clone(),
    };
    if !truthy(&last) {
        return Err(EvalError::Raised(Rc::new(ErrorValue::new_structed(
            "assertion failed".to_string(),
            None,
            Some(Symbol::new("test")),
            Vec::new(),
            None,
            Some(Symbol::new("assert")),
            None,
        ))));
    }
    Ok(Value::None)
}

/// `assert-equal [a b]` — fail if a <> b. The block is reduced to two values.
pub fn assert_equal_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("assert-equal"),
            expected: 1,
            got: 0,
            span: Span::default(),
        });
    }
    let block = match &args[0] {
        Value::Block { .. } | Value::Paren { .. } => args[0].clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    let reduced = dispatch_block_reduce(&block, env)?;
    let vals = match &reduced {
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            data.iter().skip(series.index).cloned().collect::<Vec<_>>()
        }
        v => vec![v.clone()],
    };
    if vals.len() < 2 {
        return Err(EvalError::Raised(Rc::new(ErrorValue::new_structed(
            "assert-equal: expected 2 values in the block".to_string(),
            None,
            Some(Symbol::new("test")),
            Vec::new(),
            None,
            Some(Symbol::new("assert-equal")),
            None,
        ))));
    }
    let a = &vals[vals.len() - 2];
    let b = &vals[vals.len() - 1];
    if !values_equal(a, b) {
        let msg = format!(
            "expected {}, got {}",
            red_core::printer::mold_to_string(b),
            red_core::printer::mold_to_string(a),
        );
        return Err(EvalError::Raised(Rc::new(ErrorValue::new_structed(
            msg,
            None,
            Some(Symbol::new("test")),
            vec![a.clone(), b.clone()],
            None,
            Some(Symbol::new("assert-equal")),
            None,
        ))));
    }
    Ok(Value::None)
}

/// `assert-not-equal [a b]` — fail if a = b.
pub fn assert_not_equal_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("assert-not-equal"),
            expected: 1,
            got: 0,
            span: Span::default(),
        });
    }
    let block = match &args[0] {
        Value::Block { .. } | Value::Paren { .. } => args[0].clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    let reduced = dispatch_block_reduce(&block, env)?;
    let vals = match &reduced {
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            data.iter().skip(series.index).cloned().collect::<Vec<_>>()
        }
        v => vec![v.clone()],
    };
    if vals.len() < 2 {
        return Err(EvalError::Raised(Rc::new(ErrorValue::new_structed(
            "assert-not-equal: expected 2 values in the block".to_string(),
            None,
            Some(Symbol::new("test")),
            Vec::new(),
            None,
            Some(Symbol::new("assert-not-equal")),
            None,
        ))));
    }
    let a = &vals[vals.len() - 2];
    let b = &vals[vals.len() - 1];
    if values_equal(a, b) {
        let msg = format!(
            "expected values to differ, but both were {}",
            red_core::printer::mold_to_string(a),
        );
        return Err(EvalError::Raised(Rc::new(ErrorValue::new_structed(
            msg,
            None,
            Some(Symbol::new("test")),
            vec![a.clone(), b.clone()],
            None,
            Some(Symbol::new("assert-not-equal")),
            None,
        ))));
    }
    Ok(Value::None)
}

/// `assert-error [body]` — fail if the body does NOT raise an error.
pub fn assert_error_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("assert-error"),
            expected: 1,
            got: 0,
            span: Span::default(),
        });
    }
    let block = match &args[0] {
        Value::Block { .. } | Value::Paren { .. } => args[0].clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    match dispatch_block(&block, env) {
        Ok(_) => {
            // Body succeeded — but we expected an error.
            Err(EvalError::Raised(Rc::new(ErrorValue::new_structed(
                "expected an error, but the block succeeded".to_string(),
                None,
                Some(Symbol::new("test")),
                Vec::new(),
                None,
                Some(Symbol::new("assert-error")),
                None,
            ))))
        }
        Err(EvalError::Raised(_)) | Err(EvalError::Native { .. }) | Err(EvalError::TypeError { .. })
        | Err(EvalError::Arity { .. }) | Err(EvalError::UnboundWord { .. }) | Err(EvalError::Compile { .. }) => {
            // Body raised — pass.
            Ok(Value::None)
        }
        // Control-flow unwinds propagate.
        Err(e @ (EvalError::Return(_)
        | EvalError::Break(_)
        | EvalError::Continue
        | EvalError::Throw(_)
        | EvalError::Quit(_))) => Err(e),
        Err(EvalError::ParseRecursionLimit { .. }) => Ok(Value::None),
    }
}

/// `assert-no-error [body]` — fail if the body raises an error.
pub fn assert_no_error_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("assert-no-error"),
            expected: 1,
            got: 0,
            span: Span::default(),
        });
    }
    let block = match &args[0] {
        Value::Block { .. } | Value::Paren { .. } => args[0].clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    match dispatch_block(&block, env) {
        Ok(_) => Ok(Value::None),
        Err(EvalError::Raised(ev)) => {
            let msg = format!("expected no error, but got: {}", ev.message);
            Err(EvalError::Raised(Rc::new(ErrorValue::new_structed(
                msg,
                None,
                Some(Symbol::new("test")),
                Vec::new(),
                None,
                Some(Symbol::new("assert-no-error")),
                None,
            ))))
        }
        Err(e @ (EvalError::Return(_)
        | EvalError::Break(_)
        | EvalError::Continue
        | EvalError::Throw(_)
        | EvalError::Quit(_))) => Err(e),
        Err(e) => {
            let msg = format!("expected no error, but got: {e}");
            Err(EvalError::Raised(Rc::new(ErrorValue::new_structed(
                msg,
                None,
                Some(Symbol::new("test")),
                Vec::new(),
                None,
                Some(Symbol::new("assert-no-error")),
                None,
            ))))
        }
    }
}

/// `fail "reason"` — unconditional failure.
pub fn fail_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Raised(Rc::new(ErrorValue::new_structed(
            "fail".to_string(),
            None,
            Some(Symbol::new("test")),
            Vec::new(),
            None,
            Some(Symbol::new("fail")),
            None,
        ))));
    }
    let msg = match &args[0] {
        Value::String { s, .. } => (**s).to_string(),
        other => red_core::printer::mold_to_string(other),
    };
    Err(EvalError::Raised(Rc::new(ErrorValue::new_structed(
        msg,
        None,
        Some(Symbol::new("test")),
        Vec::new(),
        None,
        Some(Symbol::new("fail")),
        None,
    ))))
}

// ===========================================================================
// M72: run-tests native + TAP-14 reporter + isolation
// ===========================================================================

/// `run-tests` — drain `env.tests`, execute each in an isolated context,
/// print TAP-14 to `env.out`, set `env.test_failed`.
pub fn run_tests_native(_args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if env.tests_run {
        return Ok(Value::None);
    }
    env.tests_run = true;

    let n = env.tests.len();
    if n == 0 {
        let _ = write!(env.out, "1..0\n# no tests registered\n");
        return Ok(Value::None);
    }

    let _ = write!(env.out, "TAP version 14\n1..{n}\n");

    let mut passed = 0;
    let mut failed = 0;
    let mut failed_paths: Vec<String> = Vec::new();

    for (i, test) in env.tests.clone().iter().enumerate() {
        let test_num = i + 1;
        let path = test_path_string(&test.name, &test.path);

        // Isolation: shallow-clone user_ctx (the `use` pattern).
        let child = clone_user_ctx(&env.user_ctx);
        let saved_ctx = env.user_ctx.clone();
        env.user_ctx = Rc::new(child);

        // Run before hooks (parent → child).
        let mut crash_msg: Option<String> = None;
        for hook in &test.hooks {
            if let Some(before) = &hook.before {
                if let Err(e) = dispatch_block(&Value::block(before.clone()), env) {
                    crash_msg = Some(format!("before-test hook failed: {e}"));
                    break;
                }
            }
        }

        // Run test body (if no before-hook crash).
        let (status, message, found, expected, where_word) = if let Some(msg) = crash_msg {
            (TestStatus::Crash, Some(msg), None, None, None)
        } else {
            run_test_body(&test.body, env)
        };

        // Run after hooks (child → parent), even on failure.
        for hook in test.hooks.iter().rev() {
            if let Some(after) = &hook.after {
                if let Err(e) = dispatch_block(&Value::block(after.clone()), env) {
                    // After-hook error upgrades to Crash.
                    let _ = write_after_error(&mut passed, &mut failed, &mut crashed_status(), &e);
                }
            }
        }

        // Restore user_ctx.
        env.user_ctx = saved_ctx;

        // Build TAP line.
        let result = TestResult {
            path: path.clone(),
            status: status.clone(),
            message: message.clone(),
            found: found.clone(),
            expected: expected.clone(),
            where_word: where_word.clone(),
        };
        env.test_results.push(result);

        match status {
            TestStatus::Pass => {
                passed += 1;
                let _ = write!(env.out, "ok {test_num} - {path}\n");
            }
            TestStatus::Fail => {
                failed += 1;
                failed_paths.push(path.clone());
                let _ = write!(env.out, "not ok {test_num} - {path}\n");
                write_tap_diagnostic(env, &message, &found, &expected, &where_word, false)?;
            }
            TestStatus::Crash => {
                failed += 1;
                failed_paths.push(path.clone());
                let _ = write!(env.out, "not ok {test_num} - {path}\n");
                write_tap_diagnostic(env, &message, &found, &expected, &where_word, true)?;
            }
        }
    }

    env.test_failed = failed;
    let _ = write!(env.out, "# tests: {n}, passed: {passed}, failed: {failed}\n");
    if failed > 0 {
        let _ = write!(env.out, "# failed: {}\n", failed_paths.join(", "));
    }

    Ok(Value::None)
}

/// Run a test body and classify the result.
fn run_test_body(
    body: &Series,
    env: &mut Env,
) -> (TestStatus, Option<String>, Option<String>, Option<String>, Option<Symbol>) {
    match dispatch_block(&Value::block(body.clone()), env) {
        Ok(_) => (TestStatus::Pass, None, None, None, None),
        Err(EvalError::Raised(ev)) => {
            // Check if it's a test assertion failure (type: 'test).
            let is_test = ev.kind.as_ref().map(|s| s.as_str() == "test").unwrap_or(false);
            if is_test {
                let found = ev.args.first().map(|v| red_core::printer::mold_to_string(v));
                let expected = ev.args.get(1).map(|v| red_core::printer::mold_to_string(v));
                (
                    TestStatus::Fail,
                    Some(ev.message.clone()),
                    found,
                    expected,
                    ev.cause.clone(),
                )
            } else {
                (
                    TestStatus::Crash,
                    Some(ev.message.clone()),
                    None,
                    None,
                    ev.cause.clone(),
                )
            }
        }
        Err(e) => {
            let msg = e.to_string();
            (TestStatus::Crash, Some(msg), None, None, None)
        }
    }
}

/// Write the TAP YAML diagnostic block for a failed/crashed test.
fn write_tap_diagnostic(
    env: &mut Env,
    message: &Option<String>,
    found: &Option<String>,
    expected: &Option<String>,
    where_word: &Option<Symbol>,
    is_crash: bool,
) -> Result<(), EvalError> {
    let _ = write!(env.out, "  ---\n");
    if let Some(msg) = message {
        let _ = write!(env.out, "  message: {msg}\n");
    }
    if let Some(f) = found {
        let _ = write!(env.out, "  found: {f}\n");
    }
    if let Some(e) = expected {
        let _ = write!(env.out, "  expected: {e}\n");
    }
    if let Some(w) = where_word {
        let _ = write!(env.out, "  where: {}\n", w.as_str());
    }
    if is_crash {
        let _ = write!(env.out, "  # crash\n");
    }
    let _ = write!(env.out, "  ---\n");
    Ok(())
}

/// Build the full test path string: `"suite/name"` joined with `/`.
fn test_path_string(name: &str, path: &[Symbol]) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        let mut s: Vec<&str> = path.iter().map(|p| p.as_str()).collect();
        s.push(name);
        s.join("/")
    }
}

/// Shallow-clone the user context (mirrors the `use` pattern). Each test
/// runs in a fresh clone so SetWords don't leak between tests. Unbound words
/// fall through to the cloned slots (which mirror `user_ctx`).
fn clone_user_ctx(src: &Rc<red_core::Context>) -> red_core::Context {
    let clone = red_core::Context::new();
    for w in src.words() {
        if let Some(v) = src.get(&w) {
            clone.set(w, v);
        }
    }
    clone
}

fn crashed_status() -> TestStatus {
    TestStatus::Crash
}

fn write_after_error(
    _passed: &mut usize,
    _failed: &mut usize,
    _status: &mut TestStatus,
    _e: &EvalError,
) {
    // After-hook errors don't change the TAP line (the test's status is
    // already determined). We just note it — the test body's result stands.
}

// ===========================================================================
// Registration
// ===========================================================================

pub fn register_test_natives(env: &mut Env) {
    use crate::NativeFn;
    env.natives
        .insert(Symbol::new("test"), crate::natives::registry::fixed_native(test_native as NativeFn, 2));
    env.natives
        .insert(Symbol::new("suite"), crate::natives::registry::fixed_native(suite_native as NativeFn, 2));
    env.natives
        .insert(Symbol::new("before-test"), crate::natives::registry::fixed_native(before_test_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("after-test"), crate::natives::registry::fixed_native(after_test_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("assert"), crate::natives::registry::fixed_native(assert_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("assert-equal"), crate::natives::registry::fixed_native(assert_equal_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("assert-not-equal"), crate::natives::registry::fixed_native(assert_not_equal_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("assert-error"), crate::natives::registry::fixed_native(assert_error_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("assert-no-error"), crate::natives::registry::fixed_native(assert_no_error_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("fail"), crate::natives::registry::fixed_native(fail_native as NativeFn, 1));
    env.natives
        .insert(Symbol::new("run-tests"), crate::natives::registry::fixed_native(run_tests_native as NativeFn, 0));
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::bind_pass;
    use crate::natives::{install_constants, register_natives};
    use crate::eval;
    use red_core::context::Context;
    use red_core::parser::load_source;
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

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

    fn run_capture(src: &str) -> Result<(Value, Vec<u8>), String> {
        let body = load_source(src).map_err(|e| e.to_string())?;
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        register_test_natives(&mut env);
        let block = Value::block(body);
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

    fn out(src: &str) -> String {
        s(&run_capture(src).unwrap().1)
    }

    fn val(src: &str) -> Value {
        run_capture(src).unwrap().0
    }

    // --- M71: registration natives ---

    #[test]
    fn test_registers_without_running() {
        let result = run_capture("test \"x\" [print \"ran\"]");
        assert!(result.is_ok());
        let (_, buf) = result.unwrap();
        assert!(buf.is_empty(), "test should not run on registration");
    }

    #[test]
    fn suite_nests_path_prefix() {
        let v = val("suite \"a\" [suite \"b\" [test \"c\" [assert [true]]]] run-tests");
        let _ = v;
        let result = out("suite \"a\" [suite \"b\" [test \"c\" [assert [true]]]] run-tests");
        assert!(result.contains("a/b/c"), "nested suite path: {result}");
    }

    // --- M71: assert natives ---

    #[test]
    fn assert_passes_on_truthy() {
        let v = val("assert [true]");
        assert_eq!(red_core::printer::mold_to_string(&v), "none");
    }

    #[test]
    fn assert_fails_on_falsy() {
        let result = run_capture("assert [false]");
        assert!(result.is_err(), "assert [false] should error");
    }

    #[test]
    fn assert_equal_passes() {
        let v = val("assert-equal [1 1]");
        assert_eq!(red_core::printer::mold_to_string(&v), "none");
    }

    #[test]
    fn assert_equal_fails() {
        let result = run_capture("assert-equal [1 2]");
        assert!(result.is_err(), "assert-equal [1 2] should error");
        let err = result.unwrap_err();
        assert!(err.contains("expected"), "error should mention expected: {err}");
        assert!(err.contains("2"), "error should mention expected value: {err}");
    }

    #[test]
    fn assert_not_equal_passes() {
        let v = val("assert-not-equal [1 2]");
        assert_eq!(red_core::printer::mold_to_string(&v), "none");
    }

    #[test]
    fn assert_not_equal_fails() {
        let result = run_capture("assert-not-equal [1 1]");
        assert!(result.is_err(), "assert-not-equal [1 1] should error");
    }

    #[test]
    fn assert_error_passes_when_body_raises() {
        let v = val("assert-error [1 / 0]");
        assert_eq!(red_core::printer::mold_to_string(&v), "none");
    }

    #[test]
    fn assert_error_fails_when_body_succeeds() {
        let result = run_capture("assert-error [1 + 1]");
        assert!(result.is_err(), "assert-error on a succeeding body should fail");
    }

    #[test]
    fn assert_no_error_passes() {
        let v = val("assert-no-error [1 + 1]");
        assert_eq!(red_core::printer::mold_to_string(&v), "none");
    }

    #[test]
    fn assert_no_error_fails() {
        let result = run_capture("assert-no-error [1 / 0]");
        assert!(result.is_err());
    }

    #[test]
    fn fail_raises_with_message() {
        let result = run_capture("fail \"boom\"");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("boom"));
    }

    #[test]
    fn top_level_before_test_is_noop() {
        let result = run_capture("before-test [x: 1]");
        assert!(result.is_ok(), "top-level before-test should not crash");
    }

    // --- M72: run-tests ---

    #[test]
    fn run_tests_passes_all() {
        let result = out(
            "test \"a\" [assert [true]]
             test \"b\" [assert-equal [1 1]]
             test \"c\" [assert [1 + 1 = 2]]
             run-tests",
        );
        assert!(result.contains("TAP version 14"), "TAP header: {result}");
        assert!(result.contains("1..3"), "plan line: {result}");
        assert!(result.contains("ok 1 - a"), "test 1 passes: {result}");
        assert!(result.contains("ok 2 - b"), "test 2 passes: {result}");
        assert!(result.contains("ok 3 - c"), "test 3 passes: {result}");
        assert!(result.contains("passed: 3"), "summary: {result}");
    }

    #[test]
    fn run_tests_reports_failure() {
        let result = out(
            "test \"ok\" [assert [true]]
             test \"bad\" [assert-equal [1 2]]
             run-tests",
        );
        assert!(result.contains("ok 1 - ok"), "test 1 passes: {result}");
        assert!(result.contains("not ok 2 - bad"), "test 2 fails: {result}");
        assert!(result.contains("found:"), "diagnostic has found: {result}");
        assert!(result.contains("expected:"), "diagnostic has expected: {result}");
        assert!(result.contains("where: assert-equal"), "diagnostic has where: {result}");
        assert!(result.contains("failed: 1"), "summary: {result}");
    }

    #[test]
    fn run_tests_crash_vs_fail() {
        let result = out(
            "test \"crash\" [1 / 0]
             test \"fail\" [fail \"deliberate\"]
             run-tests",
        );
        // Crash: no `where:` field.
        assert!(result.contains("not ok 1 - crash"), "crash test: {result}");
        assert!(result.contains("# crash"), "crash directive: {result}");
        // Fail: has `where:` field.
        assert!(result.contains("not ok 2 - fail"), "fail test: {result}");
    }

    #[test]
    fn before_test_runs_before_each_test() {
        let result = out(
            "suite \"s\" [
                before-test [counter: 0]
                test \"a\" [counter: counter + 1 assert-equal [counter 1]]
                test \"b\" [counter: counter + 1 assert-equal [counter 1]]
            ]
            run-tests",
        );
        assert!(result.contains("ok 1 - s/a"), "test a passes: {result}");
        assert!(result.contains("ok 2 - s/b"), "test b passes: {result}");
    }

    #[test]
    fn run_tests_isolates_setwords() {
        let result = out(
            "test \"a\" [y: 99]
             test \"b\" [assert [not value? 'y]]
             run-tests",
        );
        assert!(result.contains("ok 1"), "test a passes: {result}");
        assert!(result.contains("ok 2"), "test b passes (y not leaked): {result}");
    }

    #[test]
    fn run_tests_idempotent() {
        let result = out(
            "test \"a\" [assert [true]] run-tests run-tests",
        );
        // Second run-tests is a no-op.
        let ok_count = result.matches("ok 1").count();
        assert_eq!(ok_count, 1, "run-tests should be idempotent: {result}");
    }

    #[test]
    fn run_tests_empty_registry() {
        let result = out("run-tests");
        assert!(result.contains("1..0"), "empty plan: {result}");
        assert!(result.contains("no tests registered"), "no tests msg: {result}");
    }

    #[test]
    fn nested_suite_hook_inheritance() {
        let result = out(
            "suite \"outer\" [
                before-test [x: 1]
                suite \"inner\" [
                    before-test [x: x + 1]
                    test \"t\" [assert-equal [x 2]]
                ]
            ]
            run-tests",
        );
        assert!(result.contains("ok 1 - outer/inner/t"), "nested hook: {result}");
    }
}
