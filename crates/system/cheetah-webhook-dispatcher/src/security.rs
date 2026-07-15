use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::time::Duration;

/// URL validation policy used to prevent SSRF.
///
/// SSRF 防护策略。
#[derive(Debug, Clone)]
pub struct WebhookUrlPolicy {
    pub allowed_cidrs: Vec<ipnet::IpNet>,
    pub block_private: bool,
    pub resolve_timeout: Duration,
}

impl WebhookUrlPolicy {
    pub fn from_cidr_strings(cidrs: &[String]) -> Result<Self, ipnet::IpNetParseError> {
        let mut allowed_cidrs = Vec::with_capacity(cidrs.len());
        for s in cidrs {
            allowed_cidrs.push(ipnet::IpNet::from_str(s)?);
        }
        Ok(Self {
            allowed_cidrs,
            block_private: true,
            resolve_timeout: Duration::from_secs(5),
        })
    }
}

impl Default for WebhookUrlPolicy {
    fn default() -> Self {
        Self {
            allowed_cidrs: Vec::new(),
            block_private: true,
            resolve_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookUrlVerdict {
    Allow(SocketAddr, ParsedUrl),
    Deny(String),
}

/// URL parts that the HTTP client needs.
///
/// HTTP 客户端需要的 URL 部件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path_and_query: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WebhookSecurityError {
    #[error("unsupported scheme: {0}")]
    UnsupportedScheme(String),
    #[error("missing host")]
    MissingHost,
    #[error("blocked address: {0}")]
    BlockedAddress(String),
    #[error("DNS resolution failed: {0}")]
    ResolutionFailed(String),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
}

impl WebhookUrlPolicy {
    pub fn evaluate(&self, url: &str) -> Result<WebhookUrlVerdict, WebhookSecurityError> {
        let parsed =
            url::Url::parse(url).map_err(|e| WebhookSecurityError::InvalidUrl(e.to_string()))?;

        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(WebhookSecurityError::UnsupportedScheme(
                parsed.scheme().to_string(),
            ));
        }

        let host = parsed
            .host_str()
            .ok_or(WebhookSecurityError::MissingHost)?
            .to_string();
        let port = parsed
            .port_or_known_default()
            .ok_or_else(|| WebhookSecurityError::ResolutionFailed("unknown port".to_string()))?;

        let addrs: Vec<SocketAddr> = format!("{}:{}", host, port)
            .to_socket_addrs()
            .map_err(|e| WebhookSecurityError::ResolutionFailed(e.to_string()))?
            .collect();

        let addr = addrs.into_iter().next().ok_or_else(|| {
            WebhookSecurityError::ResolutionFailed(format!("no address for {host}"))
        })?;

        if self.block_private && !self.is_allowed(addr.ip()) {
            return Err(WebhookSecurityError::BlockedAddress(addr.ip().to_string()));
        }

        let path_and_query = if parsed.query().is_some() {
            format!("{}?{}", parsed.path(), parsed.query().unwrap())
        } else {
            parsed.path().to_string()
        };

        Ok(WebhookUrlVerdict::Allow(
            addr,
            ParsedUrl {
                scheme: parsed.scheme().to_string(),
                host,
                port,
                path_and_query,
            },
        ))
    }

    fn is_allowed(&self, ip: IpAddr) -> bool {
        let ip = canonicalize_ip(ip);
        if self.allowed_cidrs.iter().any(|net| net.contains(&ip)) {
            return true;
        }
        if is_loopback_or_link_local(&ip) {
            return false;
        }
        if is_metadata_service(&ip) {
            return false;
        }
        if is_private_or_unspecified(&ip) {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_blocks_private() {
        let policy = WebhookUrlPolicy::default();
        let verdict = policy.evaluate("http://127.0.0.1/hook");
        assert!(matches!(
            verdict,
            Err(WebhookSecurityError::BlockedAddress(_))
        ));
    }

    #[test]
    fn allows_public_http_url() {
        let policy = WebhookUrlPolicy::default();
        let verdict = policy.evaluate("http://example.com/hook");
        assert!(verdict.is_ok());
        if let Ok(WebhookUrlVerdict::Allow(addr, parsed)) = verdict {
            assert!(addr.port() == 80 || addr.port() == 443);
            assert_eq!(parsed.host, "example.com");
        }
    }

    #[test]
    fn allows_https_scheme() {
        let policy = WebhookUrlPolicy::default();
        let verdict = policy.evaluate("https://example.com/hook");
        assert!(verdict.is_ok(), "{verdict:?}");
    }

    #[test]
    fn blocks_unknown_scheme() {
        let policy = WebhookUrlPolicy::default();
        let verdict = policy.evaluate("ftp://example.com/hook");
        assert!(matches!(verdict, Err(WebhookSecurityError::UnsupportedScheme(s)) if s == "ftp"));
    }

    #[test]
    fn blocks_ipv6_ula() {
        let policy = WebhookUrlPolicy::default();
        let verdict = policy.evaluate("http://[fd12::1]/hook");
        assert!(matches!(
            verdict,
            Err(WebhookSecurityError::BlockedAddress(_))
        ));
    }

    #[test]
    fn allowed_cidr_can_overrule_private_block() {
        let policy = WebhookUrlPolicy::from_cidr_strings(&["127.0.0.1/32".to_string()]).unwrap();
        let verdict = policy.evaluate("http://127.0.0.1/hook");
        assert!(verdict.is_ok(), "{verdict:?}");
    }

    #[test]
    fn blocks_metadata_ip() {
        let policy = WebhookUrlPolicy::default();
        let verdict = policy.evaluate("http://169.254.169.254/latest/meta-data");
        assert!(matches!(
            verdict,
            Err(WebhookSecurityError::BlockedAddress(_))
        ));
    }

    #[test]
    fn blocks_ipv6_mapped_loopback() {
        let policy = WebhookUrlPolicy::default();
        let verdict = policy.evaluate("http://[::ffff:127.0.0.1]/hook");
        assert!(matches!(
            verdict,
            Err(WebhookSecurityError::BlockedAddress(_))
        ));
    }

    #[test]
    fn blocks_ipv6_mapped_metadata() {
        let policy = WebhookUrlPolicy::default();
        let verdict = policy.evaluate("http://[::ffff:169.254.169.254]/latest/meta-data");
        assert!(matches!(
            verdict,
            Err(WebhookSecurityError::BlockedAddress(_))
        ));
    }

    #[test]
    fn allows_ipv6_mapped_public() {
        let policy = WebhookUrlPolicy::default();
        let verdict = policy.evaluate("http://[::ffff:8.8.8.8]/hook");
        assert!(verdict.is_ok(), "{verdict:?}");
    }
}

fn canonicalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
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

fn is_loopback_or_link_local(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || is_v6_link_local(v6),
    }
}

fn is_v6_link_local(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xffc0) == 0xfe80
}

fn is_metadata_service(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if v4 == &Ipv4Addr::new(169, 254, 169, 254))
}

