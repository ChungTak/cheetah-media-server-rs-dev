//! Parameter Set Cache Capability Checklist — Phase 03 Task 01
//!
//! This integration test verifies the complete checklist for parameter set
//! caching in `cheetah-codec`:
//!
//! 1. H264: can identify SPS/PPS from Annex-B (start codes) input
//! 2. H264: can identify SPS/PPS from AVCC (length-prefixed) input
//! 3. H265: can identify VPS/SPS/PPS from Annex-B input
//! 4. H265: can identify VPS/SPS/PPS from AVCC (length-prefixed) input
//! 5. Can generate an output view that prepends cached parameter sets before IDR
//! 6. Cache size has an upper bound (no unbounded growth from abnormal sets)

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, AccessUnit, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind,
    ParameterSetCache, ParameterSetRequirement, Timebase, TrackId, PARAMETER_SET_MAX_SIZE,
};

// ============================================================================
// Checklist Item 1: H264 SPS/PPS identification from Annex-B
// ============================================================================

#[test]
fn h264_identifies_sps_pps_from_annexb_with_4byte_start_code() {
    let mut cache = ParameterSetCache::default();
    // 4-byte start codes: 00 00 00 01
    let payload = [
        0x00, 0x00, 0x00, 0x01, 0x67, 0x64, 0x00, 0x1f, // SPS
        0x00, 0x00, 0x00, 0x01, 0x68, 0xeb, 0xef, 0x20, // PPS
        0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x80, 0x40, // IDR
    ];
    assert!(cache.update_from_annexb(CodecId::H264, &payload));
    assert_eq!(cache.sps.as_deref(), Some(&[0x67, 0x64, 0x00, 0x1f][..]));
    assert_eq!(cache.pps.as_deref(), Some(&[0x68, 0xeb, 0xef, 0x20][..]));
    assert!(cache.has_required_sets(CodecId::H264));
}

#[test]
fn h264_identifies_sps_pps_from_annexb_with_3byte_start_code() {
    let mut cache = ParameterSetCache::default();
    // 3-byte start codes: 00 00 01
    let payload = [
        0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x1e, // SPS
        0x00, 0x00, 0x01, 0x68, 0xce, 0x38, 0x80, // PPS
    ];
    assert!(cache.update_from_annexb(CodecId::H264, &payload));
    assert_eq!(cache.sps.as_deref(), Some(&[0x67, 0x42, 0xc0, 0x1e][..]));
    assert_eq!(cache.pps.as_deref(), Some(&[0x68, 0xce, 0x38, 0x80][..]));
    assert!(cache.has_required_sets(CodecId::H264));
}

#[test]
fn h264_annexb_non_parameter_set_nalus_do_not_populate_cache() {
    let mut cache = ParameterSetCache::default();
    // Only IDR slice (type 5) and non-IDR slice (type 1)
    let payload = [
        0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x80, // IDR
        0x00, 0x00, 0x00, 0x01, 0x61, 0x9a, 0x00, // non-IDR
    ];
    assert!(!cache.update_from_annexb(CodecId::H264, &payload));
    assert!(!cache.has_required_sets(CodecId::H264));
}

// ============================================================================
// Checklist Item 2: H264 SPS/PPS identification from AVCC (length-prefixed)
// ============================================================================

#[test]
fn h264_identifies_sps_pps_from_avcc_length_prefixed() {
    let mut cache = ParameterSetCache::default();
    let sps = [0x67, 0x64, 0x00, 0x2a, 0xac, 0x2b];
    let pps = [0x68, 0xee, 0x3c, 0xb0];
    let idr = [0x65, 0x88, 0x80, 0x40, 0x00];

    let mut payload = Vec::new();
    payload.extend_from_slice(&(sps.len() as u32).to_be_bytes());
    payload.extend_from_slice(&sps);
    payload.extend_from_slice(&(pps.len() as u32).to_be_bytes());
    payload.extend_from_slice(&pps);
    payload.extend_from_slice(&(idr.len() as u32).to_be_bytes());
    payload.extend_from_slice(&idr);

    assert!(cache.update_from_length_prefixed(CodecId::H264, &payload));
    assert_eq!(cache.sps.as_deref(), Some(&sps[..]));
    assert_eq!(cache.pps.as_deref(), Some(&pps[..]));
    assert!(cache.has_required_sets(CodecId::H264));
}

