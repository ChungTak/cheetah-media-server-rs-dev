//! Signaling URL parsing + SSRF guard for `ws://` / `wss://` URLs.
//!
//! The HTTP client crate already validates `http(s)://` URLs. P2P
//! signaling uses WebSocket schemes (`ws` / `wss`) so we duplicate the
//! parser surface here rather than expose a generic URL type from
//! `http_client`. Keeping the parser local also makes it easy to add
//! SSRF policy specific to the signaling path without surprising the
//! WHIP/WHEP HTTP client.
//!
//! Behaviour matches the architecture document:
//!
//! * Default-deny private / loopback / link-local destinations.
//! * `allow_private_ips: true` opt-in for local development.
//! * Hostname allowlist checked before DNS so testing environments can
//!   call `localhost`-style names without disabling private-IP
//!   blocking globally.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use thiserror::Error;

/// Hard cap on a signaling URL string. Mirrors the WHIP/WHEP HTTP
/// client; no real WebSocket signaling URL approaches this size.
pub const SIGNALING_URL_MAX_BYTES: usize = 2048;

/// Parsed `ws://` / `wss://` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalingUrl {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl SignalingUrl {
    /// Render back to a canonical string (used in artifacts and logs).
    pub fn render(&self) -> String {
        let scheme = if self.secure { "wss" } else { "ws" };
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            // IPv6 literal needs brackets when concatenated with port.
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let path = if self.path.is_empty() {
            "/".to_string()
        } else {
            self.path.clone()
        };
        format!("{scheme}://{host}:{port}{path}", port = self.port)
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SignalingUrlError {
    #[error("signaling url exceeds {max} bytes")]
    TooLong { max: usize },
    #[error("missing scheme; expected ws:// or wss://")]
    MissingScheme,
    #[error("invalid scheme `{0}`; expected ws or wss")]
    InvalidScheme(String),
    #[error("missing authority")]
    MissingAuthority,
    #[error("invalid authority `{0}`")]
    InvalidAuthority(String),
    #[error("invalid port `{value}`: {reason}")]
    InvalidPort { value: String, reason: String },
    #[error("destination blocked: {0}")]
    Blocked(String),
}

/// SSRF guard configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalingUrlPolicy {
    /// When `true`, private/loopback/link-local IPs and the literal
    /// `localhost` hostname are allowed. Tests / dev environments
    /// turn this on; production keepers leave it `false`.
    pub allow_private_ips: bool,
    /// Optional explicit host allowlist. Hostnames listed here pass
    /// through even when `allow_private_ips = false`. Useful for
    /// CI runners that resolve to RFC1918 addresses.
    pub host_allowlist: Vec<String>,
    /// Hard cap on the URL length.
    pub max_url_bytes: usize,
}

impl Default for SignalingUrlPolicy {
    fn default() -> Self {
        Self {
            allow_private_ips: false,
            host_allowlist: Vec::new(),
            max_url_bytes: SIGNALING_URL_MAX_BYTES,
        }
    }
}

/// Parse and SSRF-validate a signaling URL.
///
/// SSRF validation only walks the *literal* host. Hostnames are not
/// resolved here — DNS resolution happens in the transport layer,
/// which re-checks each resolved IP against the same private-IP
/// policy. This keeps the parser pure / fast / testable.
pub fn parse(input: &str, policy: &SignalingUrlPolicy) -> Result<SignalingUrl, SignalingUrlError> {
    if input.len() > policy.max_url_bytes {
        return Err(SignalingUrlError::TooLong {
            max: policy.max_url_bytes,
        });
    }
    let (secure, rest) = if let Some(rest) = input.strip_prefix("wss://") {
        (true, rest)
    } else if let Some(rest) = input.strip_prefix("ws://") {
        (false, rest)
    } else if let Some((scheme, _)) = input.split_once("://") {
        return Err(SignalingUrlError::InvalidScheme(scheme.to_string()));
    } else {
        return Err(SignalingUrlError::MissingScheme);
    };
    let (authority, path) = match rest.split_once('/') {
        Some((auth, path)) => (auth, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    if authority.is_empty() {
        return Err(SignalingUrlError::MissingAuthority);
    }
    let (host, port) = parse_authority(authority, secure)?;

    // Plain-host policy check.
    let host_lower = host.to_ascii_lowercase();
    let allowlisted = policy
        .host_allowlist
        .iter()
        .any(|h| h.eq_ignore_ascii_case(&host_lower));

    if !allowlisted && !policy.allow_private_ips {
        // Reject literal localhost.
        if host_lower == "localhost" {
            return Err(SignalingUrlError::Blocked(format!(
                "host `{host}` resolves to loopback; set allow_private_ips or add to allowlist"
            )));
        }
        // If host parses as an IP, run the same private-IP check
        // used by the HTTP client.
        if let Ok(ip) = host.parse::<IpAddr>() {
            if is_private_ip(ip) {
                return Err(SignalingUrlError::Blocked(format!(
                    "host `{host}` is private/loopback/link-local"
                )));
            }
        }
    }

    Ok(SignalingUrl {
        secure,
        host,
        port,
        path,
    })
}

fn parse_authority(input: &str, secure: bool) -> Result<(String, u16), SignalingUrlError> {
    let default_port = if secure { 443 } else { 80 };
    if let Some(rest) = input.strip_prefix('[') {
        // IPv6 literal.
        let (literal, after) = rest
            .split_once(']')
            .ok_or_else(|| SignalingUrlError::InvalidAuthority(input.to_string()))?;
        let port = if after.is_empty() {
            default_port
        } else if let Some(p) = after.strip_prefix(':') {
            p.parse::<u16>()
                .map_err(|e| SignalingUrlError::InvalidPort {
                    value: p.to_string(),
                    reason: e.to_string(),
                })?
        } else {
            return Err(SignalingUrlError::InvalidAuthority(input.to_string()));
        };
        return Ok((literal.to_string(), port));
    }
    match input.rsplit_once(':') {
        Some((host, port_str)) if !host.is_empty() => {
            let port = port_str
                .parse::<u16>()
                .map_err(|e| SignalingUrlError::InvalidPort {
                    value: port_str.to_string(),
                    reason: e.to_string(),
                })?;
            Ok((host.to_string(), port))
        }
        _ => Ok((input.to_string(), default_port)),
    }
}

/// SSRF blocklist for resolved IPs. Mirrors the HTTP-client policy so
/// the two paths agree on what counts as "private".
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_v4(v4),
        IpAddr::V6(v6) => is_private_v6(v6),
    }
}

