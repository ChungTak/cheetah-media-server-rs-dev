//! Audio transcoding adapter backed by `avcodec-rs`.
//!
//! Only compiled when `media-processing-audio` is enabled.

use bytes::Bytes;
use cheetah_codec::{
    audio::AacAudioSpecificConfig, AVFrame, CodecExtradata, CodecId, FrameFormat, MediaKind,
    Timebase, TrackId, TrackInfo, TrackReadiness,
};
use cheetah_media_api::{error::Result, MediaError};

use crate::config::MediaProcessingModuleConfig;
use crate::provider::avcodec_registry::build_registry;

/// One-shot audio transcode result: zero or more output frames plus the
/// updated track description.
#[derive(Debug, Clone)]
pub struct AudioTranscodeResult {
    pub frames: Vec<AVFrame>,
    pub track: TrackInfo,
}

/// Transcodes a single compressed audio `AVFrame` to the requested codec,
/// sample rate and channel count.
///
/// This is a stateless, one-shot helper intended for unit tests and for
/// building higher-level streaming sessions. It creates a fresh decoder +
/// encoder pair, submits the input packet, flushes, and returns all produced
/// output packets.
pub fn transcode_audio_frame(
    input: &AVFrame,
    input_track: &TrackInfo,
    output_codec: CodecId,
    output_sample_rate: u32,
    output_channels: u8,
    config: &MediaProcessingModuleConfig,
) -> Result<AudioTranscodeResult> {
    if input.media_kind != MediaKind::Audio {
        return Err(MediaError::invalid_argument("input frame is not audio"));
    }
    if input_track.media_kind != MediaKind::Audio {
        return Err(MediaError::invalid_argument("input track is not audio"));
    }

    let input_codec = input.codec;
    let (src_av_codec, src_bitstream) =
        map_input_format(input.format, input_codec).ok_or_else(|| {
            MediaError::unsupported(format!(
                "unsupported input audio codec/format: {input_codec:?}/{:?}",
                input.format
            ))
        })?;

    let (dst_av_codec, dst_frame_format) = map_output_codec(output_codec).ok_or_else(|| {
        MediaError::unsupported(format!("unsupported output audio codec: {output_codec:?}"))
    })?;

    if output_channels == 0 || output_channels > 2 {
        return Err(MediaError::unsupported(
            "audio output channel count must be 1 or 2",
        ));
    }

    let registry = build_registry(config)?;

    let sample_rate = input_track.sample_rate.unwrap_or(input_track.clock_rate);
    let channels = input_track.channels.unwrap_or(1);
    let src_time_base = avcodec::core::TimeBase::new(input.timebase.num, input.timebase.den);

    let mut decoder_cfg = avcodec::core::AudioDecoderConfig::new(
        src_av_codec,
        sample_rate,
        channels as u16,
        channel_layout(channels),
        src_bitstream,
        src_time_base,
    )
    .with_memory_domain(avcodec::core::MemoryDomain::Host)
    .with_allow_staging(false);

    if let Some(extra) = audio_extra_data(input_track) {
        decoder_cfg = decoder_cfg.with_extra_data(Some(extra));
    }

    let dst_time_base = avcodec::core::TimeBase::new(1, output_sample_rate);
    let encoder_cfg = avcodec::core::AudioEncoderConfig::new(
        dst_av_codec,
        output_sample_rate,
        output_channels as u16,
        channel_layout(output_channels),
        avcodec::core::AudioSampleFormat::S16,
        default_bitrate(output_codec),
        dst_time_base,
    )
    .with_memory_domain(avcodec::core::MemoryDomain::Host)
    .with_allow_staging(false);

    let encoder_cfg = with_audio_profile(encoder_cfg, output_codec);
    let encoder_cfg = with_audio_frame_size(encoder_cfg, output_codec, output_sample_rate);

    let mut transcoder = avcodec::AudioTranscoder::new(&registry, &decoder_cfg, &encoder_cfg)
        .map_err(|e| MediaError::unsupported(format!("create audio transcoder: {e}")))?;

    let packet = avcodec::core::Packet::from_host_bytes(
        avcodec::core::utils::next_buffer_id(),
        src_av_codec,
        src_bitstream,
        input.payload.to_vec(),
    );

    transcoder
        .submit_packet(packet)
        .map_err(|e| MediaError::invalid_argument(format!("submit audio packet: {e}")))?;

    let mut output_frames = Vec::new();
    loop {
        match transcoder.poll_packet() {
            Ok(avcodec::core::Poll::Ready(packet)) => {
                output_frames.push(av_frame_from_packet(
                    input.track_id,
                    output_codec,
                    dst_frame_format,
                    output_sample_rate,
                    &packet,
                )?);
            }
            Ok(avcodec::core::Poll::Pending) => break,
            Ok(avcodec::core::Poll::EndOfStream) => break,
            Err(e) => {
                return Err(MediaError::invalid_argument(format!(
                    "poll audio packet: {e}"
                )))
            }
        }
    }

    transcoder
        .flush()
        .map_err(|e| MediaError::invalid_argument(format!("flush audio transcoder: {e}")))?;

    loop {
        match transcoder.poll_packet() {
            Ok(avcodec::core::Poll::Ready(packet)) => {
                output_frames.push(av_frame_from_packet(
                    input.track_id,
                    output_codec,
                    dst_frame_format,
                    output_sample_rate,
                    &packet,
                )?);
            }
            Ok(avcodec::core::Poll::EndOfStream) | Ok(avcodec::core::Poll::Pending) => break,
            Err(e) => {
                return Err(MediaError::invalid_argument(format!(
                    "poll audio packet: {e}"
                )))
            }
        }
    }

    let mut output_track = TrackInfo::new(
        input.track_id,
        MediaKind::Audio,
        output_codec,
        output_sample_rate,
    );
    output_track.sample_rate = Some(output_sample_rate);
    output_track.channels = Some(output_channels);
    output_track.extradata =
        output_codec_extradata(output_codec, output_sample_rate, output_channels)
            .unwrap_or(CodecExtradata::None);
    output_track.readiness = TrackReadiness::Ready;

    Ok(AudioTranscodeResult {
        frames: output_frames,
        track: output_track,
    })
}

