//! M162–M163: Build/task dialect — `build [...]`, `task [...]`, `run-task`.
//!
//! A block-walking dialect for defining and running named tasks with
//! dependency resolution. Tasks are registered into `env.tasks`; `run-task`
//! executes a task's body, first running any dependencies (bare words in the
//! body that match registered task names).
//!
//! Grammar:
//!   build [
//!       task clean [print "Cleaning..." ...]
//!       task compile [print "Compiling..." ...]
//!       task all [clean compile test]      ; dependencies (bare words)
//!       default all
//!   ]
//!
//!   task name [body]      ; standalone registration
//!   run-task 'name        ; run a specific task

use red_core::value::{Series, Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::interp::dispatch_block;
use crate::natives::{reg_refined, type_name};
use crate::series::word_sym;
use crate::NativeFn;

// ===========================================================================
// build native
// ===========================================================================

/// `build block` — walks the block, registering `task`s and the `default`.
/// Does NOT run tasks (except explicit `run` calls inside the block).
pub fn build_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("build"),
            expected: 1,
            got: 0,
            span: Span::default(),
        });
    }
    let block = &args[0];
    let span = block.span_or_default();
    let series = match block {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span,
            });
        }
    };

    let data = series.data.borrow();
    let elems: Vec<Value> = data.iter().skip(series.index).cloned().collect();
    drop(data);

    let mut i = 0;
    while i < elems.len() {
        let keyword = match word_sym(&elems[i]) {
            Some(s) => s.clone(),
            None => {
                return Err(EvalError::Native {
                    message: format!("build: expected keyword, got {}", type_name(&elems[i])),
                    span: elems[i].span_or_default(),
                });
            }
        };
        i += 1;

        match keyword.as_str() {
            "task" => {
                if i + 1 >= elems.len() {
                    return Err(EvalError::Native {
                        message: "build: `task` expects a name and a body block".into(),
                        span,
                    });
                }
                let name = parse_task_name(&elems[i])?;
                let body = match &elems[i + 1] {
                    Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
                    other => {
                        return Err(EvalError::Native {
                            message: format!(
                                "build: `task` body must be a block!, got {}",
                                type_name(other)
                            ),
                            span: other.span_or_default(),
                        });
                    }
                };
                env.tasks.insert(name, body);
                i += 2;
            }
            "default" => {
                if i >= elems.len() {
                    return Err(EvalError::Native {
                        message: "build: `default` expects a task name".into(),
                        span,
                    });
                }
                let name = parse_task_name(&elems[i])?;
                env.default_task = Some(name);
                i += 1;
            }
            "run" => {
                if i >= elems.len() {
                    return Err(EvalError::Native {
                        message: "build: `run` expects a task name".into(),
                        span,
                    });
                }
                let name = parse_task_name(&elems[i])?;
                run_task(&name, env, span)?;
                i += 1;
            }
            other => {
                return Err(EvalError::Native {
                    message: format!("build: unknown keyword `{other}`"),
                    span,
                });
            }
        }
    }

    Ok(Value::None)
}

/// Parse a task name from a value — accepts `word!`, `lit-word!`, `get-word!`,
/// or `string!`.
fn parse_task_name(v: &Value) -> Result<Symbol, EvalError> {
    match v {
        Value::Word { sym, .. }
        | Value::SetWord { sym, .. }
        | Value::GetWord { sym, .. }
        | Value::LitWord { sym, .. } => Ok(sym.clone()),
        Value::String { s, .. } => Ok(Symbol::new(s)),
        other => Err(EvalError::Native {
            message: format!(
                "build: task name must be a word or string, got {}",
                type_name(other)
            ),
            span: other.span_or_default(),
        }),
    }
}

// ===========================================================================
// task native (standalone registration)
// ===========================================================================

/// `task name [body]` — register a single task outside a `build` block.
pub fn task_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() < 2 {
        return Err(EvalError::Arity {
            native: Symbol::new("task"),
            expected: 2,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    let name = parse_task_name(&args[0])?;
    let span = args[1].span_or_default();
    let body = match &args[1] {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        other => {
            return Err(EvalError::Native {
                message: format!(
                    "task: body must be a block!, got {}",
                    type_name(other)
                ),
                span,
            });
        }
    };
    env.tasks.insert(name, body);
    Ok(Value::None)
}

// ===========================================================================
// run-task native
// ===========================================================================

/// `run-task 'name` — run a registered task by name. Handles dependency
/// resolution (bare words in the body matching task names are run first),
/// dedup (each task runs at most once per `build` invocation), and cycle
/// detection.
pub fn run_task_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("run-task"),
            expected: 1,
            got: 0,
            span: Span::default(),
        });
    }
    let name = parse_task_name(&args[0])?;
    let span = args[0].span_or_default();
    run_task(&name, env, span)
}

// ===========================================================================
// Task execution with dependency resolution
// ===========================================================================

/// Run a task by name. Dependencies (bare words in the body matching
/// registered task names) are run first. Each task runs at most once per
/// `build` invocation (dedup via `env.ran_tasks`). Cycle detection via a
/// `visiting` set passed down the recursion.
fn run_task(name: &Symbol, env: &mut Env, span: Span) -> Result<Value, EvalError> {
    let mut visiting: Vec<Symbol> = Vec::new();
    run_task_inner(name, env, span, &mut visiting)
}

