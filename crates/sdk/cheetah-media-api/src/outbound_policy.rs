//! Outbound URL policy for snapshot fetch, proxy pull, and other server-side
//! HTTP/RTSP requests.
//!
//! 快照抓取、代理拉流等服务器侧出站 URL 策略。

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// A resolved and policy-checked outbound endpoint.
///
/// 已通过策略校验的出站端点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}
