#![no_main]

mod common;

use cheetah_rtsp_core::{RtcpPacket, RtpPacket, RtspMessageLimits};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    let bounded = common::bounded_bytes_payload(data, MAX_INPUT_BYTES);
    let limits = RtspMessageLimits::default();
    common::fuzz_message_decoders(&bounded, limits.clone(), 8);

    let records = common::decode_or_select_records(&bounded, 1024);
    let rtp = common::udp_rtp_payloads(&records, 512);
    let rtcp = common::udp_rtcp_payloads(&records, 512);
    let datagrams = common::build_udp_fault_datagrams(&bounded, &rtp, &rtcp, 512);
    if datagrams.is_empty() {
        let fallback = common::bounded_bytes_payload(&bounded, 1400);
        let _ = RtpPacket::parse(&fallback);
        let _ = RtcpPacket::parse(&fallback);
        return;
    }

    for datagram in datagrams.iter().take(512) {
        let bounded = common::bounded_bytes_payload(datagram, 1500);
        let _ = RtpPacket::parse(&bounded);
        let _ = RtcpPacket::parse(&bounded);
    }
});
