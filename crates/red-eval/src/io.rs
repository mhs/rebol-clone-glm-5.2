//! File & shell I/O natives (Milestone 20).
//!
//! `read`/`write`/`save`/`load`-from-file operate on `file!` values; `read`
//! also accepts `url!` (fetched via `ureq` for http/https). Directory ops
//! (`dir?`/`make-dir`/`delete`/`rename`/`change-dir`/`what-dir`), file
//! metadata (`exists?`/`size?`/`modified?`), environment variables
//! (`env`/`get-env`/`set-env`), `wait` (sleep), and `call`/`shell` (gated
//! behind `env.allow_shell`) round out the set.
//!
//! File paths resolve against `env.cwd`. Relative paths join with the cwd;
//! absolute paths and home-relative paths are used as-is.
//!
//! Sandbox policy: `call`/`shell` raise `EvalError::Native` unless the CLI
//! passed `--allow-shell` (sets `env.allow_shell = true`). No shell is
//! invoked from any test fixture.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use red_core::parser::load_source;
use red_core::printer::mold_to_string;
use red_core::value::{Series, Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::natives::{arity_err, type_name};

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a `file!` path string from a value, or raise a TypeError.
fn expect_file(v: &Value) -> Result<(&Rc<str>, Span), EvalError> {
    match v {
        Value::File { path, span } => Ok((path, *span)),
        other => Err(EvalError::TypeError {
            expected: "file!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// Resolve a `file!` path against `env.cwd`. Absolute paths and paths
/// starting with `~` are returned as-is (the OS / a shell-expansion step
/// handles `~`; POC leaves `~` literal — Red itself doesn't expand `~`).
///
/// M62: exposed `pub(crate)` so the `import %file` native in `module.rs`
/// can resolve a `file!` argument the same way `read`/`load` do.
pub(crate) fn resolve_path(path_str: &str, env: &Env) -> PathBuf {
    let p = Path::new(path_str);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        env.cwd.join(p)
    }
}

/// `EvalError::Native` with a span sourced from `from` (the offending value).
fn native_err(from: &Value, msg: impl Into<String>) -> EvalError {
    EvalError::Native {
        message: msg.into(),
        span: from.span_or_default(),
    }
}

/// Wrap an io error with the file path context and the value's span.
fn io_err(from: &Value, path: &Path, ctx: &str, e: std::io::Error) -> EvalError {
    native_err(
        from,
        format!("{ctx} {display}: {e}", display = path.display()),
    )
}

// ---------------------------------------------------------------------------
// read / write / save / load (file)
// ---------------------------------------------------------------------------

/// `read file` / `read url` / `read port` → `string!`. With `/lines` returns
/// a `block!` of lines (no trailing newlines). `/binary` returns a `binary!`
/// of the raw file bytes (M41 — de-stubbed). M113: when passed a `port!`,
/// dispatches to `net::read_port` (streaming for HTTP ports, slurp for file
/// ports). The one-shot `read url!` ergonomics are preserved (equivalent to
/// `read open url!` as a single call) but now gated by `env.allow_network`.
fn read(args: &[Value], refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "read", 1, args.len()));
    }
    let binary = refs.has(&Symbol::new("binary"));
    let lines = refs.has(&Symbol::new("lines"));
    if binary && lines {
        return Err(native_err(
            &args[0],
            "read: /binary and /lines are mutually exclusive",
        ));
    }
    match &args[0] {
        Value::File { path, span } => {
            let _ = span;
            let p = resolve_path(path, env);
            if binary {
                let bytes =
                    std::fs::read(&p).map_err(|e| io_err(&args[0], &p, "cannot read", e))?;
                return Ok(Value::String8 { bytes, span: *span });
            }
            let contents =
                std::fs::read_to_string(&p).map_err(|e| io_err(&args[0], &p, "cannot read", e))?;
            if lines {
                Ok(Value::block(Series::new(
                    contents
                        .lines()
                        .map(|l| Value::string(Rc::from(l)))
                        .collect(),
                )))
            } else {
                Ok(Value::string(Rc::from(contents.as_str())))
            }
        }
        Value::Url { url, span } => {
            let _ = span;
            // M113: gate network access (mirrors `env.allow_shell` for
            // `call`/`shell`). Pre-M113 `read url!` worked unconditionally —
            // a sandbox hole this milestone closes.
            if !env.allow_network {
                return Err(native_err(
                    &args[0],
                    "read: network disabled (use --allow-network to enable)",
                ));
            }
            // M113: route through the `net/` facade (which dispatches by
            // scheme and rejects unsupported ones with
            // `NetError::UnsupportedInV09`). The one-shot `read url!`
            // ergonomics are preserved (equivalent to `read open url!` as a
            // single call) — the body is fully materialized here.
            let scheme =
                crate::net::protocol::scheme_of_url(url).map_err(|e| net_err(&args[0], e))?;
            // `read url!` only accepts http/https in v0.9 — a `file://` URL
            // is rejected (use `read %file` instead). Other reserved
            // schemes (ftp/whois/…) hit `UnsupportedInV09`.
            if scheme != red_core::value::PortScheme::Http {
                return Err(net_err(
                    &args[0],
                    crate::net::error::NetError::UnsupportedInV09(scheme),
                ));
            }
            let port_def = red_core::value::PortDef::new(scheme, Rc::clone(url));
            {
                let mut state = port_def.state.borrow_mut();
                crate::net::http::open_http(&mut state, url).map_err(|e| net_err(&args[0], e))?;
            }
            // Drain the body fully (one-shot read — not the streaming
            // `read port` path, which yields 8 KiB chunks).
            let mut body = Vec::new();
            {
                let mut state = port_def.state.borrow_mut();
                if let Some(reader) = state.http_body.as_mut() {
                    reader.read_to_end(&mut body).map_err(|e| {
                        net_err(
                            &args[0],
                            crate::net::error::NetError::HttpTransport(e.to_string()),
                        )
                    })?;
                }
            }
            if binary {
                return Ok(Value::String8 {
                    bytes: body,
                    span: *span,
                });
            }
            let body_str = String::from_utf8_lossy(&body).into_owned();
            if lines {
                Ok(Value::block(Series::new(
                    body_str
                        .lines()
                        .map(|l| Value::string(Rc::from(l)))
                        .collect(),
                )))
            } else {
                Ok(Value::string(Rc::from(body_str.as_str())))
            }
        }
        Value::Port(p) => {
            // M113: `read port` — streaming for HTTP ports (8 KiB chunks;
            // empty `string!` at EOF), whole-file slurp for file ports.
            let bytes = crate::net::read_port(p, &args[0])?;
            if binary {
                let span = args[0].span_or_default();
                return Ok(Value::String8 { bytes, span });
            }
            let body_str = String::from_utf8_lossy(&bytes).into_owned();
            if lines {
                Ok(Value::block(Series::new(
                    body_str
                        .lines()
                        .map(|l| Value::string(Rc::from(l)))
                        .collect(),
                )))
            } else {
                Ok(Value::string(Rc::from(body_str.as_str())))
            }
        }
        other => Err(EvalError::TypeError {
            expected: "file!, url!, or port!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

/// M113: wrap a `NetError` into an `EvalError::Native` with the offending
/// value's span (mirrors `native_err`).
fn net_err(from: &Value, e: crate::net::error::NetError) -> EvalError {
    EvalError::Native {
        message: e.render(),
        span: from.span_or_default(),
    }
}

/// `write file content` → `none!`. Writes `content` (a string) to the file,
/// replacing any existing contents. Refinements:
/// - `/append` — append instead of truncate.
/// - `/lines`  — `content` is a block of strings; join with newlines.
/// - `/binary` — `content` is a `binary!` (or coerced to bytes); writes the
///   raw bytes to the file (M41 — de-stubbed).
fn write(args: &[Value], refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "write", 2, args.len()));
    }
    // M113: `write port value` — dispatch to the port writer (file ports
    // write through to the file handle; HTTP ports error: v0.9 is GET-only).
    if let Value::Port(p) = &args[0] {
        let content_bytes: Vec<u8> = if refs.has(&Symbol::new("binary")) {
            match &args[1] {
                Value::String8 { bytes, .. } => bytes.clone(),
                Value::String { s, .. } => s.as_bytes().to_vec(),
                other => {
                    return Err(EvalError::TypeError {
                        expected: "binary! or string!",
                        found: type_name(other),
                        span: other.span_or_default(),
                    });
                }
            }
        } else if refs.has(&Symbol::new("lines")) {
            match &args[1] {
                Value::Block { series, .. } | Value::Paren { series, .. } => {
                    let data = series.data.borrow();
                    let mut out = String::new();
                    for (i, v) in data.iter().enumerate() {
                        if i > 0 {
                            out.push('\n');
                        }
                        out.push_str(&string_for_write(v, &args[1])?);
                    }
                    out.into_bytes()
                }
                other => {
                    return Err(EvalError::TypeError {
                        expected: "block!",
                        found: type_name(other),
                        span: other.span_or_default(),
                    });
                }
            }
        } else {
            string_for_write(&args[1], &args[1])?.into_bytes()
        };
        crate::net::write_port(p, &content_bytes, &args[0])?;
        return Ok(Value::None);
    }

    let (path, _span) = expect_file(&args[0])?;
    let p = resolve_path(path, env);

    // /binary: accept a binary!, string!, or coerce via `form`. Bytes are
    // written verbatim — no newline joins, no `form` text.
    if refs.has(&Symbol::new("binary")) {
        let bytes: Vec<u8> = match &args[1] {
            Value::String8 { bytes, .. } => bytes.clone(),
            Value::String { s, .. } => s.as_bytes().to_vec(),
            other => {
                return Err(EvalError::TypeError {
                    expected: "binary! or string!",
                    found: type_name(other),
                    span: other.span_or_default(),
                });
            }
        };
        if refs.has(&Symbol::new("append")) {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&p)
                .map_err(|e| io_err(&args[0], &p, "cannot write", e))?;
            f.write_all(&bytes)
                .map_err(|e| io_err(&args[0], &p, "cannot write", e))?;
        } else {
            std::fs::write(&p, &bytes).map_err(|e| io_err(&args[0], &p, "cannot write", e))?;
        }
        return Ok(Value::None);
    }

    let content = if refs.has(&Symbol::new("lines")) {
        // Block of strings → newline-joined.
        match &args[1] {
            Value::Block { series, .. } | Value::Paren { series, .. } => {
                let data = series.data.borrow();
                let mut out = String::new();
                for (i, v) in data.iter().enumerate() {
                    if i > 0 {
                        out.push('\n');
                    }
                    out.push_str(&string_for_write(v, &args[1])?);
                }
                out
            }
            other => {
                return Err(EvalError::TypeError {
                    expected: "block!",
                    found: type_name(other),
                    span: other.span_or_default(),
                });
            }
        }
    } else {
        string_for_write(&args[1], &args[1])?
    };

    if refs.has(&Symbol::new("append")) {
        // Append — create if missing, otherwise append.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .map_err(|e| io_err(&args[0], &p, "cannot write", e))?;
        f.write_all(content.as_bytes())
            .map_err(|e| io_err(&args[0], &p, "cannot write", e))?;
    } else {
        std::fs::write(&p, content.as_bytes())
            .map_err(|e| io_err(&args[0], &p, "cannot write", e))?;
    }
    Ok(Value::None)
}

/// Coerce a value to the string body for `write`. Strings use their raw
/// contents; any other value is `form`ed (human-readable).
fn string_for_write(v: &Value, _span_src: &Value) -> Result<String, EvalError> {
    match v {
        Value::String { s, .. } => Ok(s.to_string()),
        _ => Ok(red_core::form_to_string(v)),
    }
}

/// `save file value` → `none!`. Molds `value` (reparseable form) and writes
/// it to the file. Useful for persisting blocks/data.
fn save(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "save", 2, args.len()));
    }
    let (path, _span) = expect_file(&args[0])?;
    let p = resolve_path(path, env);
    let molded = mold_to_string(&args[1]);
    std::fs::write(&p, molded.as_bytes()).map_err(|e| io_err(&args[0], &p, "cannot save", e))?;
    Ok(Value::None)
}

