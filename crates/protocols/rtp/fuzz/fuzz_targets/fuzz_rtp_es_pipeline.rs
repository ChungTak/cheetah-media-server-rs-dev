#![no_main]

use cheetah_codec::{depacketize_payload, RtpHeader, RtpPacket};
use libfuzzer_sys::fuzz_target;

/// Run an RTP ES depacketize pipeline over arbitrary bytes. Splits the input into 1KiB chunks
/// wrapped as RTP packets and feeds them through `depacketize_payload`. Should never panic.
fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    let mut packets = Vec::new();
    let mut seq: u16 = 0;
    for chunk in data.chunks(1024) {
        packets.push(RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: 0,
                ssrc: 1,
                marker: false,
            },
            payload: bytes::Bytes::copy_from_slice(chunk),
        });
        seq = seq.wrapping_add(1);
    }
    let _ = depacketize_payload(packets);
});
