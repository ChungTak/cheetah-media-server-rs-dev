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
pub fn g711u_decode(input: &[u8]) -> Vec<i16> {
    input
        .iter()
        .map(|&b| ULAW_DECODE_TABLE[b as usize])
        .collect()
}

/// Decode G.711 A-law samples to 16-bit PCM.
pub fn g711a_decode(input: &[u8]) -> Vec<i16> {
    input
        .iter()
        .map(|&b| ALAW_DECODE_TABLE[b as usize])
        .collect()
}

/// Decode G.711 samples based on codec ID.
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
pub fn pcm16_to_g711u(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().map(|&s| pcm16_to_ulaw_sample(s)).collect()
}

/// Encode 16-bit PCM samples to G.711 A-law.
pub fn pcm16_to_g711a(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().map(|&s| pcm16_to_alaw_sample(s)).collect()
}

// ─── AAC Decoder trait ──────────────────────────────────────────────────────

/// Trait for AAC decoding. Implementations can wrap symphonia, fdk-aac, or FFmpeg.
pub trait AacDecoder: Send {
    /// Decode an AAC frame to interleaved PCM i16 samples.
    fn decode(&mut self, aac_frame: &[u8]) -> Vec<i16>;
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u8;
}

/// AAC → G.711 transcoding pipeline.
pub struct AacToG711Transcoder {
    decoder: Box<dyn AacDecoder>,
    target_codec: CodecId,
    output_track_id: TrackId,
    output_pts: i64,
    output_sample_rate: u32,
}

impl AacToG711Transcoder {
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
pub trait AacEncoder: Send {
    /// Encode PCM samples (interleaved i16) to AAC raw frame data.
    /// Returns None if not enough samples accumulated yet.
    fn encode(&mut self, pcm: &[i16]) -> Option<Bytes>;

    /// Flush any remaining buffered samples.
    fn flush(&mut self) -> Option<Bytes>;

    /// Number of samples per AAC frame (typically 1024).
    fn frame_size(&self) -> usize;

    /// Output sample rate.
    fn sample_rate(&self) -> u32;

    /// Output channel count.
    fn channels(&self) -> u8;
}

/// G.711 → AAC transcoding pipeline.
///
/// Decodes G.711 (A-law or μ-law) to PCM, optionally resamples, then encodes
/// to AAC using the provided encoder.
pub struct G711ToAacTranscoder {
    source_codec: CodecId,
    source_sample_rate: u32,
    output_track_id: TrackId,
    encoder: Box<dyn AacEncoder>,
    pcm_buffer: Vec<i16>,
    output_pts: i64,
}

impl G711ToAacTranscoder {
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

    pub fn reset(&mut self) {
        self.pcm_buffer.clear();
        self.output_pts = 0;
    }
}

/// Trait for Opus decoding. Implementations can wrap libopus or audiopus.
pub trait OpusDecoder: Send {
    /// Decode an Opus packet to interleaved PCM i16 samples.
    fn decode(&mut self, packet: &[u8]) -> Vec<i16>;

    /// Output sample rate (typically 48000).
    fn sample_rate(&self) -> u32;

    /// Output channel count.
    fn channels(&self) -> u8;
}

/// Trait for Opus encoding. Implementations can wrap libopus or audiopus.
///
/// The canonical WebRTC Opus output uses 48kHz, stereo, 960 samples per
/// frame (20ms at 48kHz).
pub trait OpusEncoder: Send {
    /// Encode PCM samples (interleaved i16) to an Opus packet.
    /// Returns `None` if not enough samples have accumulated yet.
    fn encode(&mut self, pcm: &[i16]) -> Option<Bytes>;

    /// Flush any remaining buffered samples.
    fn flush(&mut self) -> Option<Bytes>;

    /// Number of samples per Opus frame (typically 960 for 20ms at 48kHz).
    fn frame_size(&self) -> usize;

    /// Output sample rate (must be 48000 for WebRTC).
    fn sample_rate(&self) -> u32;

    /// Output channel count (typically 2 for stereo).
    fn channels(&self) -> u8;
}

/// Opus → AAC transcoding pipeline.
///
/// Decodes Opus to PCM using the provided decoder, optionally resamples,
/// then encodes to AAC using the provided encoder.
pub struct OpusToAacTranscoder {
    output_track_id: TrackId,
    decoder: Box<dyn OpusDecoder>,
    encoder: Box<dyn AacEncoder>,
    pcm_buffer: Vec<i16>,
    output_pts: i64,
}

impl OpusToAacTranscoder {
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

    pub fn reset(&mut self) {
        self.pcm_buffer.clear();
        self.output_pts = 0;
    }
}

/// AAC → Opus transcoding pipeline.
///
/// Decodes AAC to PCM using the provided decoder, resamples to 48kHz if
/// needed, then encodes to Opus using the provided encoder. The canonical
/// WebRTC Opus output is 48kHz/stereo/960 samples per frame.
pub struct AacToOpusTranscoder {
    output_track_id: TrackId,
    decoder: Box<dyn AacDecoder>,
    encoder: Box<dyn OpusEncoder>,
    pcm_buffer: Vec<i16>,
    output_pts: i64,
}

impl AacToOpusTranscoder {
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

    pub fn reset(&mut self) {
        self.pcm_buffer.clear();
        self.output_pts = 0;
    }
}

/// G.711 → Opus transcoding pipeline.
///
/// Decodes G.711 (A-law or μ-law) to PCM, resamples from 8kHz to 48kHz,
/// then encodes to Opus. Used when the source is G.711 but the client
/// profile requires Opus output (e.g. Browser profile with no G.711 support
/// in the offer).
pub struct G711ToOpusTranscoder {
    source_codec: CodecId,
    source_sample_rate: u32,
    output_track_id: TrackId,
    encoder: Box<dyn OpusEncoder>,
    pcm_buffer: Vec<i16>,
    output_pts: i64,
}

impl G711ToOpusTranscoder {
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

    pub fn reset(&mut self) {
        self.pcm_buffer.clear();
        self.output_pts = 0;
    }
}

/// Simple nearest-neighbor resampler (adequate for voice-grade audio → AAC).
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