/// `load` extended to accept a `file!` (reads the file then parses it as
/// Red source). Registered as a separate native `load-from-file` isn't
/// needed — the existing `load` native (in `natives.rs`) handles strings;
/// this variant is wired in by re-registering `load` with the file-aware
/// impl. The string case delegates to `load_source`.
fn load_extended(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "load", 1, args.len()));
    }
    match &args[0] {
        Value::String { s, span } => {
            let body = load_source(s).map_err(|e| EvalError::Native {
                message: e.to_string(),
                span: *span,
            })?;
            Ok(Value::block(body))
        }
        Value::File { path, span } => {
            let p = resolve_path(path, _env);
            let contents =
                std::fs::read_to_string(&p).map_err(|e| io_err(&args[0], &p, "cannot load", e))?;
            // The loaded file's contents are a separate source buffer from
            // the script that called `load`. A lex/parse error inside the
            // loaded file refers to byte offsets in *that* buffer, not the
            // caller's — so we translate the inner error's span into a
            // `line:col` within the loaded file and fold it into the message
            // body. The outer span points at the `load %file` call site (the
            // `file!` literal), so the user can navigate to the call too.
            let body = load_source(&contents).map_err(|e| {
                let inner_span = e.span();
                let loc = if let Some(sp) = inner_span {
                    if sp.is_default() {
                        String::new()
                    } else {
                        let map = red_core::LineMap::new(&contents);
                        let (line, col) = map.line_col(sp.start);
                        format!(" at {}:{}:{} ", p.display(), line, col)
                    }
                } else {
                    String::new()
                };
                EvalError::Native {
                    message: format!("load{}: {}", loc, e),
                    span: *span,
                }
            })?;
            Ok(Value::block(body))
        }
        other => Err(EvalError::TypeError {
            expected: "string! or file!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

// ---------------------------------------------------------------------------
// File metadata
// ---------------------------------------------------------------------------

/// `exists? file` → `logic!`.
fn exists_q(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "exists?", 1, args.len()));
    }
    let (path, _span) = expect_file(&args[0])?;
    let p = resolve_path(path, env);
    Ok(Value::Logic(p.exists()))
}

