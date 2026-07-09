#![no_main]

//! Fuzz target for the SDP offer payload extractor.
//!
//! Phase 05 Task 03 contract:
//! - `extract_offer_payloads` never panics on arbitrary input.
//! - Payload type values are always in [0, 255] (u8 range).
//! - The function completes in bounded time (no infinite loops).
//! - No unbounded allocation from adversarial input.

use cheetah_webrtc_core::{extract_offer_payloads, preprocess_remote_sdp};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };

    // Direct extraction — must not panic
    let payloads = extract_offer_payloads(input);

    // Verify invariants on the result
    if let Some(pt) = payloads.h264 {
        assert!(pt <= 127, "H264 PT must be in dynamic range");
    }
    if let Some(pt) = payloads.h265 {
        assert!(pt <= 127, "H265 PT must be in dynamic range");
    }
    if let Some(pt) = payloads.opus {
        assert!(pt <= 127, "Opus PT must be in dynamic range");
    }

    // Preprocessing + extraction composition — must not panic
    let (sanitized, _) = preprocess_remote_sdp(input);
    let _ = extract_offer_payloads(&sanitized);
});
