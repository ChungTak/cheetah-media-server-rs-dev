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
    common::fuzz_message_decoders(&bounded, limits.clone(), 8);

    let records = common::decode_or_select_records(&bounded, 1024);
    let control_payloads = common::tcp_control_payloads(&records, 512);
    if control_payloads.is_empty() {
        common::fuzz_core_entry(&bounded, limits, 9);
        return;
    }

    let mut core = RtspCore::new();
    for chunk in common::build_tcp_fault_chunks(&bounded, &control_payloads, 512) {
        let _ = core.handle_input(CoreInput::Bytes(Bytes::from(chunk)));
    }

    for payload in control_payloads.iter().take(64) {
        common::fuzz_message_decoders(payload, limits.clone(), 7);
    }

    let interleaved = common::tcp_interleaved_payloads(&records, 256);
    for (channel, payload) in interleaved {
        if let Ok(frame) = encode_interleaved_frame(channel, &payload) {
            let _ = core.handle_input(CoreInput::Bytes(Bytes::from(frame)));
        }
        let _ = RtpPacket::parse(&payload);
        let _ = RtcpPacket::parse(&payload);
    }

    let _ = core.handle_input(CoreInput::PeerClosed);
});
