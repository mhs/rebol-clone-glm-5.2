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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `NetError` variant renders to a non-empty, single-line message
    /// that begins with the documented prefix (`port:`/`<native>:`). Drives
    /// all 8 match arms in `render()` — the existing in-file tests in
    /// `net/mod.rs` only exercise `Closed`, `NetworkDisabled`, and
    /// `UnsupportedInV09` indirectly (via `run_capture` substring checks on
    /// `NetworkDisabled`/`UnsupportedInV09` messages); the rest are pure
    /// formatting arms that had no direct test.
    #[test]
    fn render_every_variant() {
        // UnsupportedInV09 — covers the `scheme.as_str()` + format arm.
        for (scheme, name) in [
            (PortScheme::Ftp, "ftp"),
            (PortScheme::Smtp, "smtp"),
            (PortScheme::Pop3, "pop3"),
            (PortScheme::Nntp, "nntp"),
            (PortScheme::Dns, "dns"),
            (PortScheme::Tcp, "tcp"),
            (PortScheme::Udp, "udp"),
            (PortScheme::Whois, "whois"),
            (PortScheme::Finger, "finger"),
            (PortScheme::Daytime, "daytime"),
        ] {
            let msg = NetError::UnsupportedInV09(scheme).render();
            assert!(
                msg.contains("not supported") && msg.contains(name),
                "UnsupportedInV09({name}) render: {msg}"
            );
        }

        // BadScheme — covers the `{s:?}` formatting of the offending input.
        let msg = NetError::BadScheme("garbage://no-slash".to_string()).render();
        assert!(
            msg.contains("bad or unrecognized url scheme"),
            "BadScheme render: {msg}"
        );
        assert!(msg.contains("garbage://no-slash"));

        // Closed — the bare string arm.
        assert_eq!(
            NetError::Closed.render(),
            "port: operation on a closed port"
        );

        // NetworkDisabled — covers the `{native}` interpolation.
        assert_eq!(
            NetError::NetworkDisabled("open").render(),
            "open: network disabled (use --allow-network to enable)"
        );
        assert_eq!(
            NetError::NetworkDisabled("read").render(),
            "read: network disabled (use --allow-network to enable)"
        );

        // HttpWriteUnsupported — the bare GET-only string arm.
        assert_eq!(
            NetError::HttpWriteUnsupported.render(),
            "port: write to http port not supported (GET-only)"
        );

        // HttpTransport — covers the `{msg}` interpolation.
        let msg = NetError::HttpTransport("connection refused".to_string()).render();
        assert!(msg.contains("http transport error"), "got: {msg}");
        assert!(msg.contains("connection refused"));

        // HttpStatus — covers the `{code}` interpolation.
        let msg = NetError::HttpStatus(404).render();
        assert_eq!(msg, "port: http request returned status 404");

        // Io — covers the `{ctx}` + `{e}` interpolation. Wraps a real
        // `std::io::Error` since the arm formats it via `Display`.
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing file");
        let msg = NetError::Io("read /tmp/x".to_string(), io_err).render();
        assert!(msg.contains("port: read /tmp/x:"), "got: {msg}");
        assert!(msg.contains("missing file"));
    }
}
