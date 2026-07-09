//! Parameter Set Bootstrap Regression Tests — Phase 03 Task 03
//!
//! Real-world regression tests simulating non-surveillance device scenarios
//! where I-frames arrive without parameter sets (SPS/PPS/VPS). This is the
//! behavior documented in ABL 2025-10-14: non-surveillance device video streams
//! may have I-frames without SPS/PPS/VPS, and the bootstrap view must prepend
//! cached parameter sets to ensure decodability.
//!
//! Test fixtures use realistic NALU byte patterns derived from common encoder
//! configurations (x264 Baseline/High, x265 Main, resolution changes).

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, AccessUnit, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind,
    ParameterSetCache, Timebase, TrackId,
};

// ============================================================================
// Realistic H264 fixtures — simulating x264 Baseline and High profile outputs
// ============================================================================

/// Realistic H264 SPS for 1920x1080 High profile, level 4.0
/// NAL type 7 (0x67), profile_idc=100 (High), level_idc=40
fn h264_sps_1080p_high() -> &'static [u8] {
    &[
        0x67, 0x64, 0x00, 0x28, 0xac, 0xd9, 0x40, 0x78, 0x02, 0x27, 0xe5, 0xc0, 0x44, 0x00, 0x00,
        0x03, 0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0xf0, 0x3c, 0x60, 0xc6, 0x58,
    ]
}

/// Realistic H264 PPS for High profile with CABAC entropy coding
/// NAL type 8 (0x68)
fn h264_pps_high() -> &'static [u8] {
    &[0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0]
}

/// Realistic H264 IDR slice (NAL type 5 = 0x65) — first few bytes of a
/// real IDR slice from x264 High profile encoder output
fn h264_idr_slice_realistic() -> &'static [u8] {
    &[
        0x65, 0x88, 0x80, 0x40, 0x00, 0x9f, 0xf0, 0x15, 0x22, 0xbe, 0x05, 0xa8, 0x10, 0x9c, 0x62,
        0x8c, 0x40, 0x3e, 0x11, 0x00, 0x04, 0x38, 0xc5, 0x18, 0x80, 0x7c, 0x22, 0x00, 0x08, 0x71,
        0x8a, 0x31, 0x00, 0xf8, 0x44, 0x00, 0x10, 0xe3, 0x14, 0x62, 0x01, 0xf0, 0x88, 0x00, 0x21,
        0xc6, 0x28, 0xc4, 0x03, 0xe1, 0x10, 0x00, 0x43, 0x8c, 0x51, 0x88,
    ]
}

/// H264 SPS for 1280x720 Baseline profile (resolution change scenario)
/// NAL type 7 (0x67), profile_idc=66 (Baseline), level_idc=31
fn h264_sps_720p_baseline() -> &'static [u8] {
    &[
        0x67, 0x42, 0xc0, 0x1f, 0xd9, 0x00, 0xa0, 0x5b, 0x20, 0x00, 0x00, 0x03, 0x00, 0x20, 0x00,
        0x00, 0x07, 0x91, 0xe2, 0xc5, 0xb2, 0xc0,
    ]
}

/// H264 PPS for Baseline profile (CAVLC entropy)
fn h264_pps_baseline() -> &'static [u8] {
    &[0x68, 0xce, 0x38, 0x80]
}

// ============================================================================
// Realistic H265 fixtures — simulating x265 Main profile outputs
// ============================================================================

/// Realistic H265 VPS (NAL type 32, header bytes 0x40 0x01)
/// Simulates x265 Main profile, level 4.0, 1920x1080
fn h265_vps_1080p_main() -> &'static [u8] {
    &[
        0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00,
        0x03, 0x00, 0x00, 0x03, 0x00, 0x78, 0x95, 0x98, 0x09,
    ]
}

/// Realistic H265 SPS (NAL type 33, header bytes 0x42 0x01)
fn h265_sps_1080p_main() -> &'static [u8] {
    &[
        0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x03, 0x00, 0x78, 0xa0, 0x03, 0xc0, 0x80, 0x10, 0xe5, 0x96, 0x56, 0x69, 0x24, 0xca, 0xe0,
        0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03, 0x01, 0xe0, 0x80,
    ]
}

