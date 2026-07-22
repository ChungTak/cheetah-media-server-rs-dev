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
use crate::video::{h26x_nalu_is_random_access, AccessUnitAssembler, ParameterSetCache};

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
    /// Maximum size of an FU-A/FU reassembly buffer; prevents unbounded growth from
    /// endless continuation fragments or a lost end marker.
    pub max_fu_reassembly_bytes: usize,
    /// Maximum size of an access-unit accumulator; prevents unbounded growth when a
    /// stream reuses the same RTP timestamp without ever setting the marker bit.
    pub max_access_unit_bytes: usize,
}

impl Default for EsDemuxerConfig {
    fn default() -> Self {
        Self {
            clock_rate_hz: 90_000,
            codec: None,
            max_fu_reassembly_bytes: 8 * 1024 * 1024,
            max_access_unit_bytes: 8 * 1024 * 1024,
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
    au_assembler: AccessUnitAssembler,
    au_timestamp: Option<u32>,
    au_codec: Option<CodecId>,
    au_random_access: bool,
    au_has_vcl: bool,
    au_size: usize,
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

    /// Push a single RTP payload with its RTP timestamp and marker bit.
    ///
    /// Returns a sequence of `TrackFound` and/or `Frame` events. VCL NAL units that
    /// share the same RTP timestamp are accumulated into one access unit and emitted
    /// as a single `Frame` when the timestamp changes or the RTP marker bit is set.
    pub fn push_packet(
        &mut self,
        payload: &[u8],
        timestamp: u32,
        marker: bool,
    ) -> Vec<EsDemuxEvent> {
        let mut events = Vec::new();
        if payload.is_empty() {
            // A marker-only packet can still terminate the current access unit.
            if marker {
                self.flush_access_unit(&mut events);
            }
            return events;
        }

        // Some streams send raw Annex-B NALUs over RTP; handle them first.
        if payload.starts_with(&[0x00, 0x00, 0x01])
            || payload.starts_with(&[0x00, 0x00, 0x00, 0x01])
        {
            self.process_annexb(payload, timestamp, &mut events);
        } else {
            let Some(codec) = self.detect_rtp_packet_codec(payload) else {
                if marker {
                    self.flush_access_unit(&mut events);
                }
                return events;
            };

            match codec {
                CodecId::H264 => self.process_h264_rtp(payload, timestamp, &mut events),
                CodecId::H265 => self.process_h265_rtp(payload, timestamp, &mut events),
                _ => {}
            }
        }

        if marker {
            self.flush_access_unit(&mut events);
        }

        events
    }

    /// Determine the H.26x codec from the RTP packet header.
    ///
    /// H.264 packetization (single NAL, STAP-A, FU-A/FU-B) is detected by the 1-byte NAL
    /// header; H.265 packetization is detected by its 2-byte NAL header and a `layer_id`
    /// of 0 with a valid temporal id. The more constrained H.265 test runs first so that
    /// H.265 SPS (`0x42 0x01...`) is not misread as H.264 NAL type 2.
    fn detect_rtp_packet_codec(&self, payload: &[u8]) -> Option<CodecId> {
        if let Some(codec) = self.config.codec {
            return Some(codec);
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
        if let Some(codec) = self.config.codec {
            return Some(codec);
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

    fn process_h264_rtp(&mut self, payload: &[u8], timestamp: u32, events: &mut Vec<EsDemuxEvent>) {
        let nal_type = payload[0] & 0x1f;
        match nal_type {
            1..=23 => self.process_nal(CodecId::H264, payload, timestamp, events),
            24 => self.process_h264_stap_a(payload, timestamp, events),
            28 => self.process_h264_fu_a(payload, timestamp, events),
            29 => self.process_h264_fu_b(payload, timestamp, events),
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

        self.extend_fu_buffer(CodecId::H264, data, timestamp, events, end);
    }

    fn process_h264_fu_b(
        &mut self,
        payload: &[u8],
        timestamp: u32,
        events: &mut Vec<EsDemuxEvent>,
    ) {
        if payload.len() < 4 {
            return;
        }
        let indicator = payload[0];
        let fu_header = payload[1];
        // Skip the 2-byte DON carried by FU-B (RFC 6184).
        let start = (fu_header & 0x80) != 0;
        let end = (fu_header & 0x40) != 0;
        let nal_type = fu_header & 0x1f;
        let data = &payload[4..];

        if start {
            let reconstructed = (indicator & 0xe0) | nal_type;
            self.fu = Some(FuState {
                codec: CodecId::H264,
                buffer: vec![reconstructed],
            });
        }

        self.extend_fu_buffer(CodecId::H264, data, timestamp, events, end);
    }

    fn process_h265_rtp(&mut self, payload: &[u8], timestamp: u32, events: &mut Vec<EsDemuxEvent>) {
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

    fn process_h265_ap(&mut self, payload: &[u8], timestamp: u32, events: &mut Vec<EsDemuxEvent>) {
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

    fn process_h265_fu(&mut self, payload: &[u8], timestamp: u32, events: &mut Vec<EsDemuxEvent>) {
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

        self.extend_fu_buffer(CodecId::H265, data, timestamp, events, end);
    }

    fn extend_fu_buffer(
        &mut self,
        codec: CodecId,
        data: &[u8],
        timestamp: u32,
        events: &mut Vec<EsDemuxEvent>,
        end: bool,
    ) {
        if let Some(ref mut fu) = self.fu {
            if fu.codec == codec {
                if fu.buffer.len().saturating_add(data.len()) > self.config.max_fu_reassembly_bytes
                {
                    self.fu = None;
                    return;
                }
                fu.buffer.extend_from_slice(data);
                if end {
                    let unit = core::mem::take(&mut fu.buffer);
                    self.fu = None;
                    self.process_nal(codec, &unit, timestamp, events);
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

        // An RTP timestamp change means the previous access unit is complete.
        if self.au_timestamp.is_some() && self.au_timestamp != Some(timestamp) {
            self.flush_access_unit(events);
        }

        if self.au_codec.is_none() {
            self.au_codec = Some(codec);
        }
        self.au_timestamp = Some(timestamp);

        // Update the parameter-set cache in canonical Annex-B form.
        let mut annexb = Vec::with_capacity(4 + nal_unit.len());
        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        annexb.extend_from_slice(nal_unit);
        let cache_changed = self.parameter_sets.update_from_annexb(codec, &annexb);

        if cache_changed {
            if let Some(extradata) = self.parameter_sets.extradata_for_codec(codec) {
                self.emit_track(codec, extradata, events);
            }
        }

        // Accumulate this NAL unit into the current access unit, but drop the pending
        // units (not the current timestamp/codec) if the cap would be exceeded.
        let new_au_size = self.au_size.saturating_add(nal_unit.len());
        if new_au_size > self.config.max_access_unit_bytes {
            self.drop_pending_access_unit();
            self.au_size = nal_unit.len();
        } else {
            self.au_size = new_au_size;
        }
        self.au_assembler
            .push_unit(Bytes::copy_from_slice(nal_unit));

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

        if is_vcl {
            self.au_has_vcl = true;
            if h26x_nalu_is_random_access(codec, nal_unit) {
                self.au_random_access = true;
            }
        }
    }

    fn flush_access_unit(&mut self, events: &mut Vec<EsDemuxEvent>) {
        let codec = match self.au_codec {
            Some(c) => c,
            None => return,
        };
        if !self.au_has_vcl {
            self.reset_access_unit();
            return;
        }

        let timestamp = self.au_timestamp.unwrap_or(0);
        let mut access_unit = self.au_assembler.take_access_unit();
        if self.au_random_access && self.parameter_sets.has_required_sets(codec) {
            self.parameter_sets
                .prepend_to_access_unit(codec, &mut access_unit);
        }

        let payload = annexb_from_units(&access_unit.units);
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            codec,
            FrameFormat::CanonicalH26x,
            i64::from(timestamp),
            i64::from(timestamp),
            Timebase::new(1, self.config.clock_rate_hz.max(1)),
            payload,
        );

        if self.au_random_access {
            frame.flags.insert(FrameFlags::KEY);
        }

        events.push(EsDemuxEvent::Frame(frame));
        self.reset_access_unit();
    }

    fn reset_access_unit(&mut self) {
        self.drop_pending_access_unit();
        self.au_timestamp = None;
        self.au_codec = None;
    }

    fn drop_pending_access_unit(&mut self) {
        self.au_assembler = AccessUnitAssembler::default();
        self.au_random_access = false;
        self.au_has_vcl = false;
        self.au_size = 0;
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
    (0..=9).contains(&nal_type) || (16..=21).contains(&nal_type) || (32..=40).contains(&nal_type)
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

/// Re-serialize a list of NAL units as an Annex-B payload with 4-byte start codes.
fn annexb_from_units(units: &[Bytes]) -> Bytes {
    let total_len = units.iter().map(|u| u.len().saturating_add(4)).sum();
    let mut out = Vec::with_capacity(total_len);
    for unit in units {
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        out.extend_from_slice(unit);
    }
    Bytes::from(out)
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

        let events = demuxer.push_packet(&payload, 90_000, true);

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

        let events1 = demuxer.push_packet(&first, 90_000, false);
        assert!(events1.is_empty(), "FU-A start should not emit until end");

        let events2 = demuxer.push_packet(&second, 90_000, true);
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
            ..Default::default()
        };
        let demuxer = EsDemuxer::new(config);

        // Even though 0x42 0x01 looks like H.265 SPS, the hint forces H.264.
        let payload: &[u8] = &[0x42, 0x01, 0x65, 0x88];
        let codec = demuxer.detect_rtp_packet_codec(payload);
        assert_eq!(codec, Some(CodecId::H264));
    }

    #[test]
    fn es_h264_fu_a_drops_buffer_when_reassembly_cap_exceeded() {
        let config = EsDemuxerConfig {
            clock_rate_hz: 90_000,
            codec: Some(CodecId::H264),
            max_fu_reassembly_bytes: 4,
            ..Default::default()
        };
        let mut demuxer = EsDemuxer::new(config);

        let indicator = 0x5c; // F=0, NRI=2, type=28 (FU-A)
        let start = {
            let mut p = vec![indicator, 0x81]; // S=1, type=1
            p.extend_from_slice(&[0x00]); // 1 byte of data
            p
        };
        let cont1 = {
            let mut p = vec![indicator, 0x01];
            p.extend_from_slice(&[0x02, 0x03, 0x04]); // 3 bytes -> buffer == 4
            p
        };
        // One more byte would exceed the 4-byte cap; the partial FU must be discarded.
        let cont_overflow = {
            let mut p = vec![indicator, 0x01];
            p.extend_from_slice(&[0x05]);
            p
        };
        // A final end marker should produce nothing because the accumulator was dropped.
        let end = {
            let mut p = vec![indicator, 0x41]; // E=1, type=1
            p.extend_from_slice(&[0x06]);
            p
        };

        assert!(demuxer.push_packet(&start, 1000, false).is_empty());
        assert!(demuxer.push_packet(&cont1, 1000, false).is_empty());
        assert!(demuxer.push_packet(&cont_overflow, 1000, false).is_empty());
        assert!(demuxer.push_packet(&end, 1000, true).is_empty());
    }

    #[test]
    fn es_h264_multiple_slices_produce_one_frame() {
        let mut demuxer = EsDemuxer::default();

        // Two H.264 P-slices (type 1) in separate RTP packets sharing a timestamp.
        let slice1: &[u8] = &[0x21, 0xaa, 0xbb];
        let slice2: &[u8] = &[0x21, 0xcc, 0xdd];

        let events1 = demuxer.push_packet(slice1, 1000, false);
        assert!(events1.is_empty(), "first slice should accumulate");

        let events2 = demuxer.push_packet(slice2, 1000, true);
        let frames: Vec<_> = events2
            .iter()
            .filter_map(|e| match e {
                EsDemuxEvent::Frame(f) => Some(f),
                _ => None,
            })
            .collect();
        assert_eq!(
            frames.len(),
            1,
            "two slices with same timestamp should emit one frame"
        );
        assert_eq!(frames[0].codec, CodecId::H264);
    }

    #[test]
    fn es_h264_timestamp_change_flushes_pending_access_unit() {
        let mut demuxer = EsDemuxer::default();

        let slice1: &[u8] = &[0x21, 0xaa];
        let slice2: &[u8] = &[0x21, 0xbb];

        assert!(demuxer.push_packet(slice1, 1000, false).is_empty());
        // Timestamp change flushes the previous slice even without a marker bit.
        let events = demuxer.push_packet(slice2, 2000, false);
        let frames: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                EsDemuxEvent::Frame(f) => Some(f),
                _ => None,
            })
            .collect();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].pts, 1000);
    }

    #[test]
    fn es_access_unit_drops_when_size_cap_exceeded() {
        let config = EsDemuxerConfig {
            clock_rate_hz: 90_000,
            codec: Some(CodecId::H264),
            max_access_unit_bytes: 4,
            ..Default::default()
        };
        let mut demuxer = EsDemuxer::new(config);

        // First slice fits (3 bytes).
        assert!(demuxer
            .push_packet(&[0x21, 0xaa, 0xbb], 1000, false)
            .is_empty());
        // Second slice would push the pending AU past the 4-byte cap; drop pending and start fresh.
        let events = demuxer.push_packet(&[0x21, 0xcc, 0xdd], 1000, true);
        let frames: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                EsDemuxEvent::Frame(f) => Some(f),
                _ => None,
            })
            .collect();
        assert_eq!(frames.len(), 1);
        // The flushed frame contains only the last NAL unit (3 bytes).
        assert_eq!(frames[0].payload.len(), 3 + 4);
    }
}
