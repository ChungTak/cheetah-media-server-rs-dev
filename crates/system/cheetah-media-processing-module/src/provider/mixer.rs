//! Audio mixer internals.
//!
//! Decodes source audio streams to interleaved f32 PCM, applies per-source gain,
//! sums, clamps with a hard limiter, and encodes the result to a single output
//! stream.

use cheetah_codec::{
    frame::FrameFormat,
    track::{CodecId, MediaKind, TrackId, TrackInfo, TrackReadiness},
    AVFrame,
};
use cheetah_media_api::{
    error::{MediaError, Result as MediaResult},
    processing::{AudioMixInput, AudioTarget},
};
use tracing::warn;

use crate::config::MediaProcessingModuleConfig;
use crate::provider::audio::{
    audio_extra_data, audio_frame_to_f32_interleaved, av_frame_from_packet, channel_layout,
    create_audio_encoder, encode_pcm_frame, map_input_format,
};
use crate::provider::avcodec_registry::build_registry;

struct SourceState {
    input_track: TrackInfo,
    decoder: Option<Box<dyn avcodec::core::AudioDecoder>>,
    gain: f32,
    /// Decoded interleaved `f32` samples waiting to be mixed.
    buffer: Vec<f32>,
    eos: bool,
}

/// Blocking audio mixer that owns decoders and the output encoder.
pub(crate) struct AudioMixer {
    registry: avcodec::core::Registry,
    output_codec: CodecId,
    output_sample_rate: u32,
    output_channels: u8,
    output_frame_size: u32,
    pub(crate) output_track: TrackInfo,
    sources: Vec<SourceState>,
    encoder: Box<dyn avcodec::core::AudioEncoder>,
    output_frame_format: FrameFormat,
    output_pts: i64,
    /// Maximum interleaved samples retained per source before old samples are
    /// dropped. Bounds decoded PCM memory when a source stalls.
    max_buffer_samples: usize,
}

impl AudioMixer {
    pub(crate) fn new(
        config: &MediaProcessingModuleConfig,
        inputs: &[AudioMixInput],
        output: &AudioTarget,
        source_tracks: &[TrackInfo],
    ) -> MediaResult<Self> {
        let registry = build_registry(config)?;

        let output_codec = codec_from_audio_target(output)?;
        let output_sample_rate = output.sample_rate.unwrap_or(48_000);
        let output_channels = output.channels.unwrap_or(1);
        let bitrate = output
            .bit_rate
            .map(|b| b as u32)
            .unwrap_or_else(|| match output_codec {
                CodecId::AAC => 128_000,
                CodecId::Opus => 64_000,
                _ => 64_000,
            });

        let output_frame_size = frame_size_for_codec(output_codec, output_sample_rate);
        let max_buffer_samples = (output_frame_size * output_channels as u32 * 64) as usize;

        let (encoder, _encoder_cfg, output_frame_format, output_track) = create_audio_encoder(
            &registry,
            output_codec,
            output_sample_rate,
            output_channels,
            bitrate,
        )?;

        let mut sources = Vec::with_capacity(inputs.len());
        for (i, input) in inputs.iter().enumerate() {
            let track = source_tracks.get(i).cloned().unwrap_or_else(|| {
                let mut t = TrackInfo::new(
                    TrackId(0),
                    MediaKind::Audio,
                    CodecId::AAC,
                    output_sample_rate,
                );
                t.sample_rate = Some(output_sample_rate);
                t.channels = Some(output_channels);
                t.readiness = TrackReadiness::Ready;
                t
            });
            let gain_db = input.gain_db.unwrap_or(0);
            let gain = 10.0_f32.powf(gain_db as f32 / 20.0);
            sources.push(SourceState {
                input_track: track,
                decoder: None,
                gain,
                buffer: Vec::new(),
                eos: false,
            });
        }

        Ok(Self {
            registry,
            output_codec,
            output_sample_rate,
            output_channels,
            output_frame_size,
            output_track,
            sources,
            encoder,
            output_frame_format,
            output_pts: 0,
            max_buffer_samples,
        })
    }

