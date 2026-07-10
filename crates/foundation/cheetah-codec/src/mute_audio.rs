//! Silent AAC audio frame generator for streams that have video but no audio.
//!
//! Driven by video frame timestamps — generates AAC-LC silence frames to fill
//! the audio timeline up to the current video position.

use crate::prelude::*;
use bytes::Bytes;

use crate::audio::AacAudioSpecificConfig;
use crate::frame::{AVFrame, FrameFlags, FrameFormat, FrameOrigin};
use crate::time::Timebase;
use crate::track::{CodecId, MediaKind, TrackId};

/// AAC-LC silent frame: 1024 samples of silence.
/// This is a minimal raw AAC frame that decodes to silence.
/// Generated for: LC profile, stereo, any sample rate.
const AAC_SILENT_FRAME: &[u8] = &[0x21, 0x10, 0x05, 0x00, 0xa0, 0x19, 0x33];

/// Generates silent AAC audio frames driven by video timestamps.
///
/// 根据视频时间戳生成静音 AAC 音频帧。
///
/// 当流只有视频没有音频时，填充音频时间线，避免需要音频的播放器卡住。
#[derive(Debug, Clone)]
pub struct MuteAudioMaker {
    track_id: TrackId,
    sample_rate: u32,
    channels: u8,
    /// Samples per AAC frame (always 1024 for AAC-LC).
    samples_per_frame: u32,
    /// Next audio PTS in timebase ticks (timebase = 1/sample_rate).
    next_pts: i64,
    /// Cached silent frame payload.
    silent_frame: Bytes,
    /// Audio Specific Config for this generator.
    asc: AacAudioSpecificConfig,
}

impl MuteAudioMaker {
    /// Create a new silent audio generator.
    ///
    /// 创建一个新的静音音频生成器。
    ///
    /// 默认：AAC-LC，44100 Hz，立体声。
    pub fn new(track_id: TrackId) -> Self {
        Self::with_params(track_id, 44100, 2)
    }

    /// Create with custom sample rate and channel count.
    ///
    /// 使用自定义采样率和通道数创建。
    pub fn with_params(track_id: TrackId, sample_rate: u32, channels: u8) -> Self {
        let sampling_frequency_index = match sample_rate {
            96000 => 0,
            88200 => 1,
            64000 => 2,
            48000 => 3,
            44100 => 4,
            32000 => 5,
            24000 => 6,
            22050 => 7,
            16000 => 8,
            12000 => 9,
            11025 => 10,
            8000 => 11,
            _ => 4, // default to 44100
        };
        let asc = AacAudioSpecificConfig {
            audio_object_type: 2, // AAC-LC
            sampling_frequency_index,
            channel_configuration: channels,
        };
        Self {
            track_id,
            sample_rate,
            channels,
            samples_per_frame: 1024,
            next_pts: 0,
            silent_frame: Bytes::from_static(AAC_SILENT_FRAME),
            asc,
        }
    }

    /// Returns the AudioSpecificConfig bytes for codec config signaling.
    ///
    /// 返回用于编解码器配置信令的 AudioSpecificConfig 字节。
    pub fn audio_specific_config(&self) -> [u8; 2] {
        self.asc.to_bytes()
    }

    /// Audio sample rate in Hz.
    ///
    /// 音频采样率（Hz）。
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Audio channel count.
    ///
    /// 音频通道数。
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Track identifier for the generated audio frames.
    ///
    /// 生成音频帧的轨道标识。
    pub fn track_id(&self) -> TrackId {
        self.track_id
    }

