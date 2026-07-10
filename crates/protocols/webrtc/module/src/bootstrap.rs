//! Codec bootstrap view for WebRTC play egress.
//!
//! This module provides the [`PlayBootstrapView`] which delegates all
//! parameter set caching and prepend logic to `cheetah-codec`'s
//! [`ParameterSetCache`]. The WebRTC module does NOT maintain its own
//! private SPS/PPS/VPS map — it uses the shared codec infrastructure.
//!
//! ## Responsibilities
//!
//! * Discover parameter sets from incoming `AVFrame` payloads (Annex-B
//!   or extradata) and feed them into the codec-level cache.
//! * On keyframes, prepend cached parameter sets so the downstream
//!   WebRTC client receives a decodable sequence even when the source
//!   IDR lacks inline SPS/PPS/VPS (ABL 2025-10-14 fix).
//! * Track whether the first decodable keyframe has been sent to the
//!   subscriber, enabling the caller to emit diagnostics when the
//!   bootstrap window expires without one.
//! * Coexist with the H264 B-frame filter: parameter set prepend
//!   operates on the payload *before* the frame is handed to the
//!   driver, while B-frame filtering is a frame-level admission
//!   decision. Both can be active simultaneously without interference.

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, ParameterSetCache,
    ParameterSetRequirement,
};
use tracing::{debug, warn};

/// Codec bootstrap state for a single WebRTC play subscriber.
///
/// Wraps `cheetah_codec::ParameterSetCache` and tracks whether the
/// subscriber has received its first decodable keyframe. The module
/// does not maintain any private parameter set storage — all caching
/// is delegated to the codec layer.
#[derive(Debug, Default)]
pub struct PlayBootstrapView {
    /// Shared parameter set cache from `cheetah-codec`. This is the
    /// single source of truth for SPS/PPS/VPS — no private map.
    cache: ParameterSetCache,
    /// Whether we have sent at least one keyframe with parameter sets
    /// to this subscriber.
    first_decodable_sent: bool,
    /// Number of keyframes that had parameter sets prepended.
    keyframes_bootstrapped: u64,
}

/// Result of processing a frame through the bootstrap view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapAction {
    /// Frame should be sent as-is (no modification needed).
    PassThrough,
    /// Frame payload was rewritten with parameter sets prepended.
    /// The caller should use the returned payload instead.
    Prepended(Bytes),
    /// Frame is a keyframe but parameter sets are not yet available.
    /// The caller should still send the frame but note that it may
    /// not be independently decodable.
    KeyframeMissingParameterSets,
}

impl PlayBootstrapView {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true if this subscriber has received at least one
    /// decodable keyframe (with parameter sets).
    pub fn has_sent_decodable_keyframe(&self) -> bool {
        self.first_decodable_sent
    }

    /// Number of keyframes that had parameter sets prepended by this
    /// view.
    pub fn keyframes_bootstrapped(&self) -> u64 {
        self.keyframes_bootstrapped
    }