/// Output specification for a streaming audio transcode session.
///
/// 流式音频转码 session 的输出规格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioTranscodeSpec {
    pub codec: CodecId,
    pub sample_rate: u32,
    pub channels: u8,
    pub bitrate: u32,
}

/// Stateful streaming audio transcode session.
///
/// 有状态的流式音频转码 session。
pub struct AudioTranscodeSession {
    config: MediaProcessingModuleConfig,
    source_track: TrackInfo,
    spec: AudioTranscodeSpec,
    transcoder: Option<avcodec::AudioTranscoder>,
    output_track: TrackInfo,
}

impl AudioTranscodeSession {
    /// Create a new streaming transcode session for a source audio track.
    pub fn new(
        input_track: &TrackInfo,
        spec: &AudioTranscodeSpec,
        config: &MediaProcessingModuleConfig,
    ) -> Result<Self> {
        if input_track.media_kind != MediaKind::Audio {
            return Err(MediaError::invalid_argument("input track is not audio"));
        }
        if !matches!(
            spec.codec,
            CodecId::G711A | CodecId::G711U | CodecId::AAC | CodecId::Opus
        ) {
            return Err(MediaError::unsupported(format!(
                "unsupported output audio codec: {codec:?}",
                codec = spec.codec
            )));
        }

        let output_track = build_audio_output_track(input_track, spec);
        Ok(Self {
            config: config.clone(),
            source_track: input_track.clone(),
            spec: *spec,
            transcoder: None,
            output_track,
        })
    }

    /// Submit one compressed source audio frame and return output frames.
    pub fn submit(&mut self, input: &AVFrame) -> Result<Vec<AVFrame>> {
        if input.media_kind != MediaKind::Audio {
            return Err(MediaError::invalid_argument("input frame is not audio"));
        }
        if self.transcoder.is_none() {
            self.transcoder = Some(self.build_transcoder(input)?);
        }
        let transcoder = self.transcoder.as_mut().unwrap();

        let input_codec = input.codec;
        let (src_av_codec, src_bitstream) = map_input_format(input.format, input_codec)
            .ok_or_else(|| {
                MediaError::unsupported(format!(
                    "unsupported input audio codec/format: {input_codec:?}/{:?}",
                    input.format
                ))
            })?;

        let packet = avcodec::core::Packet::from_host_bytes(
            avcodec::core::utils::next_buffer_id(),
            src_av_codec,
            src_bitstream,
            input.payload.to_vec(),
        );

        transcoder
            .submit_packet(packet)
            .map_err(|e| MediaError::invalid_argument(format!("submit audio packet: {e}")))?;

        let mut output_frames = Vec::new();
        drain_audio_transcoder(
            transcoder,
            input.track_id,
            self.spec.codec,
            map_output_codec(self.spec.codec)
                .map(|(_, fmt)| fmt)
                .unwrap_or(FrameFormat::Unknown),
            self.spec.sample_rate,
            &mut output_frames,
        )?;
        Ok(output_frames)
    }

