//! M130 codec natives: `checksum`, `compress`/`decompress`, `enbase`/`debase`,
//! `encode`/`decode`. All operate on `string!`/`binary!` and return `string!`
//! or `binary!` per Red parity.
//!
//! Dep choices follow the `plan8` M82 precedent (smallest pure-Rust no-async
//! crate that covers the need):
//! - `crc32fast` + `sha2` for `checksum` (`'crc32`/`'sha1`/`'sha256`).
//! - `flate2` (deflate) for `compress`/`decompress`.
//! - `base64` for `enbase`/`debase` (default base 64).
//! - `encode 'url`/`decode 'url` is inline %-encoding (no dep).

use std::io::{Read, Write};
use std::rc::Rc;

use base64::Engine;
use red_core::value::{Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::natives::reg_refined;
use crate::natives::type_name;
use crate::NativeFn;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Pull the byte payload from a `string!` (UTF-8) or `binary!` argument.
fn as_bytes(v: &Value, native: &str) -> Result<Vec<u8>, EvalError> {
    match v {
        Value::String { s, .. } => Ok(s.as_bytes().to_vec()),
        Value::String8 { bytes, .. } => Ok(bytes.clone()),
        other => Err(EvalError::TypeError {
            expected: "string! or binary!",
            found: type_name(other),
            span: other.span_or_default(),
        })
        .map_err(|e| EvalError::Native {
            message: format!("{native}: {}", e),
            span: other.span_or_default(),
        }),
    }
}

/// Build a `binary!` result from raw bytes.
fn mk_binary(bytes: Vec<u8>, span: Span) -> Value {
    Value::String8 { bytes, span }
}

/// Build a `string!` result from UTF-8 bytes (lossy — codec output is
/// always valid UTF-8 for the formats in scope).
fn mk_string(s: String, span: Span) -> Value {
    Value::String {
        s: Rc::from(s.as_str()),
        span,
    }
}

fn arity_err(native: &str, expected: usize, got: usize, span: Span) -> EvalError {
    EvalError::Arity {
        native: Symbol::new(native),
        expected,
        got,
        span,
    }
}

// ---------------------------------------------------------------------------
// checksum
// ---------------------------------------------------------------------------

/// `checksum data` with `/method word` refinement. Default method is `'crc32`.
/// Supported methods: `'crc32` (→ integer!), `'sha1`, `'sha256` (→ binary!).
fn checksum(args: &[Value], refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err("checksum", 1, args.len(), Span::new(0, 0)));
    }
    let data = as_bytes(&args[0], "checksum")?;
    let span = args[0].span_or_default();

    // `/method word` — default `'crc32`.
    let method = if let Some(vals) = refs.get(&Symbol::new("method")) {
        match vals.first() {
            Some(Value::Word { sym, .. })
            | Some(Value::LitWord { sym, .. })
            | Some(Value::GetWord { sym, .. }) => sym.as_str().to_string(),
            Some(other) => {
                return Err(EvalError::Native {
                    message: format!("checksum: /method expects a word, got {}", type_name(other)),
                    span: other.span_or_default(),
                })
            }
            None => "crc32".into(),
        }
    } else {
        "crc32".into()
    };

    match method.as_str() {
        "crc32" => {
            let mut h = crc32fast::Hasher::new();
            h.update(&data);
            Ok(Value::Integer {
                n: h.finalize() as i64,
                span,
            })
        }
        "sha1" => Err(EvalError::Native {
            message: "checksum: /method 'sha1 not supported (use 'sha256 or 'crc32)".into(),
            span,
        }),
        "sha256" => {
            use sha2::Digest;
            let mut h = sha2::Sha256::new();
            h.update(&data);
            Ok(mk_binary(h.finalize().to_vec(), span))
        }
        other => Err(EvalError::Native {
            message: format!("checksum: unknown method '{other} (use 'crc32 or 'sha256)"),
            span,
        }),
    }
}