fn is_private_or_unspecified(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_unspecified(),
        IpAddr::V6(v6) => v6.is_unspecified() || v6.is_unique_local(),
    }
}

pub mod ipnet {
    use std::fmt;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::str::FromStr;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
    pub enum IpNetParseError {
        #[error("CIDR must contain '/'")]
        MissingSlash,
        #[error("invalid prefix")]
        InvalidPrefix,
        #[error("invalid IP address")]
        InvalidAddress,
    }

    #[derive(Debug, Clone)]
    pub enum IpNet {
        V4(Ipv4Net),
        V6(Ipv6Net),
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Ipv4Net {
        addr: u32,
        mask: u32,
        prefix: u8,
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Ipv6Net {
        addr: u128,
        mask: u128,
        prefix: u8,
    }

    impl IpNet {
        pub fn contains(&self, ip: &IpAddr) -> bool {
            match (self, ip) {
                (IpNet::V4(net), IpAddr::V4(v4)) => net.contains(v4),
                (IpNet::V6(net), IpAddr::V6(v6)) => net.contains(v6),
                _ => false,
            }
        }
    }

    impl Ipv4Net {
        pub fn new(addr: Ipv4Addr, prefix: u8) -> Self {
            let prefix = prefix.min(32);
            let shift = 32u32.saturating_sub(prefix as u32);
            let mask = if prefix == 0 { 0 } else { u32::MAX << shift };
            Self {
                addr: u32::from(addr) & mask,
                mask,
                prefix,
            }
        }

        pub fn contains(&self, ip: &Ipv4Addr) -> bool {
            (u32::from(*ip) & self.mask) == self.addr
        }
    }

    impl Ipv6Net {
        pub fn new(addr: Ipv6Addr, prefix: u8) -> Self {
            let prefix = prefix.min(128);
            let shift = 128u32.saturating_sub(prefix as u32);
            let mask = if prefix == 0 { 0 } else { u128::MAX << shift };
            Self {
                addr: u128::from(addr) & mask,
                mask,
                prefix,
            }
        }

        pub fn contains(&self, ip: &Ipv6Addr) -> bool {
            (u128::from(*ip) & self.mask) == self.addr
        }
    }

    impl FromStr for IpNet {
        type Err = IpNetParseError;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            let (addr_str, prefix_str) = s.split_once('/').ok_or(IpNetParseError::MissingSlash)?;
            let prefix: u8 = prefix_str
                .parse()
                .map_err(|_| IpNetParseError::InvalidPrefix)?;
            if let Ok(v4) = addr_str.parse::<Ipv4Addr>() {
                Ok(IpNet::V4(Ipv4Net::new(v4, prefix)))
            } else if let Ok(v6) = addr_str.parse::<Ipv6Addr>() {
                Ok(IpNet::V6(Ipv6Net::new(v6, prefix)))
            } else {
                Err(IpNetParseError::InvalidAddress)
            }
        }
    }

    impl fmt::Display for IpNet {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                IpNet::V4(v4) => write!(f, "{}/{}", Ipv4Addr::from(v4.addr), v4.prefix),
                IpNet::V6(v6) => write!(f, "{}/{}", Ipv6Addr::from(v6.addr), v6.prefix),
            }
        }
    }
}
