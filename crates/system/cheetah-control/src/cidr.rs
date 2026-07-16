//! Minimal CIDR matching for trusted proxy validation.
//!
//! This stays self-contained so the control plane does not need to depend on
//! protocol-specific crates for a small network utility.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpNetParseError {
    MissingSlash,
    InvalidPrefix,
    InvalidAddress,
}

impl fmt::Display for IpNetParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpNetParseError::MissingSlash => f.write_str("CIDR must contain '/'"),
            IpNetParseError::InvalidPrefix => f.write_str("invalid prefix"),
            IpNetParseError::InvalidAddress => f.write_str("invalid IP address"),
        }
    }
}

impl std::error::Error for IpNetParseError {}

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
            if prefix > 32 {
                return Err(IpNetParseError::InvalidPrefix);
            }
            Ok(IpNet::V4(Ipv4Net::new(v4, prefix)))
        } else if let Ok(v6) = addr_str.parse::<Ipv6Addr>() {
            if prefix > 128 {
                return Err(IpNetParseError::InvalidPrefix);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn ipv4_cidr_contains() {
        let net = IpNet::from_str("10.0.0.0/8").unwrap();
        assert!(net.contains(&IpAddr::V4("10.1.2.3".parse().unwrap())));
        assert!(!net.contains(&IpAddr::V4("192.168.1.1".parse().unwrap())));
    }

    #[test]
    fn ipv6_cidr_contains() {
        let net = IpNet::from_str("2001:db8::/32").unwrap();
        assert!(net.contains(&IpAddr::V6("2001:db8::1".parse().unwrap())));
        assert!(!net.contains(&IpAddr::V6("2001:db9::1".parse().unwrap())));
    }

    #[test]
    fn mixed_families_do_not_match() {
        let net = IpNet::from_str("127.0.0.1/32").unwrap();
        assert!(!net.contains(&IpAddr::V6("::1".parse().unwrap())));
    }

    #[test]
    fn out_of_range_prefix_is_rejected() {
        assert!(matches!(
            IpNet::from_str("10.0.0.0/33"),
            Err(IpNetParseError::InvalidPrefix)
        ));
        assert!(matches!(
            IpNet::from_str("2001:db8::/129"),
            Err(IpNetParseError::InvalidPrefix)
        ));
    }
}
