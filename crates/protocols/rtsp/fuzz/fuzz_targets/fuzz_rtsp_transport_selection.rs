#![no_main]

use cheetah_rtsp_core::RtspTransport;
use libfuzzer_sys::fuzz_target;

const MAX_TEXT_BYTES: usize = 8 * 1024;

fuzz_target!(|data: &[u8]| {
    let bounded = &data[..data.len().min(MAX_TEXT_BYTES)];
    let header = build_transport_candidates(bounded);
    if let Ok(candidates) = RtspTransport::parse_multiple(&header) {
        let prefer_tcp = bounded.first().copied().unwrap_or(0) & 1 == 1;
        let prefer_multicast = bounded.get(1).copied().unwrap_or(0) & 1 == 1;
        let selected = select_best_transport(&candidates, prefer_tcp, prefer_multicast);
        if let Some(choice) = selected {
            let _ = RtspTransport::parse(&choice.to_header());
        }
    }
});

fn build_transport_candidates(data: &[u8]) -> String {
    let c1 = data.first().copied().unwrap_or(0);
    let c2 = data.get(1).copied().unwrap_or(1);
    let p = 1024u16.saturating_add(u16::from(data.get(2).copied().unwrap_or(0)) * 2);

    let tcp = format!("RTP/AVP/TCP;unicast;interleaved={}-{}", c1, c2);
    let udp = format!("RTP/AVP;unicast;client_port={}-{}", p, p.saturating_add(1));
    let mcast = format!(
        "RTP/AVP;multicast;destination=239.1.2.{};port={}-{}",
        data.get(3).copied().unwrap_or(3),
        p,
        p.saturating_add(1)
    );

    format!("{tcp}, {udp}, {mcast}")
}

fn select_best_transport(
    candidates: &[RtspTransport],
    prefer_tcp: bool,
    prefer_multicast: bool,
) -> Option<&RtspTransport> {
    candidates
        .iter()
        .filter(|transport| is_structurally_valid(transport))
        .max_by_key(|transport| score_transport(transport, prefer_tcp, prefer_multicast))
}

fn is_structurally_valid(transport: &RtspTransport) -> bool {
    if let Some((a, b)) = transport.interleaved {
        if a == b {
            return false;
        }
    }
    if let Some((a, b)) = transport.client_port {
        if a == 0 || b == 0 || a == b {
            return false;
        }
    }
    if let Some((a, b)) = transport.port {
        if a == 0 || b == 0 || a == b {
            return false;
        }
    }
    true
}

fn score_transport(transport: &RtspTransport, prefer_tcp: bool, prefer_multicast: bool) -> i32 {
    let mut score = 0i32;

    if transport.interleaved.is_some() {
        score += if prefer_tcp { 200 } else { 60 };
    }
    if transport.client_port.is_some() {
        score += if prefer_tcp { 40 } else { 140 };
    }
    if !transport.unicast {
        score += if prefer_multicast { 180 } else { 20 };
    }
    if transport.server_port.is_some() {
        score += 10;
    }
    if transport.destination.is_some() {
        score += 5;
    }

    score
}