/// `size? file` → `integer!` (bytes).
fn size_q(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "size?", 1, args.len()));
    }
    let (path, _span) = expect_file(&args[0])?;
    let p = resolve_path(path, env);
    let meta = std::fs::metadata(&p).map_err(|e| io_err(&args[0], &p, "cannot stat", e))?;
    Ok(Value::integer(meta.len() as i64))
}

/// `modified? file` → `date!` (M45). Returns the file's mtime as a
/// timezone-aware `date!` (local time + local UTC offset). Uses
/// `chrono::DateTime::<Local>::from(mtime)`.
fn modified_q(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "modified?", 1, args.len()));
    }
    let (path, _span) = expect_file(&args[0])?;
    let p = resolve_path(path, env);
    let meta = std::fs::metadata(&p).map_err(|e| io_err(&args[0], &p, "cannot stat", e))?;
    let mtime = meta
        .modified()
        .map_err(|e| io_err(&args[0], &p, "cannot read mtime", e))?;
    let dv = red_core::DateValue::from_system_time_local(mtime);
    Ok(Value::date(dv))
}

/// `now` → `date!` (M45). Returns the current local time with the system's
/// local UTC offset attached.
fn now_native(_args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::date(red_core::DateValue::now_local()))
}

/// `today` → `date!` (M45). Returns date-only at local midnight, `zone: None`.
fn today_native(_args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    Ok(Value::date(red_core::DateValue::today_local()))
}

