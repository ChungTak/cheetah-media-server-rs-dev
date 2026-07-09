//! Complex RTMP handshake (FP9 HMAC-SHA256 digest scheme).
//!
//! This module is only compiled when the `complex-handshake` feature is enabled.
//! It provides detection and verification of the HMAC-SHA256 digest handshake
//! used by Flash Player 9+ clients.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// 30-byte key used by Flash Player to sign C1.
const GENUINE_FP_KEY: &[u8] = b"Genuine Adobe Flash Player 001";

/// 36-byte key used by Flash Media Server to sign S1.
const GENUINE_FMS_KEY: &[u8] = b"Genuine Adobe Flash Media Server 001";

/// Full 68-byte key for S2 generation (FMS key + 32 bytes).
const GENUINE_FMS_KEY_FULL: &[u8] = &[
    0x47, 0x65, 0x6e, 0x75, 0x69, 0x6e, 0x65, 0x20, 0x41, 0x64, 0x6f, 0x62, 0x65, 0x20, 0x46, 0x6c,
    0x61, 0x73, 0x68, 0x20, 0x4d, 0x65, 0x64, 0x69, 0x61, 0x20, 0x53, 0x65, 0x72, 0x76, 0x65, 0x72,
    0x20, 0x30, 0x30, 0x31, // "Genuine Adobe Flash Media Server 001"
    0xf0, 0xee, 0xc2, 0x4a, 0x80, 0x68, 0xbe, 0xe8, 0x2e, 0x00, 0xd0, 0xd1, 0x02, 0x9e, 0x7e, 0x57,
    0x6e, 0xec, 0x5d, 0x2d, 0x29, 0x80, 0x6f, 0xab, 0x93, 0xb8, 0xe6, 0x36, 0xcf, 0xeb, 0x31, 0xae,
];

/// Full 62-byte key for C1 verification (FP key + 32 bytes).
#[allow(dead_code)]
const GENUINE_FP_KEY_FULL: &[u8] = &[
    0x47, 0x65, 0x6e, 0x75, 0x69, 0x6e, 0x65, 0x20, 0x41, 0x64, 0x6f, 0x62, 0x65, 0x20, 0x46, 0x6c,
    0x61, 0x73, 0x68, 0x20, 0x50, 0x6c, 0x61, 0x79, 0x65, 0x72, 0x20, 0x30, 0x30, 0x31,
    // "Genuine Adobe Flash Player 001"
    0xf0, 0xee, 0xc2, 0x4a, 0x80, 0x68, 0xbe, 0xe8, 0x2e, 0x00, 0xd0, 0xd1, 0x02, 0x9e, 0x7e, 0x57,
    0x6e, 0xec, 0x5d, 0x2d, 0x29, 0x80, 0x6f, 0xab, 0x93, 0xb8, 0xe6, 0x36, 0xcf, 0xeb, 0x31, 0xae,
];

/// Detected handshake scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeScheme {
    /// Digest at offset computed from bytes [8..12].
    Scheme0,
    /// Digest at offset computed from bytes [772..776].
    Scheme1,
}

/// Try to detect and validate the complex handshake digest in a C1 packet.
/// Returns the scheme if validation succeeds, None if this is a simple handshake.
pub fn detect_client_scheme(c1: &[u8]) -> Option<HandshakeScheme> {
    if c1.len() < 1536 {
        return None;
    }
    // Try scheme 0 first (offset from bytes 8..12)
    if validate_c1_digest(c1, HandshakeScheme::Scheme0) {
        return Some(HandshakeScheme::Scheme0);
    }
    // Try scheme 1 (offset from bytes 772..776)
    if validate_c1_digest(c1, HandshakeScheme::Scheme1) {
        return Some(HandshakeScheme::Scheme1);
    }
    None
}

/// Validate the HMAC-SHA256 digest in C1 for the given scheme.
fn validate_c1_digest(c1: &[u8], scheme: HandshakeScheme) -> bool {
    let offset = digest_offset(c1, scheme);
    if offset + 32 > 1536 {
        return false;
    }

    let expected = compute_digest(c1, offset, GENUINE_FP_KEY);
    c1[offset..offset + 32] == expected
}