/// Realistic H265 PPS (NAL type 34, header bytes 0x44 0x01)
fn h265_pps_1080p_main() -> &'static [u8] {
    &[0x44, 0x01, 0xc1, 0x72, 0xb4, 0x62, 0x40]
}

/// Realistic H265 IDR_W_RADL slice (NAL type 19, header bytes 0x26 0x01)
/// First bytes of a real x265 IDR slice
fn h265_idr_slice_realistic() -> &'static [u8] {
    &[
        0x26, 0x01, 0xaf, 0x08, 0x1c, 0x40, 0x5c, 0x68, 0x12, 0x26, 0x53, 0x94, 0xd4, 0xb1, 0x20,
        0x40, 0x60, 0x88, 0x09, 0x30, 0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0x40, 0x00, 0x00, 0x07,
        0x82, 0x00, 0x01, 0xf4, 0x80, 0x00, 0x3e, 0x90, 0x00, 0x07, 0xd2, 0x00, 0x00, 0xfa, 0x40,
        0x00, 0x1f, 0x48, 0x00, 0x03, 0xe9, 0x00, 0x00, 0x7d, 0x20, 0x00,
    ]
}

/// H265 VPS for 1280x720 (resolution change scenario)
fn h265_vps_720p() -> &'static [u8] {
    &[
        0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00,
        0x03, 0x00, 0x00, 0x03, 0x00, 0x5d, 0x95, 0x98, 0x09,
    ]
}

/// H265 SPS for 1280x720
fn h265_sps_720p() -> &'static [u8] {
    &[
        0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x03, 0x00, 0x5d, 0xa0, 0x02, 0x80, 0x80, 0x2d, 0x16, 0x59, 0x59, 0xa4, 0x93, 0x2b, 0x80,
        0x40, 0x00, 0x00, 0x03, 0x00, 0x40, 0x00, 0x00, 0x07, 0x82, 0x00,
    ]
}

/// H265 PPS for 720p
fn h265_pps_720p() -> &'static [u8] {
    &[0x44, 0x01, 0xc1, 0x72, 0xb4, 0x24, 0x20]
}

// ============================================================================
// Helper: build Annex-B payload from NALUs
// ============================================================================

fn build_annexb(nalus: &[&[u8]]) -> Bytes {
    let mut buf = Vec::new();
    for nalu in nalus {
        buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        buf.extend_from_slice(nalu);
    }
    Bytes::from(buf)
}

fn build_avframe_keyframe(codec: CodecId, payload: Bytes, pts: i64) -> AVFrame {
    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Video,
        codec,
        FrameFormat::CanonicalH26x,
        pts,
        pts,
        Timebase::new(1, 90_000),
        payload,
    );
    frame.flags.insert(FrameFlags::KEY);
    frame
}

// ============================================================================
// Regression 1: H264 IDR without SPS/PPS is bootstrapped
//
// Scenario: Non-surveillance IP camera (e.g., consumer webcam, action camera)
// sends IDR frames without preceding SPS/PPS NALUs. The parameter set cache
// was previously populated from an earlier keyframe or out-of-band signaling.
// The bootstrap view must prepend the cached SPS/PPS before the IDR.
// ============================================================================