    /// Flush the encoder/decoder and return any remaining frames.
    pub fn flush(&mut self) -> Result<Vec<AVFrame>> {
        let Some(transcoder) = self.transcoder.as_mut() else {
            return Ok(Vec::new());
        };
        transcoder
            .flush()
            .map_err(|e| MediaError::invalid_argument(format!("flush audio transcoder: {e}")))?;

        let mut output_frames = Vec::new();
        drain_audio_transcoder(
            transcoder,
            self.source_track.track_id,
            self.spec.codec,
            map_output_codec(self.spec.codec)
                .map(|(_, fmt)| fmt)
                .unwrap_or(FrameFormat::Unknown),
            self.spec.sample_rate,
            &mut output_frames,
        )?;
        Ok(output_frames)
    }

    /// Return the current output track description.
    pub fn output_track(&self) -> &TrackInfo {
        &self.output_track
    }

    fn build_transcoder(&self, input: &AVFrame) -> Result<avcodec::AudioTranscoder> {
        let input_codec = input.codec;
        let (src_av_codec, src_bitstream) = map_input_format(input.format, input_codec)
            .ok_or_else(|| {
                MediaError::unsupported(format!(
                    "unsupported input audio codec/format: {input_codec:?}/{:?}",
                    input.format
                ))
            })?;

        let (dst_av_codec, _) = map_output_codec(self.spec.codec).ok_or_else(|| {
            MediaError::unsupported(format!(
                "unsupported output audio codec: {codec:?}",
                codec = self.spec.codec
            ))
        })?;

        let registry = build_registry(&self.config)?;

        let sample_rate = self
            .source_track
            .sample_rate
            .unwrap_or(self.source_track.clock_rate);
        let channels = self.source_track.channels.unwrap_or(1);
        let src_time_base = avcodec::core::TimeBase::new(input.timebase.num, input.timebase.den);

        let mut decoder_cfg = avcodec::core::AudioDecoderConfig::new(
            src_av_codec,
            sample_rate,
            channels as u16,
            channel_layout(channels),
            src_bitstream,
            src_time_base,
        )
        .with_memory_domain(avcodec::core::MemoryDomain::Host)
        .with_allow_staging(false);

        if let Some(extra) = audio_extra_data(&self.source_track) {
            decoder_cfg = decoder_cfg.with_extra_data(Some(extra));
        }

        let dst_time_base = avcodec::core::TimeBase::new(1, self.spec.sample_rate);
        let encoder_cfg = avcodec::core::AudioEncoderConfig::new(
            dst_av_codec,
            self.spec.sample_rate,
            self.spec.channels as u16,
            channel_layout(self.spec.channels),
            avcodec::core::AudioSampleFormat::S16,
            self.spec.bitrate,
            dst_time_base,
        )
        .with_memory_domain(avcodec::core::MemoryDomain::Host)
        .with_allow_staging(false);

        let encoder_cfg = with_audio_profile(encoder_cfg, self.spec.codec);
        let encoder_cfg =
            with_audio_frame_size(encoder_cfg, self.spec.codec, self.spec.sample_rate);

        avcodec::AudioTranscoder::new(&registry, &decoder_cfg, &encoder_cfg)
            .map_err(|e| MediaError::unsupported(format!("create audio transcoder: {e}")))
    }
}

fn build_audio_output_track(input_track: &TrackInfo, spec: &AudioTranscodeSpec) -> TrackInfo {
    let mut output_track = TrackInfo::new(
        input_track.track_id,
        MediaKind::Audio,
        spec.codec,
        spec.sample_rate,
    );
    output_track.sample_rate = Some(spec.sample_rate);
    output_track.channels = Some(spec.channels);
    output_track.bitrate = Some(spec.bitrate);
    output_track.extradata = output_codec_extradata(spec.codec, spec.sample_rate, spec.channels)
        .unwrap_or(CodecExtradata::None);
    output_track.readiness = TrackReadiness::Ready;
    output_track
}