fn run_task_inner(
    name: &Symbol,
    env: &mut Env,
    span: Span,
    visiting: &mut Vec<Symbol>,
) -> Result<Value, EvalError> {
    // Cycle detection.
    if visiting.iter().any(|s| s == name) {
        let chain: Vec<String> = visiting.iter().map(|s| s.as_str().into()).collect();
        return Err(EvalError::Native {
            message: format!(
                "build: circular task dependency detected: {} -> {}",
                chain.join(" -> "),
                name.as_str()
            ),
            span,
        });
    }

    // Dedup: skip if already run.
    if env.ran_tasks.contains(name) {
        return Ok(Value::None);
    }

    // Look up the task body.
    let body = match env.tasks.get(name) {
        Some(s) => s.clone(),
        None => {
            return Err(EvalError::Native {
                message: format!("build: task `{}` is not defined", name.as_str()),
                span,
            });
        }
    };

    visiting.push(name.clone());

    // Walk the body: run dependencies (bare words matching task names) in
    // order, and collect non-dependency elements into a residual block to
    // evaluate at the end. This way, dependency words are treated as task
    // invocations, not variable lookups.
    let data = body.data.borrow();
    let elems: Vec<Value> = data.iter().skip(body.index).cloned().collect();
    drop(data);

    let mut residual: Vec<Value> = Vec::new();
    let mut last_value = Value::None;

    for elem in &elems {
        let is_dep = match word_sym(elem) {
            Some(sym) => {
                if env.tasks.contains_key(sym) && !env.ran_tasks.contains(sym) {
                    // Run the dependency.
                    run_task_inner(sym, env, span, visiting)?;
                    true
                } else if env.tasks.contains_key(sym) {
                    // Already ran — skip (dedup).
                    true
                } else {
                    false
                }
            }
            None => false,
        };
        if !is_dep {
            residual.push(elem.clone());
        }
    }

    visiting.pop();
    env.ran_tasks.insert(name.clone());

    // Evaluate the residual body (non-dependency elements).
    if !residual.is_empty() {
        let block = Value::block(Series::new(residual));
        last_value = dispatch_block(&block, env)?;
    }

    Ok(last_value)
}

// ===========================================================================
// Registration
// ===========================================================================

pub fn register_build_natives(env: &mut Env) {
    reg_refined(env, "build", build_native as NativeFn, 1, &[]);
    reg_refined(env, "task", task_native as NativeFn, 2, &[]);
    reg_refined(env, "run-task", run_task_native as NativeFn, 1, &[]);
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::register_build_natives;
    use crate::binding::bind_pass;
    use crate::natives::{install_constants, register_natives};
    use crate::eval;
    use crate::json::register_json_natives;
    use crate::html::register_html_natives;
    use crate::query::register_query_natives;
    use red_core::context::Context;
    use red_core::parser::load_source;
    use red_core::value::Value;
    use red_core::{Env, EvalError};
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

    fn run_capture_val(src: &str) -> Result<(Value, Vec<u8>), String> {
        let body = load_source(src).map_err(|e| e.to_string())?;
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        register_build_natives(&mut env);
        register_json_natives(&mut env);
        register_html_natives(&mut env);
        register_query_natives(&mut env);
        let block = Value::block(body);
        let val = match eval(&block, &mut env) {
            Ok(v) => v,
            Err(EvalError::Quit(_)) => Value::None,
            Err(e) => return Err(e.to_string()),
        };
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    fn out(src: &str) -> String {
        s(&run_capture_val(src).unwrap().1)
    }

    #[test]
    fn build_registers_tasks() {
        // Verify run-task actually executes.
        let result = out("build [task clean [print \"cleaning\"] run clean]");
        assert!(result.contains("cleaning"), "run should execute: {result}");
    }

    #[test]
    fn build_default_runs() {
        // `default` marks the task; an explicit `run` is needed to execute it
        // (the CLI `--build` flag auto-runs the default; in script context
        // we use `run`).
        let result = out(
            "build [
                task clean [print \"cleaning\"]
                default clean
                run clean
            ]",
        );
        assert!(result.contains("cleaning"), "default+run: {result}");
    }

    #[test]
    fn build_dependencies() {
        let result = out(
            "build [
                task a [print \"a\"]
                task b [a print \"b\"]
                run b
            ]",
        );
        assert!(result.contains("a"), "dep a should run: {result}");
        assert!(result.contains("b"), "task b should run: {result}");
        // a should come before b.
        let a_pos = result.find("a").unwrap();
        let b_pos = result.find("b").unwrap();
        assert!(a_pos < b_pos, "dependency order: a before b");
    }

    #[test]
    fn build_cycle_detection() {
        let result = run_capture_val(
            "build [
                task a [b]
                task b [a]
                run a
            ]",
        );
        assert!(result.is_err(), "circular dependency should error");
        assert!(result.unwrap_err().contains("circular"), "error mentions circular");
    }

    #[test]
    fn build_dedup() {
        let result = out(
            "build [
                task a [print \"a\"]
                task b [a]
                task c [a b]
                run c
            ]",
        );
        // Count occurrences of "a\n" — should be exactly 1 (deduped).
        let count = result.matches("a\n").count();
        assert_eq!(count, 1, "task a should run once (dedup): {result}");
    }

    #[test]
    fn build_standalone() {
        let result = out("task 'a [print \"hi\"] run-task 'a");
        assert!(result.contains("hi"), "standalone task + run-task: {result}");
    }

    #[test]
    fn run_task_unknown() {
        let result = run_capture_val("run-task 'nonexistent");
        assert!(result.is_err(), "unknown task should error");
    }

    #[test]
    fn build_unknown_keyword() {
        let result = run_capture_val("build [foobar]");
        assert!(result.is_err(), "unknown keyword should error");
    }
}