fn is_private_v4(v4: Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_unspecified()
        || v4.is_multicast()
}

fn is_private_v6(v6: Ipv6Addr) -> bool {
    v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        // ULA fc00::/7
        || (v6.segments()[0] & 0xfe00) == 0xfc00
        // Link-local fe80::/10
        || (v6.segments()[0] & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow_private() -> SignalingUrlPolicy {
        SignalingUrlPolicy {
            allow_private_ips: true,
            ..Default::default()
        }
    }

    #[test]
    fn parses_wss_with_default_port() {
        let url = parse("wss://signaling.example.com/p2p", &Default::default()).unwrap();
        assert!(url.secure);
        assert_eq!(url.host, "signaling.example.com");
        assert_eq!(url.port, 443);
        assert_eq!(url.path, "/p2p");
    }

    #[test]
    fn parses_ws_with_explicit_port() {
        let url = parse(
            "ws://signaling.example.com:9000/p2p?x=1",
            &Default::default(),
        )
        .unwrap();
        assert!(!url.secure);
        assert_eq!(url.port, 9000);
    }

    #[test]
    fn parses_ipv6_literal() {
        let url = parse("ws://[2001:db8::1]:8080/p", &Default::default()).unwrap();
        assert_eq!(url.host, "2001:db8::1");
        assert_eq!(url.port, 8080);
    }

    #[test]
    fn ipv4_loopback_blocked_by_default() {
        let err = parse("ws://127.0.0.1:9000/p", &Default::default()).unwrap_err();
        assert!(matches!(err, SignalingUrlError::Blocked(_)));
    }

    #[test]
    fn ipv4_loopback_allowed_when_opt_in() {
        let url = parse("ws://127.0.0.1:9000/p", &allow_private()).unwrap();
        assert_eq!(url.host, "127.0.0.1");
    }

    #[test]
    fn private_ipv4_blocked_by_default() {
        for h in [
            "10.0.0.1",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.1.1",
            "224.0.0.1",
        ] {
            let url = format!("ws://{h}:9000/p");
            let err = parse(&url, &Default::default()).unwrap_err();
            assert!(
                matches!(err, SignalingUrlError::Blocked(_)),
                "{h} should be blocked"
            );
        }
    }

    #[test]
    fn ipv6_loopback_blocked_by_default() {
        let err = parse("ws://[::1]:9000/p", &Default::default()).unwrap_err();
        assert!(matches!(err, SignalingUrlError::Blocked(_)));
    }

    #[test]
    fn localhost_blocked_by_default() {
        let err = parse("ws://localhost:9000/p", &Default::default()).unwrap_err();
        assert!(matches!(err, SignalingUrlError::Blocked(_)));
    }

    #[test]
    fn allowlist_overrides_default_block() {
        let policy = SignalingUrlPolicy {
            allow_private_ips: false,
            host_allowlist: vec!["localhost".into()],
            max_url_bytes: SIGNALING_URL_MAX_BYTES,
        };
        let url = parse("ws://localhost:9000/p", &policy).unwrap();
        assert_eq!(url.host, "localhost");
    }

    #[test]
    fn rejects_invalid_scheme() {
        let err = parse("http://example.com:9000/p", &Default::default()).unwrap_err();
        assert!(matches!(err, SignalingUrlError::InvalidScheme(_)));
    }

    #[test]
    fn rejects_missing_scheme() {
        let err = parse("example.com/p", &Default::default()).unwrap_err();
        assert!(matches!(err, SignalingUrlError::MissingScheme));
    }

    #[test]
    fn rejects_missing_authority() {
        let err = parse("ws:///p", &Default::default()).unwrap_err();
        assert!(matches!(err, SignalingUrlError::MissingAuthority));
    }

    #[test]
    fn rejects_oversize_url() {
        let huge = format!("ws://e.com/{}", "x".repeat(SIGNALING_URL_MAX_BYTES));
        let err = parse(&huge, &Default::default()).unwrap_err();
        assert!(matches!(err, SignalingUrlError::TooLong { .. }));
    }

    #[test]
    fn rejects_invalid_port() {
        let err = parse("ws://e.com:notaport/p", &Default::default()).unwrap_err();
        assert!(matches!(err, SignalingUrlError::InvalidPort { .. }));
    }

    #[test]
    fn render_round_trips_basic_urls() {
        let inputs = [
            "ws://example.com:9000/p",
            "wss://signaling.example.com:443/",
            "ws://[2001:db8::1]:8080/p",
        ];
        for raw in inputs {
            let url = parse(raw, &allow_private()).unwrap();
            // Re-parse after rendering — full round trip.
            let again = parse(&url.render(), &allow_private()).unwrap();
            assert_eq!(again, url, "round trip diverged for {raw}");
        }
    }
}