#[test]
fn h264_idr_without_sps_pps_is_bootstrapped() {
    // Step 1: Populate cache from an initial keyframe that DID contain SPS/PPS
    let mut cache = ParameterSetCache::default();
    let initial_keyframe = build_annexb(&[
        h264_sps_1080p_high(),
        h264_pps_high(),
        h264_idr_slice_realistic(),
    ]);
    assert!(
        cache.update_from_annexb(CodecId::H264, &initial_keyframe),
        "initial keyframe should populate SPS/PPS cache"
    );
    assert!(cache.has_required_sets(CodecId::H264));

    // Step 2: Simulate a subsequent IDR that arrives WITHOUT SPS/PPS
    // (non-surveillance device behavior per ABL 2025-10-14)
    let bare_idr = build_annexb(&[h264_idr_slice_realistic()]);

    // Step 3: Bootstrap view should prepend cached SPS/PPS
    let bootstrapped = cache.prepend_to_annexb_access_unit(CodecId::H264, &bare_idr);

    // Verify: output is larger than input (parameter sets were prepended)
    assert!(
        bootstrapped.len() > bare_idr.len(),
        "bootstrapped payload ({} bytes) must be larger than bare IDR ({} bytes)",
        bootstrapped.len(),
        bare_idr.len()
    );

    // Verify: SPS appears in output before IDR
    let sps_needle = h264_sps_1080p_high();
    let idr_needle = h264_idr_slice_realistic();
    let sps_pos = bootstrapped
        .windows(sps_needle.len())
        .position(|w| w == sps_needle)
        .expect("SPS must be present in bootstrapped output");
    let idr_pos = bootstrapped
        .windows(idr_needle.len())
        .position(|w| w == idr_needle)
        .expect("IDR must be present in bootstrapped output");
    assert!(
        sps_pos < idr_pos,
        "SPS (pos {sps_pos}) must appear before IDR (pos {idr_pos})"
    );

    // Verify: PPS appears between SPS and IDR
    let pps_needle = h264_pps_high();
    let pps_pos = bootstrapped
        .windows(pps_needle.len())
        .position(|w| w == pps_needle)
        .expect("PPS must be present in bootstrapped output");
    assert!(
        pps_pos > sps_pos && pps_pos < idr_pos,
        "PPS (pos {pps_pos}) must appear between SPS (pos {sps_pos}) and IDR (pos {idr_pos})"
    );

    // Verify: output has proper Annex-B start codes
    assert!(
        bootstrapped.starts_with(&[0x00, 0x00, 0x00, 0x01]),
        "bootstrapped output must start with 4-byte start code"
    );
}

#[test]
fn h264_idr_without_sps_pps_bootstrapped_via_access_unit() {
    // Same scenario but using the AccessUnit-based API
    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&CodecExtradata::H264 {
        sps: vec![Bytes::copy_from_slice(h264_sps_1080p_high())],
        pps: vec![Bytes::copy_from_slice(h264_pps_high())],
        avcc: None,
    });

    // Bare IDR as a single-unit AccessUnit (no parameter sets)
    let mut au = AccessUnit::from_units(vec![Bytes::copy_from_slice(h264_idr_slice_realistic())]);
    assert_eq!(au.units.len(), 1, "bare IDR should be single unit");

    cache.prepend_to_access_unit(CodecId::H264, &mut au);

    // Should now have SPS + PPS + IDR = 3 units
    assert_eq!(
        au.units.len(),
        3,
        "bootstrapped AU should have SPS + PPS + IDR"
    );
    assert_eq!(
        au.units[0].as_ref(),
        h264_sps_1080p_high(),
        "first unit must be SPS"
    );
    assert_eq!(
        au.units[1].as_ref(),
        h264_pps_high(),
        "second unit must be PPS"
    );
    assert_eq!(
        au.units[2].as_ref(),
        h264_idr_slice_realistic(),
        "third unit must be IDR slice"
    );
}

#[test]
fn h264_idr_without_sps_pps_bootstrapped_via_repair_frame() {
    // Test the repair_h26x_keyframe_frame path: cache was populated from
    // a prior frame, and a new bare IDR arrives as an AVFrame.
    let mut cache = ParameterSetCache::default();

    // First frame: full keyframe with SPS/PPS/IDR — populates cache
    let full_payload = build_annexb(&[
        h264_sps_1080p_high(),
        h264_pps_high(),
        h264_idr_slice_realistic(),
    ]);
    let mut first_frame = build_avframe_keyframe(CodecId::H264, full_payload, 0);
    let discovered = cache.repair_h26x_keyframe_frame(&mut first_frame);
    assert!(
        discovered.is_some(),
        "first frame should discover extradata"
    );

    // Second frame: bare IDR only (non-surveillance device behavior)
    let bare_payload = build_annexb(&[h264_idr_slice_realistic()]);
    let mut second_frame = build_avframe_keyframe(CodecId::H264, bare_payload.clone(), 90_000);

    // repair should prepend cached SPS/PPS
    cache.repair_h26x_keyframe_frame(&mut second_frame);

    assert!(
        second_frame.payload.len() > bare_payload.len(),
        "repaired frame payload must be larger than bare IDR"
    );
    // Verify SPS is at the start of the repaired payload
    assert!(
        second_frame
            .payload
            .starts_with(&[0x00, 0x00, 0x00, 0x01, 0x67]),
        "repaired frame must start with start_code + SPS NAL type"
    );
}

