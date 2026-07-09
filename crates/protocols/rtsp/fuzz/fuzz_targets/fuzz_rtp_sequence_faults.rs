#![no_main]

mod common;

use cheetah_rtsp_core::{RtcpPacket, RtpPacket};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    let bounded = common::bounded_bytes_payload(data, MAX_INPUT_BYTES);
    let records = common::decode_or_select_records(&bounded, 1024);
    let mut packets = common::udp_rtp_payloads(&records, 512);
    if packets.is_empty() {
        packets = common::tcp_interleaved_payloads(&records, 256)
            .into_iter()
            .map(|(_, payload)| payload)
            .collect();
    }
    if packets.is_empty() {
        packets.push(common::bounded_bytes_payload(&bounded, 1200));
    }

    let mut variants = Vec::new();
    variants.extend(packets.iter().take(128).cloned());
    if let Some(first) = packets.first().cloned() {
        if let Some(wrap_prev) = rtp_with_seq(&first, 0xFFFE) {
            variants.push(wrap_prev.clone());
            if let Some(wrap_last) = rtp_with_seq(&first, 0xFFFF) {
                variants.push(wrap_last);
            }
            if let Some(wrap_zero) = rtp_with_seq(&first, 0x0000) {
                variants.push(wrap_zero);
            }
            if let Some(ts_back) = rtp_with_timestamp_delta(&wrap_prev, -90_000) {
                variants.push(ts_back);
            }
        }
        variants.push(first.clone());
        variants.push(first);
    }

    if variants.len() >= 2 {
        variants.swap(0, 1);
    }
    for packet in variants.iter_mut().take(64) {
        toggle_marker_bit(packet);
    }
    if let Some(last) = variants.last_mut() {
        let keep = (last.len() / 2).max(1);
        last.truncate(keep);
    }

    for packet in variants.iter().take(512) {
        let _ = RtpPacket::parse(packet);
        let _ = RtcpPacket::parse(packet);
    }
});

fn rtp_with_seq(packet: &[u8], seq: u16) -> Option<Vec<u8>> {
    if packet.len() < 12 {
        return None;
    }
    let mut out = packet.to_vec();
    out[2..4].copy_from_slice(&seq.to_be_bytes());
    Some(out)
}

fn rtp_with_timestamp_delta(packet: &[u8], delta: i64) -> Option<Vec<u8>> {
    if packet.len() < 12 {
        return None;
    }
    let mut out = packet.to_vec();
    let current = u32::from_be_bytes([out[4], out[5], out[6], out[7]]);
    let updated = if delta.is_negative() {
        current.wrapping_sub(delta.unsigned_abs() as u32)
    } else {
        current.wrapping_add(delta as u32)
    };
    out[4..8].copy_from_slice(&updated.to_be_bytes());
    Some(out)
}

fn toggle_marker_bit(packet: &mut [u8]) {
    if packet.len() >= 2 {
        packet[1] ^= 0x80;
    }
}