/// `to-utc date` → `date!` (M45). Shifts the wall-clock `dt` by `-zone`
/// minutes (so the wall clock shows the UTC time), then sets `zone = Some(0)`.
/// On a zone-naive date, this is a no-op (zone is already treated as UTC for
/// arithmetic).
fn to_utc_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "to-utc", 1, args.len()));
    }
    match &args[0] {
        Value::Date { dt, .. } => Ok(Value::date(dt.to_utc())),
        other => Err(EvalError::TypeError {
            expected: "date!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Directory ops
// ---------------------------------------------------------------------------

/// `dir? file` → `logic!` (true if the path is a directory).
fn dir_q(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "dir?", 1, args.len()));
    }
    let (path, _span) = expect_file(&args[0])?;
    let p = resolve_path(path, env);
    Ok(Value::Logic(p.is_dir()))
}

/// `make-dir file` → `none!`. Creates the directory (and parents).
fn make_dir(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "make-dir", 1, args.len()));
    }
    let (path, _span) = expect_file(&args[0])?;
    let p = resolve_path(path, env);
    std::fs::create_dir_all(&p).map_err(|e| io_err(&args[0], &p, "cannot make-dir", e))?;
    Ok(Value::None)
}

/// `delete file` → `none!`. Removes a file or directory (recursively for
/// directories).
fn delete(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "delete", 1, args.len()));
    }
    let (path, _span) = expect_file(&args[0])?;
    let p = resolve_path(path, env);
    if p.is_dir() {
        std::fs::remove_dir_all(&p).map_err(|e| io_err(&args[0], &p, "cannot delete", e))?;
    } else {
        std::fs::remove_file(&p).map_err(|e| io_err(&args[0], &p, "cannot delete", e))?;
    }
    Ok(Value::None)
}

/// `rename from to` → `none!`. Renames/moves a file or directory.
fn rename(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "rename", 2, args.len()));
    }
    let (from_path, _span) = expect_file(&args[0])?;
    let (to_path, _span) = expect_file(&args[1])?;
    let from = resolve_path(from_path, env);
    let to = resolve_path(to_path, env);
    std::fs::rename(&from, &to).map_err(|e| {
        native_err(
            &args[0],
            format!(
                "cannot rename {from} to {to}: {e}",
                from = from.display(),
                to = to.display()
            ),
        )
    })?;
    Ok(Value::None)
}

