#![no_main]

//! Fuzz target for the trickle-ICE candidate extractor.
//!
//! Phase 05 contract: `extract_trickle_candidates` must never panic
//! and every line it yields must begin with `candidate:` and be
//! longer than the prefix (empty `candidate:` tokens are silently
//! dropped at the source).

use cheetah_webrtc_module::extract_trickle_candidates;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };
    let candidates = extract_trickle_candidates(input);
    for line in &candidates {
        assert!(
            line.starts_with("candidate:"),
            "every extracted line must start with `candidate:`"
        );
        assert!(
            line.len() > "candidate:".len(),
            "empty `candidate:` tokens must be filtered"
        );
    }
});