/// Build S1 packet with HMAC-SHA256 digest for complex handshake.
pub fn build_complex_s1(c1: &[u8], scheme: HandshakeScheme) -> [u8; 1536] {
    let mut s1 = [0u8; 1536];

    // Timestamp (4 bytes) + server version (4 bytes)
    s1[4] = 0x04;
    s1[5] = 0x05;
    s1[6] = 0x00;
    s1[7] = 0x01; // FMS version 4.5.0.1

    // Fill random data (seeded from C1 for determinism in tests)
    let mut seed = 0xdeadbeef_u64;
    for &b in c1.iter().take(64) {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(b as u64);
    }
    for byte in s1[8..].iter_mut() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *byte = (seed >> 33) as u8;
    }

    // Compute and insert digest
    let offset = digest_offset(&s1, scheme);
    let digest = compute_digest(&s1, offset, GENUINE_FMS_KEY);
    s1[offset..offset + 32].copy_from_slice(&digest);

    s1
}

/// Build S2 packet for complex handshake (keyed hash of C1 random data).
pub fn build_complex_s2(c1: &[u8], scheme: HandshakeScheme) -> [u8; 1536] {
    let mut s2 = [0u8; 1536];

    // Extract C1 digest
    let c1_digest_offset = digest_offset(c1, scheme);
    let c1_digest = &c1[c1_digest_offset..c1_digest_offset + 32];

    // Derive key from C1 digest using full FMS key
    let mut mac =
        HmacSha256::new_from_slice(GENUINE_FMS_KEY_FULL).expect("HMAC accepts any key size");
    mac.update(c1_digest);
    let temp_key = mac.finalize().into_bytes();

    // Fill S2 with random data
    let mut seed = 0xcafebabe_u64;
    for &b in c1.iter().take(32) {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(b as u64);
    }
    for byte in s2[..1536 - 32].iter_mut() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *byte = (seed >> 33) as u8;
    }

    // Compute HMAC of S2[0..1504] using derived key, place at end
    let mut mac = HmacSha256::new_from_slice(&temp_key).expect("HMAC accepts any key size");
    mac.update(&s2[..1536 - 32]);
    let signature = mac.finalize().into_bytes();
    s2[1536 - 32..].copy_from_slice(&signature);

    s2
}

/// Compute the digest offset for a given scheme.
fn digest_offset(packet: &[u8], scheme: HandshakeScheme) -> usize {
    let base = match scheme {
        HandshakeScheme::Scheme0 => 8,
        HandshakeScheme::Scheme1 => 772,
    };
    let offset_sum = packet[base] as usize
        + packet[base + 1] as usize
        + packet[base + 2] as usize
        + packet[base + 3] as usize;

    // Digest is placed within a 764-byte region starting at base+4
    let region_start = base + 4;
    region_start + (offset_sum % 728)
}

/// Compute HMAC-SHA256 digest over the packet excluding the 32-byte digest slot.
fn compute_digest(packet: &[u8], digest_offset: usize, key: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(&packet[..digest_offset]);
    if digest_offset + 32 < packet.len() {
        mac.update(&packet[digest_offset + 32..]);
    }
    let result = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_offset_is_within_bounds() {
        let packet = [0xffu8; 1536];
        let offset0 = digest_offset(&packet, HandshakeScheme::Scheme0);
        let offset1 = digest_offset(&packet, HandshakeScheme::Scheme1);
        assert!(offset0 + 32 <= 1536);
        assert!(offset1 + 32 <= 1536);
    }

    #[test]
    fn build_complex_s1_produces_valid_digest() {
        let c1 = [0x42u8; 1536];
        let s1 = build_complex_s1(&c1, HandshakeScheme::Scheme0);
        let offset = digest_offset(&s1, HandshakeScheme::Scheme0);
        let expected = compute_digest(&s1, offset, GENUINE_FMS_KEY);
        assert_eq!(&s1[offset..offset + 32], &expected);
    }

    #[test]
    fn detect_returns_none_for_simple_handshake() {
        // Simple handshake: version bytes are [0,0,0,0], no valid digest
        let c1 = [0u8; 1536];
        assert_eq!(detect_client_scheme(&c1), None);
    }

    #[test]
    fn roundtrip_scheme0() {
        // Build a C1 with valid scheme0 digest
        let mut c1 = [0x55u8; 1536];
        c1[4] = 0x09; // version > 0 to look like FP9+
        c1[5] = 0x00;
        c1[6] = 0x7c;
        c1[7] = 0x02;
        let offset = digest_offset(&c1, HandshakeScheme::Scheme0);
        let digest = compute_digest(&c1, offset, GENUINE_FP_KEY);
        c1[offset..offset + 32].copy_from_slice(&digest);

        let detected = detect_client_scheme(&c1);
        assert_eq!(detected, Some(HandshakeScheme::Scheme0));
    }
}
