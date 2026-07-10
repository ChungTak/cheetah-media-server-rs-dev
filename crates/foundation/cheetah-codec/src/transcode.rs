//! Audio transcoding infrastructure for cross-protocol compatibility.
//!
//! Provides G.711 (A-law/μ-law) decoding to PCM and a trait-based pipeline
//! for plugging in AAC encoders.

use crate::prelude::*;
use bytes::Bytes;

use crate::{AVFrame, CodecId, FrameFlags, FrameFormat, FrameOrigin, MediaKind, Timebase, TrackId};

// ─── G.711 μ-law decode table ───────────────────────────────────────────────

static ULAW_DECODE_TABLE: [i16; 256] = {
    let mut table = [0i16; 256];
    let mut i = 0u16;
    while i < 256 {
        let mut val = !(i as u8);
        let sign = (val & 0x80) != 0;
        val &= 0x7f;
        let exponent = (val >> 4) & 0x07;
        let mantissa = val & 0x0f;
        let mut sample = ((mantissa as i32) << 1 | 1) << (exponent + 2);
        sample -= 0x21; // bias
        table[i as usize] = if sign {
            -(sample as i16)
        } else {
            sample as i16
        };
        i += 1;
    }
    table
};

// ─── G.711 A-law decode table ───────────────────────────────────────────────

static ALAW_DECODE_TABLE: [i16; 256] = {
    let mut table = [0i16; 256];
    let mut i = 0u16;
    while i < 256 {
        let mut val = (i as u8) ^ 0x55;
        let sign = (val & 0x80) != 0;
        val &= 0x7f;
        let exponent = (val >> 4) & 0x07;
        let mantissa = val & 0x0f;
        let sample = if exponent == 0 {
            (mantissa as i32) << 1 | 1
        } else {
            ((mantissa as i32) << 1 | 0x21) << (exponent - 1)
        };
        // Scale to 16-bit range (A-law uses 13-bit dynamic range)
        let sample = sample << 3;
        table[i as usize] = if sign {
            -(sample as i16)
        } else {
            sample as i16
        };
        i += 1;
    }
    table
};

/// Decode G.711 μ-law samples to 16-bit PCM.
///
/// 将 G.711 μ-law 样本解码为 16 位 PCM。
pub fn g711u_decode(input: &[u8]) -> Vec<i16> {
    input
        .iter()
        .map(|&b| ULAW_DECODE_TABLE[b as usize])
        .collect()
}

/// Decode G.711 A-law samples to 16-bit PCM.
///
/// 将 G.711 A-law 样本解码为 16 位 PCM。
pub fn g711a_decode(input: &[u8]) -> Vec<i16> {
    input
        .iter()
        .map(|&b| ALAW_DECODE_TABLE[b as usize])
        .collect()
}

/// Decode G.711 samples based on codec ID.
///
/// 根据编解码器 ID 解码 G.711 样本。
pub fn g711_decode(codec: CodecId, input: &[u8]) -> Vec<i16> {
    match codec {
        CodecId::G711A => g711a_decode(input),
        CodecId::G711U => g711u_decode(input),
        _ => Vec::new(),
    }
}

// ─── PCM → G.711 encode ─────────────────────────────────────────────────────

/// Encode 16-bit PCM sample to G.711 μ-law byte.
/// Built by finding the closest match in the decode table.
fn pcm16_to_ulaw_sample(sample: i16) -> u8 {
    // Search the decode table for the closest value
    let mut best_idx = 0u8;
    let mut best_diff = i32::MAX;
    for i in 0..=255u8 {
        let decoded = ULAW_DECODE_TABLE[i as usize] as i32;
        let diff = (sample as i32 - decoded).abs();
        if diff < best_diff {
            best_diff = diff;
            best_idx = i;
        }
    }
    best_idx
}

/// Encode 16-bit PCM sample to G.711 A-law byte.
/// Built by finding the closest match in the decode table.
fn pcm16_to_alaw_sample(sample: i16) -> u8 {
    let mut best_idx = 0u8;
    let mut best_diff = i32::MAX;
    for i in 0..=255u8 {
        let decoded = ALAW_DECODE_TABLE[i as usize] as i32;
        let diff = (sample as i32 - decoded).abs();
        if diff < best_diff {
            best_diff = diff;
            best_idx = i;
        }
    }
    best_idx
}