    /// Feed a compressed frame from one source into that source's decoder and
    /// append the decoded PCM to its buffer.
    pub(crate) fn submit_source_frame(
        &mut self,
        source: usize,
        frame: &AVFrame,
    ) -> MediaResult<()> {
        let state = self
            .sources
            .get_mut(source)
            .ok_or_else(|| MediaError::invalid_argument("invalid source index"))?;

        if state.decoder.is_none() {
            state.decoder = Some(build_source_decoder(
                &self.registry,
                &state.input_track,
                frame,
            )?);
        }

        let src_codec = frame.codec;
        let src_format = frame.format;
        let (av_codec, bitstream) = map_input_format(src_format, src_codec).ok_or_else(|| {
            MediaError::unsupported(format!(
                "unsupported input audio codec/format: {src_codec:?}/{src_format:?}"
            ))
        })?;

        let packet = avcodec::core::Packet::from_host_bytes(
            avcodec::core::utils::next_buffer_id(),
            av_codec,
            bitstream,
            frame.payload.to_vec(),
        );

        let decoder = state
            .decoder
            .as_mut()
            .ok_or_else(|| MediaError::internal("missing source decoder"))?;
        decoder
            .submit_packet(packet)
            .map_err(|e| MediaError::invalid_argument(format!("submit audio packet: {e}")))?;

        let buffer = &mut state.buffer;
        loop {
            match decoder.poll_frame() {
                Ok(avcodec::core::Poll::Ready(audio_frame)) => {
                    if audio_frame.sample_rate != self.output_sample_rate {
                        return Err(MediaError::unsupported(format!(
                            "audio mix requires all sources to use output sample rate {} (found {})",
                            self.output_sample_rate, audio_frame.sample_rate
                        )));
                    }
                    if audio_frame.channels != self.output_channels as u16 {
                        return Err(MediaError::unsupported(format!(
                            "audio mix requires all sources to use {} channels (found {})",
                            self.output_channels, audio_frame.channels
                        )));
                    }
                    let samples = audio_frame_to_f32_interleaved(&audio_frame)?;
                    buffer.extend(samples);
                }
                Ok(avcodec::core::Poll::Pending) => break,
                Ok(avcodec::core::Poll::EndOfStream) => break,
                Err(e) => {
                    return Err(MediaError::invalid_argument(format!(
                        "poll audio frame: {e}"
                    )))
                }
            }
        }

        // Bound the per-source decoded buffer when a source stalls, keeping whole
        // output-frame chunks.
        let needed = (self.output_frame_size * self.output_channels as u32) as usize;
        if buffer.len() > self.max_buffer_samples {
            let excess = buffer.len() - self.max_buffer_samples;
            let drop = if excess == 0 || needed == 0 {
                excess
            } else {
                ((excess - 1) / needed + 1) * needed
            };
            buffer.drain(0..drop.min(buffer.len()));
        }

        Ok(())
    }

    /// Mark one source as finished.
    pub(crate) fn mark_source_eos(&mut self, source: usize) -> MediaResult<()> {
        let state = self
            .sources
            .get_mut(source)
            .ok_or_else(|| MediaError::invalid_argument("invalid source index"))?;
        state.eos = true;
        Ok(())
    }

    /// If every source has at least `needed` interleaved samples (or is EOS),
    /// produce one output frame.
    pub(crate) fn try_mix_frame(&mut self) -> MediaResult<Vec<AVFrame>> {
        let needed = (self.output_frame_size * self.output_channels as u32) as usize;
        let all_ready = self
            .sources
            .iter()
            .all(|s| s.eos || s.buffer.len() >= needed);
        if !all_ready {
            return Ok(Vec::new());
        }

        let mut mix = vec![0.0_f32; needed];
        for source in &mut self.sources {
            if source.buffer.len() >= needed {
                for (dst, src) in mix.iter_mut().zip(source.buffer.iter().take(needed)) {
                    *dst += *src * source.gain;
                }
                source.buffer.drain(0..needed);
            }
        }

        // Hard limiter.
        for sample in &mut mix {
            *sample = sample.clamp(-1.0, 1.0);
        }

        let frames = encode_pcm_frame(
            self.encoder.as_mut(),
            self.output_codec,
            self.output_frame_format,
            self.output_sample_rate,
            self.output_channels,
            self.output_frame_size,
            self.output_pts,
            &mix,
        )?;

        self.output_pts += self.output_frame_size as i64;
        Ok(frames)
    }

