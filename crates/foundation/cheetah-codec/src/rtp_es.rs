//! Elementary stream (ES) RTP depacketizer.
//!
//! Converts H.264/H.265 RTP payloads (single NAL, FU-A/STAP-A, AP/FU) into
//! `AVFrame` and `TrackInfo` events. The engine uses Annex-B start-code form,
//! so emitted payloads include `0x00 0x00 0x00 0x01` prefixes.

use bytes::Bytes;

use crate::frame::{AVFrame, FrameFlags, FrameFormat};
use crate::prelude::*;
use crate::time::Timebase;
use crate::track::{CodecExtradata, CodecId, MediaKind, TrackId, TrackInfo, TrackReadiness};
use crate::video::{h26x_nalu_is_random_access, ParameterSetCache};

/// Events produced by the elementary-stream depacketizer.
#[derive(Debug, Clone)]
pub enum EsDemuxEvent {
    /// One or more tracks were discovered or updated.
    TrackFound(TrackInfo),
    /// A normalized frame is ready.
    Frame(AVFrame),
}

/// Configuration for `EsDemuxer`.
#[derive(Debug, Clone, Copy)]
pub struct EsDemuxerConfig {
    /// RTP clock rate in Hz used for frame timing.
    pub clock_rate_hz: u32,
    /// Optional codec hint, usually derived from the negotiated payload type.
    pub codec: Option<CodecId>,
}

impl Default for EsDemuxerConfig {
    fn default() -> Self {
        Self {
            clock_rate_hz: 90_000,
            codec: None,
        }
    }
}

/// Reassembles H.264/H.265 RTP payloads into `AVFrame` + `TrackInfo`.
#[derive(Debug, Clone, Default)]
pub struct EsDemuxer {
    config: EsDemuxerConfig,
    parameter_sets: ParameterSetCache,
    track_emitted: bool,
    fu: Option<FuState>,
}

#[derive(Debug, Clone)]
struct FuState {
    codec: CodecId,
    buffer: Vec<u8>,
}

impl EsDemuxer {
    pub fn new(config: EsDemuxerConfig) -> Self {
        Self {
            config,
            ..Default::default()
        }
    }

    /// Push a single RTP payload with its RTP timestamp.
    ///
    /// Returns a sequence of `TrackFound` and/or `Frame` events.
    pub fn push_packet(&mut self, payload: &[u8], timestamp: u32) -> Vec<EsDemuxEvent> {
        let mut events = Vec::new();
        if payload.is_empty() {
            return events;
        }

        // Some streams send raw Annex-B NALUs over RTP; handle them first.
        if payload.starts_with(&[0x00, 0x00, 0x01])
            || payload.starts_with(&[0x00, 0x00, 0x00, 0x01])
        {
            self.process_annexb(payload, timestamp, &mut events);
            return events;
        }

        let Some(codec) = self.detect_rtp_packet_codec(payload) else {
            return events;
        };

        match codec {
            CodecId::H264 => self.process_h264_rtp(payload, timestamp, &mut events),
            CodecId::H265 => self.process_h265_rtp(payload, timestamp, &mut events),
            _ => {}
        }

        events
    }

    /// Determine the H.26x codec from the RTP packet header.
    ///
    /// H.264 packetization (single NAL, STAP-A, FU-A) is detected first by the 1-byte NAL
    /// header; H.265 packetization is detected by its 2-byte NAL header and a `layer_id`
    /// of 0 with a valid temporal id. The more constrained H.265 test runs first so that
    /// H.265 SPS (`0x42 0x01...`) is not misread as H.264 NAL type 2.
    fn detect_rtp_packet_codec(&self, payload: &[u8]) -> Option<CodecId> {
        if self.config.codec.is_some() {
            return self.config.codec;
        }

        if payload.len() >= 2 {
            let nal_type = (payload[0] >> 1) & 0x3f;
            let tid = payload[1] & 0x07;
            let layer_id = ((payload[0] & 0x01) << 5) | (payload[1] >> 3);
            if payload[0] & 0x80 == 0
                && layer_id == 0
                && (1..=8).contains(&tid)
                && is_h265_packet_type(nal_type)
            {
                return Some(CodecId::H265);
            }
        }

        // H.264 FU-A (28), FU-B (29), STAP-A (24) and single NAL (1-5, 7, 8).
        let h264_type = payload[0] & 0x1f;
        if payload[0] & 0x80 == 0
            && (matches!(h264_type, 1..=5 | 7 | 8) || matches!(h264_type, 24 | 28 | 29))
        {
            return Some(CodecId::H264);
        }

        None
    }

