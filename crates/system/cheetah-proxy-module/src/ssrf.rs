//! SSRF protection helpers for proxy source / destination URLs.
//!
//! Defaults deny loopback, link-local, private, multicast and unspecified
//! addresses. A configurable CIDR allowlist can explicitly open device or test
//! network segments.
//!
//! 代理源/目标 URL 的 SSRF 保护辅助函数。
//! 默认拒绝回环、链路本地、私有、组播和未指定地址，可通过 CIDR 白名单显式放行。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use cheetah_media_api::error::{MediaError, Result};
use tracing::warn;
use url::{Host, Url};

/// An IPv4 or IPv6 network with a prefix length.
///
/// 带前缀长度的 IPv4/IPv6 网络。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpNetwork {
    V4 { network: Ipv4Addr, prefix: u8 },
    V6 { network: Ipv6Addr, prefix: u8 },
}

impl IpNetwork {
    fn contains(&self, addr: &IpAddr) -> bool {
        match (self, addr) {
            (IpNetwork::V4 { network, prefix }, IpAddr::V4(v4)) => {
                let mask = if *prefix == 0 {
                    0u32
                } else if *prefix >= 32 {
                    u32::MAX
                } else {
                    u32::MAX << (32 - *prefix)
                };
                u32::from_be_bytes(v4.octets()) & mask
                    == u32::from_be_bytes(network.octets()) & mask
            }
            (IpNetwork::V6 { network, prefix }, IpAddr::V6(v6)) => {
                let mask = if *prefix == 0 {
                    0u128
                } else if *prefix >= 128 {
                    u128::MAX
                } else {
                    u128::MAX << (128 - *prefix)
                };
                let addr_bits = u128::from_be_bytes(v6.octets());
                let net_bits = u128::from_be_bytes(network.octets());
                addr_bits & mask == net_bits & mask
            }
            // Different address families never match.
            _ => false,
        }
    }
}

/// Parse a CIDR string such as `127.0.0.0/8` or `::1/128`.
///
/// 解析 `127.0.0.0/8` 或 `::1/128` 等 CIDR 字符串。
pub fn parse_cidr(s: &str) -> Result<IpNetwork> {
    let s = s.trim();
    let (addr_part, prefix_part) = match s.rsplit_once('/') {
        Some((a, p)) => (a, p),
        None => {
            // A bare address is treated as a /32 or /128 host route.
            let addr: IpAddr = s
                .parse()
                .map_err(|e| MediaError::invalid_argument(format!("invalid CIDR '{s}': {e}")))?;
            return Ok(match addr {
                IpAddr::V4(v4) => IpNetwork::V4 {
                    network: v4,
                    prefix: 32,
                },
                IpAddr::V6(v6) => IpNetwork::V6 {
                    network: v6,
                    prefix: 128,
                },
            });
        }
    };

    let prefix: u8 = prefix_part
        .trim()
        .parse()
        .map_err(|e| MediaError::invalid_argument(format!("invalid CIDR prefix '{s}': {e}")))?;

    let addr: IpAddr = addr_part
        .trim()
        .parse()
        .map_err(|e| MediaError::invalid_argument(format!("invalid CIDR address '{s}': {e}")))?;

    let max_prefix = match addr {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    if prefix > max_prefix {
        return Err(MediaError::invalid_argument(format!(
            "CIDR prefix {prefix} exceeds maximum {max_prefix} for '{s}'"
        )));
    }

    Ok(match addr {
        IpAddr::V4(v4) => IpNetwork::V4 {
            network: v4,
            prefix,
        },
        IpAddr::V6(v6) => IpNetwork::V6 {
            network: v6,
            prefix,
        },
    })
}

/// Parse the configured CIDR allowlist.
///
/// 解析配置中的 CIDR 白名单。
pub fn parse_allowlist(cidrs: &[String]) -> Result<Vec<IpNetwork>> {
    cidrs.iter().map(|s| parse_cidr(s)).collect()
}

/// Validate a URL against SSRF policy.
///
/// - Rejects unsupported schemes.
/// - Rejects hostnames such as `localhost`.
/// - Rejects private/loopback/link-local/multicast/unspecified IPs unless they
///   are contained in `allowlist`.
///
/// 按 SSRF 策略校验 URL。
pub fn validate_url(url: &str, allowlist: &[IpNetwork]) -> Result<()> {
    let parsed =
        Url::parse(url).map_err(|e| MediaError::invalid_argument(format!("invalid URL: {e}")))?;

    match parsed.scheme() {
        "http" | "https" | "rtmp" | "rtsp" | "srt" | "webrtc" | "rtp" => {}
        _ => {
            return Err(MediaError::invalid_argument(format!(
                "unsupported URL scheme: {}",
                parsed.scheme()
            )))
        }
    }

    let host = parsed
        .host()
        .ok_or_else(|| MediaError::invalid_argument("URL missing host".to_string()))?;

    match host {
        Host::Domain(domain) => {
            if is_forbidden_domain(domain) {
                return Err(MediaError::invalid_argument(format!(
                    "forbidden proxy target host: {domain}"
                )));
            }
            // Non-special URL schemes (rtsp, rtmp, srt, etc.) are parsed as
            // domains even when the host is an IPv4 literal. Treat it as an IP
            // address if it parses, then apply the same SSRF checks.
            if let Ok(addr) = domain.parse::<IpAddr>() {
                let addr = normalize_ip(addr);
                if is_internal_ip(&addr) && !allowlist.iter().any(|net| net.contains(&addr)) {
                    return Err(MediaError::invalid_argument(format!(
                        "forbidden proxy target address: {addr}"
                    )));
                }
            } else {
                warn!(domain = %domain, "proxy URL uses a domain name; DNS/rebinding validation not yet enforced");
            }
        }
        Host::Ipv4(ip) => {
            let addr = IpAddr::from(ip);
            let addr = normalize_ip(addr);
            if is_internal_ip(&addr) && !allowlist.iter().any(|net| net.contains(&addr)) {
                return Err(MediaError::invalid_argument(format!(
                    "forbidden proxy target address: {addr}"
                )));
            }
        }
        Host::Ipv6(ip) => {
            let addr = IpAddr::from(ip);
            let addr = normalize_ip(addr);
            if is_internal_ip(&addr) && !allowlist.iter().any(|net| net.contains(&addr)) {
                return Err(MediaError::invalid_argument(format!(
                    "forbidden proxy target address: {addr}"
                )));
            }
        }
    }

    Ok(())
}

fn normalize_ip(addr: IpAddr) -> IpAddr {
    match addr {
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                IpAddr::V4(v4)
            } else {
                IpAddr::V6(v6)
            }
        }
        other => other,
    }
}