    /// Flush any trailing samples from the sources and the encoder itself.
    pub(crate) fn flush(&mut self) -> MediaResult<Vec<AVFrame>> {
        let needed = (self.output_frame_size * self.output_channels as u32) as usize;
        let mut out = Vec::new();

        // Drain any remaining complete frames.
        loop {
            let has_full_frame = self.sources.iter().any(|s| s.buffer.len() >= needed);
            let all_ready = self
                .sources
                .iter()
                .all(|s| s.eos || s.buffer.len() >= needed);
            if !has_full_frame || !all_ready {
                break;
            }
            out.extend(self.try_mix_frame()?);
        }

        // Flush source decoders.
        for source in &mut self.sources {
            if let Some(decoder) = source.decoder.as_mut() {
                if let Err(e) = decoder.flush() {
                    warn!("source decoder flush failed: {e}");
                    continue;
                }
                loop {
                    match decoder.poll_frame() {
                        Ok(avcodec::core::Poll::Ready(frame)) => {
                            if frame.sample_rate != self.output_sample_rate {
                                warn!("flushed frame sample rate mismatch");
                                break;
                            }
                            if frame.channels != self.output_channels as u16 {
                                warn!("flushed frame channel count mismatch");
                                break;
                            }
                            match audio_frame_to_f32_interleaved(&frame) {
                                Ok(samples) => source.buffer.extend(samples),
                                Err(e) => {
                                    warn!("convert flushed frame: {e}");
                                    break;
                                }
                            }
                        }
                        Ok(avcodec::core::Poll::Pending) => break,
                        Ok(avcodec::core::Poll::EndOfStream) => break,
                        Err(e) => {
                            warn!("poll flushed source frame: {e}");
                            break;
                        }
                    }
                }
            }
        }

        // Any new complete frames after flushing decoders.
        loop {
            let has_full_frame = self.sources.iter().any(|s| s.buffer.len() >= needed);
            let all_ready = self
                .sources
                .iter()
                .all(|s| s.eos || s.buffer.len() >= needed);
            if !has_full_frame || !all_ready {
                break;
            }
            out.extend(self.try_mix_frame()?);
        }

        // Pad a final partial frame if any source has leftover samples.
        let leftover = self.sources.iter().any(|s| !s.buffer.is_empty());
        if leftover {
            let mut mix = vec![0.0_f32; needed];
            for source in &mut self.sources {
                let take = source.buffer.len().min(needed);
                for (i, src) in source.buffer.drain(0..take).enumerate() {
                    mix[i] += src * source.gain;
                }
            }
            for sample in &mut mix {
                *sample = sample.clamp(-1.0, 1.0);
            }
            out.extend(encode_pcm_frame(
                self.encoder.as_mut(),
                self.output_codec,
                self.output_frame_format,
                self.output_sample_rate,
                self.output_channels,
                self.output_frame_size,
                self.output_pts,
                &mix,
            )?);
            self.output_pts += self.output_frame_size as i64;
        }

        self.encoder
            .flush()
            .map_err(|e| MediaError::invalid_argument(format!("flush audio encoder: {e}")))?;
        loop {
            match self.encoder.poll_packet() {
                Ok(avcodec::core::Poll::Ready(packet)) => {
                    out.push(av_frame_from_packet(
                        TrackId(0),
                        self.output_codec,
                        self.output_frame_format,
                        self.output_sample_rate,
                        &packet,
                    )?);
                }
                Ok(avcodec::core::Poll::Pending) => break,
                Ok(avcodec::core::Poll::EndOfStream) => break,
                Err(e) => {
                    return Err(MediaError::invalid_argument(format!(
                        "poll flushed audio packet: {e}"
                    )))
                }
            }
        }
        Ok(out)
    }
}