// ============================================================================
// Regression 2: H265 IDR without VPS/SPS/PPS is bootstrapped
//
// Scenario: Non-surveillance H265 encoder (e.g., OBS with x265, mobile device)
// sends IDR_W_RADL frames without preceding VPS/SPS/PPS. The bootstrap view
// must prepend all three cached parameter sets.
// ============================================================================

#[test]
fn h265_idr_without_vps_sps_pps_is_bootstrapped() {
    // Step 1: Populate cache from initial keyframe with full parameter sets
    let mut cache = ParameterSetCache::default();
    let initial_keyframe = build_annexb(&[
        h265_vps_1080p_main(),
        h265_sps_1080p_main(),
        h265_pps_1080p_main(),
        h265_idr_slice_realistic(),
    ]);
    assert!(
        cache.update_from_annexb(CodecId::H265, &initial_keyframe),
        "initial H265 keyframe should populate VPS/SPS/PPS cache"
    );
    assert!(cache.has_required_sets(CodecId::H265));

    // Step 2: Subsequent IDR arrives WITHOUT any parameter sets
    let bare_idr = build_annexb(&[h265_idr_slice_realistic()]);

    // Step 3: Bootstrap view prepends all three parameter sets
    let bootstrapped = cache.prepend_to_annexb_access_unit(CodecId::H265, &bare_idr);

    // Verify: output is larger
    assert!(
        bootstrapped.len() > bare_idr.len(),
        "bootstrapped H265 payload ({} bytes) must be larger than bare IDR ({} bytes)",
        bootstrapped.len(),
        bare_idr.len()
    );

    // Verify ordering: VPS < SPS < PPS < IDR
    let vps_needle = h265_vps_1080p_main();
    let sps_needle = h265_sps_1080p_main();
    let pps_needle = h265_pps_1080p_main();
    let idr_needle = h265_idr_slice_realistic();

    let vps_pos = bootstrapped
        .windows(vps_needle.len())
        .position(|w| w == vps_needle)
        .expect("VPS must be present in bootstrapped output");
    let sps_pos = bootstrapped
        .windows(sps_needle.len())
        .position(|w| w == sps_needle)
        .expect("SPS must be present in bootstrapped output");
    let pps_pos = bootstrapped
        .windows(pps_needle.len())
        .position(|w| w == pps_needle)
        .expect("PPS must be present in bootstrapped output");
    let idr_pos = bootstrapped
        .windows(idr_needle.len())
        .position(|w| w == idr_needle)
        .expect("IDR must be present in bootstrapped output");

    assert!(
        vps_pos < sps_pos,
        "VPS (pos {vps_pos}) must appear before SPS (pos {sps_pos})"
    );
    assert!(
        sps_pos < pps_pos,
        "SPS (pos {sps_pos}) must appear before PPS (pos {pps_pos})"
    );
    assert!(
        pps_pos < idr_pos,
        "PPS (pos {pps_pos}) must appear before IDR (pos {idr_pos})"
    );
}

