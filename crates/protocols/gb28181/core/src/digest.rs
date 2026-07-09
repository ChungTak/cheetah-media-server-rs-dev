//! Lenient SIP digest authentication parsing and response generation.
//!
//! Mirrors the behaviour of `vendor-ref/ABLMediaServer-src-2026-05-09/.../DigestAuthentication.*`:
//! we tolerate parameters separated by either `,` or `;`, parameter values that are or are not
//! quoted, varying whitespace, and case differences in scheme/key names. The functions are
//! designed for both the server side (parsing `Authorization:` headers) and the client side
//! (parsing `WWW-Authenticate:` challenges).

use std::collections::HashMap;

use crate::error::Gb28181CoreError;

/// Parsed Digest challenge or response. All fields are optional because real-world devices
/// frequently omit RFC-mandated parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DigestParams {
    pub realm: Option<String>,
    pub nonce: Option<String>,
    pub username: Option<String>,
    pub uri: Option<String>,
    pub response: Option<String>,
    pub algorithm: Option<String>,
    pub opaque: Option<String>,
    pub qop: Option<String>,
    pub nc: Option<String>,
    pub cnonce: Option<String>,
    /// Any additional unrecognised parameters, preserved verbatim for diagnostics.
    pub extra: HashMap<String, String>,
}

impl DigestParams {
    /// Parse the value of a `WWW-Authenticate:` or `Authorization:` header.
    ///
    /// The leading scheme name (`Digest`) is optional; some buggy peers omit it. Parameters may
    /// be separated by `,` or `;`; values may be quoted or bare. All keys are lowercased before
    /// matching.
    pub fn parse(input: &str) -> Result<Self, Gb28181CoreError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(Gb28181CoreError::SipSyntax(
                "empty digest header value".to_string(),
            ));
        }

        // Optional `Digest ` prefix (case-insensitive). We compare via a byte slice so that
        // arbitrary UTF-8 input cannot cause `&trimmed[..6]` to panic at a non-char boundary.
        let body = match trimmed.as_bytes().get(..6) {
            Some(prefix) if prefix.eq_ignore_ascii_case(b"digest") => trimmed[6..].trim_start(),
            _ => trimmed,
        };

        let mut params = DigestParams::default();
        for raw_kv in split_digest_params(body) {
            let raw_kv = raw_kv.trim();
            if raw_kv.is_empty() {
                continue;
            }
            let Some((k, v)) = raw_kv.split_once('=') else {
                continue;
            };
            let key = k.trim().to_ascii_lowercase();
            let value = unquote(v.trim());
            match key.as_str() {
                "realm" => params.realm = Some(value),
                "nonce" => params.nonce = Some(value),
                "username" => params.username = Some(value),
                "uri" => params.uri = Some(value),
                "response" => params.response = Some(value),
                "algorithm" => params.algorithm = Some(value),
                "opaque" => params.opaque = Some(value),
                "qop" => params.qop = Some(value),
                "nc" => params.nc = Some(value),
                "cnonce" => params.cnonce = Some(value),
                _ => {
                    params.extra.insert(key, value);
                }
            }
        }

        Ok(params)
    }
}

/// Split a digest parameter list on `,` or `;`, respecting double-quoted values. Quoted
/// commas/semicolons are kept as part of the value.
fn split_digest_params(input: &str) -> Vec<&str> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_quotes = false;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_quotes = !in_quotes,
            b',' | b';' if !in_quotes => {
                if i > start {
                    out.push(&input[start..i]);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < bytes.len() {
        out.push(&input[start..]);
    }
    out
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Compute an MD5 digest response for a SIP `REGISTER` according to RFC 2617.
///
/// `qop` defaults to none when not present; when present we use `qop=auth` semantics with the
/// caller-supplied `nc` and `cnonce`. ABL devices in practice rarely advertise QOP so the
/// no-QOP path is the most common.
#[allow(clippy::too_many_arguments)]
pub fn compute_md5_response(
    username: &str,
    realm: &str,
    password: &str,
    method: &str,
    uri: &str,
    nonce: &str,
    qop: Option<&str>,
    nc: Option<&str>,
    cnonce: Option<&str>,
) -> String {
    let ha1 = md5_hex(format!("{username}:{realm}:{password}").as_bytes());
    let ha2 = md5_hex(format!("{method}:{uri}").as_bytes());
    if let (Some(qop), Some(nc), Some(cnonce)) = (qop, nc, cnonce) {
        md5_hex(format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}").as_bytes())
    } else {
        md5_hex(format!("{ha1}:{nonce}:{ha2}").as_bytes())
    }
}

fn md5_hex(input: &[u8]) -> String {
    let digest = md5::compute(input);
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_challenge() {
        let raw = r#"Digest realm="3402000000", nonce="abcd1234", algorithm=MD5, qop="auth""#;
        let parsed = DigestParams::parse(raw).unwrap();
        assert_eq!(parsed.realm.as_deref(), Some("3402000000"));
        assert_eq!(parsed.nonce.as_deref(), Some("abcd1234"));
        assert_eq!(parsed.algorithm.as_deref(), Some("MD5"));
        assert_eq!(parsed.qop.as_deref(), Some("auth"));
    }

    #[test]
    fn parses_semicolon_separated_response_without_scheme() {
        // Some vendors emit `;` separators and omit the `Digest ` scheme prefix.
        let raw = r#"username="34020000001320000001"; realm="3402000000"; nonce="x"; uri="sip:34020000002000000001@3402000000"; response="deadbeef""#;
        let parsed = DigestParams::parse(raw).unwrap();
        assert_eq!(parsed.username.as_deref(), Some("34020000001320000001"));
        assert_eq!(parsed.realm.as_deref(), Some("3402000000"));
        assert_eq!(parsed.nonce.as_deref(), Some("x"));
        assert_eq!(parsed.response.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn unquoted_values_round_trip() {
        let raw = "Digest realm=cheetah, nonce=abcd, algorithm=MD5";
        let parsed = DigestParams::parse(raw).unwrap();
        assert_eq!(parsed.realm.as_deref(), Some("cheetah"));
        assert_eq!(parsed.nonce.as_deref(), Some("abcd"));
        assert_eq!(parsed.algorithm.as_deref(), Some("MD5"));
    }

    #[test]
    fn quoted_commas_stay_in_value() {
        let raw = r#"Digest realm="cheetah,inc", nonce="x""#;
        let parsed = DigestParams::parse(raw).unwrap();
        assert_eq!(parsed.realm.as_deref(), Some("cheetah,inc"));
    }

    #[test]
    fn computes_response_no_qop_known_vector() {
        // RFC 2617 sample (Authorization for "Digest realm=...").
        let response = compute_md5_response(
            "Mufasa",
            "testrealm@host.com",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            "dcd98b7102dd2f0e8b11d0f600bfb0c093",
            None,
            None,
            None,
        );
        // Pre-computed reference value (RFC 2617 §3.5 example).
        assert_eq!(response, "670fd8c2df070c60b045671b8b24ff02");
    }
}