    /// Determine the H.26x codec from a single Annex-B NAL unit (no start code).
    fn detect_unit_codec(&self, unit: &[u8]) -> Option<CodecId> {
        if self.config.codec.is_some() {
            return self.config.codec;
        }

        if unit.len() >= 2 {
            let nal_type = (unit[0] >> 1) & 0x3f;
            let tid = unit[1] & 0x07;
            let layer_id = ((unit[0] & 0x01) << 5) | (unit[1] >> 3);
            if unit[0] & 0x80 == 0
                && layer_id == 0
                && (1..=8).contains(&tid)
                && is_h265_nal_type(nal_type)
            {
                return Some(CodecId::H265);
            }
        }

        if !unit.is_empty() {
            let h264_type = unit[0] & 0x1f;
            if unit[0] & 0x80 == 0 && matches!(h264_type, 1..=5 | 7 | 8) {
                return Some(CodecId::H264);
            }
        }

        None
    }

    fn process_annexb(&mut self, payload: &[u8], timestamp: u32, events: &mut Vec<EsDemuxEvent>) {
        let units: Vec<&[u8]> = split_annexb(payload);
        for unit in units {
            if let Some(codec) = self.detect_unit_codec(unit) {
                self.process_nal(codec, unit, timestamp, events);
            }
        }
    }

    fn process_h264_rtp(
        &mut self,
        payload: &[u8],
        timestamp: u32,
        events: &mut Vec<EsDemuxEvent>,
    ) {
        let nal_type = payload[0] & 0x1f;
        match nal_type {
            1..=23 => self.process_nal(CodecId::H264, payload, timestamp, events),
            24 => self.process_h264_stap_a(payload, timestamp, events),
            28 | 29 => self.process_h264_fu_a(payload, timestamp, events),
            _ => {}
        }
    }

    fn process_h264_stap_a(
        &mut self,
        payload: &[u8],
        timestamp: u32,
        events: &mut Vec<EsDemuxEvent>,
    ) {
        let mut offset = 1;
        while offset + 2 <= payload.len() {
            let size = u16::from_be_bytes([payload[offset], payload[offset + 1]]) as usize;
            offset += 2;
            if offset + size > payload.len() {
                break;
            }
            let unit = &payload[offset..offset + size];
            offset += size;
            self.process_nal(CodecId::H264, unit, timestamp, events);
        }
    }

    fn process_h264_fu_a(
        &mut self,
        payload: &[u8],
        timestamp: u32,
        events: &mut Vec<EsDemuxEvent>,
    ) {
        if payload.len() < 2 {
            return;
        }
        let indicator = payload[0];
        let fu_header = payload[1];
        let start = (fu_header & 0x80) != 0;
        let end = (fu_header & 0x40) != 0;
        let nal_type = fu_header & 0x1f;
        let data = &payload[2..];

        if start {
            let reconstructed = (indicator & 0xe0) | nal_type;
            self.fu = Some(FuState {
                codec: CodecId::H264,
                buffer: vec![reconstructed],
            });
        }

        if let Some(ref mut fu) = self.fu {
            if fu.codec == CodecId::H264 {
                fu.buffer.extend_from_slice(data);
                if end {
                    let unit = core::mem::take(&mut fu.buffer);
                    self.fu = None;
                    self.process_nal(CodecId::H264, &unit, timestamp, events);
                }
            }
        }
    }

    fn process_h265_rtp(
        &mut self,
        payload: &[u8],
        timestamp: u32,
        events: &mut Vec<EsDemuxEvent>,
    ) {
        if payload.len() < 2 {
            return;
        }
        let nal_type = (payload[0] >> 1) & 0x3f;
        match nal_type {
            0..=40 => self.process_nal(CodecId::H265, payload, timestamp, events),
            48 => self.process_h265_ap(payload, timestamp, events),
            49 => self.process_h265_fu(payload, timestamp, events),
            _ => {}
        }
    }

    fn process_h265_ap(
        &mut self,
        payload: &[u8],
        timestamp: u32,
        events: &mut Vec<EsDemuxEvent>,
    ) {
        let mut offset = 2;
        while offset + 2 <= payload.len() {
            let size = u16::from_be_bytes([payload[offset], payload[offset + 1]]) as usize;
            offset += 2;
            if offset + size > payload.len() {
                break;
            }
            let unit = &payload[offset..offset + size];
            offset += size;
            self.process_nal(CodecId::H265, unit, timestamp, events);
        }
    }