#[test]
fn h265_idr_without_vps_sps_pps_bootstrapped_via_access_unit() {
    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&CodecExtradata::H265 {
        vps: vec![Bytes::copy_from_slice(h265_vps_1080p_main())],
        sps: vec![Bytes::copy_from_slice(h265_sps_1080p_main())],
        pps: vec![Bytes::copy_from_slice(h265_pps_1080p_main())],
        hvcc: None,
    });

    // Bare IDR as single-unit AccessUnit
    let mut au = AccessUnit::from_units(vec![Bytes::copy_from_slice(h265_idr_slice_realistic())]);
    assert_eq!(au.units.len(), 1);

    cache.prepend_to_access_unit(CodecId::H265, &mut au);

    // Should now have VPS + SPS + PPS + IDR = 4 units
    assert_eq!(
        au.units.len(),
        4,
        "bootstrapped H265 AU should have VPS + SPS + PPS + IDR"
    );
    assert_eq!(
        au.units[0].as_ref(),
        h265_vps_1080p_main(),
        "first unit must be VPS"
    );
    assert_eq!(
        au.units[1].as_ref(),
        h265_sps_1080p_main(),
        "second unit must be SPS"
    );
    assert_eq!(
        au.units[2].as_ref(),
        h265_pps_1080p_main(),
        "third unit must be PPS"
    );
    assert_eq!(
        au.units[3].as_ref(),
        h265_idr_slice_realistic(),
        "fourth unit must be IDR"
    );
}

#[test]
fn h265_idr_without_vps_sps_pps_bootstrapped_via_repair_frame() {
    let mut cache = ParameterSetCache::default();

    // First frame: full H265 keyframe with VPS/SPS/PPS/IDR
    let full_payload = build_annexb(&[
        h265_vps_1080p_main(),
        h265_sps_1080p_main(),
        h265_pps_1080p_main(),
        h265_idr_slice_realistic(),
    ]);
    let mut first_frame = build_avframe_keyframe(CodecId::H265, full_payload, 0);
    let discovered = cache.repair_h26x_keyframe_frame(&mut first_frame);
    assert!(
        discovered.is_some(),
        "first H265 frame should discover extradata"
    );

    // Second frame: bare IDR only
    let bare_payload = build_annexb(&[h265_idr_slice_realistic()]);
    let mut second_frame = build_avframe_keyframe(CodecId::H265, bare_payload.clone(), 90_000);

    cache.repair_h26x_keyframe_frame(&mut second_frame);

    assert!(
        second_frame.payload.len() > bare_payload.len(),
        "repaired H265 frame must be larger than bare IDR"
    );
    // Verify VPS is at the start (0x40 is VPS NAL type first byte for H265)
    assert!(
        second_frame
            .payload
            .starts_with(&[0x00, 0x00, 0x00, 0x01, 0x40]),
        "repaired H265 frame must start with start_code + VPS NAL type"
    );
}

// ============================================================================
// Regression 3: Parameter set change mid-stream (resolution change)
//
// Scenario: Encoder changes resolution mid-stream (e.g., adaptive bitrate
// switch from 1080p to 720p). The cache must update to the new parameter sets,
// and subsequent keyframes must use the NEW parameter sets, not the old ones.
// ============================================================================

#[test]
fn h264_parameter_set_change_updates_cache_and_subsequent_bootstrap() {
    let mut cache = ParameterSetCache::default();

    // Phase 1: Initial 1080p stream — populate cache with High profile SPS/PPS
    let keyframe_1080p = build_annexb(&[
        h264_sps_1080p_high(),
        h264_pps_high(),
        h264_idr_slice_realistic(),
    ]);
    cache.update_from_annexb(CodecId::H264, &keyframe_1080p);
    assert_eq!(
        cache.sps.as_deref(),
        Some(h264_sps_1080p_high()),
        "cache should hold 1080p SPS"
    );
    assert_eq!(
        cache.pps.as_deref(),
        Some(h264_pps_high()),
        "cache should hold High profile PPS"
    );

    // Phase 2: Resolution change — new keyframe arrives with 720p Baseline SPS/PPS
    let keyframe_720p = build_annexb(&[
        h264_sps_720p_baseline(),
        h264_pps_baseline(),
        h264_idr_slice_realistic(),
    ]);
    let changed = cache.update_from_annexb(CodecId::H264, &keyframe_720p);
    assert!(changed, "cache should detect parameter set change");
    assert_eq!(
        cache.sps.as_deref(),
        Some(h264_sps_720p_baseline()),
        "cache must now hold 720p SPS after resolution change"
    );
    assert_eq!(
        cache.pps.as_deref(),
        Some(h264_pps_baseline()),
        "cache must now hold Baseline PPS after resolution change"
    );

    // Phase 3: Next bare IDR should be bootstrapped with the NEW 720p parameters
    let bare_idr = build_annexb(&[h264_idr_slice_realistic()]);
    let bootstrapped = cache.prepend_to_annexb_access_unit(CodecId::H264, &bare_idr);

    // Verify new SPS is used, not old
    let new_sps = h264_sps_720p_baseline();
    let old_sps = h264_sps_1080p_high();
    assert!(
        bootstrapped.windows(new_sps.len()).any(|w| w == new_sps),
        "bootstrapped output must contain NEW 720p SPS"
    );
    assert!(
        !bootstrapped.windows(old_sps.len()).any(|w| w == old_sps),
        "bootstrapped output must NOT contain OLD 1080p SPS"
    );

    // Verify new PPS is used
    let new_pps = h264_pps_baseline();
    let old_pps = h264_pps_high();
    assert!(
        bootstrapped.windows(new_pps.len()).any(|w| w == new_pps),
        "bootstrapped output must contain NEW Baseline PPS"
    );
    assert!(
        !bootstrapped.windows(old_pps.len()).any(|w| w == old_pps),
        "bootstrapped output must NOT contain OLD High PPS"
    );
}