/// `change-dir file` → `none!`. Updates `env.cwd` and the `system/options/path`
/// slot if present.
fn change_dir(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "change-dir", 1, args.len()));
    }
    let (path, _span) = expect_file(&args[0])?;
    let p = resolve_path(path, env);
    if !p.is_dir() {
        return Err(native_err(
            &args[0],
            format!(
                "change-dir: not a directory: {display}",
                display = p.display()
            ),
        ));
    }
    env.cwd = p.clone();
    // Mirror into system/options/path if the slot exists.
    update_system_path(env, &p);
    Ok(Value::None)
}

/// `what-dir` → `file!`. Returns the current working directory as a file!
/// path string.
fn what_dir(_args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    let s = env.cwd.to_string_lossy().to_string();
    Ok(Value::file(Rc::from(s.as_str())))
}

// ---------------------------------------------------------------------------
// Environment variables
// ---------------------------------------------------------------------------

/// `get-env name` → `string!` or `none!` (if unset). `name` may be a string
/// or a word (its name is used).
fn get_env(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "get-env", 1, args.len()));
    }
    let name = env_name(&args[0])?;
    match std::env::var(&name) {
        Ok(val) => Ok(Value::string(Rc::from(val.as_str()))),
        Err(_) => Ok(Value::None),
    }
}

/// `set-env name value` → `none!`. `value` coerced to string via `form`.
fn set_env(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(arity_err(args, "set-env", 2, args.len()));
    }
    let name = env_name(&args[0])?;
    let val = match &args[1] {
        Value::String { s, .. } => s.to_string(),
        Value::None => String::new(),
        _ => red_core::form_to_string(&args[1]),
    };
    std::env::set_var(&name, &val);
    Ok(Value::None)
}

/// `env` → `block!` of `"KEY=value"` strings for every environment variable.
fn env_native(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if !args.is_empty() {
        return Err(arity_err(args, "env", 0, args.len()));
    }
    let entries: Vec<Value> = std::env::vars()
        .map(|(k, v)| {
            let s = format!("{k}={v}");
            Value::string(Rc::from(s.as_str()))
        })
        .collect();
    Ok(Value::block(Series::new(entries)))
}

/// Extract an env-var name from a string or word value.
fn env_name(v: &Value) -> Result<String, EvalError> {
    match v {
        Value::String { s, .. } => Ok(s.to_string()),
        Value::Word { sym, .. }
        | Value::SetWord { sym, .. }
        | Value::GetWord { sym, .. }
        | Value::LitWord { sym, .. } => Ok(sym.as_str().to_string()),
        other => Err(EvalError::TypeError {
            expected: "string! or word!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

// ---------------------------------------------------------------------------
// wait
// ---------------------------------------------------------------------------

/// `wait seconds` → `none!`. Sleeps the current thread for `seconds` (int or
/// float). Sub-second precision via float.
fn wait(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "wait", 1, args.len()));
    }
    let secs = match &args[0] {
        Value::Integer { n, .. } => *n as f64,
        Value::Float { f, .. } => *f,
        other => {
            return Err(EvalError::TypeError {
                expected: "integer! or float!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    if secs > 0.0 {
        std::thread::sleep(Duration::from_secs_f64(secs));
    }
    Ok(Value::None)
}

// ---------------------------------------------------------------------------
// call / shell (gated)
// ---------------------------------------------------------------------------

/// `call command` / `shell command` — run an external command. Gated behind
/// `env.allow_shell`: raises `EvalError::Native` if disabled. `command` is a
/// string; it's split on whitespace into program + args (POC: no quoting).
/// Returns the command's exit code as an `integer!`.
fn call(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "call", 1, args.len()));
    }
    if !env.allow_shell {
        return Err(native_err(
            &args[0],
            "call: shell disabled (use --allow-shell to enable)",
        ));
    }
    let cmd_str = match &args[0] {
        Value::String { s, .. } => s.to_string(),
        other => {
            return Err(EvalError::TypeError {
                expected: "string!",
                found: type_name(other),
                span: other.span_or_default(),
            });
        }
    };
    let mut parts = cmd_str.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| native_err(&args[0], "call: empty command".to_string()))?;
    let status = std::process::Command::new(program)
        .args(parts)
        .status()
        .map_err(|e| native_err(&args[0], format!("call: {e}")))?;
    Ok(Value::integer(status.code().unwrap_or(-1) as i64))
}

/// `shell command` — alias for `call`. Kept as a separate native so Red
/// scripts can distinguish intent; behavior is identical.
fn shell(args: &[Value], refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if !env.allow_shell {
        return Err(native_err(
            &args[0],
            "shell: shell disabled (use --allow-shell to enable)",
        ));
    }
    call(args, refs, env)
}

// ---------------------------------------------------------------------------
// system/options mirroring
// ---------------------------------------------------------------------------

