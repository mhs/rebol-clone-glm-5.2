//! M113: `port!` abstraction + minimal synchronous networking.
//!
//! Ships a synthetic `Value::Port` value type and a protocol facade
//! (`crates/red-eval/src/net/`) layered over `ureq` for HTTP/HTTPS. v0.9
//! scope: GET-only HTTP/HTTPS via the existing `ureq = "2"` dep (TLS on by
//! default in ureq 2.x — no new dependency). Other protocols from the
//! project's crate-recommendation research (`ftp`/`smtp`/`pop3`/`nntp`/
//! `dns`/`tcp`/`udp`/`whois`/`finger`/`daytime`) are reserved as
//! `PortScheme` enum variants that error with `NetError::UnsupportedInV09`
//! so the dispatch table is obviously extensible for v0.10+.
//!
//! This is explicitly **not** the async/`Channel`-backed port model from
//! `docs/plans/future-plan-concurrency.md` — it's the synchronous subset that makes
//! `port!` exist as a value, unifies file I/O under the same `open`/`close`/
//! `read`/`write`/`create` verbs Red scripts expect, gates network access
//! (closing the sandbox hole where `read http://` worked unconditionally),
//! and adds streaming so `read open url!` doesn't slurp the whole body.

pub mod error;
pub mod http;
pub mod protocol;
pub mod request;

use std::path::Path;
use std::rc::Rc;

use red_core::value::{PortDef, PortScheme, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::io::resolve_path;
use crate::natives::type_name;
use crate::net::error::NetError;

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

/// Register the v0.9 port!/networking natives. Called from
/// `natives::registry::register_natives` immediately after `register_io_natives`
/// (and before `invalidate_native_index`).
pub fn register_net_natives(env: &mut Env) {
    use red_core::value::FuncDef;

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

    reg(env, "open", open_native as NF, 1);
    reg(env, "close", close_native as NF, 1);
    reg(env, "create", create_native as NF, 1);
    reg(env, "port?", port_predicate as NF, 1);
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Wrap a `NetError` into an `EvalError::Native` with the offending value's
/// span (mirrors `io.rs::native_err`).
fn net_err(from: &Value, e: NetError) -> EvalError {
    EvalError::Native {
        message: e.render(),
        span: from.span_or_default(),
    }
}

/// Extract a `Port` value's `Rc<RefCell<PortDef>>`, or raise a TypeError.
fn expect_port(v: &Value) -> Result<Rc<std::cell::RefCell<PortDef>>, EvalError> {
    match v {
        Value::Port(p) => Ok(Rc::clone(p)),
        other => Err(EvalError::TypeError {
            expected: "port!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// Capability gate: error if `env.allow_network` is false. Mirrors the
/// `env.allow_shell` gate in `io.rs::call`/`shell`. Called by `open` on a
/// URL and by `read`/`write` on an HTTP port (file ports are *not* gated —
/// filesystem access has its own OS-level permissions).
fn ensure_network_allowed(env: &Env, native: &'static str, from: &Value) -> Result<(), EvalError> {
    if env.allow_network {
        Ok(())
    } else {
        Err(net_err(from, NetError::NetworkDisabled(native)))
    }
}

// ---------------------------------------------------------------------------
// open / close / create / port?
// ---------------------------------------------------------------------------

/// `open <file!|url!>` → `port!`. For a `file!`, wraps the file path in a
/// `PortDef` (read/write semantics unchanged — `read port` slurps the file;
/// `write port` truncates/appends). For a `url!` with scheme `http`/`https`,
/// issues a `ureq` GET at `open` time (fail-fast on DNS/connection/HTTP
/// errors) and holds the body `Read` for streaming `read port` calls.
fn open_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "open", 1, args.len()));
    }
    let (scheme, target) = match &args[0] {
        Value::File { path, .. } => (PortScheme::File, Rc::clone(path)),
        Value::Url { url, .. } => {
            // Network capability gate (file ports are not gated).
            ensure_network_allowed(env, "open", &args[0])?;
            let scheme = protocol::scheme_of_url(url).map_err(|e| net_err(&args[0], e))?;
            protocol::ensure_supported_in_v09(scheme).map_err(|e| net_err(&args[0], e))?;
            (scheme, Rc::clone(url))
        }
        other => {
            return Err(EvalError::TypeError {
                expected: "file! or url!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };

    let port_def = PortDef::new(scheme, target);
    // For HTTP ports, issue the ureq GET at `open` time (fail-fast). The body
    // reader is installed on `PortState::http_body` for streaming `read port`.
    // For file ports, just flip `state.open` to true — no file handle is
    // opened here (file-port reads slurp via `std::fs::read`; writes open
    // the handle lazily in `write_port`).
    if scheme == PortScheme::Http {
        let url = port_def.target.as_ref().to_string();
        let mut state = port_def.state.borrow_mut();
        http::open_http(&mut state, &url).map_err(|e| net_err(&args[0], e))?;
    } else {
        port_def.state.borrow_mut().open = true;
    }
    Ok(Value::port(port_def))
}

/// `close port` → `none`. Drops the `PortState`, releasing the `ureq` body
/// `Read` handle or file handle. A second `read`/`write` on a closed port
/// raises `NetError::Closed`.
fn close_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "close", 1, args.len()));
    }
    let p = expect_port(&args[0])?;
    let binding = p.borrow_mut();
    let mut state = binding.state.borrow_mut();
    state.open = false;
    state.http_body = None;
    state.file_handle = None;
    state.cursor = 0;
    Ok(Value::None)
}

/// `create <file!>` → `port!`. Red's `create` opens-or-truncates; v0.9
/// aliases it to `open` on a file! (the existing `write`-with-truncate path
/// is what `write port` does for a file port — `create` just hands back a
/// port ready for `write`). URLs are not supported (`create http://` is
/// `NetError::UnsupportedInV09("http-write")`-adjacent — GET-only v0.9).
fn create_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "create", 1, args.len()));
    }
    match &args[0] {
        Value::File { path, .. } => {
            let port_def = PortDef::new(PortScheme::File, Rc::clone(path));
            // Open the file in write-create-truncate mode so the port is
            // ready for `write port` calls. Held on `state.file_handle`.
            let p = resolve_path(path, _env);
            let f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&p)
                .map_err(|e| {
                    net_err(&args[0], NetError::Io(format!("create {}", p.display()), e))
                })?;
            port_def.state.borrow_mut().file_handle = Some(f);
            port_def.state.borrow_mut().open = true;
            Ok(Value::port(port_def))
        }
        other => Err(EvalError::TypeError {
            expected: "file!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// `port? value` → `logic!`.
fn port_predicate(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "port?", 1, args.len()));
    }
    Ok(Value::Logic(matches!(args[0], Value::Port(_))))
}

