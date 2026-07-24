//! Outbound URL policy for snapshot fetch, proxy pull, and other server-side
//! HTTP/RTSP requests.
//!
//! 快照抓取、代理拉流等服务器侧出站 URL 策略。

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::error::MediaError;

/// Query parameter keys that commonly carry credentials or session secrets.
/// Used by `redact_url_secrets_for_debug` to avoid leaking tokens in logs.
const DEBUG_SECRET_QUERY_KEYS: &[&str] = &[
    "authorization",
    "token",
    "access_token",
    "refresh_token",
    "api_key",
    "apikey",
    "key",
    "secret",
    "signature",
    "sign",
    "auth",
    "ticket",
    "password",
    "passwd",
    "x-api-key",
    "x_zlm_secret",
    "x-zlm-secret",
    "cookie",
    "proxy-authorization",
];

fn is_secret_query_key(name: &str) -> bool {
    let lower = name.to_lowercase();
    DEBUG_SECRET_QUERY_KEYS.iter().any(|k| lower == *k)
}

/// A resolved and policy-checked outbound endpoint.
///
/// 已通过策略校验的出站端点。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedEndpoint {
    /// Original URL supplied by the caller.
    pub original_url: String,
    /// Canonical URL with userinfo removed and scheme/host/port normalized.
    pub canonical_url: String,
    /// IP addresses the host resolved to, if DNS pinning is required.
    pub resolved_ips: Vec<IpAddr>,
    pub host: String,
    pub port: u16,
    pub is_tls: bool,
}

impl std::fmt::Debug for ResolvedEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedEndpoint")
            .field(
                "original_url",
                &redact_url_secrets_for_debug(&self.original_url),
            )
            .field(
                "canonical_url",
                &redact_url_secrets_for_debug(&self.canonical_url),
            )
            .field("resolved_ips", &self.resolved_ips)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("is_tls", &self.is_tls)
            .finish()
    }
}

/// A single allow-list entry for an outbound host or CIDR.
///
/// 单个出站主机或 CIDR 允许项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AllowedDestination {
    /// Exact hostname.
    Host(String),
    /// IPv4 or IPv6 CIDR block.
    Cidr(String),
    /// All addresses matching the deployment zone label.
    Zone(String),
}

/// Outbound URL policy enforced before any server-side fetch or connect.
///
/// 服务器侧抓取或连接前强制执行的出站 URL 策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundUrlPolicy {
    pub allowed_schemes: Vec<String>,
    pub allowed_destinations: Vec<AllowedDestination>,
    pub block_private_ranges: bool,
    pub block_loopback: bool,
    pub block_link_local: bool,
    pub block_multicast: bool,
    pub block_unspecified: bool,
    pub require_dns_resolution: bool,
    pub max_redirects: u32,
    pub max_url_length: u32,
    pub require_tls: bool,
    pub deny_unknown_query_keys: Vec<String>,
}

impl Default for OutboundUrlPolicy {
    fn default() -> Self {
        Self {
            allowed_schemes: vec!["https".to_string()],
            allowed_destinations: Vec::new(),
            block_private_ranges: true,
            block_loopback: true,
            block_link_local: true,
            block_multicast: true,
            block_unspecified: true,
            require_dns_resolution: true,
            max_redirects: 3,
            max_url_length: 4096,
            require_tls: false,
            deny_unknown_query_keys: Vec::new(),
        }
    }
}

impl OutboundUrlPolicy {
    /// Sanitize a URL for storage, audit and event logging.
    ///
    /// - Rejects URLs that contain userinfo (`user:pass@`).
    /// - Removes query keys listed in `deny_unknown_query_keys`.
    /// - Strips the fragment.
    /// - Returns a canonical `scheme://host[:port]/path` form.
    ///
    /// 对 URL 进行脱敏，供存储、审计和事件日志使用。
    pub fn sanitize_url(&self, url: &str) -> Result<String, MediaError> {
        let parsed = url::Url::parse(url)
            .map_err(|e| MediaError::invalid_argument(format!("invalid URL: {e}")))?;

        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(MediaError::invalid_argument(
                "URL must not contain userinfo",
            ));
        }

        let mut cleaned = parsed.clone();
        cleaned
            .set_username("")
            .map_err(|_| MediaError::invalid_argument("cannot remove URL userinfo"))?;
        cleaned
            .set_password(None)
            .map_err(|_| MediaError::invalid_argument("cannot remove URL password"))?;
        cleaned.set_fragment(None);

        let deny: std::collections::HashSet<_> = self
            .deny_unknown_query_keys
            .iter()
            .map(|s| s.as_str())
            .collect();
        let pairs: Vec<_> = parsed
            .query_pairs()
            .filter(|(k, _)| !deny.contains(k.as_ref()))
            .collect();
        if pairs.is_empty() {
            cleaned.set_query(None);
        } else {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in pairs {
                serializer.append_pair(k.as_ref(), v.as_ref());
            }
            cleaned.set_query(Some(&serializer.finish()));
        }

        Ok(cleaned.to_string())
    }
}

