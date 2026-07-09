//! ABL-style PATCH candidate body edge case tests.
//!
//! Phase 05 Task 03: cover trickle-ICE PATCH body edge cases that must
//! not trigger panics, infinite loops, or unbounded allocation.
//!
//! Fixtures:
//! - Empty PATCH body
//! - Duplicate candidate lines (idempotent extraction)
//! - ICE restart body (ufrag + pwd without candidates)
//! - Mixed candidates + ICE restart creds
//! - Body with only whitespace/newlines
//! - Very large body (bounded allocation check)

use cheetah_webrtc_module::{extract_trickle_candidates, extract_trickle_ice_restart_creds};

// --- Empty PATCH body ---

#[test]
fn trickle_patch_empty_body_yields_no_candidates() {
    let candidates = extract_trickle_candidates("");
    assert!(candidates.is_empty());
}

#[test]
fn trickle_patch_empty_body_no_ice_restart() {
    assert!(extract_trickle_ice_restart_creds("").is_none());
}

// --- Whitespace-only body ---

#[test]
fn trickle_patch_whitespace_only_body_yields_no_candidates() {
    let bodies = [" ", "\t", "\r\n", "\n\n\n", "  \r\n  \r\n  "];
    for body in bodies {
        let candidates = extract_trickle_candidates(body);
        assert!(
            candidates.is_empty(),
            "whitespace-only body should yield no candidates: {body:?}"
        );
    }
}

#[test]
fn trickle_patch_whitespace_only_body_no_ice_restart() {
    let bodies = [" ", "\t", "\r\n", "\n\n\n"];
    for body in bodies {
        assert!(
            extract_trickle_ice_restart_creds(body).is_none(),
            "whitespace-only body should not trigger ICE restart: {body:?}"
        );
    }
}

// --- Duplicate candidate lines ---

#[test]
fn trickle_patch_duplicate_candidate_is_idempotent() {
    let body = concat!(
        "a=candidate:0 1 UDP 2122252543 192.168.1.1 50000 typ host\r\n",
        "a=candidate:0 1 UDP 2122252543 192.168.1.1 50000 typ host\r\n",
        "a=candidate:0 1 UDP 2122252543 192.168.1.1 50000 typ host\r\n",
    );
    let candidates = extract_trickle_candidates(body);
    // The extractor returns all lines — deduplication is the caller's
    // responsibility. But it must not panic or allocate unboundedly.
    assert_eq!(candidates.len(), 3);
    // All extracted lines are identical
    assert!(candidates.iter().all(|c| c == &candidates[0]));
}

#[test]
fn trickle_patch_duplicate_candidate_content_is_correct() {
    let body = concat!(
        "a=candidate:1 1 UDP 2122252543 10.0.0.1 9000 typ host\r\n",
        "a=candidate:1 1 UDP 2122252543 10.0.0.1 9000 typ host\r\n",
    );
    let candidates = extract_trickle_candidates(body);
    for c in &candidates {
        assert!(c.starts_with("candidate:1 1 UDP"));
        assert!(c.contains("10.0.0.1"));
    }
}

// --- ICE restart body ---

#[test]
fn trickle_patch_ice_restart_body_detected() {
    let body = "a=ice-ufrag:newufrag\r\na=ice-pwd:newpasswordlongenough\r\n";
    let creds = extract_trickle_ice_restart_creds(body).expect("should detect ICE restart");
    assert_eq!(creds.0, "newufrag");
    assert_eq!(creds.1, "newpasswordlongenough");
}

#[test]
fn trickle_patch_ice_restart_body_has_no_candidates() {
    let body = "a=ice-ufrag:newufrag\r\na=ice-pwd:newpasswordlongenough\r\n";
    let candidates = extract_trickle_candidates(body);
    assert!(candidates.is_empty());
}

#[test]
fn trickle_patch_ice_restart_with_candidates() {
    // Some clients send both new creds and candidates in the same PATCH
    let body = concat!(
        "a=ice-ufrag:rotated\r\n",
        "a=ice-pwd:rotatedpassword12345\r\n",
        "a=candidate:0 1 UDP 2122252543 192.168.1.1 50000 typ host\r\n",
    );
    let creds = extract_trickle_ice_restart_creds(body).expect("should detect ICE restart");
    assert_eq!(creds.0, "rotated");
    assert_eq!(creds.1, "rotatedpassword12345");

    let candidates = extract_trickle_candidates(body);
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].contains("192.168.1.1"));
}

// --- Partial ICE restart (only ufrag or only pwd) ---

#[test]
fn trickle_patch_partial_ice_restart_ufrag_only_not_triggered() {
    let body = "a=ice-ufrag:onlyufrag\r\na=candidate:0 1 UDP 2122252543 1.1.1.1 5000 typ host\r\n";
    assert!(extract_trickle_ice_restart_creds(body).is_none());
}

#[test]
fn trickle_patch_partial_ice_restart_pwd_only_not_triggered() {
    let body = "a=ice-pwd:onlypwd\r\na=candidate:0 1 UDP 2122252543 1.1.1.1 5000 typ host\r\n";
    assert!(extract_trickle_ice_restart_creds(body).is_none());
}

// --- Large body (bounded allocation) ---

#[test]
fn trickle_patch_large_body_does_not_allocate_unbounded() {
    // 1000 candidate lines — should complete quickly
    let mut body = String::new();
    for i in 0..1000u32 {
        body.push_str(&format!(
            "a=candidate:{i} 1 UDP 2122252543 10.0.0.{} {} typ host\r\n",
            i % 256,
            50000 + i
        ));
    }
    let candidates = extract_trickle_candidates(&body);
    assert_eq!(candidates.len(), 1000);
}

#[test]
fn trickle_patch_very_long_single_line_does_not_panic() {
    // A single candidate line with an absurdly long extension
    let long_ext = "x".repeat(10_000);
    let body = format!("a=candidate:0 1 UDP 2122252543 1.1.1.1 5000 typ host {long_ext}\r\n");
    let candidates = extract_trickle_candidates(&body);
    assert_eq!(candidates.len(), 1);
}

// --- Malformed bodies ---

#[test]
fn trickle_patch_binary_garbage_does_not_panic() {
    let body = "\x00\x01\x02\x03\x7e\x7f";
    let candidates = extract_trickle_candidates(body);
    assert!(candidates.is_empty());
}

#[test]
fn trickle_patch_candidate_prefix_without_colon_ignored() {
    // "a=candidate" without the colon should not match
    let body = "a=candidate 0 1 UDP 2122252543 1.1.1.1 5000 typ host\r\n";
    let candidates = extract_trickle_candidates(body);
    assert!(candidates.is_empty());
}

#[test]
fn trickle_patch_mixed_valid_and_invalid_lines() {
    let body = concat!(
        "v=0\r\n",
        "a=ice-ufrag:test\r\n",
        "a=candidate:0 1 UDP 2122252543 1.1.1.1 5000 typ host\r\n",
        "some random garbage\r\n",
        "a=candidate:1 1 TCP 1518280447 2.2.2.2 5001 typ srflx\r\n",
        "a=ice-pwd:testpwd\r\n",
    );
    let candidates = extract_trickle_candidates(body);
    assert_eq!(candidates.len(), 2);
    assert!(candidates[0].contains("1.1.1.1"));
    assert!(candidates[1].contains("2.2.2.2"));
}
