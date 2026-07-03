//! M113: uniform request model. v0.9 only uses `NetworkOptions` for the TLS
//! flag (which `ureq` infers from the URL scheme — `https://` enables TLS
//! automatically, no explicit flag needed). The full `NetworkRequest` struct
//! is reserved for v0.10+ when `write http://` (POST/PUT), headers, cookies,
//! and auth land; v0.9's `net::http::open_http` does not construct a
//! `NetworkRequest` — it calls `ureq::get(url)` directly.

/// Per-request options. Most fields are reserved for v0.10+; v0.9 only reads
/// `tls` (and that indirectly, via the URL scheme).
#[derive(Clone, Debug, Default)]
pub struct NetworkOptions {
    /// Whether TLS is in use. Inferred from the URL scheme (`https://` →
    /// true). Read-only in v0.9; a future `tls-config` field lands in v0.10+.
    pub tls: bool,
    /// Reserved v0.10+: custom request headers.
    pub headers: Vec<(String, String)>,
    /// Reserved v0.10+: redirect policy (`follow`/`none`/`limit(n)`).
    pub redirects: Option<u32>,
}

/// Reserved v0.10+: a uniform request shape for `write http://` (POST/PUT).
/// v0.9's `net::http::open_http` does not construct this — it uses
/// `ureq::get` directly.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct NetworkRequest {
    pub method: String,
    pub url: String,
    pub options: NetworkOptions,
    pub body: Option<Vec<u8>>,
}