    /// Process a frame through the bootstrap view.
    ///
    /// This method:
    /// 1. Discovers parameter sets from the frame payload and updates
    ///    the cache (delegated to `cheetah-codec`).
    /// 2. For keyframes of H264/H265/H266 codecs, prepends cached
    ///    parameter sets if available.
    /// 3. Returns the appropriate action for the caller.
    ///
    /// The B-frame filter is orthogonal to this logic: B-frame
    /// filtering is a frame-level admission decision (drop or keep)
    /// that happens independently. This method only concerns itself
    /// with ensuring keyframes carry parameter sets.
    pub fn process_frame(&mut self, frame: &AVFrame) -> BootstrapAction {
        let codec = frame.codec;

        // Only H264/H265/H266 need parameter set handling.
        let needs_parameter_sets = matches!(codec, CodecId::H264 | CodecId::H265 | CodecId::H266);
        if !needs_parameter_sets {
            // For non-H26x codecs (VP8, VP9, AV1, Opus, G711), keyframes
            // are self-contained. Mark decodable on first keyframe.
            if frame.flags.contains(FrameFlags::KEY) && frame.media_kind == MediaKind::Video {
                self.first_decodable_sent = true;
            }
            return BootstrapAction::PassThrough;
        }

        // Only process video frames for parameter set discovery.
        if frame.media_kind != MediaKind::Video {
            return BootstrapAction::PassThrough;
        }

        // Discover parameter sets from the payload. The cache handles
        // both Annex-B and length-prefixed formats.
        let is_annexb = matches!(frame.format, FrameFormat::CanonicalH26x);
        if is_annexb && !frame.payload.is_empty() {
            self.cache.update_from_annexb(codec, frame.payload.as_ref());
        }

        // Also update from extradata if the frame carries CONFIG flag.
        // This handles the case where parameter sets arrive as a
        // separate config frame before the keyframe.
        if frame.flags.contains(FrameFlags::CONFIG) {
            // CONFIG frames with Annex-B payload may contain SPS/PPS
            // inline — already handled above.
            return BootstrapAction::PassThrough;
        }

        // For keyframes, check if we need to prepend parameter sets.
        if !frame.flags.contains(FrameFlags::KEY) {
            return BootstrapAction::PassThrough;
        }

        // This is a keyframe. Check parameter set availability.
        let requirement = self.cache.requirement_for_frame(codec, true);
        match requirement {
            ParameterSetRequirement::RequiredPresent => {
                // Parameter sets are available — prepend them.
                let new_payload = self
                    .cache
                    .prepend_to_annexb_access_unit(codec, frame.payload.as_ref());
                self.first_decodable_sent = true;
                self.keyframes_bootstrapped += 1;
                debug!(
                    codec = ?codec,
                    "bootstrap: prepended parameter sets to keyframe"
                );
                BootstrapAction::Prepended(new_payload)
            }
            ParameterSetRequirement::RequiredMissing => {
                // Keyframe arrived but we don't have parameter sets yet.
                // Send the frame anyway (it might have inline params we
                // didn't parse, or the decoder might have them from a
                // prior session).
                warn!(
                    codec = ?codec,
                    "bootstrap: keyframe without cached parameter sets, \
                     frame may not be independently decodable"
                );
                BootstrapAction::KeyframeMissingParameterSets
            }
            ParameterSetRequirement::NotRequired => {
                // Codec doesn't need parameter sets (shouldn't reach
                // here for H26x, but handle gracefully).
                self.first_decodable_sent = true;
                BootstrapAction::PassThrough
            }
        }
    }

    /// Seed the cache from codec extradata (e.g., from TrackInfo).
    ///
    /// Called when the subscriber first connects and track info is
    /// available from the engine. This pre-populates the cache so
    /// the first keyframe can be bootstrapped immediately.
    pub fn seed_from_extradata(&mut self, extradata: &cheetah_codec::CodecExtradata) {
        self.cache.update_from_extradata(extradata);
    }

    /// Check if the cache has the required parameter sets for a codec.
    pub fn has_required_sets(&self, codec: CodecId) -> bool {
        self.cache.has_required_sets(codec)
    }
}