/// Encode 16-bit PCM samples to G.711 μ-law.
///
/// 将 16 位 PCM 样本编码为 G.711 μ-law。
pub fn pcm16_to_g711u(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().map(|&s| pcm16_to_ulaw_sample(s)).collect()
}

/// Encode 16-bit PCM samples to G.711 A-law.
///
/// 将 16 位 PCM 样本编码为 G.711 A-law。
pub fn pcm16_to_g711a(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().map(|&s| pcm16_to_alaw_sample(s)).collect()
}

// ─── AAC Decoder trait ──────────────────────────────────────────────────────

/// Trait for AAC decoding. Implementations can wrap symphonia, fdk-aac, or FFmpeg.
///
/// AAC 解码 trait。实现可封装 symphonia、fdk-aac 或 FFmpeg。
pub trait AacDecoder: Send {
    /// Decode an AAC frame to interleaved PCM i16 samples.
    ///
    /// 将 AAC 帧解码为交错 16 位 PCM 样本。
    fn decode(&mut self, aac_frame: &[u8]) -> Vec<i16>;
    /// Output sample rate.
    ///
    /// 输出采样率。
    fn sample_rate(&self) -> u32;
    /// Output channel count.
    ///
    /// 输出通道数。
    fn channels(&self) -> u8;
}

/// AAC → G.711 transcoding pipeline.
///
/// AAC 到 G.711 转码流水线。
pub struct AacToG711Transcoder {
    decoder: Box<dyn AacDecoder>,
    target_codec: CodecId,
    output_track_id: TrackId,
    output_pts: i64,
    output_sample_rate: u32,
}

impl AacToG711Transcoder {
    /// Create a new AAC → G.711 transcoder.
    ///
    /// 创建新的 AAC → G.711 转码器。
    pub fn new(
        decoder: Box<dyn AacDecoder>,
        target_codec: CodecId,
        output_track_id: TrackId,
    ) -> Self {
        Self {
            decoder,
            target_codec,
            output_track_id,
            output_pts: 0,
            output_sample_rate: 8000,
        }
    }

    /// Transcode an AAC AVFrame to G.711 AVFrame(s).
    ///
    /// 将 AAC AVFrame 转码为一个或多个 G.711 AVFrame。
    pub fn transcode(&mut self, input: &AVFrame) -> Vec<AVFrame> {
        let pcm = self.decoder.decode(input.payload.as_ref());
        if pcm.is_empty() {
            return Vec::new();
        }
        // Resample to 8kHz if needed
        let pcm = if self.decoder.sample_rate() != self.output_sample_rate {
            resample_nearest(&pcm, self.decoder.sample_rate(), self.output_sample_rate)
        } else {
            pcm
        };
        // Encode to G.711
        let g711_data = match self.target_codec {
            CodecId::G711A => pcm16_to_g711a(&pcm),
            _ => pcm16_to_g711u(&pcm),
        };
        let duration = g711_data.len() as i64;
        let pts = self.output_pts;
        let pts_us = pts.saturating_mul(1_000_000) / i64::from(self.output_sample_rate).max(1);
        self.output_pts += duration;

        vec![AVFrame {
            track_id: self.output_track_id,
            media_kind: MediaKind::Audio,
            codec: self.target_codec,
            format: FrameFormat::Unknown,
            pts,
            dts: pts,
            timebase: Timebase::new(1, self.output_sample_rate),
            pts_us,
            dts_us: pts_us,
            duration,
            duration_us: duration.saturating_mul(1_000_000)
                / i64::from(self.output_sample_rate).max(1),
            flags: FrameFlags::KEY,
            payload: Bytes::from(g711_data),
            side_data: smallvec::smallvec![],
            origin: FrameOrigin::Generated,
        }]
    }
}

// ─── AAC Encoder trait ──────────────────────────────────────────────────────