// ---------------------------------------------------------------------------
// read_port / write_port — invoked from io.rs::read / io.rs::write when the
// argument is a `Value::Port`. Kept here (next to `open`/`close`) rather
// than in io.rs so the port surface is cohesive.
// ---------------------------------------------------------------------------

/// `read port` — streaming for HTTP ports (8 KiB chunks; empty `string!` at
/// EOF), whole-file slurp for file ports (matches today's `read %file`
/// behavior). Returns `string!` by default; the caller (io.rs::read) honors
/// `/binary`/`/lines` on the result for the one-shot `read url!` path.
pub(crate) fn read_port(
    port: &Rc<std::cell::RefCell<PortDef>>,
    from: &Value,
) -> Result<Vec<u8>, EvalError> {
    let p = port.borrow();
    let mut state = p.state.borrow_mut();
    if !state.open {
        return Err(net_err(from, NetError::Closed));
    }
    match p.scheme {
        PortScheme::Http => {
            let bytes = http::read_http(&mut state).map_err(|e| net_err(from, e))?;
            Ok(bytes)
        }
        PortScheme::File => {
            // File-port read: slurp the whole file (matches `read %file`).
            // The path was captured at `open` time as `port_def.target`.
            let path = Path::new(p.target.as_ref());
            let bytes = std::fs::read(path)
                .map_err(|e| net_err(from, NetError::Io(format!("read {}", path.display()), e)))?;
            Ok(bytes)
        }
        // Unreachable: `open` rejects unsupported schemes with
        // `UnsupportedInV09` before constructing a port.
        _ => Err(net_err(from, NetError::UnsupportedInV09(p.scheme))),
    }
}

