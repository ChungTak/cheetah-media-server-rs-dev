#![no_main]

mod common;

use bytes::Bytes;
use cheetah_rtsp_core::{
    encode_interleaved_frame, CoreInput, RtcpPacket, RtpPacket, RtspCore, RtspMessageLimits,
};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    let bounded = common::bounded_bytes_payload(data, MAX_INPUT_BYTES);
    let limits = RtspMessageLimits::default();
    let records = common::decode_or_select_records(&bounded, 768);

    let mut core = RtspCore::with_limits(limits.clone());
    let control = common::tcp_control_payloads(&records, 256);
    for chunk in common::build_tcp_fault_chunks(&bounded, &control, 256) {
        let _ = core.handle_input(CoreInput::Bytes(Bytes::from(chunk)));
    }

    for (channel, payload) in common::tcp_interleaved_payloads(&records, 128) {
        if let Ok(frame) = encode_interleaved_frame(channel, &payload) {
            let _ = core.handle_input(CoreInput::Bytes(Bytes::from(frame)));
        }
        let _ = RtpPacket::parse(&payload);
        let _ = RtcpPacket::parse(&payload);
    }

    let rtp = common::udp_rtp_payloads(&records, 256);
    let rtcp = common::udp_rtcp_payloads(&records, 256);
    for payload in common::build_udp_fault_datagrams(&bounded, &rtp, &rtcp, 256) {
        let _ = RtpPacket::parse(&payload);
        let _ = RtcpPacket::parse(&payload);
    }

    common::fuzz_message_decoders(&bounded, limits, 6);
    let _ = core.handle_input(CoreInput::PeerClosed);
});
