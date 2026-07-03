//! M113: protocol-scheme detection. The `PortScheme` enum itself lives in
//! `red-core::value` (so the printer/`type_name` can name variants without a
//! red-eval dependency); this module holds the helpers that classify a
//! `url!`/`file!` argument into a `PortScheme` and gate unsupported schemes
//! with `NetError::UnsupportedInV09`.

use red_core::value::PortScheme;

use crate::net::error::{NetError, NetResult};

/// Classify a `url!` string (e.g. `http://host/path`, `https://host/path`,
/// `ftp://host/...`) into a `PortScheme`. The scheme is the substring before
/// the first `://` (case-insensitive). Returns `Err(NetError::BadScheme)`
/// when no `://` is present.
pub fn scheme_of_url(url: &str) -> NetResult<PortScheme> {
    let raw = url
        .split("://")
        .next()
        .ok_or_else(|| NetError::BadScheme(url.to_string()))?;
    let s = raw.to_ascii_lowercase();
    let scheme = match s.as_str() {
        "http" | "https" => PortScheme::Http,
        "file" => PortScheme::File,
        "ftp" | "ftps" => PortScheme::Ftp,
        "smtp" => PortScheme::Smtp,
        "pop3" => PortScheme::Pop3,
        "nntp" => PortScheme::Nntp,
        "dns" => PortScheme::Dns,
        "tcp" => PortScheme::Tcp,
        "udp" => PortScheme::Udp,
        "whois" => PortScheme::Whois,
        "finger" => PortScheme::Finger,
        "daytime" => PortScheme::Daytime,
        other => return Err(NetError::BadScheme(other.to_string())),
    };
    Ok(scheme)
}

/// Verify `scheme` is one of the v0.9 live schemes (`File`/`Http`). Other
/// reserved variants return `NetError::UnsupportedInV09(scheme)`.
pub fn ensure_supported_in_v09(scheme: PortScheme) -> NetResult<()> {
    if scheme.is_supported_in_v09() {
        Ok(())
    } else {
        Err(NetError::UnsupportedInV09(scheme))
    }
}