/// Trait for AAC encoding. Implementations can wrap fdk-aac or other encoders.
///
/// AAC 编码 trait。实现可封装 fdk-aac 或其他编码器。
pub trait AacEncoder: Send {
    /// Encode PCM samples (interleaved i16) to AAC raw frame data.
    /// Returns None if not enough samples accumulated yet.
    ///
    /// 将 PCM 样本（交错 i16）编码为 AAC 原始帧数据。
    /// 样本数不足时返回 None。
    fn encode(&mut self, pcm: &[i16]) -> Option<Bytes>;

    /// Flush any remaining buffered samples.
    ///
    /// 刷新所有剩余缓冲样本。
    fn flush(&mut self) -> Option<Bytes>;

    /// Number of samples per AAC frame (typically 1024).
    ///
    /// 每 AAC 帧的采样数（通常为 1024）。
    fn frame_size(&self) -> usize;

    /// Output sample rate.
    ///
    /// 输出采样率。
    fn sample_rate(&self) -> u32;

    /// Output channel count.
    ///
    /// 输出通道数。
    fn channels(&self) -> u8;
}

/// G.711 → AAC transcoding pipeline.
///
/// G.711 到 AAC 转码流水线。
///
/// 将 G.711（A-law 或 μ-law）解码为 PCM，按需重采样，然后用给定编码器编码为 AAC。
pub struct G711ToAacTranscoder {
    source_codec: CodecId,
    source_sample_rate: u32,
    output_track_id: TrackId,
    encoder: Box<dyn AacEncoder>,
    pcm_buffer: Vec<i16>,
    output_pts: i64,
}

impl G711ToAacTranscoder {
    /// Create a new G.711 → AAC transcoder.
    ///
    /// 创建新的 G.711 → AAC 转码器。
    pub fn new(
        source_codec: CodecId,
        source_sample_rate: u32,
        output_track_id: TrackId,
        encoder: Box<dyn AacEncoder>,
    ) -> Self {
        Self {
            source_codec,
            source_sample_rate,
            output_track_id,
            encoder,
            pcm_buffer: Vec::new(),
            output_pts: 0,
        }
    }

    /// Transcode a G.711 AVFrame to zero or more AAC AVFrames.
    ///
    /// 将 G.711 AVFrame 转码为零个或多个 AAC AVFrame。
    pub fn transcode(&mut self, input: &AVFrame) -> Vec<AVFrame> {
        let pcm = g711_decode(self.source_codec, input.payload.as_ref());
        if pcm.is_empty() {
            return Vec::new();
        }

        // Simple nearest-neighbor resample if rates differ
        let resampled = if self.source_sample_rate != self.encoder.sample_rate() {
            resample_nearest(&pcm, self.source_sample_rate, self.encoder.sample_rate())
        } else {
            pcm
        };

        self.pcm_buffer.extend_from_slice(&resampled);

        let mut output = Vec::new();
        let frame_size = self.encoder.frame_size() * self.encoder.channels() as usize;
        let output_sample_rate = self.encoder.sample_rate();
        let timebase = Timebase::new(1, output_sample_rate);
        let duration = self.encoder.frame_size() as i64;

        while self.pcm_buffer.len() >= frame_size {
            let frame_pcm: Vec<i16> = self.pcm_buffer.drain(..frame_size).collect();
            if let Some(aac_data) = self.encoder.encode(&frame_pcm) {
                let pts = self.output_pts;
                let pts_us = pts.saturating_mul(1_000_000) / i64::from(output_sample_rate).max(1);
                output.push(AVFrame {
                    track_id: self.output_track_id,
                    media_kind: MediaKind::Audio,
                    codec: CodecId::AAC,
                    format: FrameFormat::AacRaw,
                    pts,
                    dts: pts,
                    timebase,
                    pts_us,
                    dts_us: pts_us,
                    duration,
                    duration_us: duration.saturating_mul(1_000_000)
                        / i64::from(output_sample_rate).max(1),
                    flags: FrameFlags::KEY,
                    payload: aac_data,
                    side_data: smallvec::smallvec![],
                    origin: FrameOrigin::Generated,
                });
                self.output_pts += duration;
            }
        }
        output
    }