// ---------------------------------------------------------------------------
// compress / decompress
// ---------------------------------------------------------------------------

fn compress(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err("compress", 1, args.len(), Span::new(0, 0)));
    }
    let data = as_bytes(&args[0], "compress")?;
    let span = args[0].span_or_default();
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(&data).map_err(|e| EvalError::Native {
        message: format!("compress: {e}"),
        span,
    })?;
    let out = e.finish().map_err(|e| EvalError::Native {
        message: format!("compress: {e}"),
        span,
    })?;
    Ok(mk_binary(out, span))
}

fn decompress(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err("decompress", 1, args.len(), Span::new(0, 0)));
    }
    let data = as_bytes(&args[0], "decompress")?;
    let span = args[0].span_or_default();
    let mut d = flate2::read::ZlibDecoder::new(&data[..]);
    let mut out = Vec::new();
    d.read_to_end(&mut out).map_err(|e| EvalError::Native {
        message: format!("decompress: {e}"),
        span,
    })?;
    Ok(mk_binary(out, span))
}

// ---------------------------------------------------------------------------
// enbase / debase
// ---------------------------------------------------------------------------

fn enbase(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err("enbase", 1, args.len(), Span::new(0, 0)));
    }
    let data = as_bytes(&args[0], "enbase")?;
    let span = args[0].span_or_default();
    let enc = base64::engine::general_purpose::STANDARD.encode(&data);
    Ok(mk_string(enc, span))
}

fn debase(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err("debase", 1, args.len(), Span::new(0, 0)));
    }
    let data = as_bytes(&args[0], "debase")?;
    let span = args[0].span_or_default();
    let dec = base64::engine::general_purpose::STANDARD
        .decode(&data)
        .map_err(|e| EvalError::Native {
            message: format!("debase: {e}"),
            span,
        })?;
    Ok(mk_binary(dec, span))
}

// ---------------------------------------------------------------------------
// encode / decode  (v0.10: 'url only)
// ---------------------------------------------------------------------------

fn encode(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() < 2 {
        return Err(arity_err("encode", 2, args.len(), Span::new(0, 0)));
    }
    let fmt = match &args[0] {
        Value::Word { sym, .. } | Value::LitWord { sym, .. } => sym.as_str().to_string(),
        other => {
            return Err(EvalError::Native {
                message: format!("encode: format must be a word, got {}", type_name(other)),
                span: other.span_or_default(),
            })
        }
    };
    let data = as_bytes(&args[1], "encode")?;
    let span = args[1].span_or_default();
    match fmt.as_str() {
        "url" => {
            let mut out = String::with_capacity(data.len());
            for &b in &data {
                match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        out.push(b as char);
                    }
                    _ => out.push_str(&format!("%{b:02X}")),
                }
            }
            Ok(mk_string(out, span))
        }
        other => Err(EvalError::Native {
            message: format!("encode: unknown format '{other} (v0.10 supports 'url only)"),
            span,
        }),
    }
}

fn decode(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.len() < 2 {
        return Err(arity_err("decode", 2, args.len(), Span::new(0, 0)));
    }
    let fmt = match &args[0] {
        Value::Word { sym, .. } | Value::LitWord { sym, .. } => sym.as_str().to_string(),
        other => {
            return Err(EvalError::Native {
                message: format!("decode: format must be a word, got {}", type_name(other)),
                span: other.span_or_default(),
            })
        }
    };
    let data = as_bytes(&args[1], "decode")?;
    let span = args[1].span_or_default();
    match fmt.as_str() {
        "url" => {
            let mut out = Vec::with_capacity(data.len());
            let mut i = 0;
            while i < data.len() {
                if data[i] == b'%' && i + 2 < data.len() {
                    let hi = (data[i + 1] as char).to_digit(16);
                    let lo = (data[i + 2] as char).to_digit(16);
                    match (hi, lo) {
                        (Some(h), Some(l)) => {
                            out.push((h * 16 + l) as u8);
                            i += 3;
                        }
                        _ => {
                            out.push(data[i]);
                            i += 1;
                        }
                    }
                } else {
                    out.push(data[i]);
                    i += 1;
                }
            }
            Ok(mk_binary(out, span))
        }
        other => Err(EvalError::Native {
            message: format!("decode: unknown format '{other} (v0.10 supports 'url only)"),
            span,
        }),
    }
}