/// Determines whether a frame should be filtered by the H264 B-frame
/// filter.
///
/// This is intentionally separate from parameter set prepend logic.
/// The B-frame filter is a frame-level admission decision: frames
/// with `FrameFlags::B_FRAME` set are dropped to avoid decode
/// glitches on WebRTC clients that don't support reordering.
///
/// Parameter set prepend operates on admitted frames only, so the two
/// features compose cleanly:
///   1. B-frame filter decides: admit or drop
///   2. Bootstrap view processes admitted frames: discover params,
///      prepend to keyframes
pub fn should_filter_bframe(frame: &AVFrame, filter_enabled: bool) -> bool {
    if !filter_enabled {
        return false;
    }
    // Only filter H264 video B-frames.
    if frame.media_kind != MediaKind::Video {
        return false;
    }
    if frame.codec != CodecId::H264 {
        return false;
    }
    frame.flags.contains(FrameFlags::B_FRAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use cheetah_codec::{CodecExtradata, Timebase, TrackId};

    fn make_h264_keyframe(payload: &[u8]) -> AVFrame {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::copy_from_slice(payload),
        );
        frame.flags.insert(FrameFlags::KEY);
        frame
    }

    fn make_h264_non_keyframe(payload: &[u8]) -> AVFrame {
        AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            1000,
            1000,
            Timebase::new(1, 90_000),
            Bytes::copy_from_slice(payload),
        )
    }

    fn make_h264_bframe(payload: &[u8]) -> AVFrame {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            500,
            1000,
            Timebase::new(1, 90_000),
            Bytes::copy_from_slice(payload),
        );
        frame.flags.insert(FrameFlags::B_FRAME);
        frame
    }

    /// New subscriber receives a decodable keyframe when parameter
    /// sets are available in the cache.
    #[test]
    fn webrtc_new_subscriber_receives_decodable_keyframe() {
        let mut view = PlayBootstrapView::new();

        // Seed with SPS/PPS from extradata (simulating track info).
        view.seed_from_extradata(&CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1f])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x38, 0x80])],
            avcc: None,
        });

        assert!(view.has_required_sets(CodecId::H264));
        assert!(!view.has_sent_decodable_keyframe());

        // First keyframe arrives (IDR without inline SPS/PPS).
        let idr_payload = [0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x80, 0x40];
        let frame = make_h264_keyframe(&idr_payload);
        let action = view.process_frame(&frame);

        match action {
            BootstrapAction::Prepended(payload) => {
                // Verify SPS start code is present.
                assert!(payload.len() > idr_payload.len());
                // The prepended payload should contain the SPS NALU type.
                let payload_bytes = payload.as_ref();
                // Look for SPS (0x67) after a start code.
                let has_sps = payload_bytes
                    .windows(5)
                    .any(|w| w[0..4] == [0, 0, 0, 1] && (w[4] & 0x1f) == 7);
                assert!(has_sps, "prepended payload must contain SPS");
                // Look for PPS (0x68) after a start code.
                let has_pps = payload_bytes
                    .windows(5)
                    .any(|w| w[0..4] == [0, 0, 0, 1] && (w[4] & 0x1f) == 8);
                assert!(has_pps, "prepended payload must contain PPS");
            }
            other => panic!("expected Prepended, got {other:?}"),
        }

        assert!(view.has_sent_decodable_keyframe());
        assert_eq!(view.keyframes_bootstrapped(), 1);
    }

    /// When no parameter sets are cached, keyframe reports missing.
    #[test]
    fn bootstrap_timeout_reports_missing_keyframe() {
        let mut view = PlayBootstrapView::new();

        // No extradata seeded — cache is empty.
        assert!(!view.has_required_sets(CodecId::H264));

        // Keyframe arrives but we have no parameter sets.
        let idr_payload = [0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x80, 0x40];
        let frame = make_h264_keyframe(&idr_payload);
        let action = view.process_frame(&frame);

        assert_eq!(action, BootstrapAction::KeyframeMissingParameterSets);
        assert!(!view.has_sent_decodable_keyframe());
    }

    /// Non-keyframes pass through without modification.
    #[test]
    fn non_keyframe_passes_through() {
        let mut view = PlayBootstrapView::new();
        view.seed_from_extradata(&CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42])],
            pps: vec![Bytes::from_static(&[0x68, 0xce])],
            avcc: None,
        });

        let frame = make_h264_non_keyframe(&[0x00, 0x00, 0x00, 0x01, 0x41, 0x9a]);
        let action = view.process_frame(&frame);
        assert_eq!(action, BootstrapAction::PassThrough);
    }

    /// Parameter sets discovered from frame payload update the cache.
    #[test]
    fn discovers_parameter_sets_from_payload() {
        let mut view = PlayBootstrapView::new();
        assert!(!view.has_required_sets(CodecId::H264));

        // Frame with SPS + PPS + IDR inline.
        let payload = [
            0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f, // SPS
            0x00, 0x00, 0x00, 0x01, 0x68, 0xce, 0x38, 0x80, // PPS
            0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x80, 0x40, // IDR
        ];
        let frame = make_h264_keyframe(&payload);
        let action = view.process_frame(&frame);

        // Should discover SPS/PPS and prepend (even though they're
        // already inline, the prepend is idempotent for decodability).
        assert!(view.has_required_sets(CodecId::H264));
        assert!(matches!(action, BootstrapAction::Prepended(_)));
        assert!(view.has_sent_decodable_keyframe());
    }

    /// B-frame filter and parameter set prepend are independent.
    #[test]
    fn bframe_filter_and_bootstrap_are_independent() {
        let mut view = PlayBootstrapView::new();
        view.seed_from_extradata(&CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42])],
            pps: vec![Bytes::from_static(&[0x68, 0xce])],
            avcc: None,
        });

        // B-frame should be filtered (admission decision).
        let bframe = make_h264_bframe(&[0x00, 0x00, 0x00, 0x01, 0x41, 0x9a]);
        assert!(should_filter_bframe(&bframe, true));

        // Keyframe should NOT be filtered and gets parameter sets.
        let keyframe = make_h264_keyframe(&[0x00, 0x00, 0x00, 0x01, 0x65, 0x88]);
        assert!(!should_filter_bframe(&keyframe, true));
        let action = view.process_frame(&keyframe);
        assert!(matches!(action, BootstrapAction::Prepended(_)));

        // Non-keyframe P-frame should NOT be filtered.
        let pframe = make_h264_non_keyframe(&[0x00, 0x00, 0x00, 0x01, 0x41, 0x9a]);
        assert!(!should_filter_bframe(&pframe, true));
        let action = view.process_frame(&pframe);
        assert_eq!(action, BootstrapAction::PassThrough);
    }

    /// B-frame filter disabled passes everything through.
    #[test]
    fn bframe_filter_disabled_passes_all() {
        let bframe = make_h264_bframe(&[0x00, 0x00, 0x00, 0x01, 0x41, 0x9a]);
        assert!(!should_filter_bframe(&bframe, false));
    }

    /// VP8/VP9/AV1 keyframes mark decodable without parameter sets.
    #[test]
    fn non_h26x_keyframe_marks_decodable() {
        let mut view = PlayBootstrapView::new();

        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::VP8,
            FrameFormat::CanonicalVp8Frame,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x10, 0x20, 0x30]),
        );
        frame.flags.insert(FrameFlags::KEY);

        let action = view.process_frame(&frame);
        assert_eq!(action, BootstrapAction::PassThrough);
        assert!(view.has_sent_decodable_keyframe());
    }

    /// Audio frames always pass through regardless of codec.
    #[test]
    fn audio_frames_pass_through() {
        let mut view = PlayBootstrapView::new();

        let frame = AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::Opus,
            FrameFormat::OpusPacket,
            0,
            0,
            Timebase::new(1, 48_000),
            Bytes::from_static(&[0xfc, 0x00, 0x01]),
        );

        let action = view.process_frame(&frame);
        assert_eq!(action, BootstrapAction::PassThrough);
    }

    /// H265 keyframe gets VPS/SPS/PPS prepended.
    #[test]
    fn h265_keyframe_gets_vps_sps_pps_prepended() {
        let mut view = PlayBootstrapView::new();
        view.seed_from_extradata(&CodecExtradata::H265 {
            vps: vec![Bytes::from_static(&[0x40, 0x01, 0x0c])],
            sps: vec![Bytes::from_static(&[0x42, 0x01, 0x01])],
            pps: vec![Bytes::from_static(&[0x44, 0x01, 0xc0])],
            hvcc: None,
        });

        // H265 IDR (NAL type 19 = 0x26>>1 & 0x3f = 19).
        let idr_payload = [0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xaf, 0x08];
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::copy_from_slice(&idr_payload),
        );
        frame.flags.insert(FrameFlags::KEY);

        let action = view.process_frame(&frame);
        match action {
            BootstrapAction::Prepended(payload) => {
                assert!(payload.len() > idr_payload.len());
                assert!(view.has_sent_decodable_keyframe());
            }
            other => panic!("expected Prepended, got {other:?}"),
        }
    }
}
