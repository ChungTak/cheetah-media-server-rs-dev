//! SSRF protection helpers for proxy source / destination URLs.
//!
//! Defaults deny loopback, link-local, private, multicast and unspecified
//! addresses. A configurable CIDR allowlist can explicitly open device or test
//! network segments.
//!
//! 代理源/目标 URL 的 SSRF 保护辅助函数。
//! 默认拒绝回环、链路本地、私有、组播和未指定地址，可通过 CIDR 白名单显式放行。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use cheetah_media_api::error::{MediaError, Result};
use cheetah_runtime_api::RuntimeApi;
use tracing::{info, warn};
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

/// Validate a URL against SSRF policy without performing DNS resolution.
///
/// - Rejects unsupported schemes.
/// - Rejects private/loopback/link-local/multicast/unspecified IP literals unless
///   they are contained in `allowlist`.
/// - Allows domain names (they must be resolved and re-validated by
///   [`resolve_and_validate_url`] before any connection is made).
///
/// 按 SSRF 策略静态校验 URL（不执行 DNS 解析）。  
/// 主要用于测试和 IP 字面量预检；生产连接应使用 [`resolve_and_validate_url`]。  
#[allow(dead_code)]
pub fn validate_url(url: &str, allowlist: &[IpNetwork]) -> Result<()> {
    let parsed =
        Url::parse(url).map_err(|e| MediaError::invalid_argument(format!("invalid URL: {e}")))?;

    validate_scheme(&parsed)?;

    let host = parsed
        .host()
        .ok_or_else(|| MediaError::invalid_argument("URL missing host".to_string()))?;

    match host {
        Host::Domain(domain) => {
            // Non-special URL schemes (rtsp, rtmp, srt, etc.) are parsed as
            // domains even when the host is an IPv4 literal. Treat it as an IP
            // address if it parses, then apply the same SSRF checks.
            if let Ok(addr) = domain.parse::<IpAddr>() {
                check_ip_addr(addr, allowlist)?;
            } else {
                warn!(domain = %domain, "proxy URL uses a domain name; DNS validation not yet performed");
            }
        }
        Host::Ipv4(ip) => check_ip_addr(IpAddr::from(ip), allowlist)?,
        Host::Ipv6(ip) => check_ip_addr(IpAddr::from(ip), allowlist)?,
    }

    Ok(())
}

/// Resolve the URL hostname, validate all A/AAAA records, and return a URL
/// whose host has been rewritten to the first allowed resolved IP address.
///
/// This prevents DNS rebinding attacks: the validated IP is used for the
/// connection and is not re-resolved by the protocol driver on reconnect.
/// Any redirect target should be passed through the same validation.
///
/// 解析 URL 主机名并校验所有 A/AAAA 记录，返回已将主机替换为首个合规 IP 的 URL。  
/// 通过使用已校验的 IP 地址，避免 DNS 重绑定；重定向目标应再次经过同样校验。  
pub async fn resolve_and_validate_url(
    url: &str,
    allowlist: &[IpNetwork],
    runtime_api: &Arc<dyn RuntimeApi>,
) -> Result<String> {
    let parsed =
        Url::parse(url).map_err(|e| MediaError::invalid_argument(format!("invalid URL: {e}")))?;

    validate_scheme(&parsed)?;

    let host = parsed
        .host()
        .ok_or_else(|| MediaError::invalid_argument("URL missing host".to_string()))?;

    let resolved_ip = match host {
        Host::Domain(domain) => {
            if let Ok(addr) = domain.parse::<IpAddr>() {
                check_ip_addr(addr, allowlist)?;
                return Ok(url.to_string());
            }
            resolve_domain(domain, allowlist, runtime_api).await?
        }
        Host::Ipv4(ip) => {
            check_ip_addr(IpAddr::from(ip), allowlist)?;
            return Ok(url.to_string());
        }
        Host::Ipv6(ip) => {
            check_ip_addr(IpAddr::from(ip), allowlist)?;
            return Ok(url.to_string());
        }
    };

    let mut rewritten = parsed.clone();
    let host_str = if resolved_ip.is_ipv6() {
        format!("[{resolved_ip}]")
    } else {
        resolved_ip.to_string()
    };
    rewritten
        .set_host(Some(&host_str))
        .map_err(|e| MediaError::invalid_argument(format!("failed to rewrite URL host: {e}")))?;

    info!(
        scheme = %parsed.scheme(),
        original_host = %parsed.host_str().unwrap_or(""),
        resolved = %resolved_ip,
        "proxy URL resolved and validated"
    );

    Ok(rewritten.to_string())
}

fn validate_scheme(parsed: &Url) -> Result<()> {
    match parsed.scheme() {
        "http" | "https" | "rtmp" | "rtsp" | "srt" | "webrtc" | "rtp" => Ok(()),
        _ => Err(MediaError::invalid_argument(format!(
            "unsupported URL scheme: {}",
            parsed.scheme()
        ))),
    }
}

fn check_ip_addr(addr: IpAddr, allowlist: &[IpNetwork]) -> Result<()> {
    let addr = normalize_ip(addr);
    if is_internal_ip(&addr) && !allowlist.iter().any(|net| net.contains(&addr)) {
        return Err(MediaError::invalid_argument(format!(
            "forbidden proxy target address: {addr}"
        )));
    }
    Ok(())
}

async fn resolve_domain(
    domain: &str,
    allowlist: &[IpNetwork],
    runtime_api: &Arc<dyn RuntimeApi>,
) -> Result<IpAddr> {
    let addrs = runtime_api.resolve_host(domain).await.map_err(|e| {
        MediaError::invalid_argument(format!("DNS resolve failed for {domain}: {e}"))
    })?;

    if addrs.is_empty() {
        return Err(MediaError::invalid_argument(format!(
            "DNS resolve returned no addresses for {domain}"
        )));
    }

    for addr in &addrs {
        if is_internal_ip(addr) && !allowlist.iter().any(|net| net.contains(addr)) {
            return Err(MediaError::invalid_argument(format!(
                "forbidden proxy target address: {addr}"
            )));
        }
    }

    Ok(addrs[0])
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
    use cheetah_runtime_api::RuntimeApi;

    #[test]
    fn localhost_and_private_ip_literals_rejected() {
        let allowlist = [];
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

    fn tokio_runtime() -> Arc<dyn RuntimeApi> {
        Arc::new(cheetah_runtime_tokio::TokioRuntime::new())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn localhost_resolved_with_loopback_allowlist() {
        let runtime = tokio_runtime();
        let allowlist = [
            parse_cidr("127.0.0.0/8").unwrap(),
            parse_cidr("::1/128").unwrap(),
        ];
        let resolved = resolve_and_validate_url("http://localhost/x", &allowlist, &runtime)
            .await
            .expect("localhost should resolve to loopback with loopback allowlist");
        assert!(
            resolved.contains("127.") || resolved.contains("[::1]"),
            "resolved URL should contain a loopback IP: {resolved}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn localhost_rejected_without_allowlist() {
        let runtime = tokio_runtime();
        let allowlist = [];
        assert!(
            resolve_and_validate_url("http://localhost/x", &allowlist, &runtime)
                .await
                .is_err()
        );
    }
}