/// Update the `path` slot of `system/options` (if the `system` object exists
/// in the user context) to reflect a cwd change. Best-effort: silently
/// no-ops if the slots aren't present (e.g. in tests that don't install
/// `system`).
fn update_system_path(env: &Env, cwd: &Path) {
    let sys = match env.user_ctx.get(&Symbol::new("system")) {
        Some(Value::Object(obj)) => obj,
        _ => return,
    };
    let sys = sys.borrow();
    if let Some(Value::Object(opts)) = sys.ctx.get(&Symbol::new("options")) {
        let path_str = cwd.to_string_lossy().to_string();
        opts.borrow().ctx.set(
            Symbol::new("path"),
            Value::file(Rc::from(path_str.as_str())),
        );
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn register_io_natives(env: &mut Env) {
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

    // read / write / save — refinement-bearing.
    reg_refined(env, "read", read as NF, 1, &[("lines", 0), ("binary", 0)]);
    reg_refined(
        env,
        "write",
        write as NF,
        2,
        &[("append", 0), ("lines", 0), ("binary", 0)],
    );
    reg(env, "save", save as NF, 2);

    // `load` — re-register with the file-aware impl (overrides the M9
    // string-only `load` from `natives.rs`).
    reg(env, "load", load_extended as NF, 1);

    // File metadata.
    reg(env, "exists?", exists_q as NF, 1);
    reg(env, "size?", size_q as NF, 1);
    reg(env, "modified?", modified_q as NF, 1);

    // M45: date/time natives.
    reg(env, "now", now_native as NF, 0);
    reg(env, "today", today_native as NF, 0);
    reg(env, "to-utc", to_utc_native as NF, 1);

    // Directory ops.
    reg(env, "dir?", dir_q as NF, 1);
    reg(env, "make-dir", make_dir as NF, 1);
    reg(env, "delete", delete as NF, 1);
    reg(env, "rename", rename as NF, 2);
    reg(env, "change-dir", change_dir as NF, 1);
    reg(env, "what-dir", what_dir as NF, 0);

    // Environment variables.
    reg(env, "get-env", get_env as NF, 1);
    reg(env, "set-env", set_env as NF, 2);
    reg(env, "env", env_native as NF, 0);

    // wait.
    reg(env, "wait", wait as NF, 1);

    // call / shell (gated).
    reg(env, "call", call as NF, 1);
    reg(env, "shell", shell as NF, 1);

    // Predicates (file! / url!).
    reg(env, "file?", file_q as NF, 1);
    reg(env, "url?", url_q as NF, 1);
}

/// `file? value` → `logic!`.
fn file_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "file?", 1, args.len()));
    }
    Ok(Value::Logic(matches!(args[0], Value::File { .. })))
}

