//! Lightweight SDP candidate / extension utilities used at the
//! driver boundary.
//!
//! `cheetah-webrtc-core` does the heavy SDP parsing through `str0m`.
//! The driver only needs a handful of summary metrics for
//! observability — counting candidate types in a generated answer
//! to feed `WebRtcDriverDiagnosticKind` style events, for example.
//! These helpers are kept stand-alone (no `str0m` dependency) so
//! they stay cheap and quick to call from the hot path.

use std::net::IpAddr;

/// Per-type candidate counts extracted from a single SDP. Mirrors
/// the harness `assertions::CandidateCounts` shape but lives in the
/// driver crate so module / runtime callers can use it without
/// pulling the test target into their dep graph.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LocalCandidateCounts {
    /// Number of `typ host` candidates.
    pub host: usize,
    /// Number of `typ srflx` candidates.
    pub srflx: usize,
    /// Number of `typ prflx` candidates (peer-reflexive; rare in
    /// generated SDPs but present in some str0m configurations).
    pub prflx: usize,
    /// Number of `typ relay` candidates.
    pub relay: usize,
    /// Number of UDP-transport candidates.
    pub udp: usize,
    /// Number of TCP-transport candidates.
    pub tcp: usize,
    /// Number of IPv4 (or mDNS) candidates.
    pub ipv4: usize,
    /// Number of IPv6 candidates.
    pub ipv6: usize,
}

impl LocalCandidateCounts {
    /// Total candidate count summed across the four types.
    pub fn total(&self) -> usize {
        self.host + self.srflx + self.prflx + self.relay
    }
}

/// Per-session local candidate output policy.
///
/// OME exposes this through `?transport=`. The driver applies it to
/// generated SDP immediately before emitting `AnswerReady`/`OfferReady`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateTransportPolicy {
    /// `All` variant.
    /// `All` 变体.
    All,
    /// `UdpOnly` variant.
    /// `UdpOnly` 变体.
    UdpOnly,
    /// `TcpOnly` variant.
    /// `TcpOnly` 变体.
    TcpOnly,
    /// `RelayOnly` variant.
    /// `RelayOnly` 变体.
    RelayOnly,
    /// `UdpTcp` variant.
    /// `UdpTcp` 变体.
    UdpTcp,
}

/// Count `a=candidate:` lines in a generated SDP and bucket them
/// by candidate type and transport. Used by operators to observe
/// the local candidate gathering result without drinking from the
/// firehose of `str0m` events.
///
/// The parser is intentionally permissive: malformed lines are
/// skipped silently, and the IPv4 / IPv6 split is determined by
/// the presence of `:` in the address field. mDNS hostnames
/// (ending in `.local`) are counted as `ipv4` since they resolve
/// at runtime to a host candidate.
pub fn count_local_candidates(sdp: &str) -> LocalCandidateCounts {
    let mut c = LocalCandidateCounts::default();
    for line in sdp.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("a=candidate:") else {
            continue;
        };
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() < 8 {
            continue;
        }
        // RFC 5245 §15.1: foundation component transport priority
        // address port "typ" type [other...]
        match parts[2].to_ascii_uppercase().as_str() {
            "UDP" => c.udp += 1,
            "TCP" => c.tcp += 1,
            _ => {}
        }
        let address = parts[4];
        if let Ok(ip) = address.parse::<IpAddr>() {
            match ip {
                IpAddr::V4(_) => c.ipv4 += 1,
                IpAddr::V6(_) => c.ipv6 += 1,
            }
        } else if address.contains(':') {
            // Non-parseable address with `:` — assume IPv6 (e.g.
            // a partial v6 from a non-compliant peer).
            c.ipv6 += 1;
        } else {
            c.ipv4 += 1;
        }
        if parts[6].eq_ignore_ascii_case("typ") {
            match parts[7].to_ascii_lowercase().as_str() {
                "host" => c.host += 1,
                "srflx" => c.srflx += 1,
                "prflx" => c.prflx += 1,
                "relay" => c.relay += 1,
                _ => {}
            }
        }
    }
    c
}

/// Filter local `a=candidate:` lines in SDP according to a transport
/// policy. Non-candidate SDP attributes are preserved verbatim.
pub fn filter_local_candidates(sdp: &str, policy: CandidateTransportPolicy) -> String {
    if policy == CandidateTransportPolicy::All {
        return sdp.to_string();
    }

    let mut out = String::with_capacity(sdp.len());
    for line in sdp.split_inclusive('\n') {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("a=candidate:") else {
            out.push_str(line);
            continue;
        };
        if candidate_allowed(rest, policy) {
            out.push_str(line);
        }
    }
    out
}