fn build_source_decoder(
    registry: &avcodec::core::Registry,
    input_track: &TrackInfo,
    input: &AVFrame,
) -> MediaResult<Box<dyn avcodec::core::AudioDecoder>> {
    if input.media_kind != MediaKind::Audio || input_track.media_kind != MediaKind::Audio {
        return Err(MediaError::invalid_argument("input is not audio"));
    }
    let (src_codec, bitstream) = map_input_format(input.format, input.codec).ok_or_else(|| {
        MediaError::unsupported(format!(
            "unsupported input audio codec/format: {:?}/{:?}",
            input.codec, input.format
        ))
    })?;
    let sample_rate = input_track.sample_rate.unwrap_or(input_track.clock_rate);
    let channels = input_track.channels.unwrap_or(1);
    let src_time_base = avcodec::core::TimeBase::new(input.timebase.num, input.timebase.den);
    let mut decoder_cfg = avcodec::core::AudioDecoderConfig::new(
        src_codec,
        sample_rate,
        channels as u16,
        channel_layout(channels),
        bitstream,
        src_time_base,
    )
    .with_memory_domain(avcodec::core::MemoryDomain::Host)
    .with_allow_staging(false);
    if let Some(extra) = audio_extra_data(input_track) {
        decoder_cfg = decoder_cfg.with_extra_data(Some(extra));
    }
    registry
        .create_audio_decoder(&decoder_cfg)
        .map_err(|e| MediaError::unsupported(format!("create audio decoder: {e}")))
}

fn frame_size_for_codec(codec: CodecId, sample_rate: u32) -> u32 {
    match codec {
        // AAC-LC uses 1024 samples per frame.
        CodecId::AAC => 1024,
        // Opus uses 20 ms frames; the frame size in samples is sample_rate / 50.
        CodecId::Opus => sample_rate / 50,
        _ => 1024,
    }
}