fn drain_audio_transcoder(
    transcoder: &mut avcodec::AudioTranscoder,
    track_id: TrackId,
    codec: CodecId,
    frame_format: FrameFormat,
    sample_rate: u32,
    out: &mut Vec<AVFrame>,
) -> Result<()> {
    loop {
        match transcoder.poll_packet() {
            Ok(avcodec::core::Poll::Ready(packet)) => {
                out.push(av_frame_from_packet(
                    track_id,
                    codec,
                    frame_format,
                    sample_rate,
                    &packet,
                )?);
            }
            Ok(avcodec::core::Poll::Pending) => break,
            Ok(avcodec::core::Poll::EndOfStream) => break,
            Err(e) => {
                return Err(MediaError::invalid_argument(format!(
                    "poll audio packet: {e}"
                )))
            }
        }
    }
    Ok(())
}

pub(crate) fn map_input_format(
    format: FrameFormat,
    codec: CodecId,
) -> Option<(avcodec::core::CodecId, avcodec::core::BitstreamFormat)> {
    match (format, codec) {
        (FrameFormat::G711Packet, CodecId::G711A) | (FrameFormat::Unknown, CodecId::G711A) => {
            Some((
                avcodec::core::CodecId::G711A,
                avcodec::core::BitstreamFormat::G711A,
            ))
        }
        (FrameFormat::G711Packet, CodecId::G711U) | (FrameFormat::Unknown, CodecId::G711U) => {
            Some((
                avcodec::core::CodecId::G711U,
                avcodec::core::BitstreamFormat::G711U,
            ))
        }
        (FrameFormat::AacRaw, CodecId::AAC) | (FrameFormat::Unknown, CodecId::AAC) => Some((
            avcodec::core::CodecId::Aac,
            avcodec::core::BitstreamFormat::AacRaw,
        )),
        (FrameFormat::OpusPacket, CodecId::Opus) | (FrameFormat::Unknown, CodecId::Opus) => Some((
            avcodec::core::CodecId::Opus,
            avcodec::core::BitstreamFormat::OpusPacket,
        )),
        (FrameFormat::Mp3Frame, CodecId::MP3) | (FrameFormat::Unknown, CodecId::MP3) => Some((
            avcodec::core::CodecId::Mp3,
            avcodec::core::BitstreamFormat::Mp3Frame,
        )),
        _ => None,
    }
}

pub(crate) fn map_output_codec(codec: CodecId) -> Option<(avcodec::core::CodecId, FrameFormat)> {
    match codec {
        CodecId::G711A => Some((avcodec::core::CodecId::G711A, FrameFormat::G711Packet)),
        CodecId::G711U => Some((avcodec::core::CodecId::G711U, FrameFormat::G711Packet)),
        CodecId::AAC => Some((avcodec::core::CodecId::Aac, FrameFormat::AacRaw)),
        CodecId::Opus => Some((avcodec::core::CodecId::Opus, FrameFormat::OpusPacket)),
        _ => None,
    }
}

pub(crate) fn channel_layout(channels: u8) -> avcodec::core::AudioChannelLayout {
    match channels {
        1 => avcodec::core::AudioChannelLayout::Mono,
        2 => avcodec::core::AudioChannelLayout::Stereo,
        n => avcodec::core::AudioChannelLayout::Unspecified { channels: n as u16 },
    }
}

pub(crate) fn audio_extra_data(track: &TrackInfo) -> Option<avcodec::core::BufferSlice> {
    use cheetah_codec::CodecExtradata;
    let bytes: Option<Bytes> = match &track.extradata {
        CodecExtradata::AAC { asc } => Some(asc.clone()),
        CodecExtradata::Opus {
            channel_mapping: Some(bytes),
            ..
        }
        | CodecExtradata::AV1 {
            sequence_header: Some(bytes),
            ..
        }
        | CodecExtradata::VP8 {
            config: Some(bytes),
        }
        | CodecExtradata::VP9 {
            config: Some(bytes),
        }
        | CodecExtradata::Raw(bytes)
        | CodecExtradata::MP3 {
            side_info: Some(bytes),
        } => Some(bytes.clone()),
        _ => None,
    };
    bytes.and_then(|b| {
        if b.is_empty() {
            return None;
        }
        let len = b.len();
        let handle = avcodec::core::BufferHandle::from_host_bytes(0, b.to_vec());
        Some(avcodec::core::BufferSlice::new(handle, 0, len))
    })
}

pub(crate) fn default_bitrate(codec: CodecId) -> u32 {
    match codec {
        CodecId::G711A | CodecId::G711U => 64_000,
        CodecId::Opus => 64_000,
        CodecId::AAC => 128_000,
        _ => 128_000,
    }
}