/// Ensure each media section advertises ICE gathering completion for
/// non-trickle HTTP answers/offers. Some browser WHEP clients keep ICE in
/// checking without sending connectivity checks when a full SDP answer has
/// candidates but no `a=end-of-candidates` marker.
pub fn ensure_end_of_candidates(sdp: &str) -> String {
    let mut out = String::with_capacity(sdp.len() + 64);
    let mut media_section = Vec::new();
    for line in sdp.split_inclusive('\n') {
        if line.trim_start().starts_with("m=") {
            flush_media_section(&mut out, &mut media_section);
        }
        if media_section.is_empty() && !line.trim_start().starts_with("m=") {
            out.push_str(line);
        } else {
            media_section.push(line);
        }
    }
    flush_media_section(&mut out, &mut media_section);
    out
}

fn flush_media_section(out: &mut String, section: &mut Vec<&str>) {
    if section.is_empty() {
        return;
    }
    if section
        .iter()
        .any(|line| line.trim().eq_ignore_ascii_case("a=end-of-candidates"))
    {
        for line in section.drain(..) {
            out.push_str(line);
        }
        return;
    }

    let mut inserted = false;
    for line in section.drain(..) {
        out.push_str(line);
        if !inserted && line.trim().eq_ignore_ascii_case("a=ice-options:trickle") {
            out.push_str("a=end-of-candidates\r\n");
            inserted = true;
        }
    }
    if !inserted {
        out.push_str("a=end-of-candidates\r\n");
    }
}