#[test]
fn h264_identifies_sps_pps_from_extradata_struct() {
    let mut cache = ParameterSetCache::default();
    let extradata = CodecExtradata::H264 {
        sps: vec![Bytes::from_static(&[0x67, 0x64, 0x00, 0x1f])],
        pps: vec![Bytes::from_static(&[0x68, 0xeb, 0xef, 0x20])],
        avcc: None,
    };
    assert!(cache.update_from_extradata(&extradata));
    assert!(cache.has_required_sets(CodecId::H264));
}

// ============================================================================
// Checklist Item 3: H265 VPS/SPS/PPS identification from Annex-B
// ============================================================================

#[test]
fn h265_identifies_vps_sps_pps_from_annexb() {
    let mut cache = ParameterSetCache::default();
    // H265 NAL types: VPS=32 (0x40>>1 & 0x3f), SPS=33 (0x42>>1 & 0x3f), PPS=34 (0x44>>1 & 0x3f)
    let payload = [
        0x00, 0x00, 0x00, 0x01, 0x40, 0x01, 0x0c, 0x01, 0xff, // VPS
        0x00, 0x00, 0x00, 0x01, 0x42, 0x01, 0x01, 0x01, 0x60, // SPS
        0x00, 0x00, 0x00, 0x01, 0x44, 0x01, 0xc0, 0xf7, 0xc0, // PPS
        0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xaf, 0x08, // IDR_W_RADL
    ];
    assert!(cache.update_from_annexb(CodecId::H265, &payload));
    assert_eq!(
        cache.vps.as_deref(),
        Some(&[0x40, 0x01, 0x0c, 0x01, 0xff][..])
    );
    assert_eq!(
        cache.sps.as_deref(),
        Some(&[0x42, 0x01, 0x01, 0x01, 0x60][..])
    );
    assert_eq!(
        cache.pps.as_deref(),
        Some(&[0x44, 0x01, 0xc0, 0xf7, 0xc0][..])
    );
    assert!(cache.has_required_sets(CodecId::H265));
}

#[test]
fn h265_annexb_with_3byte_start_codes() {
    let mut cache = ParameterSetCache::default();
    let payload = [
        0x00, 0x00, 0x01, 0x40, 0x01, 0x0c, // VPS
        0x00, 0x00, 0x01, 0x42, 0x01, 0x01, // SPS
        0x00, 0x00, 0x01, 0x44, 0x01, 0xc0, // PPS
    ];
    assert!(cache.update_from_annexb(CodecId::H265, &payload));
    assert!(cache.has_required_sets(CodecId::H265));
}

#[test]
fn h265_requires_all_three_parameter_sets() {
    let mut cache = ParameterSetCache::default();
    // Only VPS and SPS, no PPS
    let payload = [
        0x00, 0x00, 0x01, 0x40, 0x01, 0x0c, // VPS
        0x00, 0x00, 0x01, 0x42, 0x01, 0x01, // SPS
    ];
    cache.update_from_annexb(CodecId::H265, &payload);
    assert!(
        !cache.has_required_sets(CodecId::H265),
        "H265 requires VPS+SPS+PPS, missing PPS should report incomplete"
    );
}

// ============================================================================
// Checklist Item 4: H265 VPS/SPS/PPS identification from AVCC (length-prefixed)
// ============================================================================

#[test]
fn h265_identifies_vps_sps_pps_from_length_prefixed() {
    let mut cache = ParameterSetCache::default();
    let vps = [0x40, 0x01, 0x0c, 0x01, 0xff, 0xff];
    let sps = [0x42, 0x01, 0x01, 0x01, 0x60, 0x00];
    let pps = [0x44, 0x01, 0xc0, 0xf7, 0xc0];

    let mut payload = Vec::new();
    payload.extend_from_slice(&(vps.len() as u32).to_be_bytes());
    payload.extend_from_slice(&vps);
    payload.extend_from_slice(&(sps.len() as u32).to_be_bytes());
    payload.extend_from_slice(&sps);
    payload.extend_from_slice(&(pps.len() as u32).to_be_bytes());
    payload.extend_from_slice(&pps);

    assert!(cache.update_from_length_prefixed(CodecId::H265, &payload));
    assert_eq!(cache.vps.as_deref(), Some(&vps[..]));
    assert_eq!(cache.sps.as_deref(), Some(&sps[..]));
    assert_eq!(cache.pps.as_deref(), Some(&pps[..]));
    assert!(cache.has_required_sets(CodecId::H265));
}

