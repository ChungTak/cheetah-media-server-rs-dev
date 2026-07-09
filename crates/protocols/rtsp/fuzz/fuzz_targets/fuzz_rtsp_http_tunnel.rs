#![no_main]

mod common;

use base64::{engine::general_purpose::STANDARD, Engine};
use bytes::Bytes;
use cheetah_rtsp_core::{CoreInput, RtspCore, RtspMessageLimits};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_DECODED_BYTES: usize = 96 * 1024;

fuzz_target!(|data: &[u8]| {
    let bounded = common::bounded_bytes_payload(data, MAX_INPUT_BYTES);
    let limits = RtspMessageLimits::default();
    let records = common::decode_or_select_records(&bounded, 512);

    let control = common::tcp_control_payloads(&records, 256);
    let mut chunks = common::build_tcp_fault_chunks(&bounded, &control, 256);
    if chunks.is_empty() {
        chunks.push(common::build_mixed_rtsp_interleaved_input(
            &bounded,
            limits.max_interleaved_frame_size,
        ));
    }

    let mut raw = Vec::new();
    for payload in chunks.iter().take(256) {
        raw.extend_from_slice(payload);
        if raw.len() >= MAX_DECODED_BYTES {
            raw.truncate(MAX_DECODED_BYTES);
            break;
        }
    }

    let encoded = STANDARD.encode(raw);
    let split_mode = bounded.first().copied().unwrap_or(0) & 1 == 0;
    let invalid_mode = bounded.get(1).copied().unwrap_or(0) & 1 == 1;
    let decoded = decode_http_tunnel_post_stream(&encoded, &bounded, split_mode, invalid_mode);

    let mut core = RtspCore::with_limits(limits.clone());
    for chunk in decoded.chunks(usize::from(bounded.get(2).copied().unwrap_or(8) % 32 + 1)) {
        let _ = core.handle_input(CoreInput::Bytes(Bytes::copy_from_slice(chunk)));
    }
    common::fuzz_message_decoders(&decoded, limits, 7);
    let _ = core.handle_input(CoreInput::PeerClosed);
});

fn decode_http_tunnel_post_stream(
    encoded: &str,
    data: &[u8],
    split_mode: bool,
    invalid_mode: bool,
) -> Vec<u8> {
    let mut staged = encoded.as_bytes().to_vec();
    if invalid_mode && !staged.is_empty() {
        let idx = usize::from(data.get(3).copied().unwrap_or(0)) % staged.len();
        staged[idx] = b'!';
    }

    if !split_mode {
        return STANDARD
            .decode(staged)
            .unwrap_or_default()
            .into_iter()
            .take(MAX_DECODED_BYTES)
            .collect();
    }

    let mut out = Vec::new();
    let mut carry = Vec::new();
    let step = usize::from(data.get(4).copied().unwrap_or(0) % 3) + 1;
    for part in staged.chunks(step) {
        carry.extend_from_slice(part);
        let aligned = carry.len() / 4 * 4;
        if aligned == 0 {
            continue;
        }
        let remainder = carry.split_off(aligned);
        if let Ok(decoded) = STANDARD.decode(&carry) {
            out.extend_from_slice(&decoded);
            if out.len() >= MAX_DECODED_BYTES {
                out.truncate(MAX_DECODED_BYTES);
                return out;
            }
        }
        carry = remainder;
    }

    if !carry.is_empty() {
        while carry.len() % 4 != 0 {
            carry.push(b'=');
        }
        if let Ok(decoded) = STANDARD.decode(&carry) {
            out.extend_from_slice(&decoded);
        }
    }

    if out.len() > MAX_DECODED_BYTES {
        out.truncate(MAX_DECODED_BYTES);
    }
    out
}
