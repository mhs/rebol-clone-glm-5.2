//! M113: HTTP/HTTPS GET via the existing `ureq = "2"` dep (TLS on by default
//! in ureq 2.x — no new dependency). HTTPS comes free with the default
//! features (`rustls` + `webpki-roots` are already in `Cargo.lock`).
//!
//! v0.9 scope: GET-only. `open_http` issues `ureq::get(url).call()` at
//! `open` time (so DNS/connection/HTTP errors fail fast) but holds the
//! response body `Read` handle on `PortState::http_body` so the body is
//! *not* slurped at `open` time. Subsequent `read_http` calls drain the body
//! in 8 KiB chunks (POC deviation — see M113 open question 4); an empty
//! chunk at EOF signals completion (returns an empty `string!`).
//!
//! Reserved v0.10+: POST/PUT, request headers/cookies/auth, redirect
//! control, `wait`-based async reads. A script hitting any of these gets a
//! clear `NetError::HttpWriteUnsupported` (for `write http://`) rather than
//! a silent wrong result.

use std::io::Read;

use red_core::value::PortState;

use crate::net::error::{NetError, NetResult};

/// Chunk size for a single `read port` call on an HTTP port. 8 KiB matches
/// a typical socket read window; larger bodies require multiple `read port`
/// calls (the body is *not* slurped at `open` time).
pub const HTTP_READ_CHUNK: usize = 8 * 1024;

/// Issue a `ureq::get(url).call()` and install the response body reader on
/// `state.http_body`. Called by `net::open` at port-`open` time — DNS/
/// connection/HTTP errors surface here (fail-fast), but the body is held
/// back for streaming reads via `read_http`.
///
/// `state.open` is flipped to `true` on success.
pub fn open_http(state: &mut PortState, url: &str) -> NetResult<()> {
    let resp = ureq::get(url).call();
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _r)) => return Err(NetError::HttpStatus(code)),
        Err(e) => return Err(NetError::HttpTransport(e.to_string())),
    };
    // 2xx: install the body reader on PortState — `read_http` drains it in
    // HTTP_READ_CHUNK-byte chunks. `into_reader()` consumes the response; the
    // reader is `Send`.
    state.http_body = Some(Box::new(resp.into_reader()) as Box<dyn Read + Send>);
    state.open = true;
    Ok(())
}

/// Read up to `HTTP_READ_CHUNK` bytes from an HTTP port's body reader.
/// Returns the bytes read (which may be fewer than the chunk size at EOF);
/// an empty `Vec` signals the body has been fully consumed (EOF). After EOF
/// the reader is dropped (further `read_http` calls keep returning empty).
///
/// Errors map `std::io::Error` to `NetError::HttpTransport`.
pub fn read_http(state: &mut PortState) -> NetResult<Vec<u8>> {
    let Some(reader) = state.http_body.as_mut() else {
        // No body reader: either EOF-reached (we dropped it) or the port was
        // never opened. Either way, return empty (EOF semantics) — the
        // `closed` check is the caller's responsibility.
        return Ok(Vec::new());
    };
    let mut buf = vec![0u8; HTTP_READ_CHUNK];
    let n = reader
        .read(&mut buf)
        .map_err(|e| NetError::HttpTransport(e.to_string()))?;
    if n == 0 {
        // EOF — drop the reader so future reads return empty without
        // re-attempting `read`.
        state.http_body = None;
        Ok(Vec::new())
    } else {
        buf.truncate(n);
        Ok(buf)
    }
}