#[test]
fn h265_identifies_from_extradata_struct() {
    let mut cache = ParameterSetCache::default();
    let extradata = CodecExtradata::H265 {
        vps: vec![Bytes::from_static(&[0x40, 0x01, 0x0c])],
        sps: vec![Bytes::from_static(&[0x42, 0x01, 0x01])],
        pps: vec![Bytes::from_static(&[0x44, 0x01, 0xc0])],
        hvcc: None,
    };
    assert!(cache.update_from_extradata(&extradata));
    assert!(cache.has_required_sets(CodecId::H265));
}

// ============================================================================
// Checklist Item 5: Generate output view prepending parameter sets before IDR
// ============================================================================

#[test]
fn h264_prepend_generates_annexb_output_with_sps_pps_before_idr() {
    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&CodecExtradata::H264 {
        sps: vec![Bytes::from_static(&[0x67, 0x64, 0x00, 0x1f])],
        pps: vec![Bytes::from_static(&[0x68, 0xeb, 0xef, 0x20])],
        avcc: None,
    });

    // IDR-only payload (no parameter sets)
    let idr_payload = [0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x80, 0x40];
    let output = cache.prepend_to_annexb_access_unit(CodecId::H264, &idr_payload);

    // Output should be: start_code + SPS + start_code + PPS + start_code + IDR
    assert!(output.len() > idr_payload.len());
    // Verify SPS appears before IDR
    let sps_pos = output
        .windows(4)
        .position(|w| w == [0x67, 0x64, 0x00, 0x1f])
        .expect("SPS should be in output");
    let idr_pos = output
        .windows(4)
        .position(|w| w == [0x65, 0x88, 0x80, 0x40])
        .expect("IDR should be in output");
    assert!(sps_pos < idr_pos, "SPS must appear before IDR");
}

#[test]
fn h264_prepend_to_access_unit_adds_sps_pps_units() {
    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&CodecExtradata::H264 {
        sps: vec![Bytes::from_static(&[0x67, 0x42])],
        pps: vec![Bytes::from_static(&[0x68, 0xce])],
        avcc: None,
    });

    let mut au = AccessUnit::from_units(vec![Bytes::from_static(&[0x65, 0x88])]);
    cache.prepend_to_access_unit(CodecId::H264, &mut au);

    assert_eq!(au.units.len(), 3, "should have SPS + PPS + IDR");
    assert_eq!(au.units[0].as_ref(), &[0x67, 0x42]); // SPS first
    assert_eq!(au.units[1].as_ref(), &[0x68, 0xce]); // PPS second
    assert_eq!(au.units[2].as_ref(), &[0x65, 0x88]); // IDR last
}

#[test]
fn h265_prepend_generates_output_with_vps_sps_pps_before_idr() {
    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&CodecExtradata::H265 {
        vps: vec![Bytes::from_static(&[0x40, 0x01, 0x0c])],
        sps: vec![Bytes::from_static(&[0x42, 0x01, 0x01])],
        pps: vec![Bytes::from_static(&[0x44, 0x01, 0xc0])],
        hvcc: None,
    });

    // IDR_W_RADL only payload
    let idr_payload = [0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xaf, 0x08];
    let output = cache.prepend_to_annexb_access_unit(CodecId::H265, &idr_payload);

    assert!(output.len() > idr_payload.len());
    // Verify VPS appears before IDR
    let vps_pos = output
        .windows(3)
        .position(|w| w == [0x40, 0x01, 0x0c])
        .expect("VPS should be in output");
    let idr_pos = output
        .windows(4)
        .position(|w| w == [0x26, 0x01, 0xaf, 0x08])
        .expect("IDR should be in output");
    assert!(vps_pos < idr_pos, "VPS must appear before IDR");
}

