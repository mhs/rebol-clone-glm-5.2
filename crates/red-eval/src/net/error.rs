//! M113: network/port errors. All net errors map to `EvalError::Native`
//! (the existing io-error pattern) — they are not structured `Raised` errors
//! since the v0.9 surface is a small, synchronous subset. M114's error-
//! rendering audit may revisit this; for now the message-body prefix
//! (`port:`/`net:`/`import:`-style) is what tests assert against.

use red_core::value::PortScheme;

/// Result alias for net operations (returned by the `net::http`/`net::open`
/// helpers; converted to `EvalError::Native` at the native boundary in
/// `net/mod.rs`).
pub type NetResult<T> = Result<T, NetError>;

#[derive(Debug)]
pub enum NetError {
    /// A reserved-but-unimplemented scheme was requested (e.g. `ftp://`,
    /// `whois://`). The v0.10+ protocol surface is the home for these.
    UnsupportedInV09(PortScheme),
    /// A url! with no `://` separator, or an unrecognized scheme prefix.
    BadScheme(String),
    /// `read`/`write` on a port that has been closed (or never `open`-ed).
    Closed,
    /// `env.allow_network` is false and a networking native was invoked.
    /// Includes the offending native name (`"read"`/`"open"`/etc.) for the
    /// error message.
    NetworkDisabled(&'static str),
    /// `write` on an HTTP port — v0.9 is GET-only (`NetError::UnsupportedInV09`
    /// with a synthesized `PortScheme::Http` would mislabel the cause; this
    /// dedicated variant makes the message clear).
    HttpWriteUnsupported,
    /// A connection/transport-layer failure surfaced by `ureq`. Includes the
    /// ureq error string for the message body.
    HttpTransport(String),
    /// An HTTP status error (4xx/5xx). Includes the status code.
    HttpStatus(u16),
    /// A filesystem error from a file-port operation. Wraps the underlying
    /// `std::io::Error` + the path context for the message.
    Io(String, std::io::Error),
}

impl NetError {
    /// Render the error to a single-line message body (no `*** Error:`
    /// prefix — that's `render_error`'s job; this is just the body that gets
    /// wrapped in `EvalError::Native { message, .. }`).
    pub fn render(&self) -> String {
        match self {
            NetError::UnsupportedInV09(scheme) => {
                format!(
                    "port: scheme {:?} not supported (only file/http in this release)",
                    scheme.as_str()
                )
            }
            NetError::BadScheme(s) => {
                format!("port: bad or unrecognized url scheme: {s:?}")
            }
            NetError::Closed => "port: operation on a closed port".to_string(),
            NetError::NetworkDisabled(native) => {
                format!("{native}: network disabled (use --allow-network to enable)")
            }
            NetError::HttpWriteUnsupported => {
                "port: write to http port not supported (GET-only)".to_string()
            }
            NetError::HttpTransport(msg) => format!("port: http transport error: {msg}"),
            NetError::HttpStatus(code) => format!("port: http request returned status {code}"),
            NetError::Io(ctx, e) => format!("port: {ctx}: {e}"),
        }
    }
}
