#![no_main]

use cheetah_rtsp_core::RtspTransport;
use libfuzzer_sys::fuzz_target;

const MAX_TEXT_BYTES: usize = 8 * 1024;

fuzz_target!(|data: &[u8]| {
    let bounded = &data[..data.len().min(MAX_TEXT_BYTES)];
    let header = build_multicast_transport_header(bounded);

    let _ = RtspTransport::parse(&header);
    if let Ok(candidates) = RtspTransport::parse_multiple(&header) {
        for candidate in candidates.iter().take(64) {
            let roundtrip = candidate.to_header();
            let _ = RtspTransport::parse(&roundtrip);
            let _ = is_multicast_candidate_usable(candidate);
        }
    }
});

fn build_multicast_transport_header(data: &[u8]) -> String {
    let dst_a = data.first().copied().unwrap_or(239);
    let dst_b = data.get(1).copied().unwrap_or(1);
    let dst_c = data.get(2).copied().unwrap_or(2);
    let ttl = data.get(4).copied().unwrap_or(16);
    let layers = u32::from(data.get(5).copied().unwrap_or(1));
    let port_lo = 1024u16.saturating_add(u16::from(data.get(6).copied().unwrap_or(0)) * 2);
    let port_hi = port_lo.saturating_add(1);

    let multicast = format!(
        "RTP/AVP;multicast;destination=239.{}.{}.{};ttl={};layers={};port={}-{}",
        dst_a, dst_b, dst_c, ttl, layers, port_lo, port_hi
    );
    let udp_unicast = format!(
        "RTP/AVP;unicast;client_port={}-{};server_port={}-{}",
        port_lo,
        port_hi,
        port_lo.saturating_add(100),
        port_hi.saturating_add(100)
    );
    let tcp = format!(
        "RTP/AVP/TCP;unicast;interleaved={}-{}",
        data.get(7).copied().unwrap_or(0),
        data.get(8).copied().unwrap_or(1)
    );

    if data.get(9).copied().unwrap_or(0) & 1 == 1 {
        format!("{multicast}, {udp_unicast}, {tcp}")
    } else {
        format!("{tcp}, {udp_unicast}, {multicast}")
    }
}

fn is_multicast_candidate_usable(transport: &RtspTransport) -> bool {
    if transport.unicast {
        return false;
    }
    let Some((port_a, port_b)) = transport.port else {
        return false;
    };
    if port_a == 0 || port_b == 0 || port_a >= port_b {
        return false;
    }

    let Some(destination) = transport.destination.as_deref() else {
        return false;
    };

    destination.starts_with("224.")
        || destination.starts_with("225.")
        || destination.starts_with("226.")
        || destination.starts_with("227.")
        || destination.starts_with("228.")
        || destination.starts_with("229.")
        || destination.starts_with("230.")
        || destination.starts_with("231.")
        || destination.starts_with("232.")
        || destination.starts_with("233.")
        || destination.starts_with("234.")
        || destination.starts_with("235.")
        || destination.starts_with("236.")
        || destination.starts_with("237.")
        || destination.starts_with("238.")
        || destination.starts_with("239.")
}