#[test]
fn h265_prepend_to_access_unit_adds_vps_sps_pps_units() {
    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&CodecExtradata::H265 {
        vps: vec![Bytes::from_static(&[0x40, 0x01])],
        sps: vec![Bytes::from_static(&[0x42, 0x01])],
        pps: vec![Bytes::from_static(&[0x44, 0x01])],
        hvcc: None,
    });

    let mut au = AccessUnit::from_units(vec![Bytes::from_static(&[0x26, 0x01])]);
    cache.prepend_to_access_unit(CodecId::H265, &mut au);

    assert_eq!(au.units.len(), 4, "should have VPS + SPS + PPS + IDR");
    assert_eq!(au.units[0].as_ref(), &[0x40, 0x01]); // VPS first
    assert_eq!(au.units[1].as_ref(), &[0x42, 0x01]); // SPS second
    assert_eq!(au.units[2].as_ref(), &[0x44, 0x01]); // PPS third
    assert_eq!(au.units[3].as_ref(), &[0x26, 0x01]); // IDR last
}

#[test]
fn prepend_does_nothing_when_cache_is_empty() {
    let cache = ParameterSetCache::default();
    let idr_payload = [0x00, 0x00, 0x00, 0x01, 0x65, 0x88];
    let output = cache.prepend_to_annexb_access_unit(CodecId::H264, &idr_payload);
    // When cache is empty, output should be a copy of the original
    assert_eq!(output.as_ref(), &idr_payload[..]);
}

#[test]
fn requirement_for_frame_reports_missing_when_cache_empty_and_keyframe() {
    let cache = ParameterSetCache::default();
    assert_eq!(
        cache.requirement_for_frame(CodecId::H264, true),
        ParameterSetRequirement::RequiredMissing
    );
    assert_eq!(
        cache.requirement_for_frame(CodecId::H265, true),
        ParameterSetRequirement::RequiredMissing
    );
}

#[test]
fn requirement_for_frame_reports_present_when_cache_populated_and_keyframe() {
    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&CodecExtradata::H264 {
        sps: vec![Bytes::from_static(&[0x67, 0x42])],
        pps: vec![Bytes::from_static(&[0x68, 0xce])],
        avcc: None,
    });
    assert_eq!(
        cache.requirement_for_frame(CodecId::H264, true),
        ParameterSetRequirement::RequiredPresent
    );
}

#[test]
fn requirement_for_frame_not_required_for_non_keyframe() {
    let cache = ParameterSetCache::default();
    assert_eq!(
        cache.requirement_for_frame(CodecId::H264, false),
        ParameterSetRequirement::NotRequired
    );
}

// ============================================================================
// Checklist Item 6: Cache size has upper bound
// ============================================================================

#[test]
fn cache_rejects_oversized_h264_sps_from_annexb() {
    let mut cache = ParameterSetCache::default();
    // Create SPS exceeding max size
    let mut oversized = vec![0x67]; // H264 SPS type byte
    oversized.resize(PARAMETER_SET_MAX_SIZE + 1, 0xAA);

    let mut payload = vec![0x00, 0x00, 0x00, 0x01];
    payload.extend_from_slice(&oversized);

    let changed = cache.update_from_annexb(CodecId::H264, &payload);
    assert!(!changed, "oversized SPS should be rejected");
    assert!(cache.sps.is_none());
}

#[test]
fn cache_rejects_oversized_h264_pps_from_annexb() {
    let mut cache = ParameterSetCache::default();
    let mut oversized = vec![0x68]; // H264 PPS type byte
    oversized.resize(PARAMETER_SET_MAX_SIZE + 1, 0xBB);

    let mut payload = vec![0x00, 0x00, 0x00, 0x01];
    payload.extend_from_slice(&oversized);

    let changed = cache.update_from_annexb(CodecId::H264, &payload);
    assert!(!changed, "oversized PPS should be rejected");
    assert!(cache.pps.is_none());
}

#[test]
fn cache_rejects_oversized_h265_vps_from_annexb() {
    let mut cache = ParameterSetCache::default();
    let mut oversized = vec![0x40, 0x01]; // H265 VPS type bytes
    oversized.resize(PARAMETER_SET_MAX_SIZE + 1, 0xCC);

    let mut payload = vec![0x00, 0x00, 0x00, 0x01];
    payload.extend_from_slice(&oversized);

    let changed = cache.update_from_annexb(CodecId::H265, &payload);
    assert!(!changed, "oversized VPS should be rejected");
    assert!(cache.vps.is_none());
}