pub(crate) fn with_audio_profile(
    cfg: avcodec::core::AudioEncoderConfig,
    codec: CodecId,
) -> avcodec::core::AudioEncoderConfig {
    match codec {
        CodecId::AAC => cfg.with_profile(Some(avcodec::core::AudioCodecProfile::AacLc)),
        CodecId::Opus => cfg.with_profile(Some(avcodec::core::AudioCodecProfile::OpusAudio)),
        _ => cfg,
    }
}

pub(crate) fn with_audio_frame_size(
    cfg: avcodec::core::AudioEncoderConfig,
    codec: CodecId,
    sample_rate: u32,
) -> avcodec::core::AudioEncoderConfig {
    match codec {
        // AAC-LC requires 1024 samples per frame.
        CodecId::AAC => cfg.with_frame_size(Some(1024)),
        // Opus uses 20 ms frames; the frame size in samples is sample_rate / 50.
        CodecId::Opus => cfg.with_frame_size(Some(sample_rate / 50)),
        _ => cfg,
    }
}

pub(crate) fn output_codec_extradata(
    codec: CodecId,
    sample_rate: u32,
    channels: u8,
) -> Option<CodecExtradata> {
    match codec {
        CodecId::AAC => {
            let index = aac_sample_rate_index(sample_rate)?;
            let asc = AacAudioSpecificConfig {
                audio_object_type: 2, // AAC-LC
                sampling_frequency_index: index,
                channel_configuration: channels,
            };
            Some(CodecExtradata::AAC {
                asc: Bytes::copy_from_slice(&asc.to_bytes()),
            })
        }
        CodecId::Opus => Some(CodecExtradata::Opus {
            fmtp: None,
            channel_mapping: None,
        }),
        _ => None,
    }
}

pub(crate) fn aac_sample_rate_index(sample_rate: u32) -> Option<u8> {
    match sample_rate {
        96_000 => Some(0),
        88_200 => Some(1),
        64_000 => Some(2),
        48_000 => Some(3),
        44_100 => Some(4),
        32_000 => Some(5),
        24_000 => Some(6),
        22_050 => Some(7),
        16_000 => Some(8),
        12_000 => Some(9),
        11_025 => Some(10),
        8_000 => Some(11),
        7_350 => Some(12),
        _ => None,
    }
}

pub(crate) fn av_frame_from_packet(
    track_id: TrackId,
    codec: CodecId,
    format: FrameFormat,
    sample_rate: u32,
    packet: &avcodec::core::Packet,
) -> Result<AVFrame> {
    let payload_bytes = packet
        .data
        .host_bytes()
        .map_err(|e| MediaError::internal(format!("read audio packet payload: {e}")))?
        .ok_or_else(|| MediaError::internal("audio packet payload not in host memory"))?;

    let pts = packet.pts.unwrap_or(0);
    let dts = packet.dts.unwrap_or(pts);
    let timebase = Timebase::new(1, sample_rate);

    let mut frame = AVFrame::new(
        track_id,
        MediaKind::Audio,
        codec,
        format,
        pts,
        dts,
        timebase,
        Bytes::copy_from_slice(payload_bytes),
    );
    if let Some(duration) = packet.duration {
        let _ = frame.set_duration(duration);
    }
    Ok(frame)
}