    fn process_h265_fu(
        &mut self,
        payload: &[u8],
        timestamp: u32,
        events: &mut Vec<EsDemuxEvent>,
    ) {
        if payload.len() < 3 {
            return;
        }
        let payload_header0 = payload[0];
        let payload_header1 = payload[1];
        let fu_header = payload[2];
        let start = (fu_header & 0x80) != 0;
        let end = (fu_header & 0x40) != 0;
        let nal_type = fu_header & 0x3f;
        let data = &payload[3..];

        if start {
            let reconstructed0 = (payload_header0 & 0x81) | ((nal_type & 0x3f) << 1);
            self.fu = Some(FuState {
                codec: CodecId::H265,
                buffer: vec![reconstructed0, payload_header1],
            });
        }

        if let Some(ref mut fu) = self.fu {
            if fu.codec == CodecId::H265 {
                fu.buffer.extend_from_slice(data);
                if end {
                    let unit = core::mem::take(&mut fu.buffer);
                    self.fu = None;
                    self.process_nal(CodecId::H265, &unit, timestamp, events);
                }
            }
        }
    }

    fn process_nal(
        &mut self,
        codec: CodecId,
        nal_unit: &[u8],
        timestamp: u32,
        events: &mut Vec<EsDemuxEvent>,
    ) {
        if nal_unit.is_empty() {
            return;
        }

        // Feed the cache in canonical Annex-B form.
        let mut annexb = Vec::with_capacity(4 + nal_unit.len());
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        annexb.extend_from_slice(nal_unit);
        let cache_changed = self.parameter_sets.update_from_annexb(codec, &annexb);

        if cache_changed && !self.track_emitted {
            if let Some(extradata) = self.parameter_sets.extradata_for_codec(codec) {
                self.emit_track(codec, extradata, events);
            }
        }

        let is_vcl = match codec {
            CodecId::H264 => {
                let t = nal_unit[0] & 0x1f;
                matches!(t, 1..=5)
            }
            CodecId::H265 => {
                let t = (nal_unit[0] >> 1) & 0x3f;
                (0..=9).contains(&t) || (16..=21).contains(&t)
            }
            _ => false,
        };

        if !is_vcl {
            return;
        }

        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            codec,
            FrameFormat::CanonicalH26x,
            i64::from(timestamp),
            i64::from(timestamp),
            Timebase::new(1, self.config.clock_rate_hz.max(1)),
            Bytes::from(annexb),
        );

        if h26x_nalu_is_random_access(codec, nal_unit) {
            frame.flags.insert(FrameFlags::KEY);
            if self.parameter_sets.has_required_sets(codec) {
                let prepended = self
                    .parameter_sets
                    .prepend_to_annexb_access_unit(codec, frame.payload.as_ref());
                frame.payload = prepended;
            }
        }

        events.push(EsDemuxEvent::Frame(frame));
    }

    fn emit_track(
        &mut self,
        codec: CodecId,
        extradata: CodecExtradata,
        events: &mut Vec<EsDemuxEvent>,
    ) {
        let mut track = TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            codec,
            self.config.clock_rate_hz,
        );
        track.extradata = extradata;
        track.readiness = TrackReadiness::Ready;
        self.track_emitted = true;
        events.push(EsDemuxEvent::TrackFound(track));
    }
}

/// H.265 RTP packet types (NAL unit types) we recognize: VCL, parameter sets and
/// aggregation/fragment headers.
fn is_h265_packet_type(nal_type: u8) -> bool {
    (0..=9).contains(&nal_type)
        || (16..=21).contains(&nal_type)
        || (32..=40).contains(&nal_type)
        || (48..=50).contains(&nal_type)
}

/// H.265 NAL unit types encountered inside an Annex-B stream or aggregated packet.
fn is_h265_nal_type(nal_type: u8) -> bool {
    (0..=9).contains(&nal_type)
        || (16..=21).contains(&nal_type)
        || (32..=40).contains(&nal_type)
}

