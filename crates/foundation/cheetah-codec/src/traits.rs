use crate::prelude::*;
use bytes::Bytes;

use crate::{AVFrame, Timebase, TrackInfo};

/// `CodecNormalizer` trait.
/// `CodecNormalizer` trait.
pub trait CodecNormalizer {
    type Input;

    fn normalize(&mut self, input: Self::Input) -> Option<AVFrame>;
    fn track_info(&self) -> Option<&TrackInfo>;
}

/// `Packetizer` trait.
/// `Packetizer` trait.
pub trait Packetizer {
    type Packet;

    fn packetize(&mut self, frame: &AVFrame) -> Vec<Self::Packet>;
}

/// `Depacketizer` trait.
/// `Depacketizer` trait.
pub trait Depacketizer {
    type Packet;

    fn depacketize(&mut self, packet: Self::Packet) -> Option<AVFrame>;
}

/// `TrackExporter` trait.
/// `TrackExporter` trait.
pub trait TrackExporter {
    type Output;

    fn export(&self, track: &TrackInfo) -> Option<Self::Output>;
}

/// `PassthroughNormalizer` data structure.
/// `PassthroughNormalizer` 数据结构.
#[derive(Debug, Default)]
pub struct PassthroughNormalizer {
    /// `track` field.
    /// `track` 字段.
    track: Option<TrackInfo>,
}

impl CodecNormalizer for PassthroughNormalizer {
    type Input = AVFrame;

    fn normalize(&mut self, input: Self::Input) -> Option<AVFrame> {
        if self.track.is_none() {
            let clock_rate = derive_clock_rate(input.timebase);
            self.track = Some(TrackInfo::new(
                input.track_id,
                input.media_kind,
                input.codec,
                clock_rate,
            ));
        }
        Some(input)
    }

    fn track_info(&self) -> Option<&TrackInfo> {
        self.track.as_ref()
    }
}

/// `RawPayloadPacketizer` data structure.
/// `RawPayloadPacketizer` 数据结构.
#[derive(Debug, Default)]
pub struct RawPayloadPacketizer;

impl Packetizer for RawPayloadPacketizer {
    type Packet = Bytes;

    fn packetize(&mut self, frame: &AVFrame) -> Vec<Self::Packet> {
        vec![frame.payload.clone()]
    }
}

/// `RawPayloadDepacketizer` data structure.
/// `RawPayloadDepacketizer` 数据结构.
#[derive(Debug, Clone)]
pub struct RawPayloadDepacketizer {
    /// `template` field of type `AVFrame`.
    /// `template` 字段，类型为 `AVFrame`.
    pub template: AVFrame,
    /// `next_pts` field of type `i64`.
    /// `next_pts` 字段，类型为 `i64`.
    pub next_pts: i64,
    /// `step` field of type `i64`.
    /// `step` 字段，类型为 `i64`.
    pub step: i64,
}

impl Depacketizer for RawPayloadDepacketizer {
    type Packet = Bytes;

    fn depacketize(&mut self, packet: Self::Packet) -> Option<AVFrame> {
        let mut frame = self.template.clone();
        frame.pts = self.next_pts;
        frame.dts = self.next_pts;
        frame.pts_us = Timebase::to_micros(frame.timebase, self.next_pts);
        frame.dts_us = frame.pts_us;
        frame.payload = packet;
        self.next_pts = self.next_pts.saturating_add(self.step.max(1));
        Some(frame)
    }
}

/// `SimpleTrackExporter` data structure.
/// `SimpleTrackExporter` 数据结构.
#[derive(Debug, Default)]
pub struct SimpleTrackExporter;

impl TrackExporter for SimpleTrackExporter {
    type Output = String;

    fn export(&self, track: &TrackInfo) -> Option<Self::Output> {
        Some(format!(
            "track={} kind={:?} codec={:?} clock={}",
            track.track_id.0, track.media_kind, track.codec, track.clock_rate
        ))
    }
}

fn derive_clock_rate(timebase: Timebase) -> u32 {
    if timebase.num == 0 {
        return 90_000;
    }
    let rate = (timebase.den as u64) / (timebase.num as u64);
    if rate == 0 {
        90_000
    } else {
        rate.min(u32::MAX as u64) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CodecId, FrameFormat, MediaKind, TrackId};

    #[test]
    fn passthrough_normalizer_tracks_first_frame() {
        let mut normalizer = PassthroughNormalizer::default();
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from_static(b"nalu"),
        );
        let normalized = normalizer.normalize(frame).expect("frame");
        assert_eq!(normalized.payload, Bytes::from_static(b"nalu"));
        assert!(normalizer.track_info().is_some());
    }
}