/// `url? value` → `logic!`.
fn url_q(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(arity_err(args, "url?", 1, args.len()));
    }
    Ok(Value::Logic(matches!(args[0], Value::Url { .. })))
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
    use std::cell::RefCell;
    use std::io::Write;

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

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    #[test]
    fn read_file_returns_contents() {
        let f = fixture_dir().join("hello.txt");
        let src = format!("read %{}", f.display());
        let v = val(&src);
        match v {
            Value::String { s, .. } => assert_eq!(s.as_ref(), "hello world\n"),
            other => panic!("expected string, got {:?}", other),
        }
    }

    #[test]
    fn read_lines_returns_block() {
        let f = fixture_dir().join("lines.txt");
        let src = format!("read/lines %{}", f.display());
        let v = val(&src);
        match v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                assert_eq!(data.len(), 3);
                assert_eq!(mold_to_string(&data[0]), "\"one\"");
                assert_eq!(mold_to_string(&data[1]), "\"two\"");
                assert_eq!(mold_to_string(&data[2]), "\"three\"");
            }
            other => panic!("expected block, got {:?}", other),
        }
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        let pstr = path.to_string_lossy().to_string();
        let write_src = format!("write %{} \"abc\"", pstr);
        let read_src = format!("read %{}", pstr);
        run_capture_val(&write_src).unwrap();
        let v = val(&read_src);
        match v {
            Value::String { s, .. } => assert_eq!(s.as_ref(), "abc"),
            other => panic!("expected string, got {:?}", other),
        }
    }

    #[test]
    fn write_append_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.txt");
        let pstr = path.to_string_lossy().to_string();
        run_capture_val(&format!("write %{} \"a\"", pstr)).unwrap();
        run_capture_val(&format!("write/append %{} \"b\"", pstr)).unwrap();
        let v = val(&format!("read %{}", pstr));
        match v {
            Value::String { s, .. } => assert_eq!(s.as_ref(), "ab"),
            other => panic!("expected string, got {:?}", other),
        }
    }

    #[test]
    fn write_lines_joins_with_newlines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l.txt");
        let pstr = path.to_string_lossy().to_string();
        run_capture_val(&format!("write/lines %{} [\"x\" \"y\" \"z\"]", pstr)).unwrap();
        let v = val(&format!("read %{}", pstr));
        match v {
            Value::String { s, .. } => assert_eq!(s.as_ref(), "x\ny\nz"),
            other => panic!("expected string, got {:?}", other),
        }
    }

    #[test]
    fn exists_nonexistent_is_false() {
        let v = val("exists? %/nonexistent/path/that/should/not/exist");
        assert!(matches!(v, Value::Logic(false)));
    }

    #[test]
    fn exists_existing_is_true() {
        let f = fixture_dir().join("hello.txt");
        let v = val(&format!("exists? %{}", f.display()));
        assert!(matches!(v, Value::Logic(true)));
    }

    #[test]
    fn size_of_fixture() {
        let f = fixture_dir().join("hello.txt");
        let v = val(&format!("size? %{}", f.display()));
        match v {
            Value::Integer { n, .. } => assert_eq!(n, 12), // "hello world\n"
            other => panic!("expected integer, got {:?}", other),
        }
    }

    #[test]
    fn make_dir_and_delete_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub/nested");
        let pstr = sub.to_string_lossy().to_string();
        run_capture_val(&format!("make-dir %{}", pstr)).unwrap();
        let v = val(&format!("dir? %{}", pstr));
        assert!(matches!(v, Value::Logic(true)));
        run_capture_val(&format!("delete %{}", pstr)).unwrap();
        let v = val(&format!("exists? %{}", pstr));
        assert!(matches!(v, Value::Logic(false)));
    }

    #[test]
    fn get_env_set_env_round_trip() {
        // Set then get a scratch var.
        let v = val("set-env \"REBOL_CLONE_TEST_VAR\" \"hello\" get-env \"REBOL_CLONE_TEST_VAR\"");
        match v {
            Value::String { s, .. } => assert_eq!(s.as_ref(), "hello"),
            other => panic!("expected string, got {:?}", other),
        }
        std::env::remove_var("REBOL_CLONE_TEST_VAR");
    }

    #[test]
    fn get_env_unset_returns_none() {
        let v = val("get-env \"REBOL_CLONE_DEFINITELY_UNSET_VAR_XYZ\"");
        assert!(matches!(v, Value::None));
    }

    #[test]
    fn env_returns_block_of_strings() {
        std::env::set_var("REBOL_CLONE_ENV_TEST", "v");
        let v = val("env");
        match v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                let found = data.iter().any(|e| {
                    matches!(e, Value::String { s, .. } if s.contains("REBOL_CLONE_ENV_TEST=v"))
                });
                assert!(found, "env block should contain the test var: {:?}", data);
            }
            other => panic!("expected block, got {:?}", other),
        }
        std::env::remove_var("REBOL_CLONE_ENV_TEST");
    }

    #[test]
    fn wait_zero_returns_none() {
        let v = val("wait 0");
        assert!(matches!(v, Value::None));
    }

    #[test]
    fn call_disabled_raises() {
        let err = run_capture_val("call \"echo hi\"").unwrap_err();
        assert!(err.contains("shell disabled"), "got: {err}");
    }

    #[test]
    fn shell_disabled_raises() {
        let err = run_capture_val("shell \"echo hi\"").unwrap_err();
        assert!(err.contains("shell disabled"), "got: {err}");
    }

    #[test]
    fn call_enabled_runs() {
        let body = load_source("call \"true\"").unwrap();
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        env.allow_shell = true;
        let block = Value::block(body);
        let v = eval(&block, &mut env).unwrap();
        match v {
            Value::Integer { n, .. } => assert_eq!(n, 0), // `true` exits 0
            other => panic!("expected integer, got {:?}", other),
        }
    }

    #[test]
    fn what_dir_returns_file() {
        let v = val("what-dir");
        match v {
            Value::File { path, .. } => {
                assert!(!path.is_empty());
            }
            other => panic!("expected file, got {:?}", other),
        }
    }

    #[test]
    fn change_dir_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let pstr = dir.path().to_string_lossy().to_string();
        // change-dir + what-dir in ONE script (state doesn't persist across
        // separate run_capture_val calls — each builds a fresh env).
        let v = val(&format!("change-dir %{} what-dir", pstr));
        match v {
            Value::File { path, .. } => {
                assert!(path.as_ref().starts_with(&*pstr), "got {path}");
            }
            other => panic!("expected file, got {:?}", other),
        }
    }

    #[test]
    fn read_nonexistent_raises() {
        let err = run_capture_val("read %/nonexistent/file/xyz").unwrap_err();
        assert!(err.contains("cannot read"), "got: {err}");
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved.red");
        let pstr = path.to_string_lossy().to_string();
        run_capture_val(&format!("save %{} [1 2 3]", pstr)).unwrap();
        let v = val(&format!("load %{}", pstr));
        // `load` returns a block wrapping the parsed body; for source
        // `[1 2 3]` the body is a single Block value, so the outer block
        // has one element which is the inner `[1 2 3]` block.
        match v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                assert_eq!(data.len(), 1);
                match &data[0] {
                    Value::Block { series: inner, .. } => {
                        let inner = inner.data.borrow();
                        assert_eq!(inner.len(), 3);
                        assert_eq!(mold_to_string(&inner[0]), "1");
                        assert_eq!(mold_to_string(&inner[1]), "2");
                        assert_eq!(mold_to_string(&inner[2]), "3");
                    }
                    other => panic!("expected inner block, got {:?}", other),
                }
            }
            other => panic!("expected block, got {:?}", other),
        }
    }

    #[test]
    fn read_url_wrong_scheme_errors() {
        // M113: `read url!` is now gated by `env.allow_network`. The scheme
        // check (UnsupportedInV09) is only reached when the gate is open;
        // with the gate closed, the gate message fires first. This test
        // exercises the gate-closed path (no real network call attempted).
        let err = run_capture_val("read file://localhost/x").unwrap_err();
        assert!(
            err.contains("network disabled") || err.contains("not supported"),
            "got: {err}"
        );
    }

    #[test]
    fn read_url_wrong_scheme_errors_when_network_allowed() {
        // M113: with `allow_network = true`, the scheme check fires and
        // surfaces the `UnsupportedInV09` message for a `file://` URL.
        let body = load_source("read file://localhost/x").unwrap();
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        env.allow_network = true;
        let block = Value::block(body);
        let err = eval(&block, &mut env).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not supported") || msg.contains("bad or unrecognized"),
            "got: {msg}"
        );
    }

    // --- M41: read/binary + write/binary ---

    #[test]
    fn read_binary_returns_binary_value() {
        // Fixture file `hello.txt` contains "hello world\n".
        let v = val("read/binary %tests/fixtures/hello.txt");
        match v {
            Value::String8 { bytes, .. } => {
                assert_eq!(bytes, b"hello world\n".to_vec());
            }
            other => panic!("expected String8, got {:?}", other),
        }
    }

    #[test]
    fn write_binary_then_read_binary_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin.dat");
        let pstr = path.to_string_lossy().to_string();
        // Write raw bytes via a binary! literal (`48656C6C6F` = "Hello").
        // (Escape `{{`/`}}` for Rust's format! macro so the `#{...}` reaches
        // Red intact.)
        run_capture_val(&format!("write/binary %{} #{{48656C6C6F}}", pstr)).unwrap();
        let v = val(&format!("read/binary %{}", pstr));
        match v {
            Value::String8 { bytes, .. } => {
                assert_eq!(bytes, b"Hello".to_vec());
            }
            other => panic!("expected String8, got {:?}", other),
        }
    }

    #[test]
    fn write_binary_append_appends_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.bin");
        let pstr = path.to_string_lossy().to_string();
        run_capture_val(&format!("write/binary %{} #{{41}}", pstr)).unwrap();
        run_capture_val(&format!("write/append/binary %{} #{{42}}", pstr)).unwrap();
        let v = val(&format!("read/binary %{}", pstr));
        match v {
            Value::String8 { bytes, .. } => {
                assert_eq!(bytes, vec![0x41, 0x42]);
            }
            other => panic!("expected String8, got {:?}", other),
        }
    }

    #[test]
    fn write_binary_from_string_writes_utf8_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.bin");
        let pstr = path.to_string_lossy().to_string();
        run_capture_val(&format!("write/binary %{} \"hi\"", pstr)).unwrap();
        let v = val(&format!("read/binary %{}", pstr));
        match v {
            Value::String8 { bytes, .. } => {
                assert_eq!(bytes, b"hi".to_vec());
            }
            other => panic!("expected String8, got {:?}", other),
        }
    }

    #[test]
    fn write_binary_with_non_binary_value_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.bin");
        let pstr = path.to_string_lossy().to_string();
        let r = run_capture_val(&format!("write/binary %{} 42", pstr));
        assert!(r.is_err(), "expected type error");
    }

    #[test]
    fn read_binary_and_binary_lines_mutually_exclusive() {
        let r = run_capture_val("read/binary/lines %tests/fixtures/hello.txt");
        assert!(r.is_err(), "expected error from /binary + /lines");
    }
}
