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

#[cfg(test)]
mod tests {
    use super::*;

    /// `scheme_of_url` classifies each recognized scheme prefix into the
    /// matching `PortScheme` variant. Covers all 12 match arms (http/https
    /// both route to `Http`; the 10 reserved schemes each map to their own
    /// variant). Existing in-file tests in `net/mod.rs` only exercise `http`,
    /// `ftp`, and `whois` (indirectly via `open`-with-`run_capture`); the
    /// other 9 arms had no direct test.
    #[test]
    fn scheme_of_url_recognizes_every_scheme() {
        // (url, expected scheme)
        let cases: &[(&str, PortScheme)] = &[
            ("http://example.com/", PortScheme::Http),
            ("https://example.com/", PortScheme::Http),
            ("file:///tmp/x", PortScheme::File),
            ("ftp://example.com/", PortScheme::Ftp),
            ("ftps://example.com/", PortScheme::Ftp),
            ("smtp://example.com/", PortScheme::Smtp),
            ("pop3://example.com/", PortScheme::Pop3),
            ("nntp://example.com/", PortScheme::Nntp),
            ("dns://example.com/", PortScheme::Dns),
            ("tcp://example.com/", PortScheme::Tcp),
            ("udp://example.com/", PortScheme::Udp),
            ("whois://example.com", PortScheme::Whois),
            ("finger://example.com", PortScheme::Finger),
            ("daytime://example.com", PortScheme::Daytime),
        ];
        for (url, expected) in cases {
            let got = scheme_of_url(url).expect("recognized scheme should parse");
            assert_eq!(got, *expected, "scheme_of_url({url:?})");
        }
    }

    /// `scheme_of_url` is case-insensitive on the scheme prefix (lowercased
    /// before the match). Exercises the `to_ascii_lowercase` call.
    #[test]
    fn scheme_of_url_is_case_insensitive() {
        assert_eq!(
            scheme_of_url("HTTP://Example.com/").unwrap(),
            PortScheme::Http
        );
        assert_eq!(
            scheme_of_url("HTTPS://Example.com/").unwrap(),
            PortScheme::Http
        );
        assert_eq!(
            scheme_of_url("FTP://Example.com/").unwrap(),
            PortScheme::Ftp
        );
    }

    /// Missing `://` → `NetError::BadScheme`. Exercises the
    /// `split("://").next()` + `ok_or_else` branch (the `split` iterator
    /// always yields at least one element, so this is the `None`-from-
    /// `ok_or_else` shape when no `://` is present — actually `split`
    /// returns the whole string as the sole element, so the `ok_or_else`
    /// never fires; the real "no `://`" path lands in the unknown-scheme
    /// arm below. Both are covered here.)
    #[test]
    fn scheme_of_url_no_separator_is_bad_scheme() {
        // `split("://")` on a string with no separator returns the whole
        // string as the lone element, so this hits the unknown-scheme arm
        // (which is the practical "no `://`" path).
        let err = scheme_of_url("just-a-path-no-scheme").unwrap_err();
        match err {
            NetError::BadScheme(s) => assert!(s.contains("just-a-path-no-scheme")),
            other => panic!("expected BadScheme, got {other:?}"),
        }
    }

    /// Unknown scheme prefix → `NetError::BadScheme`. Exercises the `other`
    /// arm of the match (the return-from-match path, distinct from the
    /// `ok_or_else` pre-match branch).
    #[test]
    fn scheme_of_url_unknown_scheme_is_bad_scheme() {
        let err = scheme_of_url("garbage://example.com/").unwrap_err();
        match err {
            NetError::BadScheme(s) => assert_eq!(s, "garbage"),
            other => panic!("expected BadScheme, got {other:?}"),
        }
    }

    /// `ensure_supported_in_v09` returns `Ok` for `File`/`Http` and
    /// `UnsupportedInV09` for every reserved variant. The `Ok` arm is
    /// exercised indirectly by every passing `open %file`/`open http://`
    /// test; the `Err` arm is exercised by `port_unsupported_scheme_ftp`
    /// /`port_unsupported_scheme_whois`. This test pins both arms and the
    /// full set of reserved variants directly.
    #[test]
    fn ensure_supported_in_v09_gates_reserved_schemes() {
        assert!(ensure_supported_in_v09(PortScheme::File).is_ok());
        assert!(ensure_supported_in_v09(PortScheme::Http).is_ok());
        for reserved in [
            PortScheme::Ftp,
            PortScheme::Smtp,
            PortScheme::Pop3,
            PortScheme::Nntp,
            PortScheme::Dns,
            PortScheme::Tcp,
            PortScheme::Udp,
            PortScheme::Whois,
            PortScheme::Finger,
            PortScheme::Daytime,
        ] {
            let err = ensure_supported_in_v09(reserved).unwrap_err();
            match err {
                NetError::UnsupportedInV09(s) => assert_eq!(s, reserved),
                other => panic!("expected UnsupportedInV09, got {other:?}"),
            }
        }
    }
}
