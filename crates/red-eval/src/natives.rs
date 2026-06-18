//! Native (Rust-implemented) operations. Milestone 6 registers the I/O
//! natives `print`, `prin`, and `probe`, plus the constant words `none`,
//! `true`, `false`, `newline` (installed into the user context so they
//! resolve during eval).
//!
//! String rendering note: `print`/`prin`/`probe` mold every argument
//! uniformly (including strings, which appear quoted). This diverges from
//! real Red's `form`-based printing but keeps the POC printer surface small;
//! the divergence is documented for the M12 audit pass.

use std::io::Write;
use std::rc::Rc;

use red_core::context::Context;
use red_core::printer::mold_to_string;
use red_core::value::{FuncDef, Symbol, Value};
use red_core::{Env, EvalError, NativeFn};

/// `print`: mold each arg, join with a single space, append a newline.
/// Variadic — consumes all remaining args in the enclosing block up to the
/// next native word. Returns `Value::None`.
fn print(args: &[Value], env: &mut Env) -> Result<Value, EvalError> {
    let joined = join_molded(args);
    let _ = writeln!(env.out, "{joined}");
    Ok(Value::None)
}

/// `prin`: like `print` but without the trailing newline.
fn prin(args: &[Value], env: &mut Env) -> Result<Value, EvalError> {
    let joined = join_molded(args);
    let _ = write!(env.out, "{joined}");
    Ok(Value::None)
}

/// `probe`: print `== <mold>` for each arg (joined with space), newline,
/// and return the first arg (or `none` if no args).
fn probe(args: &[Value], env: &mut Env) -> Result<Value, EvalError> {
    let joined = join_molded(args);
    let _ = writeln!(env.out, "== {joined}");
    Ok(args.first().cloned().unwrap_or(Value::None))
}

fn join_molded(args: &[Value]) -> String {
    let mut out = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&mold_to_string(a));
    }
    out
}

fn fixed_native(f: NativeFn, arity: usize) -> Rc<FuncDef> {
    let params: Vec<Symbol> = (0..arity)
        .map(|i| Symbol::new(&format!("__arg{i}")))
        .collect();
    Rc::new(FuncDef {
        params,
        native: Some(f),
        variadic: false,
        ..Default::default()
    })
}

/// Register the M6 native words into `env.natives`. Each takes exactly one
/// argument (Red's real arity for `print`/`prin`/`probe`). Constants
/// (`none`/`true`/`false`/`newline`) are installed separately into the user
/// context via [`install_constants`] before the binding pass runs.
pub fn register_natives(env: &mut Env) {
    env.natives
        .insert(Symbol::new("print"), fixed_native(print as NativeFn, 1));
    env.natives
        .insert(Symbol::new("prin"), fixed_native(prin as NativeFn, 1));
    env.natives
        .insert(Symbol::new("probe"), fixed_native(probe as NativeFn, 1));
}

/// Install the predefined constant words (`none`, `true`, `false`, `newline`)
/// into a user context. Must be called before `bind_pass` so references to
/// these words get `Local` bindings to the constant slots.
pub fn install_constants(ctx: &mut Context) {
    ctx.set(Symbol::new("none"), Value::None);
    ctx.set(Symbol::new("true"), Value::Logic(true));
    ctx.set(Symbol::new("false"), Value::Logic(false));
    ctx.set(
        Symbol::new("newline"),
        Value::String(std::rc::Rc::from("\n")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use red_core::parser::load_source;
    use std::cell::RefCell;
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
        use crate::interp::{bind_pass, eval};
        let body = load_source(src).map_err(|e| e.to_string())?;
        let mut ctx = Context::new();
        install_constants(&mut ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        let block = Value::block(body);
        let _ = eval(&block, &mut env).map_err(|e| e.to_string())?;
        let out = buf.borrow().clone();
        Ok(out)
    }

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    #[test]
    fn print_integer() {
        assert_eq!(s(&run_capture("print 5").unwrap()), "5\n");
    }

    #[test]
    fn prin_concat() {
        // mold-everything: strings render quoted, so `prin "a" prin "b"`
        // yields `"a""b"`. Each `prin` takes exactly one argument.
        assert_eq!(
            s(&run_capture("prin \"a\" prin \"b\"").unwrap()),
            "\"a\"\"b\""
        );
    }

    #[test]
    fn print_block() {
        assert_eq!(s(&run_capture("print [1 2 3]").unwrap()), "[1 2 3]\n");
    }

    #[test]
    fn print_string_molded() {
        assert_eq!(
            s(&run_capture("print \"Hello, World!\"").unwrap()),
            "\"Hello, World!\"\n"
        );
    }

    #[test]
    fn probe_value() {
        assert_eq!(s(&run_capture("probe 42").unwrap()), "== 42\n");
    }

    #[test]
    fn print_returns_none() {
        // `print` always returns none; the surrounding block's last value
        // after `print 5` is none.
        let (val, _) = run_capture_val("print 5").unwrap();
        assert_eq!(mold_to_string(&val), "none");
    }

    fn run_capture_val(src: &str) -> Result<(Value, Vec<u8>), String> {
        use crate::interp::{bind_pass, eval};
        let body = load_source(src).map_err(|e| e.to_string())?;
        let mut ctx = Context::new();
        install_constants(&mut ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        let block = Value::block(body);
        let val = eval(&block, &mut env).map_err(|e| e.to_string())?;
        let out = buf.borrow().clone();
        Ok((val, out))
    }
}