fn candidate_allowed(candidate: &str, policy: CandidateTransportPolicy) -> bool {
    let parts: Vec<&str> = candidate.split_whitespace().collect();
    if parts.len() < 8 || !parts[6].eq_ignore_ascii_case("typ") {
        return false;
    }
    let protocol = parts[2].to_ascii_uppercase();
    let candidate_type = parts[7].to_ascii_lowercase();
    match policy {
        CandidateTransportPolicy::All => true,
        CandidateTransportPolicy::UdpOnly => protocol == "UDP" && candidate_type != "relay",
        CandidateTransportPolicy::TcpOnly => protocol == "TCP" && candidate_type != "relay",
        CandidateTransportPolicy::RelayOnly => candidate_type == "relay",
        CandidateTransportPolicy::UdpTcp => {
            candidate_type != "relay" && matches!(protocol.as_str(), "UDP" | "TCP")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_local_candidates_buckets_canonical_lines() {
        let sdp = "v=0\r\n\
                   a=candidate:1 1 UDP 2113937151 192.168.1.1 5000 typ host\r\n\
                   a=candidate:2 1 UDP 1685987327 203.0.113.5 5000 typ srflx raddr 192.168.1.1 rport 5000\r\n\
                   a=candidate:3 1 TCP 2105524479 192.168.1.1 9 typ host tcptype active\r\n\
                   a=candidate:4 1 UDP 33554431 198.51.100.4 49152 typ relay raddr 203.0.113.5 rport 5000\r\n\
                   a=candidate:5 1 UDP 2113937150 fe80::1 5000 typ host\r\n";
        let c = count_local_candidates(sdp);
        assert_eq!(c.host, 3);
        assert_eq!(c.srflx, 1);
        assert_eq!(c.relay, 1);
        assert_eq!(c.prflx, 0);
        assert_eq!(c.tcp, 1);
        assert_eq!(c.udp, 4);
        assert_eq!(c.ipv4, 4);
        assert_eq!(c.ipv6, 1);
        assert_eq!(c.total(), 5);
    }

    #[test]
    fn count_local_candidates_handles_dirty_type_case() {
        let sdp = "a=candidate:1 1 UDP 0 192.168.1.1 5000 TYP HOST\r\n\
                   a=candidate:2 1 TCP 0 198.51.100.4 3478 Typ RELAY\r\n";
        let c = count_local_candidates(sdp);
        assert_eq!(c.host, 1);
        assert_eq!(c.relay, 1);
        assert_eq!(c.udp, 1);
        assert_eq!(c.tcp, 1);
    }

    #[test]
    fn count_local_candidates_handles_prflx_and_mdns() {
        let sdp = "a=candidate:1 1 UDP 0 abcd-1234.local 5000 typ host\r\n\
                   a=candidate:2 1 UDP 0 1.1.1.1 5000 typ prflx\r\n";
        let c = count_local_candidates(sdp);
        assert_eq!(c.host, 1);
        assert_eq!(c.prflx, 1);
        assert_eq!(c.total(), 2);
        assert_eq!(c.ipv4, 2, "mDNS hostnames bucket as ipv4 by convention");
    }

    #[test]
    fn count_local_candidates_skips_malformed_lines() {
        let sdp = "a=candidate:1 1 UDP\r\n\
                   a=candidate:malformed\r\n\
                   a=candidate:1 1 UDP 100 1.1.1.1 5000 typ host\r\n";
        let c = count_local_candidates(sdp);
        assert_eq!(c.host, 1, "only the well-formed line should count");
        assert_eq!(c.total(), 1);
    }

    #[test]
    fn count_local_candidates_returns_zero_for_empty_sdp() {
        let c = count_local_candidates("v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n");
        assert_eq!(c, LocalCandidateCounts::default());
    }

    #[test]
    fn filters_local_candidates_for_udp_tcp_without_relay() {
        let sdp = "v=0\r\n\
                   a=candidate:1 1 UDP 2113937151 192.168.1.1 5000 typ host\r\n\
                   a=candidate:2 1 TCP 2105524479 192.168.1.1 9 typ host tcptype passive\r\n\
                   a=candidate:3 1 UDP 33554431 198.51.100.4 49152 typ relay\r\n";
        let filtered = filter_local_candidates(sdp, CandidateTransportPolicy::UdpTcp);
        assert!(filtered.contains("a=candidate:1 1 UDP"));
        assert!(filtered.contains("a=candidate:2 1 TCP"));
        assert!(!filtered.contains("typ relay"));
    }

    #[test]
    fn filters_local_candidates_for_tcp_only() {
        let sdp = "v=0\r\n\
                   a=candidate:1 1 UDP 2113937151 192.168.1.1 5000 typ host\r\n\
                   a=candidate:2 1 TCP 2105524479 192.168.1.1 9 typ host tcptype passive\r\n\
                   a=end-of-candidates\r\n";
        let filtered = filter_local_candidates(sdp, CandidateTransportPolicy::TcpOnly);
        assert!(!filtered.contains("a=candidate:1 1 UDP"));
        assert!(filtered.contains("a=candidate:2 1 TCP"));
        assert!(filtered.contains("a=end-of-candidates"));
    }

    #[test]
    fn filters_local_candidates_for_relay_only() {
        let sdp = "v=0\r\n\
                   a=candidate:1 1 UDP 2113937151 192.168.1.1 5000 typ host\r\n\
                   a=candidate:2 1 UDP 33554431 198.51.100.4 49152 typ relay\r\n";
        let filtered = filter_local_candidates(sdp, CandidateTransportPolicy::RelayOnly);
        assert!(!filtered.contains("typ host"));
        assert!(filtered.contains("typ relay"));
    }

    #[test]
    fn filters_candidate_type_case_insensitively() {
        let sdp = "v=0\r\n\
                   a=candidate:1 1 UDP 2113937151 192.168.1.1 5000 TYP HOST\r\n\
                   a=candidate:2 1 UDP 33554431 198.51.100.4 49152 typ RELAY\r\n";

        let direct = filter_local_candidates(sdp, CandidateTransportPolicy::UdpTcp);
        assert!(direct.contains("TYP HOST"));
        assert!(!direct.contains("typ RELAY"));

        let relay = filter_local_candidates(sdp, CandidateTransportPolicy::RelayOnly);
        assert!(!relay.contains("TYP HOST"));
        assert!(relay.contains("typ RELAY"));
    }

    #[test]
    fn ensure_end_of_candidates_adds_marker_after_ice_options() {
        let sdp = "v=0\r\n\
                   m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
                   a=ice-options:trickle\r\n\
                   a=mid:0\r\n";

        let updated = ensure_end_of_candidates(sdp);
        assert!(
            updated.contains("a=ice-options:trickle\r\na=end-of-candidates\r\n"),
            "updated SDP must mark ICE gathering completion:\n{updated}"
        );
    }

    #[test]
    fn ensure_end_of_candidates_is_idempotent() {
        let sdp = "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
                   a=ice-options:trickle\r\n\
                   a=end-of-candidates\r\n";

        assert_eq!(ensure_end_of_candidates(sdp), sdp);
    }

    #[test]
    fn ensure_end_of_candidates_completes_each_media_section() {
        let sdp = "m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
                   a=ice-options:trickle\r\n\
                   a=end-of-candidates\r\n\
                   m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
                   a=ice-options:trickle\r\n\
                   a=mid:1\r\n";

        let updated = ensure_end_of_candidates(sdp);
        assert_eq!(updated.matches("a=end-of-candidates").count(), 2);
        assert!(updated.contains(
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
                                  a=ice-options:trickle\r\n\
                                  a=end-of-candidates\r\n\
                                  a=mid:1\r\n"
        ));
    }

    #[test]
    fn ensure_end_of_candidates_adds_marker_without_ice_options() {
        let sdp = "v=0\r\n\
                   m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
                   a=mid:0\r\n";

        let updated = ensure_end_of_candidates(sdp);
        assert!(updated.ends_with("a=mid:0\r\na=end-of-candidates\r\n"));
    }
}
