use crate::prelude::*;
use bytes::Bytes;

use crate::{AVFrame, Timebase, TrackInfo};

/// Trait for converting protocol-specific input into canonical `AVFrame`s.
///
/// A normalizer owns any state needed to interpret the input (e.g. parameter sets
/// or timing context) and exposes the derived `TrackInfo`.
///
/// 将协议特定输入转换为标准 `AVFrame` 的 trait。
///
/// normalizer 拥有解释输入所需的任何状态（如参数集或时间上下文），并暴露派生的 `TrackInfo`。
pub trait CodecNormalizer {
    type Input;

    fn normalize(&mut self, input: Self::Input) -> Option<AVFrame>;
    fn track_info(&self) -> Option<&TrackInfo>;
}

/// Trait for splitting a canonical `AVFrame` into one or more wire packets.
///
/// 将标准 `AVFrame` 拆分为一个或多个线包 packet 的 trait。
pub trait Packetizer {
    type Packet;

    fn packetize(&mut self, frame: &AVFrame) -> Vec<Self::Packet>;
}

/// Trait for reassembling one or more wire packets into a canonical `AVFrame`.
///
/// 将一个或多个线包 packet 重新组装为标准 `AVFrame` 的 trait。
pub trait Depacketizer {
    type Packet;

    fn depacketize(&mut self, packet: Self::Packet) -> Option<AVFrame>;
}

/// Trait for exporting a `TrackInfo` into a protocol-specific representation.
///
/// 将 `TrackInfo` 导出为协议特定表示的 trait。
pub trait TrackExporter {
    type Output;

    fn export(&self, track: &TrackInfo) -> Option<Self::Output>;
}

/// A normalizer that forwards `AVFrame` unchanged and lazily creates a `TrackInfo`.
///
/// Used when the input is already in canonical form, such as internal generated frames
/// or loopback sources.
///
/// 一个直接转发 `AVFrame` 并延迟创建 `TrackInfo` 的 normalizer。
///
/// 用于输入已经是标准形式的情况，如内部生成帧或回环源。
#[derive(Debug, Default)]
pub struct PassthroughNormalizer {
    track: Option<TrackInfo>,
}

impl CodecNormalizer for PassthroughNormalizer {
    /// On the first frame, derive a clock rate from the frame's timebase and build
    /// the track. Subsequent frames pass through unchanged.
    ///
    /// 第一帧从帧 timebase 派生时钟率并构建轨道；后续帧原样通过。
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

/// Packetizer that emits the raw `AVFrame` payload as a single packet.
///
/// 将原始 `AVFrame` 负载作为单个 packet 发送的 packetizer。
#[derive(Debug, Default)]
pub struct RawPayloadPacketizer;

impl Packetizer for RawPayloadPacketizer {
    type Packet = Bytes;

    fn packetize(&mut self, frame: &AVFrame) -> Vec<Self::Packet> {
        vec![frame.payload.clone()]
    }
}

/// Depacketizer that wraps raw payloads into a template `AVFrame`.
///
/// Each packet increments `next_pts` by `step` so a sequence of raw payloads produces
/// a monotonically advancing timeline.
///
/// 将原始 payload 包装进模板 `AVFrame` 的 depacketizer。
///
/// 每个 packet 将 `next_pts` 按 `step` 递增，使原始 payload 序列产生单调推进的时间线。
#[derive(Debug, Clone)]
pub struct RawPayloadDepacketizer {
    pub template: AVFrame,
    pub next_pts: i64,
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

/// Track exporter that produces a human-readable debug string.
///
/// 生成人类可读调试字符串的 track exporter。
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

/// Derive a clock rate from a timebase by computing `den / num`.
///
/// Falls back to 90 kHz when the result is invalid or zero, which is the standard
/// video RTP clock rate.
///
/// 从 timebase 计算 `den / num` 派生时钟率。
///
/// 当结果无效或为零时回退到 90 kHz，这是标准视频 RTP 时钟率。
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