    /// Generate silent audio frames to fill up to `video_pts_us` (microseconds).
    ///
    /// 生成静音音频帧以填充到 `video_pts_us`（微秒）。
    ///
    /// 每次视频帧到达时调用，返回生成的帧。
    pub fn fill_until(&mut self, video_pts_us: i64) -> Vec<AVFrame> {
        if video_pts_us <= 0 {
            return Vec::new();
        }
        // Convert video PTS (microseconds) to audio sample count
        let target_samples =
            (video_pts_us as u64).saturating_mul(u64::from(self.sample_rate)) / 1_000_000;
        let target_pts = target_samples as i64;

        let mut frames = Vec::new();
        let duration = self.samples_per_frame as i64;
        let timebase = Timebase::new(1, self.sample_rate);
        let duration_us = (duration as u64).saturating_mul(1_000_000) / u64::from(self.sample_rate);

        while self.next_pts + duration <= target_pts {
            let pts = self.next_pts;
            let pts_us = (pts as u64).saturating_mul(1_000_000) / u64::from(self.sample_rate);
            frames.push(AVFrame {
                track_id: self.track_id,
                media_kind: MediaKind::Audio,
                codec: CodecId::AAC,
                format: FrameFormat::AacRaw,
                pts,
                dts: pts,
                timebase,
                pts_us: pts_us as i64,
                dts_us: pts_us as i64,
                duration,
                duration_us: duration_us as i64,
                flags: FrameFlags::KEY | FrameFlags::GENERATED,
                payload: self.silent_frame.clone(),
                side_data: smallvec::smallvec![],
                origin: FrameOrigin::Generated,
            });
            self.next_pts += duration;
        }
        frames
    }

    /// Reset the generator (e.g., when real audio arrives).
    ///
    /// 重置生成器（例如当真实音频到达时）。
    pub fn reset(&mut self) {
        self.next_pts = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_silence_frames_up_to_video_pts() {
        let mut maker = MuteAudioMaker::new(TrackId(1));
        // 1024 samples at 44100 Hz ≈ 23.2ms per frame
        // 100ms of video should produce ~4 frames
        let frames = maker.fill_until(100_000); // 100ms in microseconds
        assert_eq!(frames.len(), 4);
        for frame in &frames {
            assert_eq!(frame.codec, CodecId::AAC);
            assert_eq!(frame.media_kind, MediaKind::Audio);
            assert!(frame.flags.contains(FrameFlags::KEY));
            assert!(frame.flags.contains(FrameFlags::GENERATED));
            assert_eq!(frame.origin, FrameOrigin::Generated);
            assert_eq!(frame.duration, 1024);
        }
        // PTS should be monotonically increasing
        assert_eq!(frames[0].pts, 0);
        assert_eq!(frames[1].pts, 1024);
        assert_eq!(frames[2].pts, 2048);
        assert_eq!(frames[3].pts, 3072);
    }

    #[test]
    fn does_not_generate_duplicate_frames_on_repeated_calls() {
        let mut maker = MuteAudioMaker::new(TrackId(1));
        let first = maker.fill_until(50_000); // 50ms → 2 frames
        assert_eq!(first.len(), 2);
        let second = maker.fill_until(50_000); // same PTS → no new frames
        assert!(second.is_empty());
        let third = maker.fill_until(100_000); // 100ms → 2 more frames
        assert_eq!(third.len(), 2);
    }

    #[test]
    fn returns_empty_for_zero_or_negative_pts() {
        let mut maker = MuteAudioMaker::new(TrackId(1));
        assert!(maker.fill_until(0).is_empty());
        assert!(maker.fill_until(-1000).is_empty());
    }

    #[test]
    fn audio_specific_config_is_valid() {
        let maker = MuteAudioMaker::new(TrackId(1));
        let asc_bytes = maker.audio_specific_config();
        let parsed = AacAudioSpecificConfig::from_bytes(&asc_bytes).unwrap();
        assert_eq!(parsed.audio_object_type, 2); // AAC-LC
        assert_eq!(parsed.sampling_frequency_index, 4); // 44100
        assert_eq!(parsed.channel_configuration, 2); // stereo
    }

    #[test]
    fn reset_restarts_generation() {
        let mut maker = MuteAudioMaker::new(TrackId(1));
        let _ = maker.fill_until(100_000);
        maker.reset();
        let frames = maker.fill_until(50_000);
        assert_eq!(frames[0].pts, 0); // starts from 0 again
    }
}
