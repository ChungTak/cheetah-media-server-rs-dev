#![no_main]

//! Fuzz target for ICE candidate fragment parsing.
//!
//! Asserts:
//! 1. `extract_trickle_candidates` and `extract_trickle_ice_restart_creds`
//!    never panic on arbitrary inputs.
//! 2. Output candidates are non-empty and start with the expected prefix.
//! 3. ICE restart credentials, when present, are non-empty strings.

use cheetah_webrtc_module::compat::{extract_trickle_candidates, extract_trickle_ice_restart_creds};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };
    let candidates = extract_trickle_candidates(input);
    for c in &candidates {
        assert!(!c.is_empty(), "candidate must be non-empty");
        assert!(
            c.starts_with("candidate:"),
            "candidate must start with 'candidate:'"
        );
    }
    let creds = extract_trickle_ice_restart_creds(input);
    if let Some((ufrag, pwd)) = creds {
        assert!(!ufrag.is_empty(), "ufrag must be non-empty");
        assert!(!pwd.is_empty(), "pwd must be non-empty");
    }
});
