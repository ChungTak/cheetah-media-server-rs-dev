#![no_main]

use cheetah_codec::{RtpReorderBuffer, RtpReorderSettings};
use cheetah_rtsp_core::RtpPacket;
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_PACKET_BYTES: usize = 1500;
const MAX_EVENTS: usize = 2048;

fuzz_target!(|data: &[u8]| {
    let bounded = &data[..data.len().min(MAX_INPUT_BYTES)];
    if bounded.is_empty() {
        return;
    }

    let settings = RtpReorderSettings {
        max_packets: usize::from(bounded.first().copied().unwrap_or(0) % 64) + 1,
        max_delay_ms: u64::from(bounded.get(1).copied().unwrap_or(0)) * 4,
    };

    let mut reorder = RtpReorderBuffer::new(settings);
    let mut seq = u16::from_be_bytes([
        bounded.get(2).copied().unwrap_or(0),
        bounded.get(3).copied().unwrap_or(0),
    ]);
    let mut arrival = 0u64;

    for chunk in bounded.chunks(4).take(MAX_EVENTS) {
        let delta = chunk.first().copied().unwrap_or(0) % 5;
        seq = match delta {
            0 => seq.wrapping_add(1),
            1 => seq.wrapping_add(2),
            2 => seq.wrapping_sub(1),
            3 => seq,
            _ => seq.wrapping_add(u16::from(chunk.get(1).copied().unwrap_or(0))),
        };
        arrival = arrival.saturating_add(u64::from(chunk.get(2).copied().unwrap_or(0) % 8));

        let packet = build_packet_payload(bounded, seq, usize::from(chunk.get(3).copied().unwrap_or(0)));
        let released = reorder.push(seq, arrival, packet);
        for payload in released.into_iter().take(16) {
            let _ = RtpPacket::parse(&payload);
        }

        if chunk.get(1).copied().unwrap_or(0) & 0x80 == 0x80 {
            reorder.reset();
        }
    }

    let _ = reorder.pending_len();
});

fn build_packet_payload(seed: &[u8], seq: u16, size_hint: usize) -> Vec<u8> {
    let size = size_hint.clamp(12, MAX_PACKET_BYTES);
    let mut packet = vec![0u8; size];
    packet[0] = 0x80;
    packet[1] = 96;
    packet[2..4].copy_from_slice(&seq.to_be_bytes());

    let ts = u32::from(seq).wrapping_mul(3000);
    packet[4..8].copy_from_slice(&ts.to_be_bytes());
    packet[8..12].copy_from_slice(&0x0102_0304u32.to_be_bytes());

    if size > 12 && !seed.is_empty() {
        for i in 12..size {
            packet[i] = seed[i % seed.len()];
        }
    }

    packet
}
