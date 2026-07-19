//! Honest preflight for [`MediaProcessingApi`].
//!
//! Probes the compiled avcodec profile to report which operations are actually
//! available, which backend was selected, and why a particular codec or
//! operator is unsupported. All blocking work is run on `RuntimeApi::spawn_blocking`.

use std::collections::HashMap;

use cheetah_media_api::{error::Result as MediaResult, processing::ProcessingPreflightReport};

use crate::config::MediaProcessingModuleConfig;

#[cfg(feature = "media-processing-cpu")]
use cheetah_media_api::MediaError;
#[cfg(feature = "media-processing-cpu")]
use cheetah_sdk::{RuntimeApi, SpawnError};
#[cfg(feature = "media-processing-cpu")]
use futures::channel::oneshot;

#[cfg(feature = "media-processing-cpu")]
use crate::provider::avcodec_registry::build_registry;

#[cfg(feature = "media-processing-cpu")]
use avcodec::core::{
    AudioChannelLayout, AudioDecoderConfig, AudioEncoderConfig, AudioSampleFormat, BitstreamFormat,
    CodecId, DecoderConfig, EncoderConfig, ImageInfo, ImageOpKind, ImageProcessorConfig,
    JpegDecoderConfig, JpegEncoderConfig, MemoryDomain, SelectionPreflight, TimeBase,
};

#[cfg(feature = "media-processing-cpu")]
use avcodec::AudioTranscoder;

/// Run a blocking closure on the runtime's blocking pool and return its result.
#[cfg(feature = "media-processing-cpu")]
async fn run_blocking<F, R>(runtime: &dyn RuntimeApi, name: &str, f: F) -> MediaResult<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    let task = Box::new(move || {
        let _ = tx.send(f());
    });
    let handle = runtime
        .spawn_blocking(name, task)
        .map_err(|e: SpawnError| MediaError::internal(format!("spawn blocking preflight: {e}")))?;
    let result = rx
        .await
        .map_err(|_| MediaError::internal("preflight blocking task canceled"))?;
    handle
        .wait()
        .await
        .map_err(|e| MediaError::internal(format!("preflight blocking task failed: {e}")))?;
    Ok(result)
}

/// Produce a `ProcessingPreflightReport` by probing the compiled profile.
pub(crate) async fn preflight_processing(
    _ctx: &cheetah_sdk::EngineContext,
    config: &MediaProcessingModuleConfig,
) -> MediaResult<ProcessingPreflightReport> {
    let profile = config.profile.clone();
    let mut features = Vec::new();
    if cfg!(feature = "media-processing-caption") {
        features.push("media-processing-caption".to_string());
    }
    if cfg!(feature = "media-processing-cpu") {
        features.push("media-processing-cpu".to_string());
    }
    if cfg!(feature = "media-processing-image") {
        features.push("media-processing-image".to_string());
    }
    if cfg!(feature = "media-processing-image-overlay") {
        features.push("media-processing-image-overlay".to_string());
    }
    if cfg!(feature = "media-processing-audio") {
        features.push("media-processing-audio".to_string());
    }
    if cfg!(feature = "media-processing-video") {
        features.push("media-processing-video".to_string());
    }
    if cfg!(feature = "avcodec-profile-software") {
        features.push("avcodec-profile-software".to_string());
    }
    if cfg!(feature = "avcodec-profile-native-free") {
        features.push("avcodec-profile-native-free".to_string());
    }

    let mut operations = Vec::new();
    let mut diagnostics = HashMap::new();
    let mut selection = HashMap::new();

    if cfg!(feature = "media-processing-caption") {
        operations.push("caption_extract".to_string());
        selection.insert(
            "caption_extract".to_string(),
            "cheetah-codec/cea".to_string(),
        );
    } else {
        diagnostics.insert(
            "caption_extract".to_string(),
            "media-processing-caption feature not compiled".to_string(),
        );
    }

    #[cfg(feature = "media-processing-cpu")]
    {
        let runtime = _ctx.runtime_api.clone();
        let cfg = config.clone();
        let probe = run_blocking(runtime.as_ref(), "processing-preflight", move || {
            probe_cpu_capabilities(&cfg)
        })
        .await?;
        for op in probe.available {
            operations.push(op);
        }
        for (op, reason) in probe.diagnostics {
            diagnostics.insert(op, reason);
        }
        for (op, sel) in probe.selection {
            selection.insert(op, sel);
        }
    }
    #[cfg(not(feature = "media-processing-cpu"))]
    {
        for op in [
            "transcode",
            "abr_ladder",
            "audio_mix",
            "video_mosaic",
            "image_process",
            "audio_resample",
        ] {
            diagnostics.insert(
                op.to_string(),
                "media-processing-cpu feature not compiled".to_string(),
            );
        }
    }

    let available = !operations.is_empty();
    let avcodec_revision = if cfg!(feature = "media-processing-cpu") {
        Some(env!("AVCODEC_REVISION").to_string())
    } else {
        None
    };
    Ok(ProcessingPreflightReport {
        profile,
        available,
        operations,
        diagnostics,
        avcodec_revision,
        features,
        selection,
    })
}