#[test]
fn h265_parameter_set_change_updates_cache_and_subsequent_bootstrap() {
    let mut cache = ParameterSetCache::default();

    // Phase 1: Initial 1080p stream
    let keyframe_1080p = build_annexb(&[
        h265_vps_1080p_main(),
        h265_sps_1080p_main(),
        h265_pps_1080p_main(),
        h265_idr_slice_realistic(),
    ]);
    cache.update_from_annexb(CodecId::H265, &keyframe_1080p);
    assert_eq!(cache.vps.as_deref(), Some(h265_vps_1080p_main()));
    assert_eq!(cache.sps.as_deref(), Some(h265_sps_1080p_main()));
    assert_eq!(cache.pps.as_deref(), Some(h265_pps_1080p_main()));

    // Phase 2: Resolution change to 720p — all three parameter sets change
    let keyframe_720p = build_annexb(&[
        h265_vps_720p(),
        h265_sps_720p(),
        h265_pps_720p(),
        h265_idr_slice_realistic(),
    ]);
    let changed = cache.update_from_annexb(CodecId::H265, &keyframe_720p);
    assert!(changed, "H265 cache should detect parameter set change");
    assert_eq!(
        cache.vps.as_deref(),
        Some(h265_vps_720p()),
        "VPS must update to 720p"
    );
    assert_eq!(
        cache.sps.as_deref(),
        Some(h265_sps_720p()),
        "SPS must update to 720p"
    );
    assert_eq!(
        cache.pps.as_deref(),
        Some(h265_pps_720p()),
        "PPS must update to 720p"
    );

    // Phase 3: Next bare IDR bootstrapped with NEW 720p parameters
    let bare_idr = build_annexb(&[h265_idr_slice_realistic()]);
    let bootstrapped = cache.prepend_to_annexb_access_unit(CodecId::H265, &bare_idr);

    // Verify new VPS/SPS/PPS are used
    assert!(
        bootstrapped
            .windows(h265_vps_720p().len())
            .any(|w| w == h265_vps_720p()),
        "must contain NEW 720p VPS"
    );
    assert!(
        bootstrapped
            .windows(h265_sps_720p().len())
            .any(|w| w == h265_sps_720p()),
        "must contain NEW 720p SPS"
    );
    assert!(
        bootstrapped
            .windows(h265_pps_720p().len())
            .any(|w| w == h265_pps_720p()),
        "must contain NEW 720p PPS"
    );

    // Verify old 1080p parameters are NOT present
    assert!(
        !bootstrapped
            .windows(h265_vps_1080p_main().len())
            .any(|w| w == h265_vps_1080p_main()),
        "must NOT contain OLD 1080p VPS"
    );
    assert!(
        !bootstrapped
            .windows(h265_sps_1080p_main().len())
            .any(|w| w == h265_sps_1080p_main()),
        "must NOT contain OLD 1080p SPS"
    );
    assert!(
        !bootstrapped
            .windows(h265_pps_1080p_main().len())
            .any(|w| w == h265_pps_1080p_main()),
        "must NOT contain OLD 1080p PPS"
    );
}