    /// Reset the internal PCM buffer and output PTS counter.
    ///
    /// 重置内部 PCM 缓冲区和输出 PTS 计数器。
    pub fn reset(&mut self) {
        self.pcm_buffer.clear();
        self.output_pts = 0;
    }
}

/// Trait for Opus decoding. Implementations can wrap libopus or audiopus.
///
/// Opus 解码 trait。实现可封装 libopus 或 audiopus。
pub trait OpusDecoder: Send {
    /// Decode an Opus packet to interleaved PCM i16 samples.
    ///
    /// 将 Opus 包解码为交错 16 位 PCM 样本。
    fn decode(&mut self, packet: &[u8]) -> Vec<i16>;

    /// Output sample rate (typically 48000).
    ///
    /// 输出采样率（通常为 48000）。
    fn sample_rate(&self) -> u32;

    /// Output channel count.
    ///
    /// 输出通道数。
    fn channels(&self) -> u8;
}

/// Trait for Opus encoding. Implementations can wrap libopus or audiopus.
///
/// Opus 编码 trait。实现可封装 libopus 或 audiopus。
///
/// The canonical WebRTC Opus output uses 48kHz, stereo, 960 samples per
/// frame (20ms at 48kHz).
///
/// 标准 WebRTC Opus 输出为 48kHz、立体声、每帧 960 个采样（48kHz 下 20ms）。
pub trait OpusEncoder: Send {
    /// Encode PCM samples (interleaved i16) to an Opus packet.
    /// Returns `None` if not enough samples have accumulated yet.
    ///
    /// 将 PCM 样本（交错 i16）编码为 Opus 包。
    /// 样本数不足时返回 `None`。
    fn encode(&mut self, pcm: &[i16]) -> Option<Bytes>;

    /// Flush any remaining buffered samples.
    ///
    /// 刷新所有剩余缓冲样本。
    fn flush(&mut self) -> Option<Bytes>;

    /// Number of samples per Opus frame (typically 960 for 20ms at 48kHz).
    ///
    /// 每 Opus 帧的采样数（48kHz 下 20ms 通常为 960）。
    fn frame_size(&self) -> usize;

    /// Output sample rate (must be 48000 for WebRTC).
    ///
    /// 输出采样率（WebRTC 必须为 48000）。
    fn sample_rate(&self) -> u32;

    /// Output channel count (typically 2 for stereo).
    ///
    /// 输出通道数（立体声通常为 2）。
    fn channels(&self) -> u8;
}

/// Opus → AAC transcoding pipeline.
///
/// Opus 到 AAC 转码流水线。
///
/// 使用给定解码器将 Opus 解码为 PCM，按需重采样，再用给定编码器编码为 AAC。
pub struct OpusToAacTranscoder {
    output_track_id: TrackId,
    decoder: Box<dyn OpusDecoder>,
    encoder: Box<dyn AacEncoder>,
    pcm_buffer: Vec<i16>,
    output_pts: i64,
}

impl OpusToAacTranscoder {
    /// Create a new Opus → AAC transcoder.
    ///
    /// 创建新的 Opus → AAC 转码器。
    pub fn new(
        output_track_id: TrackId,
        decoder: Box<dyn OpusDecoder>,
        encoder: Box<dyn AacEncoder>,
    ) -> Self {
        Self {
            output_track_id,
            decoder,
            encoder,
            pcm_buffer: Vec::new(),
            output_pts: 0,
        }
    }