// ---------------------------------------------------------------------------
// registration
// ---------------------------------------------------------------------------

pub fn register_codec_natives(env: &mut Env) {
    reg_refined(env, "checksum", checksum as NativeFn, 1, &[("method", 1)]);
    reg_refined(env, "compress", compress as NativeFn, 1, &[]);
    reg_refined(env, "decompress", decompress as NativeFn, 1, &[]);
    reg_refined(env, "enbase", enbase as NativeFn, 1, &[]);
    reg_refined(env, "debase", debase as NativeFn, 1, &[]);
    reg_refined(env, "encode", encode as NativeFn, 2, &[]);
    reg_refined(env, "decode", decode as NativeFn, 2, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use red_core::context::Context;
    use red_core::value::Symbol;
    use red_core::Env;

    fn env() -> Env {
        Env::new(std::rc::Rc::new(Context::new()))
    }

    #[test]
    fn checksum_crc32() {
        let mut e = env();
        register_codec_natives(&mut e);
        let s = Value::String {
            s: Rc::from("123456789"),
            span: Span::new(0, 0),
        };
        let r = checksum(&[s], &RefineArgs::default(), &mut e).unwrap();
        assert!(matches!(
            r,
            Value::Integer {
                n: 3_421_780_262,
                ..
            }
        ));
    }

    #[test]
    fn compress_roundtrip() {
        let mut e = env();
        register_codec_natives(&mut e);
        let s = Value::String {
            s: Rc::from("hello world hello world hello world"),
            span: Span::new(0, 0),
        };
        let c = compress(std::slice::from_ref(&s), &RefineArgs::default(), &mut e).unwrap();
        let d = decompress(&[c], &RefineArgs::default(), &mut e).unwrap();
        match d {
            Value::String8 { bytes, .. } => {
                assert_eq!(bytes, b"hello world hello world hello world");
            }
            _ => panic!("expected binary!"),
        }
    }

    #[test]
    fn enbase_roundtrip() {
        let mut e = env();
        register_codec_natives(&mut e);
        let s = Value::String {
            s: Rc::from("hello"),
            span: Span::new(0, 0),
        };
        let enc = enbase(std::slice::from_ref(&s), &RefineArgs::default(), &mut e).unwrap();
        match enc {
            Value::String { s, .. } => assert_eq!(&*s, "aGVsbG8="),
            _ => panic!("expected string!"),
        }
        let dec_input = Value::String {
            s: Rc::from("aGVsbG8="),
            span: Span::new(0, 0),
        };
        let dec = debase(&[dec_input], &RefineArgs::default(), &mut e).unwrap();
        match dec {
            Value::String8 { bytes, .. } => assert_eq!(bytes, b"hello"),
            _ => panic!("expected binary!"),
        }
    }

    #[test]
    fn encode_url() {
        let mut e = env();
        register_codec_natives(&mut e);
        let fmt = Value::Word {
            sym: Symbol::new("url"),
            binding: red_core::value::Binding::Unbound,
            span: Span::new(0, 0),
        };
        let s = Value::String {
            s: Rc::from("a b"),
            span: Span::new(0, 0),
        };
        let r = encode(&[fmt, s], &RefineArgs::default(), &mut e).unwrap();
        match r {
            Value::String { s, .. } => assert_eq!(&*s, "a%20b"),
            _ => panic!("expected string!"),
        }
    }
}