/// `write port value` — file ports write through to the underlying file
/// handle (append or truncate depending on the handle's open mode). HTTP
/// ports error: v0.9 is GET-only (`NetError::HttpWriteUnsupported`). Returns
/// `none` (matching `write %file`).
pub(crate) fn write_port(
    port: &Rc<std::cell::RefCell<PortDef>>,
    content: &[u8],
    from: &Value,
) -> Result<(), EvalError> {
    use std::io::Write;
    let p = port.borrow();
    let mut state = p.state.borrow_mut();
    if !state.open {
        return Err(net_err(from, NetError::Closed));
    }
    match p.scheme {
        PortScheme::File => {
            // `create` opens with truncate; `open` on a file doesn't open a
            // handle yet (file ports are read-oriented via slurp). If
            // `file_handle` is `None`, open one in write-append mode so a
            // `write` after `open %file` doesn't truncate (matches `write`
            // semantics on a `file!`).
            if state.file_handle.is_none() {
                let path = Path::new(p.target.as_ref());
                let f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|e| {
                        net_err(from, NetError::Io(format!("write {}", path.display()), e))
                    })?;
                state.file_handle = Some(f);
            }
            let f = state.file_handle.as_mut().unwrap();
            f.write_all(content)
                .map_err(|e| net_err(from, NetError::Io("write".to_string(), e)))?;
            Ok(())
        }
        PortScheme::Http => Err(net_err(from, NetError::HttpWriteUnsupported)),
        _ => Err(net_err(from, NetError::UnsupportedInV09(p.scheme))),
    }
}

// ---------------------------------------------------------------------------
// local helpers
// ---------------------------------------------------------------------------