    /// Transcode an Opus AVFrame to zero or more AAC AVFrames.
    ///
    /// 将 Opus AVFrame 转码为零个或多个 AAC AVFrame。
    pub fn transcode(&mut self, input: &AVFrame) -> Vec<AVFrame> {
        let pcm = self.decoder.decode(input.payload.as_ref());
        if pcm.is_empty() {
            return Vec::new();
        }

        // Resample if decoder output rate differs from encoder input rate
        let resampled = if self.decoder.sample_rate() != self.encoder.sample_rate() {
            resample_nearest(&pcm, self.decoder.sample_rate(), self.encoder.sample_rate())
        } else {
            pcm
        };

        self.pcm_buffer.extend_from_slice(&resampled);

        let mut output = Vec::new();
        let frame_size = self.encoder.frame_size() * self.encoder.channels() as usize;
        let output_sample_rate = self.encoder.sample_rate();
        let timebase = Timebase::new(1, output_sample_rate);
        let duration = self.encoder.frame_size() as i64;

        while self.pcm_buffer.len() >= frame_size {
            let frame_pcm: Vec<i16> = self.pcm_buffer.drain(..frame_size).collect();
            if let Some(aac_data) = self.encoder.encode(&frame_pcm) {
                let pts = self.output_pts;
                let pts_us = pts.saturating_mul(1_000_000) / i64::from(output_sample_rate).max(1);
                output.push(AVFrame {
                    track_id: self.output_track_id,
                    media_kind: MediaKind::Audio,
                    codec: CodecId::AAC,
                    format: FrameFormat::AacRaw,
                    pts,
                    dts: pts,
                    timebase,
                    pts_us,
                    dts_us: pts_us,
                    duration,
                    duration_us: duration.saturating_mul(1_000_000)
                        / i64::from(output_sample_rate).max(1),
                    flags: FrameFlags::KEY,
                    payload: aac_data,
                    side_data: smallvec::smallvec![],
                    origin: FrameOrigin::Generated,
                });
                self.output_pts += duration;
            }
        }
        output
    }

    /// Reset the internal PCM buffer and output PTS counter.
    ///
    /// 重置内部 PCM 缓冲区和输出 PTS 计数器。
    pub fn reset(&mut self) {
        self.pcm_buffer.clear();
        self.output_pts = 0;
    }
}

/// AAC → Opus transcoding pipeline.
///
/// AAC 到 Opus 转码流水线。
///
/// 使用给定解码器将 AAC 解码为 PCM，按需重采样到 48kHz，再用给定编码器编码为 Opus。
/// 标准 WebRTC Opus 输出为 48kHz/立体声/每帧 960 采样。
pub struct AacToOpusTranscoder {
    output_track_id: TrackId,
    decoder: Box<dyn AacDecoder>,
    encoder: Box<dyn OpusEncoder>,
    pcm_buffer: Vec<i16>,
    output_pts: i64,
}

impl AacToOpusTranscoder {
    /// Create a new AAC → Opus transcoder.
    ///
    /// 创建新的 AAC → Opus 转码器。
    pub fn new(
        output_track_id: TrackId,
        decoder: Box<dyn AacDecoder>,
        encoder: Box<dyn OpusEncoder>,
    ) -> Self {
        Self {
            output_track_id,
            decoder,
            encoder,
            pcm_buffer: Vec::new(),
            output_pts: 0,
        }
    }

    /// Transcode an AAC AVFrame to zero or more Opus AVFrames.
    ///
    /// 将 AAC AVFrame 转码为零个或多个 Opus AVFrame。
    pub fn transcode(&mut self, input: &AVFrame) -> Vec<AVFrame> {
        let pcm = self.decoder.decode(input.payload.as_ref());
        if pcm.is_empty() {
            return Vec::new();
        }

        // Resample to encoder rate (48kHz) if decoder output differs
        let resampled = if self.decoder.sample_rate() != self.encoder.sample_rate() {
            resample_nearest(&pcm, self.decoder.sample_rate(), self.encoder.sample_rate())
        } else {
            pcm
        };

        self.pcm_buffer.extend_from_slice(&resampled);

        let mut output = Vec::new();
        let frame_size = self.encoder.frame_size() * self.encoder.channels() as usize;
        let output_sample_rate = self.encoder.sample_rate();
        let timebase = Timebase::new(1, output_sample_rate);
        let duration = self.encoder.frame_size() as i64;

        while self.pcm_buffer.len() >= frame_size {
            let frame_pcm: Vec<i16> = self.pcm_buffer.drain(..frame_size).collect();
            if let Some(opus_data) = self.encoder.encode(&frame_pcm) {
                let pts = self.output_pts;
                let pts_us = pts.saturating_mul(1_000_000) / i64::from(output_sample_rate).max(1);
                output.push(AVFrame {
                    track_id: self.output_track_id,
                    media_kind: MediaKind::Audio,
                    codec: CodecId::Opus,
                    format: FrameFormat::OpusPacket,
                    pts,
                    dts: pts,
                    timebase,
                    pts_us,
                    dts_us: pts_us,
                    duration,
                    duration_us: duration.saturating_mul(1_000_000)
                        / i64::from(output_sample_rate).max(1),
                    flags: FrameFlags::KEY,
                    payload: opus_data,
                    side_data: smallvec::smallvec![],
                    origin: FrameOrigin::Generated,
                });
                self.output_pts += duration;
            }
        }
        output
    }

