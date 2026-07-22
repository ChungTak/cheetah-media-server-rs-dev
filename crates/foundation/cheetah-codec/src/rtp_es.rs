//! Elementary stream (ES) RTP depacketizer.
//!
//! Converts H.264/H.265 RTP payloads (single NAL, FU-A, STAP-A/AP) into
//! `AVFrame` and `TrackInfo` events. The engine uses Annex-B start-code form,
//! so emitted payloads include `0x00 0x00 0x00 0x01` prefixes.

use bytes::Bytes;

use crate::frame::{AVFrame, FrameFlags, FrameFormat};
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
}

impl Default for EsDemuxerConfig {
    fn default() -> Self {
        Self {
            clock_rate_hz: 90_000,
        }
    }
}

/// Reassembles H.264/H.265 RTP payloads into `AVFrame` + `TrackInfo`.
#[derive(Debug, Clone, Default)]
pub struct EsDemuxer {
    config: EsDemuxerConfig,
    codec: Option<CodecId>,
    parameter_sets: ParameterSetCache,
    track_emitted: bool,
    fu: Option<FuState>,
}

#[derive(Debug, Clone)]
struct FuState {
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

        let Some(codec) = self.infer_or_get_codec(payload) else {
            return events;
        };

        match codec {
            CodecId::H264 => self.process_h264_rtp(payload, timestamp, &mut events),
            CodecId::H265 => self.process_h265_rtp(payload, timestamp, &mut events),
            _ => {}
        }

        events
    }

    fn infer_or_get_codec(&mut self, payload: &[u8]) -> Option<CodecId> {
        if let Some(codec) = self.codec {
            return Some(codec);
        }

        // H.264 heuristic: 1-byte NAL header, forbidden-zero bit clear, type in VCL/PS range.
        let h264_type = payload[0] & 0x1f;
        let h264_forbidden = payload[0] & 0x80;
        if h264_forbidden == 0
            && (matches!(h264_type, 1..=5 | 7 | 8) || matches!(h264_type, 24 | 28))
        {
            self.codec = Some(CodecId::H264);
            return self.codec;
        }

        // H.265 heuristic: 2-byte NAL header, forbidden-zero bit clear, temporal_id >= 1.
        if payload.len() >= 2 {
            let h265_forbidden = payload[0] & 0x80;
            let nal_type = (payload[0] >> 1) & 0x3f;
            let tid = payload[1] & 0x07;
            if h265_forbidden == 0 && (1..=21).contains(&nal_type) && (1..=8).contains(&tid) {
                self.codec = Some(CodecId::H265);
                return self.codec;
            }
        }

        None
    }

    fn process_annexb(&mut self, payload: &[u8], timestamp: u32, events: &mut Vec<EsDemuxEvent>) {
        let units: Vec<&[u8]> = split_annexb(payload);
        for unit in units {
            let Some(codec) = self.infer_or_get_codec_for_unit(unit) else {
                continue;
            };
            self.process_nal(codec, unit, timestamp, events);
        }
    }

    fn infer_or_get_codec_for_unit(&mut self, unit: &[u8]) -> Option<CodecId> {
        if let Some(codec) = self.codec {
            return Some(codec);
        }
        if unit.is_empty() {
            return None;
        }

        // H.264 single-byte NAL header.
        let h264_type = unit[0] & 0x1f;
        if unit[0] & 0x80 == 0 && matches!(h264_type, 1..=5 | 7 | 8) {
            self.codec = Some(CodecId::H264);
            return self.codec;
        }

        // H.265 two-byte NAL header.
        if unit.len() >= 2 {
            let nal_type = (unit[0] >> 1) & 0x3f;
            let tid = unit[1] & 0x07;
            if unit[0] & 0x80 == 0 && (1..=21).contains(&nal_type) && (1..=8).contains(&tid) {
                self.codec = Some(CodecId::H265);
                return self.codec;
            }
        }

        None
    }

    fn process_h264_rtp(&mut self, payload: &[u8], timestamp: u32, events: &mut Vec<EsDemuxEvent>) {
        let nal_type = payload[0] & 0x1f;

        match nal_type {
            1..=23 => self.process_nal(CodecId::H264, payload, timestamp, events),
            24 => self.process_h264_stap_a(payload, timestamp, events),
            28 => self.process_h264_fu_a(payload, timestamp, events),
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
                buffer: vec![reconstructed],
            });
        }

        if let Some(ref mut fu) = self.fu {
            fu.buffer.extend_from_slice(data);
            if end {
                let unit = std::mem::take(&mut fu.buffer);
                self.fu = None;
                self.process_nal(CodecId::H264, &unit, timestamp, events);
            }
        }
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
                buffer: vec![reconstructed0, payload_header1],
            });
        }

        if let Some(ref mut fu) = self.fu {
            fu.buffer.extend_from_slice(data);
            if end {
                let unit = std::mem::take(&mut fu.buffer);
                self.fu = None;
                self.process_nal(CodecId::H265, &unit, timestamp, events);
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
                matches!(t, 1 | 5)
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
                // prepend_to_annexb_access_unit returns a Bytes-like? It returns Bytes.
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
        let found_frame = events
            .iter()
            .any(|e| matches!(e, EsDemuxEvent::Frame(f) if f.is_key_frame()));
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
}