fn is_forbidden_domain(domain: &str) -> bool {
    let lower = domain.to_lowercase();
    lower == "localhost"
        || lower == "localhost.localdomain"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
}

fn is_internal_ip(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_documentation()
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_internal_ip(&IpAddr::V4(v4));
            }
            is_ipv6_unique_local(v6)
                || is_ipv6_link_local(v6)
                || v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
        }
    }
}

fn is_ipv6_unique_local(v6: &Ipv6Addr) -> bool {
    // fc00::/7
    v6.segments()[0] & 0xfe00 == 0xfc00
}

fn is_ipv6_link_local(v6: &Ipv6Addr) -> bool {
    // fe80::/10
    v6.segments()[0] & 0xffc0 == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localhost_and_private_rejected() {
        let allowlist = [];
        assert!(validate_url("http://localhost/x", &allowlist).is_err());
        assert!(validate_url("http://127.0.0.1/x", &allowlist).is_err());
        assert!(validate_url("http://10.0.0.1/x", &allowlist).is_err());
        assert!(validate_url("http://[::1]/x", &allowlist).is_err());
        assert!(validate_url("http://192.168.1.1/x", &allowlist).is_err());
    }

    #[test]
    fn public_targets_accepted() {
        let allowlist = [];
        assert!(validate_url("rtsp://example.com/stream", &allowlist).is_ok());
        assert!(validate_url("rtmp://8.8.8.8/live", &allowlist).is_ok());
    }

    #[test]
    fn allowlist_opens_loopback() {
        let allowlist = [parse_cidr("127.0.0.0/8").unwrap()];
        assert!(validate_url("http://127.0.0.1/x", &allowlist).is_ok());
        assert!(validate_url("http://127.255.255.255/x", &allowlist).is_ok());
        assert!(validate_url("http://10.0.0.1/x", &allowlist).is_err());
    }

    #[test]
    fn allowlist_opens_ipv6_loopback() {
        let allowlist = [parse_cidr("::1/128").unwrap()];
        assert!(validate_url("http://[::1]/x", &allowlist).is_ok());
        assert!(validate_url("http://[::ffff:127.0.0.1]/x", &allowlist).is_err());
    }

    #[test]
    fn rtsp_ipv4_literal_rejected_without_allowlist() {
        let allowlist = [];
        assert!(validate_url("rtsp://127.0.0.1/stream", &allowlist).is_err());
        assert!(validate_url("rtsp://10.0.0.1/stream", &allowlist).is_err());
    }

    #[test]
    fn unsupported_schemes_rejected() {
        let allowlist = [];
        assert!(validate_url("ftp://example.com/x", &allowlist).is_err());
    }
}