    /// Reset the internal PCM buffer and output PTS counter.
    ///
    /// 重置内部 PCM 缓冲区和输出 PTS 计数器。
    pub fn reset(&mut self) {
        self.pcm_buffer.clear();
        self.output_pts = 0;
    }
}

/// G.711 → Opus transcoding pipeline.
///
/// G.711 到 Opus 转码流水线。
///
/// 将 G.711（A-law 或 μ-law）解码为 PCM，从 8kHz 重采样到 48kHz，再编码为 Opus。
/// 用于源为 G.711 但客户端配置要求 Opus 输出的场景。
pub struct G711ToOpusTranscoder {
    source_codec: CodecId,
    source_sample_rate: u32,
    output_track_id: TrackId,
    encoder: Box<dyn OpusEncoder>,
    pcm_buffer: Vec<i16>,
    output_pts: i64,
}

impl G711ToOpusTranscoder {
    /// Create a new G.711 → Opus transcoder.
    ///
    /// 创建新的 G.711 → Opus 转码器。
    pub fn new(
        source_codec: CodecId,
        source_sample_rate: u32,
        output_track_id: TrackId,
        encoder: Box<dyn OpusEncoder>,
    ) -> Self {
        Self {
            source_codec,
            source_sample_rate,
            output_track_id,
            encoder,
            pcm_buffer: Vec::new(),
            output_pts: 0,
        }
    }

    /// Transcode a G.711 AVFrame to zero or more Opus AVFrames.
    ///
    /// 将 G.711 AVFrame 转码为零个或多个 Opus AVFrame。
    pub fn transcode(&mut self, input: &AVFrame) -> Vec<AVFrame> {
        let pcm = g711_decode(self.source_codec, input.payload.as_ref());
        if pcm.is_empty() {
            return Vec::new();
        }

        // Resample from 8kHz to 48kHz
        let resampled = if self.source_sample_rate != self.encoder.sample_rate() {
            resample_nearest(&pcm, self.source_sample_rate, self.encoder.sample_rate())
        } else {
            pcm
        };

        self.pcm_buffer.extend_from_slice(&resampled);

        let mut output = Vec::new();
        let frame_size = self.encoder.frame_size() * self.encoder.channels() as usize;
        let output_sample_rate = self.encoder.sample_rate();
        let timebase = Timebase::new(1, output_sample_rate);
        let duration = self.encoder.frame_size() as i64;

        while self.pcm_buffer.len() >= frame_size {
            let frame_pcm: Vec<i16> = self.pcm_buffer.drain(..frame_size).collect();
            if let Some(opus_data) = self.encoder.encode(&frame_pcm) {
                let pts = self.output_pts;
                let pts_us = pts.saturating_mul(1_000_000) / i64::from(output_sample_rate).max(1);
                output.push(AVFrame {
                    track_id: self.output_track_id,
                    media_kind: MediaKind::Audio,
                    codec: CodecId::Opus,
                    format: FrameFormat::OpusPacket,
                    pts,
                    dts: pts,
                    timebase,
                    pts_us,
                    dts_us: pts_us,
                    duration,
                    duration_us: duration.saturating_mul(1_000_000)
                        / i64::from(output_sample_rate).max(1),
                    flags: FrameFlags::KEY,
                    payload: opus_data,
                    side_data: smallvec::smallvec![],
                    origin: FrameOrigin::Generated,
                });
                self.output_pts += duration;
            }
        }
        output
    }

