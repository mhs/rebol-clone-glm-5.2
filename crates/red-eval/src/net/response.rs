//! M113: uniform response model. v0.9 only uses `NetworkStatus` (success /
//! HTTP-error / transport-error) — the body is streamed lazily via the
//! `ureq::Response`'s `Read` handle held in `PortState::http_body`. The full
//! `NetworkResponse` struct (with materialized headers) is reserved for
//! v0.10+ when `read port` exposes response metadata to scripts.

use crate::net::error::{NetError, NetResult};

/// Materialized response status. In v0.9 `open_http` returns just the
/// `Success` arm; the `ureq::Response` body reader is held in
/// `PortState::http_body` for streaming reads. Errors map to `NetError`.
#[derive(Debug)]
pub enum NetworkStatus {
    /// 2xx response. `status` is the HTTP status code (200/201/etc.); the
    /// body reader is held separately on the `PortState`.
    Success { status: u16 },
}

/// Reserved v0.10+: a fully-materialized response (headers + body). v0.9's
/// `net::http::open_http` does not construct this — it streams the body.
#[allow(dead_code)]
#[derive(Debug)]
pub struct NetworkResponse {
    pub status: NetworkStatus,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Convert an `ureq` result into a `NetworkStatus`, mapping transport errors
/// and HTTP-error statuses into `NetError`. (v0.9: ureq follows redirects by
/// default, so a 3xx never surfaces here.)
pub fn from_ureq_result(resp: Result<ureq::Response, ureq::Error>) -> NetResult<NetworkStatus> {
    match resp {
        Ok(r) => {
            let code = r.status();
            Ok(NetworkStatus::Success { status: code })
        }
        Err(ureq::Error::Status(code, _r)) => Err(NetError::HttpStatus(code)),
        Err(e) => Err(NetError::HttpTransport(e.to_string())),
    }
}
