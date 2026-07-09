#![no_main]

//! Fuzz target for the `a=ssrc-group:SIM` RID-injection preprocessor.
//!
//! Asserts:
//! 1. `inject_rid_from_ssrc_group_sim` never panics on arbitrary inputs.
//! 2. The injection is idempotent: a second pass on the output never
//!    re-injects and produces the same result.

use cheetah_webrtc_core::inject_rid_from_ssrc_group_sim;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };
    let mut sdp = input.to_string();
    let _ = inject_rid_from_ssrc_group_sim(&mut sdp);
    // Idempotency: a second pass should never inject again
    let mut sdp2 = sdp.clone();
    let injected_again = inject_rid_from_ssrc_group_sim(&mut sdp2);
    assert!(!injected_again, "second pass must not inject again");
    assert_eq!(sdp, sdp2, "second pass must not change the SDP");
});