/// Best-effort redaction of userinfo and known secret query keys in a URL.
///
/// Intended for `Debug` implementations so that request types containing
/// caller-supplied URLs do not leak credentials or session tokens in logs.
/// Falls back to string heuristics when the URL cannot be parsed.
pub fn redact_url_secrets_for_debug(url: &str) -> String {
    if let Ok(parsed) = url::Url::parse(url) {
        let mut cleaned = parsed.clone();
        let _ = cleaned.set_username("");
        let _ = cleaned.set_password(None);
        cleaned.set_fragment(None);

        let pairs: Vec<_> = parsed.query_pairs().collect();
        if pairs.is_empty() {
            cleaned.set_query(None);
        } else {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in pairs {
                let v_redacted = if is_secret_query_key(k.as_ref()) {
                    "<redacted>"
                } else {
                    v.as_ref()
                };
                serializer.append_pair(k.as_ref(), v_redacted);
            }
            cleaned.set_query(Some(&serializer.finish()));
        }
        return cleaned.to_string();
    }
    redact_raw_url_string(url)
}

fn redact_raw_url_string(url: &str) -> String {
    let without_userinfo = if let Some(at) = url.find('@') {
        let scheme_host_end = url[..at].find("://").map(|i| i + 3).unwrap_or(0);
        format!("{}<userinfo>@{}", &url[..scheme_host_end], &url[at + 1..])
    } else {
        url.to_string()
    };

    if let Some((path, query)) = without_userinfo.split_once('?') {
        let redacted_query = query
            .split('&')
            .map(|part| {
                if let Some((key, _value)) = part.split_once('=') {
                    if is_secret_query_key(key) {
                        return format!("{key}=<redacted>");
                    }
                }
                part.to_string()
            })
            .collect::<Vec<_>>()
            .join("&");
        return format!("{path}?{redacted_query}");
    }
    without_userinfo
}

/// Static check result returned by `OutboundUrlPolicyApi::check_static`.
///
/// `OutboundUrlPolicyApi::check_static` 返回的静态检查结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UrlPolicyVerdict {
    Allow,
    Deny,
    RequiresResolution,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_url_policy_round_trips() {
        let policy = OutboundUrlPolicy {
            allowed_schemes: vec!["https".to_string(), "rtsps".to_string()],
            allowed_destinations: vec![
                AllowedDestination::Host("example.com".to_string()),
                AllowedDestination::Cidr("203.0.113.0/24".to_string()),
            ],
            block_private_ranges: true,
            block_loopback: true,
            block_link_local: true,
            block_multicast: true,
            block_unspecified: true,
            require_dns_resolution: true,
            max_redirects: 3,
            max_url_length: 4096,
            require_tls: false,
            deny_unknown_query_keys: vec!["token".to_string()],
        };
        let json = serde_json::to_string(&policy).unwrap();
        let decoded: OutboundUrlPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, decoded);
    }

    #[test]
    fn sanitize_url_removes_fragment_and_denlisted_query_keys() {
        let policy = OutboundUrlPolicy {
            deny_unknown_query_keys: vec!["token".to_string(), "secret".to_string()],
            ..Default::default()
        };

        let sanitized = policy
            .sanitize_url("https://Example.COM:8443/path?token=abc&keep=1&secret=xyz#frag")
            .unwrap();
        assert!(sanitized.contains("example.com:8443"), "{sanitized}");
        assert!(sanitized.contains("/path"), "{sanitized}");
        assert!(sanitized.contains("keep=1"), "{sanitized}");
        assert!(!sanitized.contains("token"), "{sanitized}");
        assert!(!sanitized.contains("secret"), "{sanitized}");
        assert!(!sanitized.contains("#"), "{sanitized}");
    }

    #[test]
    fn sanitize_url_rejects_userinfo() {
        let policy = OutboundUrlPolicy::default();
        let result = policy.sanitize_url("https://user:pass@example.com/path");
        assert!(result.is_err());
    }

    #[test]
    fn redact_url_secrets_for_debug_strips_userinfo_and_secret_query_keys() {
        let url = "https://user:pass@Example.COM:8443/path?token=abc&keep=1&secret=xyz#frag";
        let redacted = redact_url_secrets_for_debug(url);
        assert!(redacted.contains("example.com:8443"), "{redacted}");
        assert!(redacted.contains("/path"), "{redacted}");
        assert!(redacted.contains("keep=1"), "{redacted}");
        assert!(!redacted.contains("user:pass"), "{redacted}");
        assert!(!redacted.contains("abc"), "{redacted}");
        assert!(!redacted.contains("xyz"), "{redacted}");
        assert!(!redacted.contains("#frag"), "{redacted}");
    }

    #[test]
    fn redact_url_secrets_for_debug_handles_unparseable_url() {
        let raw = "rtmp://user:pass@host/app?token=secret&keep=1";
        let redacted = redact_url_secrets_for_debug(raw);
        assert!(!redacted.contains("user:pass"), "{redacted}");
        assert!(!redacted.contains("secret"), "{redacted}");
        assert!(redacted.contains("keep=1"), "{redacted}");
    }
}