/// Split an Annex-B payload into NAL units without the start code.
fn split_annexb(payload: &[u8]) -> Vec<&[u8]> {
    let mut units = Vec::new();
    let mut cursor = 0usize;
    while cursor < payload.len() {
        // skip leading start code
        if payload[cursor] == 0x00 {
            if cursor + 2 < payload.len()
                && payload[cursor + 1] == 0x00
                && payload[cursor + 2] == 0x01
            {
                cursor += 3;
            } else if cursor + 3 < payload.len()
                && payload[cursor + 1] == 0x00
                && payload[cursor + 2] == 0x00
                && payload[cursor + 3] == 0x01
            {
                cursor += 4;
            } else {
                cursor += 1;
                continue;
            }
        } else {
            cursor += 1;
            continue;
        }

        if cursor >= payload.len() {
            break;
        }

        let start = cursor;
        while cursor < payload.len() {
            if payload[cursor] == 0x00
                && cursor + 2 < payload.len()
                && payload[cursor + 1] == 0x00
                && payload[cursor + 2] == 0x01
            {
                break;
            }
            if payload[cursor] == 0x00
                && cursor + 3 < payload.len()
                && payload[cursor + 1] == 0x00
                && payload[cursor + 2] == 0x00
                && payload[cursor + 3] == 0x01
            {
                break;
            }
            cursor += 1;
        }

        if start < cursor {
            units.push(&payload[start..cursor]);
        }
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn es_single_h264_idr_emits_track_and_frame() {
        // Minimal H.264 SPS and PPS followed by an IDR slice in a single RTP packet.
        let mut demuxer = EsDemuxer::default();

        let sps: &[u8] = &[0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40, 0x50, 0x05, 0xbb];
        let pps: &[u8] = &[0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0];
        let idr: &[u8] = &[0x65, 0x88, 0x84];

        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        payload.extend_from_slice(sps);
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        payload.extend_from_slice(pps);
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        payload.extend_from_slice(idr);

        let events = demuxer.push_packet(&payload, 90_000);

        let found_track = events
            .iter()
            .any(|e| matches!(e, EsDemuxEvent::TrackFound(_)));
        let found_frame = events.iter().any(|e| {
            matches!(
                e,
                EsDemuxEvent::Frame(f) if f.codec == CodecId::H264 && f.is_key_frame()
            )
        });
        assert!(found_track);
        assert!(found_frame);
    }

    #[test]
    fn es_h264_fu_a_reassembles_and_emits_frame() {
        let mut demuxer = EsDemuxer::default();

        // Use a simple non-parameter NAL (type 1, P-slice) split with FU-A.
        let indicator = 0x5c; // F=0, NRI=2, type=28 (FU-A)
        let fu_header_start = 0x81; // S=1, type=1
        let fu_header_end = 0x41; // E=1, type=1

        let first = {
            let mut p = vec![indicator, fu_header_start];
            p.extend_from_slice(&[0x09, 0x10]);
            p
        };
        let second = {
            let mut p = vec![indicator, fu_header_end];
            p.extend_from_slice(&[0x11, 0x12]);
            p
        };

        let events1 = demuxer.push_packet(&first, 90_000);
        assert!(events1.is_empty(), "FU-A start should not emit until end");

        let events2 = demuxer.push_packet(&second, 90_000);
        assert_eq!(events2.len(), 1, "FU-A end should emit one frame");
    }

    #[test]
    fn es_h265_sps_is_not_misclassified_as_h264() {
        // H.265 SPS: F=0, nal_type=33, layer_id=0, tid=1.
        let h265_sps: &[u8] = &[
            0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x80, 0x00, 0x00, 0x03, 0x00,
            0x00, 0x03, 0x00, 0x78, 0xac, 0x09,
        ];

        let demuxer = EsDemuxer::default();
        let codec = demuxer.detect_rtp_packet_codec(h265_sps);
        assert_eq!(codec, Some(CodecId::H265));
    }

    #[test]
    fn es_h265_fu_is_detected_correctly() {
        // H.265 FU: payload header type=49, layer_id=0, tid=1; FU header S=1, type=19 (IDR).
        let h265_fu_start: &[u8] = &[0x62, 0x01, 0x93, 0x00, 0x11, 0x22];

        let demuxer = EsDemuxer::default();
        let codec = demuxer.detect_rtp_packet_codec(h265_fu_start);
        assert_eq!(codec, Some(CodecId::H265));
    }

    #[test]
    fn es_codec_hint_overrides_detection() {
        let config = EsDemuxerConfig {
            clock_rate_hz: 90_000,
            codec: Some(CodecId::H264),
        };
        let demuxer = EsDemuxer::new(config);

        // Even though 0x42 0x01 looks like H.265 SPS, the hint forces H.264.
        let payload: &[u8] = &[0x42, 0x01, 0x65, 0x88];
        let codec = demuxer.detect_rtp_packet_codec(payload);
        assert_eq!(codec, Some(CodecId::H264));
    }
}