fn codec_from_audio_target(output: &AudioTarget) -> MediaResult<CodecId> {
    use cheetah_media_api::processing::AudioCodec;
    match output.codec {
        AudioCodec::Aac => Ok(CodecId::AAC),
        AudioCodec::Opus => Ok(CodecId::Opus),
        AudioCodec::G711A => Ok(CodecId::G711A),
        AudioCodec::G711U => Ok(CodecId::G711U),
        AudioCodec::Mp3 | AudioCodec::Pcm => Err(MediaError::unsupported(
            "audio mix output codec unsupported",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use cheetah_codec::{frame::FrameFormat, track::TrackReadiness, Timebase};

    fn g711a_track() -> TrackInfo {
        let mut track = TrackInfo::new(TrackId(0), MediaKind::Audio, CodecId::G711A, 8_000);
        track.sample_rate = Some(8_000);
        track.channels = Some(1);
        track.readiness = TrackReadiness::Ready;
        track
    }

    fn g711a_frame(payload: Vec<u8>, pts: i64) -> AVFrame {
        AVFrame::new(
            TrackId(0),
            MediaKind::Audio,
            CodecId::G711A,
            FrameFormat::G711Packet,
            pts,
            pts,
            Timebase::new(1, 8_000),
            Bytes::from(payload),
        )
    }

    #[test]
    fn mix_two_g711_sources_to_aac() {
        let mut config = MediaProcessingModuleConfig::default();
        config.profile = "software".to_string();

        let inputs = vec![
            AudioMixInput {
                source: cheetah_media_api::ids::MediaKey::with_default_vhost("app", "src1", None)
                    .unwrap(),
                gain_db: Some(0),
            },
            AudioMixInput {
                source: cheetah_media_api::ids::MediaKey::with_default_vhost("app", "src2", None)
                    .unwrap(),
                gain_db: Some(-6),
            },
        ];
        let output = AudioTarget {
            codec: cheetah_media_api::processing::AudioCodec::Aac,
            sample_rate: Some(8_000),
            channels: Some(1),
            bit_rate: Some(64_000),
        };

        let mut mixer = AudioMixer::new(&config, &inputs, &output, &[g711a_track(), g711a_track()])
            .expect("create mixer");

        for i in 0..13 {
            let pts = i * 80;
            mixer
                .submit_source_frame(0, &g711a_frame(vec![0u8; 80], pts))
                .expect("submit source 0");
            mixer
                .submit_source_frame(1, &g711a_frame(vec![0u8; 80], pts))
                .expect("submit source 1");
        }

        let out = mixer.flush().expect("flush mixer");
        assert!(!out.is_empty(), "mixer should produce AAC output");
        assert!(out.iter().all(|f| f.codec == CodecId::AAC));
    }

    #[test]
    fn flush_terminates_after_all_sources_eos() {
        let mut config = MediaProcessingModuleConfig::default();
        config.profile = "software".to_string();

        let inputs = vec![
            AudioMixInput {
                source: cheetah_media_api::ids::MediaKey::with_default_vhost("app", "src1", None)
                    .unwrap(),
                gain_db: Some(0),
            },
            AudioMixInput {
                source: cheetah_media_api::ids::MediaKey::with_default_vhost("app", "src2", None)
                    .unwrap(),
                gain_db: Some(0),
            },
        ];
        let output = AudioTarget {
            codec: cheetah_media_api::processing::AudioCodec::Aac,
            sample_rate: Some(8_000),
            channels: Some(1),
            bit_rate: Some(64_000),
        };

        let mut mixer = AudioMixer::new(&config, &inputs, &output, &[g711a_track(), g711a_track()])
            .expect("create mixer");

        for i in 0..13 {
            let pts = i * 80;
            mixer
                .submit_source_frame(0, &g711a_frame(vec![0u8; 80], pts))
                .expect("submit source 0");
            mixer
                .submit_source_frame(1, &g711a_frame(vec![0u8; 80], pts))
                .expect("submit source 1");
        }

        mixer.mark_source_eos(0).expect("eos source 0");
        mixer.mark_source_eos(1).expect("eos source 1");

        let out = mixer.flush().expect("flush mixer");
        assert!(!out.is_empty(), "mixer should produce AAC output after EOS");
        assert!(out.iter().all(|f| f.codec == CodecId::AAC));
    }

    #[test]
    fn per_source_buffer_is_capped_when_a_source_stalls() {
        let mut config = MediaProcessingModuleConfig::default();
        config.profile = "software".to_string();

        let inputs = vec![
            AudioMixInput {
                source: cheetah_media_api::ids::MediaKey::with_default_vhost("app", "src1", None)
                    .unwrap(),
                gain_db: Some(0),
            },
            AudioMixInput {
                source: cheetah_media_api::ids::MediaKey::with_default_vhost("app", "src2", None)
                    .unwrap(),
                gain_db: Some(0),
            },
        ];
        let output = AudioTarget {
            codec: cheetah_media_api::processing::AudioCodec::Aac,
            sample_rate: Some(8_000),
            channels: Some(1),
            bit_rate: Some(64_000),
        };

        let mut mixer = AudioMixer::new(&config, &inputs, &output, &[g711a_track(), g711a_track()])
            .expect("create mixer");

        // Feed source 0 many frames without feeding source 1. The per-source
        // decoded buffer must stay bounded.
        for i in 0..1000 {
            mixer
                .submit_source_frame(0, &g711a_frame(vec![0u8; 80], i * 80))
                .expect("submit source 0");
        }

        let max = mixer.max_buffer_samples;
        let len = mixer.sources[0].buffer.len();
        assert!(
            len <= max,
            "stalled source buffer {len} should not exceed {max}"
        );

        // Allow the mixer to produce output once both sources have data.
        for i in 0..1000 {
            mixer
                .submit_source_frame(1, &g711a_frame(vec![0u8; 80], i * 80))
                .expect("submit source 1");
        }

        mixer.mark_source_eos(0).expect("eos source 0");
        mixer.mark_source_eos(1).expect("eos source 1");

        let out = mixer.flush().expect("flush mixer");
        assert!(
            !out.is_empty(),
            "mixer should still produce output after cap"
        );
        assert!(out.iter().all(|f| f.codec == CodecId::AAC));
    }
}