/// Arity-error helper, mirroring the one in `natives/mod.rs` (kept local so
/// this module is self-contained — `natives::arity_err` is `pub(crate)` but
/// the indirection is unnecessary for a single call site per native).
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::bind_pass;
    use crate::interp::eval;
    use crate::natives::{install_constants, register_natives};
    use red_core::context::Context;
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use red_core::Env;
    use std::cell::RefCell;
    use std::io::Write;
    use std::net::TcpListener;
    use std::rc::Rc;

    /// Test writer that buffers output in a shared `Vec<u8>`.
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

    /// Build a fresh env with constants + natives registered. Network gate
    /// is OFF by default; tests that need network set `env.allow_network = true`.
    fn run_capture(src: &str, allow_network: bool) -> Result<Value, String> {
        let body = load_source(src).map_err(|e| e.to_string())?;
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        env.allow_network = allow_network;
        let block = Value::block(body);
        eval(&block, &mut env).map_err(|e| e.to_string())
    }

    // ----- file-port round-trip -----

    #[test]
    fn port_file_roundtrip() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path();
        let pstr = path.to_string_lossy().replace('\\', "/");
        let src = format!("p: open %{} write p {{hi}} close p read %{}", pstr, pstr);
        let v = run_capture(&src, false).expect("file-port roundtrip");
        assert_eq!(mold_to_string(&v), "\"hi\"");
    }

    // ----- port? predicate -----

    #[test]
    fn port_predicate() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let pstr = tmp.path().to_string_lossy().replace('\\', "/");
        let src = format!("port? open %{}", pstr);
        let v = run_capture(&src, false).expect("port?");
        assert_eq!(mold_to_string(&v), "true");
    }

    #[test]
    fn port_predicate_false_on_non_port() {
        let v = run_capture("port? 42", false).expect("port? non-port");
        assert_eq!(mold_to_string(&v), "false");
    }

    // ----- closed port errors cleanly -----

    #[test]
    fn port_read_after_close_errors() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let pstr = tmp.path().to_string_lossy().replace('\\', "/");
        // Open then close then read → "operation on a closed port".
        let src = format!("p: open %{} close p read p", pstr);
        let err = run_capture(&src, false).unwrap_err();
        assert!(err.contains("closed port"), "got: {err}");
    }

    // ----- unsupported scheme dispatch -----

    #[test]
    fn port_unsupported_scheme_ftp() {
        // `open ftp://...` with allow_network=true → UnsupportedInV09("ftp").
        // (With the gate closed, the gate message fires first; this test
        // opens the gate to exercise the scheme-dispatch path.)
        let err = run_capture("open ftp://example.com/", true).unwrap_err();
        assert!(
            err.contains("not supported") && err.contains("ftp"),
            "got: {err}"
        );
    }

    #[test]
    fn port_unsupported_scheme_whois() {
        let err = run_capture("open whois://example.com", true).unwrap_err();
        assert!(
            err.contains("not supported") && err.contains("whois"),
            "got: {err}"
        );
    }

    // ----- network capability gate -----

    #[test]
    fn read_url_blocked_when_network_disabled() {
        // `read http://...` with allow_network=false → gate error, no
        // network call attempted (the gate fires before any ureq dispatch).
        let err = run_capture("read http://127.0.0.1:1/", false).unwrap_err();
        assert!(
            err.contains("network disabled") && err.contains("--allow-network"),
            "got: {err}"
        );
    }

    #[test]
    fn open_url_blocked_when_network_disabled() {
        let err = run_capture("open http://127.0.0.1:1/", false).unwrap_err();
        assert!(err.contains("network disabled"), "got: {err}");
    }

    // ----- HTTP read against an in-process test server -----

    #[test]
    fn port_http_read_in_process() {
        // Bind a listener, accept in a thread, read via `read url!`.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body_text = "hello-from-test-server";
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let resp = format!(
                    "HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
                    body_text.len(),
                    body_text
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        let src = format!("read http://127.0.0.1:{}/", port);
        let v = run_capture(&src, true).expect("http read");
        assert_eq!(mold_to_string(&v), "\"hello-from-test-server\"");
    }

    #[test]
    fn port_http_streaming_in_process() {
        // Verify `read port` (not `read url!`) returns chunks — the body is
        // NOT slurped at `open` time. We use a body larger than the 8 KiB
        // chunk size to force multiple reads.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // 20 KiB body of repeated 'A's.
        let body_size = 20 * 1024;
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let body = "A".repeat(body_size);
                let resp = format!(
                    "HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        // `open` then two `read port` calls — the first returns a non-empty
        // chunk (8 KiB), the second returns more; total reassembles to 20 KiB.
        // Use `rejoin` to concatenate the chunk strings.
        let src = format!(
            "p: open http://127.0.0.1:{}/ a: read p b: read p c: read p close p rejoin [{{}} a {{}} b {{}} c {{}}]",
            port
        );
        let v = run_capture(&src, true).expect("http streaming");
        // The total length should equal the body size (20 KiB of 'A's).
        match v {
            Value::String { s, .. } => {
                assert_eq!(s.len(), body_size, "streaming reassembly length");
                assert!(s.chars().all(|c| c == 'A'), "all chars are A");
            }
            other => panic!("expected string!, got {:?}", other),
        }
    }

    // ----- mold of port! (non-reparseable) -----

    #[test]
    fn port_mold_is_placeholder() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let pstr = tmp.path().to_string_lossy().replace('\\', "/");
        let src = format!("mold open %{}", pstr);
        let v = run_capture(&src, false).expect("mold port");
        match v {
            Value::String { s, .. } => {
                let mold = s.to_string();
                assert!(
                    mold.starts_with("#[port file://"),
                    "expected `#[port file://...]`, got: {mold}"
                );
            }
            other => panic!("expected string!, got {:?}", other),
        }
    }

    // ----- same? on ports (identity) -----

    #[test]
    fn port_same_predicate_identity() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let pstr = tmp.path().to_string_lossy().replace('\\', "/");
        // `same? p p` → true (same Rc); `same? p open %<same>` → false.
        let src = format!("p: open %{} q: p same? p q", pstr);
        let v = run_capture(&src, false).expect("same? port");
        assert_eq!(mold_to_string(&v), "true");
    }

    // ----- M135: HTTP error-path coverage -----
    // The existing in-process tests only exercise the 200-OK happy path.
    // These add 404 (HttpStatus arm) and connection-refused (HttpTransport
    // arm) to cover the two error branches in `http::open_http`.

    #[test]
    fn port_http_404_returns_status_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let resp = "HTTP/1.0 404 Not Found\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        let src = format!("open http://127.0.0.1:{}/", port);
        let err = run_capture(&src, true).unwrap_err();
        assert!(
            err.contains("status 404"),
            "expected status 404 error, got: {err}"
        );
    }

    #[test]
    fn port_http_connection_refused_returns_transport_error() {
        // Connect to a port that nothing is listening on → connection refused.
        // Use a port that's almost certainly free (1 is privileged, so it'll
        // be refused on most systems without privileges).
        let src = "open http://127.0.0.1:1/";
        let err = run_capture(src, true).unwrap_err();
        assert!(
            err.contains("transport error") || err.contains("Connection refused"),
            "expected transport error, got: {err}"
        );
    }
}