#[test]
fn cache_rejects_oversized_h265_sps_from_length_prefixed() {
    let mut cache = ParameterSetCache::default();
    let mut oversized = vec![0x42, 0x01]; // H265 SPS type bytes
    oversized.resize(PARAMETER_SET_MAX_SIZE + 1, 0xDD);

    let mut payload = Vec::new();
    payload.extend_from_slice(&(oversized.len() as u32).to_be_bytes());
    payload.extend_from_slice(&oversized);

    let changed = cache.update_from_length_prefixed(CodecId::H265, &payload);
    assert!(
        !changed,
        "oversized H265 SPS should be rejected from length-prefixed"
    );
    assert!(cache.sps.is_none());
}

#[test]
fn cache_accepts_parameter_set_at_max_size_boundary() {
    let mut cache = ParameterSetCache::default();
    // Create SPS exactly at max size — should be accepted
    let mut at_limit = vec![0x67]; // H264 SPS type byte
    at_limit.resize(PARAMETER_SET_MAX_SIZE, 0xEE);

    let mut payload = vec![0x00, 0x00, 0x00, 0x01];
    payload.extend_from_slice(&at_limit);

    let changed = cache.update_from_annexb(CodecId::H264, &payload);
    assert!(changed, "SPS at exactly max size should be accepted");
    assert!(cache.sps.is_some());
    assert_eq!(cache.sps.as_ref().unwrap().len(), PARAMETER_SET_MAX_SIZE);
}

#[test]
fn cache_update_replaces_old_parameter_set_no_accumulation() {
    let mut cache = ParameterSetCache::default();

    // First SPS
    let payload1 = [0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x1e];
    cache.update_from_annexb(CodecId::H264, &payload1);
    assert_eq!(cache.sps.as_deref(), Some(&[0x67, 0x42, 0xc0, 0x1e][..]));

    // Second different SPS — should replace, not accumulate
    let payload2 = [0x00, 0x00, 0x00, 0x01, 0x67, 0x64, 0x00, 0x2a];
    cache.update_from_annexb(CodecId::H264, &payload2);
    assert_eq!(cache.sps.as_deref(), Some(&[0x67, 0x64, 0x00, 0x2a][..]));

    // Only one SPS stored — no accumulation
    // (The cache stores Option<Bytes>, not Vec<Bytes>)
}

// ============================================================================
// Integration: repair_h26x_keyframe_frame end-to-end
// ============================================================================

#[test]
fn repair_h26x_keyframe_discovers_and_prepends_for_h264() {
    let mut cache = ParameterSetCache::default();
    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        9_000,
        9_000,
        Timebase::new(1, 90_000),
        Bytes::from_static(&[
            0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, // SPS
            0x00, 0x00, 0x01, 0x68, 0xce, 0x38, // PPS
            0x00, 0x00, 0x01, 0x65, 0x88, 0x80, // IDR
        ]),
    );
    frame.flags.insert(FrameFlags::KEY);

    let discovered = cache.repair_h26x_keyframe_frame(&mut frame);
    assert!(discovered.is_some(), "should discover extradata");
    assert!(cache.has_required_sets(CodecId::H264));
    // Frame payload should start with parameter sets
    assert!(frame.payload.starts_with(&[0x00, 0x00, 0x00, 0x01, 0x67]));
}

#[test]
fn repair_h26x_keyframe_discovers_and_prepends_for_h265() {
    let mut cache = ParameterSetCache::default();
    let mut frame = AVFrame::new(
        TrackId(2),
        MediaKind::Video,
        CodecId::H265,
        FrameFormat::CanonicalH26x,
        18_000,
        18_000,
        Timebase::new(1, 90_000),
        Bytes::from_static(&[
            0x00, 0x00, 0x01, 0x40, 0x01, 0x0c, // VPS
            0x00, 0x00, 0x01, 0x42, 0x01, 0x01, // SPS
            0x00, 0x00, 0x01, 0x44, 0x01, 0xc0, // PPS
            0x00, 0x00, 0x01, 0x26, 0x01, 0xaf, // IDR_W_RADL
        ]),
    );
    frame.flags.insert(FrameFlags::KEY);

    let discovered = cache.repair_h26x_keyframe_frame(&mut frame);
    assert!(discovered.is_some(), "should discover H265 extradata");
    assert!(cache.has_required_sets(CodecId::H265));
}