#[cfg(feature = "media-processing-cpu")]
struct CpuProbeResult {
    available: Vec<String>,
    diagnostics: HashMap<String, String>,
    selection: HashMap<String, String>,
}

#[cfg(feature = "media-processing-cpu")]
fn probe_cpu_capabilities(config: &MediaProcessingModuleConfig) -> CpuProbeResult {
    let mut available = Vec::new();
    let mut diagnostics = HashMap::new();
    let mut selection = HashMap::new();

    let registry = match build_registry(config) {
        Ok(r) => r,
        Err(e) => {
            for op in [
                "transcode",
                "abr_ladder",
                "audio_mix",
                "video_mosaic",
                "image_process",
                "audio_resample",
            ] {
                diagnostics.insert(op.to_string(), format!("avcodec registry unavailable: {e}"));
            }
            return CpuProbeResult {
                available,
                diagnostics,
                selection,
            };
        }
    };

    let video_decoders = vec![
        (CodecId::H264, BitstreamFormat::H264AnnexB, "h264"),
        (CodecId::H265, BitstreamFormat::H265AnnexB, "h265"),
        (CodecId::Mjpeg, BitstreamFormat::JpegInterchange, "mjpeg"),
    ];
    let video_encoders = vec![
        (CodecId::H264, "h264"),
        (CodecId::H265, "h265"),
        (CodecId::Mjpeg, "mjpeg"),
    ];
    let audio_decoders = vec![
        (CodecId::G711A, BitstreamFormat::G711A, "g711a"),
        (CodecId::G711U, BitstreamFormat::G711U, "g711u"),
        (CodecId::Aac, BitstreamFormat::AacRaw, "aac"),
        (CodecId::Opus, BitstreamFormat::OpusPacket, "opus"),
        (CodecId::Mp3, BitstreamFormat::Mp3Frame, "mp3"),
    ];
    let audio_encoders = vec![
        (CodecId::G711A, "g711a"),
        (CodecId::G711U, "g711u"),
        (CodecId::Aac, "aac"),
        (CodecId::Opus, "opus"),
    ];

    let mut video_decode_selections = Vec::new();
    for (codec, _fmt, name) in &video_decoders {
        let cfg = DecoderConfig::new(*codec, TimeBase::new(1, 30))
            .with_allow_staging(false)
            .with_memory_domain(MemoryDomain::Host)
            .with_packet_input_domain(MemoryDomain::Host)
            .with_output_image_format(ImageInfo::Yuv420p);
        match registry.preflight_decoder(&cfg) {
            Ok(p) => {
                let sel = selection_string(&p);
                video_decode_selections.push(format!("{name}: {sel}"));
            }
            Err(e) => {
                diagnostics.insert(format!("video_decode:{name}"), format!("{e:?}"));
            }
        }
    }
    if !video_decode_selections.is_empty() {
        selection.insert(
            "video_decode".to_string(),
            video_decode_selections.join(", "),
        );
    }

    let mut video_encode_selections = Vec::new();
    for (codec, name) in &video_encoders {
        let cfg = EncoderConfig::new(
            *codec,
            128,
            128,
            ImageInfo::Yuv420p,
            TimeBase::new(1, 30),
            256_000,
        )
        .with_allow_staging(false)
        .with_memory_domain(MemoryDomain::Host)
        .with_packet_output_domain(MemoryDomain::Host);
        match registry.preflight_encoder(&cfg) {
            Ok(p) => {
                let sel = selection_string(&p);
                video_encode_selections.push(format!("{name}: {sel}"));
            }
            Err(e) => {
                diagnostics.insert(format!("video_encode:{name}"), format!("{e:?}"));
            }
        }
    }
    if !video_encode_selections.is_empty() {
        selection.insert(
            "video_encode".to_string(),
            video_encode_selections.join(", "),
        );
    }

    let mut audio_decode_selections = Vec::new();
    for (codec, fmt, name) in &audio_decoders {
        let sample_rate = 48_000u32;
        let cfg = AudioDecoderConfig::new(
            *codec,
            sample_rate,
            2,
            AudioChannelLayout::Stereo,
            *fmt,
            TimeBase::new(1, sample_rate),
        )
        .with_allow_staging(false)
        .with_memory_domain(MemoryDomain::Host);
        match registry.preflight_audio_decoder(&cfg) {
            Ok(p) => {
                let sel = selection_string(&p);
                audio_decode_selections.push(format!("{name}: {sel}"));
            }
            Err(e) => {
                diagnostics.insert(format!("audio_decode:{name}"), format!("{e:?}"));
            }
        }
    }
    if !audio_decode_selections.is_empty() {
        selection.insert(
            "audio_decode".to_string(),
            audio_decode_selections.join(", "),
        );
    }

    let mut audio_encode_selections = Vec::new();
    for (codec, name) in &audio_encoders {
        let sample_rate = 48_000u32;
        let cfg = AudioEncoderConfig::new(
            *codec,
            sample_rate,
            2,
            AudioChannelLayout::Stereo,
            AudioSampleFormat::S16,
            128_000,
            TimeBase::new(1, sample_rate),
        )
        .with_allow_staging(false)
        .with_memory_domain(MemoryDomain::Host);
        match registry.preflight_audio_encoder(&cfg) {
            Ok(p) => {
                let sel = selection_string(&p);
                audio_encode_selections.push(format!("{name}: {sel}"));
            }
            Err(e) => {
                diagnostics.insert(format!("audio_encode:{name}"), format!("{e:?}"));
            }
        }
    }
    if !audio_encode_selections.is_empty() {
        selection.insert(
            "audio_encode".to_string(),
            audio_encode_selections.join(", "),
        );
    }

    let mut image_ops = Vec::new();
    if cfg!(feature = "media-processing-image") {
        for (kind, name) in [
            (ImageOpKind::Csc, "csc"),
            (ImageOpKind::ResizePad, "resize_pad"),
            (ImageOpKind::Blend, "blend"),
        ] {
            let mut cfg = ImageProcessorConfig::new();
            cfg.memory_domain = MemoryDomain::Host;
            cfg.target_op = Some(kind);
            match registry.preflight_image_processor(&cfg) {
                Ok(p) => {
                    let sel = selection_string(&p);
                    image_ops.push(format!("{name}: {sel}"));
                }
                Err(e) => {
                    diagnostics.insert(format!("image_op:{name}"), format!("{e:?}"));
                }
            }
        }

        let jpeg_dec_cfg =
            JpegDecoderConfig::new(ImageInfo::Rgb24).with_memory_domain(MemoryDomain::Host);
        match registry.preflight_jpeg_decoder(&jpeg_dec_cfg) {
            Ok(p) => {
                let sel = selection_string(&p);
                image_ops.push(format!("jpeg_decode: {sel}"));
            }
            Err(e) => {
                diagnostics.insert("image_op:jpeg_decode".to_string(), format!("{e:?}"));
            }
        }

        let jpeg_enc_cfg =
            JpegEncoderConfig::new(80, CodecId::Jpeg).with_memory_domain(MemoryDomain::Host);
        match registry.preflight_jpeg_encoder(&jpeg_enc_cfg) {
            Ok(p) => {
                let sel = selection_string(&p);
                image_ops.push(format!("jpeg_encode: {sel}"));
            }
            Err(e) => {
                diagnostics.insert("image_op:jpeg_encode".to_string(), format!("{e:?}"));
            }
        }
    }
    if !image_ops.is_empty() {
        available.push("image_process".to_string());
        selection.insert("image_process".to_string(), image_ops.join(", "));
    } else if cfg!(feature = "media-processing-image") {
        diagnostics.insert(
            "image_process".to_string(),
            "no image operators or JPEG codec available".to_string(),
        );
    } else {
        diagnostics.insert(
            "image_process".to_string(),
            "media-processing-image feature not compiled".to_string(),
        );
    }

    // transcode: needs at least one video or audio decode + encode path.
    let has_video = !video_decode_selections.is_empty() && !video_encode_selections.is_empty();
    let has_audio = !audio_decode_selections.is_empty() && !audio_encode_selections.is_empty();
    if has_video || has_audio {
        available.push("transcode".to_string());
        let mut parts = Vec::new();
        if has_video {
            parts.push("video".to_string());
        }
        if has_audio {
            parts.push("audio".to_string());
        }
        selection.insert("transcode".to_string(), parts.join(","));
    } else {
        diagnostics.insert(
            "transcode".to_string(),
            "no supported decode/encode codec pair available".to_string(),
        );
    }

    // abr_ladder: needs h264 or h265 video encode.
    let h264_video_encode = registry
        .preflight_encoder(
            &EncoderConfig::new(
                CodecId::H264,
                128,
                128,
                ImageInfo::Yuv420p,
                TimeBase::new(1, 30),
                256_000,
            )
            .with_allow_staging(false)
            .with_memory_domain(MemoryDomain::Host)
            .with_packet_output_domain(MemoryDomain::Host),
        )
        .map(|p| selection_string(&p))
        .ok();
    let h265_video_encode = registry
        .preflight_encoder(
            &EncoderConfig::new(
                CodecId::H265,
                128,
                128,
                ImageInfo::Yuv420p,
                TimeBase::new(1, 30),
                256_000,
            )
            .with_allow_staging(false)
            .with_memory_domain(MemoryDomain::Host)
            .with_packet_output_domain(MemoryDomain::Host),
        )
        .map(|p| selection_string(&p))
        .ok();
    if h264_video_encode.is_some() || h265_video_encode.is_some() {
        available.push("abr_ladder".to_string());
        let mut parts = Vec::new();
        if let Some(ref sel) = h264_video_encode {
            parts.push(format!("h264: {sel}"));
        }
        if let Some(ref sel) = h265_video_encode {
            parts.push(format!("h265: {sel}"));
        }
        selection.insert("abr_ladder".to_string(), parts.join(", "));
    } else {
        diagnostics.insert(
            "abr_ladder".to_string(),
            "no H.264/H.265 encoder available".to_string(),
        );
    }

    // audio_mix: needs at least one audio decode and one audio encode.
    if !audio_decode_selections.is_empty() && !audio_encode_selections.is_empty() {
        available.push("audio_mix".to_string());
        selection.insert(
            "audio_mix".to_string(),
            "requires audio decode + encode".to_string(),
        );
    } else {
        diagnostics.insert(
            "audio_mix".to_string(),
            "audio decode/encode pair unavailable".to_string(),
        );
    }

    // video_mosaic: needs at least one video decode and a video encoder.
    if !video_decode_selections.is_empty() && !video_encode_selections.is_empty() {
        available.push("video_mosaic".to_string());
        let mut parts = Vec::new();
        if !video_decode_selections.is_empty() {
            parts.push("decode ready".to_string());
        }
        if h264_video_encode.is_some() {
            parts.push("h264 encode".to_string());
        }
        if h265_video_encode.is_some() {
            parts.push("h265 encode".to_string());
        }
        selection.insert("video_mosaic".to_string(), parts.join(", "));
    } else {
        diagnostics.insert(
            "video_mosaic".to_string(),
            "video decode/encode unavailable".to_string(),
        );
    }

    // audio_resample/channel_adapt: instantiate a transcoder with mismatched rates/channels.
    let dec_cfg = AudioDecoderConfig::new(
        CodecId::Aac,
        48_000,
        2,
        AudioChannelLayout::Stereo,
        BitstreamFormat::AacRaw,
        TimeBase::new(1, 48_000),
    )
    .with_allow_staging(false)
    .with_memory_domain(MemoryDomain::Host);
    let enc_cfg = AudioEncoderConfig::new(
        CodecId::Opus,
        8_000,
        1,
        AudioChannelLayout::Mono,
        AudioSampleFormat::S16,
        64_000,
        TimeBase::new(1, 8_000),
    )
    .with_allow_staging(false)
    .with_memory_domain(MemoryDomain::Host);
    match AudioTranscoder::new(&registry, &dec_cfg, &enc_cfg) {
        Ok(mut t) => match t.reset() {
            Ok(_) => {
                available.push("audio_resample".to_string());
                selection.insert(
                    "audio_resample".to_string(),
                    "48kHz stereo AAC -> 8kHz mono Opus with reset".to_string(),
                );
            }
            Err(e) => {
                diagnostics.insert(
                    "audio_resample".to_string(),
                    format!("audio resample/channel adapt reset probe failed: {e}"),
                );
            }
        },
        Err(e) => {
            diagnostics.insert(
                "audio_resample".to_string(),
                format!("audio resample/channel adapt probe failed: {e}"),
            );
        }
    }

    CpuProbeResult {
        available,
        diagnostics,
        selection,
    }
}

#[cfg(feature = "media-processing-cpu")]
fn selection_string(preflight: &SelectionPreflight) -> String {
    let trace = preflight.trace();
    if let Some(id) = trace.selected_backend {
        return format!("backend:{id};domain:Host;staging:false");
    }
    if let Some(c) = preflight.candidates().first() {
        return format!("backend:{};domain:Host;staging:false", c.backend_id());
    }
    "none".to_string()
}