    /// Reset the internal PCM buffer and output PTS counter.
    ///
    /// 重置内部 PCM 缓冲区和输出 PTS 计数器。
    pub fn reset(&mut self) {
        self.pcm_buffer.clear();
        self.output_pts = 0;
    }
}

/// Simple nearest-neighbor resampler (adequate for voice-grade audio → AAC).
///
/// 简单最近邻重采样器（适用于语音级音频转 AAC）。
pub fn resample_nearest(input: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if from_rate == to_rate || from_rate == 0 {
        return input.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let output_len = ceil_f64(input.len() as f64 * ratio) as usize;
    let mut output = Vec::with_capacity(output_len);
    for i in 0..output_len {
        let src_idx = ((i as f64) / ratio) as usize;
        output.push(input[src_idx.min(input.len().saturating_sub(1))]);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g711u_decode_silence_is_near_zero() {
        // μ-law silence is encoded as 0x7F (positive near-zero) or 0xFF (negative near-zero)
        let positive_silence = g711u_decode(&[0x7F]);
        assert!(
            positive_silence[0].unsigned_abs() <= 33,
            "μ-law 0x7F should decode near zero, got {}",
            positive_silence[0]
        );
    }

    #[test]
    fn g711a_decode_silence_is_near_zero() {
        // A-law silence: 0x55 XOR 0x55 = 0x00 → exponent=0, mantissa=0
        let silence = g711a_decode(&[0x55]);
        assert!(
            silence[0].unsigned_abs() <= 8,
            "A-law 0x55 should decode near zero, got {}",
            silence[0]
        );
    }

    #[test]
    fn resample_nearest_doubles_rate() {
        let input = vec![100i16, 200, 300, 400];
        let output = resample_nearest(&input, 8000, 16000);
        assert_eq!(output.len(), 8);
        // Each sample should appear approximately twice
        assert_eq!(output[0], 100);
        assert_eq!(output[1], 100);
        assert_eq!(output[2], 200);
    }

    #[test]
    fn resample_nearest_6x_for_g711_to_48k() {
        let input: Vec<i16> = (0..160).map(|i| i as i16).collect(); // 20ms at 8kHz
        let output = resample_nearest(&input, 8000, 48000);
        // 160 samples at 8kHz → 960 samples at 48kHz
        assert_eq!(output.len(), 960);
    }

    #[test]
    fn pcm_g711u_roundtrip_preserves_signal() {
        // G.711 μ-law has ~14-bit dynamic range compressed to 8 bits.
        // Quantization error increases with signal magnitude.
        let pcm: Vec<i16> = vec![100, -100, 1000, -1000, 8000, -8000];
        let encoded = pcm16_to_g711u(&pcm);
        let decoded = g711u_decode(&encoded);
        for (orig, dec) in pcm.iter().zip(decoded.iter()) {
            assert_eq!(
                orig.signum(),
                dec.signum(),
                "sign mismatch: {orig} vs {dec}"
            );
            // Error should be < 5% of magnitude for typical speech levels
            let max_err = (orig.unsigned_abs() / 10).max(100);
            let error = (orig - dec).unsigned_abs();
            assert!(
                error <= max_err,
                "error too large: {orig} vs {dec}, err={error}, max={max_err}"
            );
        }
    }

    #[test]
    fn pcm_g711a_roundtrip_preserves_signal() {
        let pcm: Vec<i16> = vec![100, -100, 1000, -1000, 8000, -8000];
        let encoded = pcm16_to_g711a(&pcm);
        let decoded = g711a_decode(&encoded);
        for (orig, dec) in pcm.iter().zip(decoded.iter()) {
            assert_eq!(
                orig.signum(),
                dec.signum(),
                "sign mismatch: {orig} vs {dec}"
            );
            let max_err = (orig.unsigned_abs() / 10).max(100);
            let error = (orig - dec).unsigned_abs();
            assert!(
                error <= max_err,
                "error too large: {orig} vs {dec}, err={error}, max={max_err}"
            );
        }
    }
}