/// Convert an [`avcodec::core::AudioFrame`] to interleaved `f32` samples.
pub(crate) fn audio_frame_to_f32_interleaved(
    frame: &avcodec::core::AudioFrame,
) -> Result<Vec<f32>> {
    use avcodec::core::AudioSampleFormat;

    let channels = frame.channels as usize;
    let samples = frame.samples_per_channel as usize;
    if channels == 0 || samples == 0 {
        return Ok(Vec::new());
    }
    let mut out = vec![0.0_f32; channels * samples];

    match frame.format {
        AudioSampleFormat::S16 => {
            let plane = frame
                .plane_host_bytes(0)
                .map_err(|e| MediaError::internal(format!("read S16 audio plane: {e}")))?
                .ok_or_else(|| MediaError::internal("missing S16 audio plane"))?;
            for i in 0..samples * channels {
                let bytes = &plane[i * 2..i * 2 + 2];
                let sample = i16::from_ne_bytes([bytes[0], bytes[1]]) as f32 / i16::MAX as f32;
                out[i] = sample.clamp(-1.0, 1.0);
            }
        }
        AudioSampleFormat::F32 => {
            let plane = frame
                .plane_host_bytes(0)
                .map_err(|e| MediaError::internal(format!("read F32 audio plane: {e}")))?
                .ok_or_else(|| MediaError::internal("missing F32 audio plane"))?;
            for i in 0..samples * channels {
                let bytes = &plane[i * 4..i * 4 + 4];
                out[i] = f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            }
        }
        AudioSampleFormat::S16Planar | AudioSampleFormat::F32Planar => {
            // For planar formats de-interleave into output interleaved buffer.
            for c in 0..channels {
                let plane = frame
                    .plane_host_bytes(c)
                    .map_err(|e| MediaError::internal(format!("read planar audio plane {c}: {e}")))?
                    .ok_or_else(|| {
                        MediaError::internal(format!("missing planar audio plane {c}"))
                    })?;
                match frame.format {
                    AudioSampleFormat::S16Planar => {
                        for s in 0..samples {
                            let bytes = &plane[s * 2..s * 2 + 2];
                            let sample =
                                i16::from_ne_bytes([bytes[0], bytes[1]]) as f32 / i16::MAX as f32;
                            out[s * channels + c] = sample.clamp(-1.0, 1.0);
                        }
                    }
                    AudioSampleFormat::F32Planar => {
                        for s in 0..samples {
                            let bytes = &plane[s * 4..s * 4 + 4];
                            out[s * channels + c] =
                                f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }
        _ => {
            return Err(MediaError::unsupported(format!(
                "unsupported decoded audio sample format: {:?}",
                frame.format
            )))
        }
    }
    Ok(out)
}

/// Clamp and convert interleaved `f32` samples to host-endian `i16` bytes.
pub(crate) fn f32_interleaved_to_s16_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let value = (clamped * i16::MAX as f32) as i16;
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

/// Build an [`avcodec::core::AudioEncoder`] for the requested output codec and
/// return its configuration and target [`FrameFormat`].
pub(crate) fn create_audio_encoder(
    registry: &avcodec::core::Registry,
    output_codec: CodecId,
    sample_rate: u32,
    channels: u8,
    bitrate: u32,
) -> Result<(
    Box<dyn avcodec::core::AudioEncoder>,
    avcodec::core::AudioEncoderConfig,
    FrameFormat,
    TrackInfo,
)> {
    let (dst_av_codec, frame_format) = map_output_codec(output_codec).ok_or_else(|| {
        MediaError::unsupported(format!("unsupported output audio codec: {output_codec:?}"))
    })?;
    if channels == 0 || channels > 2 {
        return Err(MediaError::unsupported(
            "audio output channel count must be 1 or 2",
        ));
    }

    let dst_time_base = avcodec::core::TimeBase::new(1, sample_rate);
    let mut encoder_cfg = avcodec::core::AudioEncoderConfig::new(
        dst_av_codec,
        sample_rate,
        channels as u16,
        channel_layout(channels),
        avcodec::core::AudioSampleFormat::S16,
        bitrate,
        dst_time_base,
    )
    .with_memory_domain(avcodec::core::MemoryDomain::Host)
    .with_allow_staging(false);
    encoder_cfg = with_audio_profile(encoder_cfg, output_codec);
    encoder_cfg = with_audio_frame_size(encoder_cfg, output_codec, sample_rate);

    let encoder = registry
        .create_audio_encoder(&encoder_cfg)
        .map_err(|e| MediaError::unsupported(format!("create audio encoder: {e}")))?;

    let mut output_track = TrackInfo::new(TrackId(0), MediaKind::Audio, output_codec, sample_rate);
    output_track.sample_rate = Some(sample_rate);
    output_track.channels = Some(channels);
    output_track.bitrate = Some(bitrate);
    output_track.extradata =
        output_codec_extradata(output_codec, sample_rate, channels).unwrap_or(CodecExtradata::None);
    output_track.readiness = TrackReadiness::Ready;

    Ok((encoder, encoder_cfg, frame_format, output_track))
}

/// Submit a single frame of interleaved `f32` PCM to an [`avcodec::core::AudioEncoder`]
/// and drain all produced compressed packets into [`AVFrame`]s.
#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_pcm_frame(
    encoder: &mut dyn avcodec::core::AudioEncoder,
    output_codec: CodecId,
    frame_format: FrameFormat,
    sample_rate: u32,
    channels: u8,
    samples_per_channel: u32,
    pts: i64,
    samples: &[f32],
) -> Result<Vec<AVFrame>> {
    let s16_bytes = f32_interleaved_to_s16_bytes(samples);
    let audio_frame = avcodec::core::AudioFrame::new_host_interleaved_s16(
        sample_rate,
        channel_layout(channels),
        channels as u16,
        samples_per_channel,
        s16_bytes,
    )
    .map_err(|e| MediaError::invalid_argument(format!("build audio encoder input frame: {e}")))?;
    let mut frame = audio_frame;
    frame.pts = Some(pts);
    frame.dts = Some(pts);
    frame.duration = Some(samples_per_channel as i64);

    encoder
        .submit_frame(frame)
        .map_err(|e| MediaError::invalid_argument(format!("submit audio frame: {e}")))?;

    let mut out = Vec::new();
    loop {
        match encoder.poll_packet() {
            Ok(avcodec::core::Poll::Ready(packet)) => {
                out.push(av_frame_from_packet(
                    TrackId(0),
                    output_codec,
                    frame_format,
                    sample_rate,
                    &packet,
                )?);
            }
            Ok(avcodec::core::Poll::Pending) => break,
            Ok(avcodec::core::Poll::EndOfStream) => break,
            Err(e) => {
                return Err(MediaError::invalid_argument(format!(
                    "poll audio packet: {e}"
                )))
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn g711a_packet(payload: &[u8]) -> (AVFrame, TrackInfo) {
        let payload = Bytes::copy_from_slice(payload);
        let track = TrackInfo::new(TrackId(0), MediaKind::Audio, CodecId::G711A, 8_000);
        let mut track = track;
        track.sample_rate = Some(8_000);
        track.channels = Some(1);
        track.readiness = TrackReadiness::Ready;
        let frame = AVFrame::new(
            TrackId(0),
            MediaKind::Audio,
            CodecId::G711A,
            FrameFormat::G711Packet,
            0,
            0,
            Timebase::new(1, 8_000),
            payload,
        );
        (frame, track)
    }

    #[test]
    fn g711a_to_g711u_preserves_length() {
        let cfg = MediaProcessingModuleConfig::default();
        let payload = vec![0u8; 80]; // 10 ms of G.711 @ 8 kHz mono
        let (frame, track) = g711a_packet(&payload);

        let result = transcode_audio_frame(&frame, &track, CodecId::G711U, 8_000, 1, &cfg)
            .expect("transcode");

        assert_eq!(result.frames.len(), 1);
        assert_eq!(result.frames[0].payload.len(), 80);
        assert_eq!(result.frames[0].codec, CodecId::G711U);
        assert_eq!(result.track.codec, CodecId::G711U);
        assert_eq!(result.track.sample_rate, Some(8_000));
    }

    #[test]
    fn g711a_to_opus_resamples_to_48k() {
        let cfg = MediaProcessingModuleConfig::default();
        // 20 ms @ 8 kHz mono = 160 bytes
        let payload = (0..160).map(|v| v as u8).collect::<Vec<_>>();
        let (frame, track) = g711a_packet(&payload);

        let result = transcode_audio_frame(&frame, &track, CodecId::Opus, 48_000, 1, &cfg)
            .expect("transcode");

        assert!(!result.frames.is_empty());
        assert_eq!(result.frames[0].codec, CodecId::Opus);
        assert_eq!(result.track.codec, CodecId::Opus);
        assert_eq!(result.track.sample_rate, Some(48_000));
    }

    #[test]
    fn g711a_to_aac_48k() {
        let cfg = MediaProcessingModuleConfig::default();
        // 20 ms @ 8 kHz mono = 160 bytes
        let payload = (0..160).map(|v| v as u8).collect::<Vec<_>>();
        let (frame, track) = g711a_packet(&payload);

        let result = transcode_audio_frame(&frame, &track, CodecId::AAC, 48_000, 1, &cfg)
            .expect("transcode");

        assert!(!result.frames.is_empty());
        assert_eq!(result.frames[0].codec, CodecId::AAC);
        assert_eq!(result.track.codec, CodecId::AAC);
        assert_eq!(result.track.sample_rate, Some(48_000));
    }

    #[test]
    fn opus_to_g711u_resamples_to_8k() {
        let cfg = MediaProcessingModuleConfig::default();
        // 20 ms @ 8 kHz mono = 160 bytes
        let payload = (0..160).map(|v| v as u8).collect::<Vec<_>>();
        let (frame, track) = g711a_packet(&payload);

        let opus = transcode_audio_frame(&frame, &track, CodecId::Opus, 48_000, 1, &cfg)
            .expect("encode to opus")
            .frames
            .pop()
            .expect("one opus packet");

        let mut opus_track = TrackInfo::new(TrackId(0), MediaKind::Audio, CodecId::Opus, 48_000);
        opus_track.sample_rate = Some(48_000);
        opus_track.channels = Some(1);
        opus_track.readiness = TrackReadiness::Ready;

        let result = transcode_audio_frame(&opus, &opus_track, CodecId::G711U, 8_000, 1, &cfg)
            .expect("transcode opus to g711u");

        assert!(!result.frames.is_empty());
        assert_eq!(result.frames[0].codec, CodecId::G711U);
        assert_eq!(result.track.codec, CodecId::G711U);
        assert_eq!(result.track.sample_rate, Some(8_000));
    }

    #[test]
    fn g711a_to_aac_to_g711u_roundtrip() {
        let cfg = MediaProcessingModuleConfig::default();
        // 20 ms @ 8 kHz mono = 160 bytes
        let payload = (0..160).map(|v| v as u8).collect::<Vec<_>>();
        let (frame, track) = g711a_packet(&payload);

        let aac = transcode_audio_frame(&frame, &track, CodecId::AAC, 48_000, 1, &cfg)
            .expect("encode to aac");
        let aac_frame = aac.frames.into_iter().next().expect("one aac packet");
        let aac_track = aac.track;

        let result = transcode_audio_frame(&aac_frame, &aac_track, CodecId::G711U, 8_000, 1, &cfg)
            .expect("decode aac to g711u");

        assert!(!result.frames.is_empty());
        assert_eq!(result.frames[0].codec, CodecId::G711U);
        assert_eq!(result.track.codec, CodecId::G711U);
        assert_eq!(result.track.sample_rate, Some(8_000));
    }

    #[test]
    fn aac_to_opus_and_opus_to_aac_matrix() {
        let cfg = MediaProcessingModuleConfig::default();
        // 20 ms @ 8 kHz mono = 160 bytes
        let payload = (0..160).map(|v| v as u8).collect::<Vec<_>>();
        let (frame, track) = g711a_packet(&payload);

        // G.711 -> AAC provides a valid AAC packet + ASC.
        let aac = transcode_audio_frame(&frame, &track, CodecId::AAC, 48_000, 1, &cfg)
            .expect("encode to aac");
        let aac_frame = aac.frames.into_iter().next().expect("one aac packet");
        let aac_track = aac.track;

        let opus = transcode_audio_frame(&aac_frame, &aac_track, CodecId::Opus, 48_000, 1, &cfg)
            .expect("aac to opus")
            .frames
            .pop()
            .expect("one opus packet");

        let mut opus_track = TrackInfo::new(TrackId(0), MediaKind::Audio, CodecId::Opus, 48_000);
        opus_track.sample_rate = Some(48_000);
        opus_track.channels = Some(1);
        opus_track.readiness = TrackReadiness::Ready;

        let back_to_aac = transcode_audio_frame(&opus, &opus_track, CodecId::AAC, 48_000, 1, &cfg)
            .expect("opus to aac");

        assert!(!back_to_aac.frames.is_empty());
        assert_eq!(back_to_aac.frames[0].codec, CodecId::AAC);
        assert_eq!(back_to_aac.track.codec, CodecId::AAC);
    }

    #[test]
    fn streaming_session_produces_opus_output() {
        let cfg = MediaProcessingModuleConfig::default();
        let payload = (0..160).map(|v| v as u8).collect::<Vec<_>>();
        let (frame, track) = g711a_packet(&payload);

        let spec = AudioTranscodeSpec {
            codec: CodecId::Opus,
            sample_rate: 48_000,
            channels: 1,
            bitrate: 64_000,
        };
        let mut session = AudioTranscodeSession::new(&track, &spec, &cfg)
            .expect("create audio transcode session");

        let mut output = Vec::new();
        for i in 0..10 {
            let mut frame = frame.clone();
            frame.pts = i * 160;
            frame.dts = i * 160;
            output.extend(session.submit(&frame).expect("submit g711a frame"));
        }
        assert!(!output.is_empty(), "session should produce opus output");
        assert!(output.iter().all(|f| f.codec == CodecId::Opus));
        assert_eq!(session.output_track().codec, CodecId::Opus);
        assert_eq!(session.output_track().sample_rate, Some(48_000));
        assert_eq!(session.output_track().channels, Some(1));

        let flushed = session.flush().expect("flush session");
        assert!(flushed.iter().all(|f| f.codec == CodecId::Opus));
    }
}