#[test]
fn h264_parameter_set_change_via_repair_frame_uses_new_sets() {
    let mut cache = ParameterSetCache::default();

    // Frame 1: 1080p keyframe — populates cache
    let payload_1080p = build_annexb(&[
        h264_sps_1080p_high(),
        h264_pps_high(),
        h264_idr_slice_realistic(),
    ]);
    let mut frame1 = build_avframe_keyframe(CodecId::H264, payload_1080p, 0);
    cache.repair_h26x_keyframe_frame(&mut frame1);

    // Frame 2: 720p keyframe — updates cache
    let payload_720p = build_annexb(&[
        h264_sps_720p_baseline(),
        h264_pps_baseline(),
        h264_idr_slice_realistic(),
    ]);
    let mut frame2 = build_avframe_keyframe(CodecId::H264, payload_720p, 90_000);
    cache.repair_h26x_keyframe_frame(&mut frame2);

    // Verify cache now holds 720p parameters
    assert_eq!(cache.sps.as_deref(), Some(h264_sps_720p_baseline()));
    assert_eq!(cache.pps.as_deref(), Some(h264_pps_baseline()));

    // Frame 3: bare IDR — should be bootstrapped with 720p parameters
    let bare_payload = build_annexb(&[h264_idr_slice_realistic()]);
    let mut frame3 = build_avframe_keyframe(CodecId::H264, bare_payload, 180_000);
    cache.repair_h26x_keyframe_frame(&mut frame3);

    // Verify the repaired frame contains 720p SPS
    let new_sps = h264_sps_720p_baseline();
    assert!(
        frame3.payload.windows(new_sps.len()).any(|w| w == new_sps),
        "repaired frame must use NEW 720p SPS after resolution change"
    );
}

#[test]
fn h265_parameter_set_change_via_repair_frame_uses_new_sets() {
    let mut cache = ParameterSetCache::default();

    // Frame 1: 1080p keyframe
    let payload_1080p = build_annexb(&[
        h265_vps_1080p_main(),
        h265_sps_1080p_main(),
        h265_pps_1080p_main(),
        h265_idr_slice_realistic(),
    ]);
    let mut frame1 = build_avframe_keyframe(CodecId::H265, payload_1080p, 0);
    cache.repair_h26x_keyframe_frame(&mut frame1);

    // Frame 2: 720p keyframe — updates cache
    let payload_720p = build_annexb(&[
        h265_vps_720p(),
        h265_sps_720p(),
        h265_pps_720p(),
        h265_idr_slice_realistic(),
    ]);
    let mut frame2 = build_avframe_keyframe(CodecId::H265, payload_720p, 90_000);
    cache.repair_h26x_keyframe_frame(&mut frame2);

    // Verify cache holds 720p parameters
    assert_eq!(cache.vps.as_deref(), Some(h265_vps_720p()));
    assert_eq!(cache.sps.as_deref(), Some(h265_sps_720p()));
    assert_eq!(cache.pps.as_deref(), Some(h265_pps_720p()));

    // Frame 3: bare IDR — bootstrapped with 720p parameters
    let bare_payload = build_annexb(&[h265_idr_slice_realistic()]);
    let mut frame3 = build_avframe_keyframe(CodecId::H265, bare_payload, 180_000);
    cache.repair_h26x_keyframe_frame(&mut frame3);

    // Verify repaired frame starts with 720p VPS
    assert!(
        frame3.payload.starts_with(&[0x00, 0x00, 0x00, 0x01, 0x40]),
        "repaired H265 frame must start with VPS"
    );
    let new_vps = h265_vps_720p();
    assert!(
        frame3.payload.windows(new_vps.len()).any(|w| w == new_vps),
        "repaired H265 frame must use NEW 720p VPS"
    );
}
