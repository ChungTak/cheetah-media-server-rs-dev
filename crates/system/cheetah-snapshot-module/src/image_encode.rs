//! Image encoding backend for the snapshot module.
//!
//! 快照模块的图片编码后端。

use std::fmt;
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_codec::{CodecExtradata, CodecId, MediaKind};
use cheetah_media_api::error::{MediaError, MediaErrorCode, Result};
use cheetah_media_api::image::{ImageArtifact, ImageEncodeApi, ImageEncodeRequest, ImageFormat};
use cheetah_media_api::port::MediaRequestContext;
use cheetah_sdk::{FfmpegApi, FfmpegInput, FfmpegJobSpec, FfmpegOutput, FfmpegResourceLimits};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat as ImageCrateFormat};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempFileGuard(Vec<PathBuf>);

impl TempFileGuard {
    fn new(paths: Vec<PathBuf>) -> Self {
        Self(paths)
    }
    fn disarm(mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.0)
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = fs::remove_file(p);
        }
    }
}

/// A real image encode backend that decodes MJPEG payloads, or transcodes H.264
/// keyframes via FFmpeg, and re-encodes them as JPEG or PNG with optional
/// down-scaling.
///
/// 真实的图片编码后端。支持解码 MJPEG 负载，或通过 FFmpeg 将 H.264
/// 关键帧转码为 JPEG/PNG，并支持可选缩放。
#[derive(Clone, Default)]
pub struct ImageEncoderBackend {
    ffmpeg_api: Option<Arc<dyn FfmpegApi>>,
}

impl fmt::Debug for ImageEncoderBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageEncoderBackend")
            .field("ffmpeg_api", &self.ffmpeg_api.is_some())
            .finish()
    }
}

impl ImageEncoderBackend {
    /// Create a new image encoder backend without an FFmpeg executor.
    pub fn new() -> Self {
        Self { ffmpeg_api: None }
    }

    /// Use the provided FFmpeg executor to transcode H.264 keyframes.
    pub fn with_ffmpeg_api(mut self, ffmpeg_api: Arc<dyn FfmpegApi>) -> Self {
        self.ffmpeg_api = Some(ffmpeg_api);
        self
    }

    fn temp_prefix() -> String {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("cheetah-snapshot-{pid}-{now}-{counter}")
    }
}

#[async_trait]
impl ImageEncodeApi for ImageEncoderBackend {
    async fn encode(
        &self,
        _ctx: &MediaRequestContext,
        request: ImageEncodeRequest,
    ) -> Result<ImageArtifact> {
        let frame = Arc::clone(&request.frame);
        if frame.media_kind != MediaKind::Video {
            return Err(MediaError::unsupported(
                "image encode requires a video frame",
            ));
        }

        match frame.codec {
            CodecId::MJPEG => self.encode_mjpeg(request).await,
            CodecId::H264 => self.encode_h264(request).await,
            _ => Err(MediaError::unsupported(format!(
                "image encode does not support codec {:?}",
                frame.codec
            ))),
        }
    }
}

impl ImageEncoderBackend {
    async fn encode_mjpeg(&self, request: ImageEncodeRequest) -> Result<ImageArtifact> {
        let img = image::load_from_memory(&request.frame.payload).map_err(|e| {
            MediaError::invalid_argument(format!("failed to decode mjpeg payload: {e}"))
        })?;
        encode_decoded_image(
            img,
            request.format,
            request.quality,
            request.max_width,
            request.max_height,
        )
    }

    async fn encode_h264(&self, request: ImageEncodeRequest) -> Result<ImageArtifact> {
        let Some(ffmpeg_api) = self.ffmpeg_api.as_ref() else {
            return Err(MediaError::unavailable(
                "H264 snapshot encoding requires an FFmpeg executor",
            ));
        };

        let stream = build_h264_annex_b(&request)?;
        let temp_dir = std::env::temp_dir();
        let prefix = Self::temp_prefix();
        let input_path = temp_dir.join(format!("{prefix}.h264"));
        let output_path = temp_dir.join(format!("{prefix}.jpg"));

        let guard = TempFileGuard::new(vec![input_path.clone(), output_path.clone()]);

        fs::write(&input_path, &stream).map_err(|e| {
            MediaError::storage_failed(format!("write H264 snapshot temp file: {e}"))
        })?;

        let job_id = prefix;
        let spec = FfmpegJobSpec {
            profile_id: "default".to_string(),
            input: FfmpegInput::Url {
                url: input_path.to_string_lossy().into_owned(),
            },
            output: FfmpegOutput::Url {
                url: output_path.to_string_lossy().into_owned(),
            },
            input_options: vec![
                "-fflags".to_string(),
                "+genpts".to_string(),
                "-framerate".to_string(),
                "1".to_string(),
            ],
            output_options: vec![
                "-frames:v".to_string(),
                "1".to_string(),
                "-f".to_string(),
                "image2".to_string(),
            ],
            resource_limits: FfmpegResourceLimits {
                max_runtime_ms: 10_000,
                max_stderr_lines: 64,
            },
        };

        ffmpeg_api
            .submit(job_id.clone(), spec)
            .await
            .map_err(|e| MediaError::unavailable(format!("submit H264 snapshot job: {e}")))?;

        let status = ffmpeg_api
            .wait(&job_id)
            .await
            .map_err(|e| MediaError::unavailable(format!("wait H264 snapshot job: {e}")))?;
        let _ = ffmpeg_api.remove(&job_id).await;

        if status.state != cheetah_sdk::FfmpegJobState::Exited || status.exit_code != Some(0) {
            return Err(MediaError::new(
                MediaErrorCode::Unavailable,
                format!("H264 snapshot decode failed: {}", status.exit_summary),
            ));
        }

        let guard_paths = guard.disarm();
        let read_result = fs::read(&output_path)
            .map_err(|e| MediaError::storage_failed(format!("read H264 snapshot output: {e}")));

        for p in &guard_paths {
            let _ = fs::remove_file(p);
        }

        let bytes = read_result?;

        if bytes.is_empty() {
            return Err(MediaError::storage_failed(
                "H264 snapshot produced empty JPEG output".to_string(),
            ));
        }

        let img = image::load_from_memory(&bytes).map_err(|e| {
            MediaError::storage_failed(format!("H264 snapshot output is not a valid JPEG: {e}"))
        })?;

        encode_decoded_image(
            img,
            request.format,
            request.quality,
            request.max_width,
            request.max_height,
        )
    }
}

fn build_h264_annex_b(request: &ImageEncodeRequest) -> Result<Bytes> {
    let CodecExtradata::H264 { sps, pps, .. } = &request.track_info.extradata else {
        return Err(MediaError::invalid_argument(
            "H264 snapshot requires SPS/PPS in track extradata".to_string(),
        ));
    };

    if sps.is_empty() || pps.is_empty() {
        return Err(MediaError::invalid_argument(
            "H264 snapshot requires non-empty SPS/PPS".to_string(),
        ));
    }

    let mut buf = Vec::new();
    for nal in sps.iter().chain(pps.iter()) {
        buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        buf.extend_from_slice(nal);
    }
    buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    buf.extend_from_slice(&request.frame.payload);
    Ok(Bytes::from(buf))
}

fn encode_decoded_image(
    img: DynamicImage,
    format: ImageFormat,
    quality: u8,
    max_width: Option<u32>,
    max_height: Option<u32>,
) -> Result<ImageArtifact> {
    let img = match (max_width, max_height) {
        (None, None) => img,
        _ => {
            let max_w = max_width.unwrap_or(u32::MAX);
            let max_h = max_height.unwrap_or(u32::MAX);
            img.thumbnail(max_w, max_h)
        }
    };

    let width = img.width();
    let height = img.height();
    let mut buf = Cursor::new(Vec::new());

    match format {
        ImageFormat::Jpeg => {
            let quality = quality.clamp(1, 100);
            let mut encoder = JpegEncoder::new_with_quality(&mut buf, quality);
            encoder
                .encode_image(&img)
                .map_err(|e| MediaError::storage_failed(format!("jpeg encode failed: {e}")))?;
        }
        ImageFormat::Png => {
            img.write_to(&mut buf, ImageCrateFormat::Png)
                .map_err(|e| MediaError::storage_failed(format!("png encode failed: {e}")))?;
        }
    }

    let payload = Bytes::from(buf.into_inner());
    Ok(ImageArtifact {
        payload,
        content_type: format.content_type().to_string(),
        format,
        width,
        height,
    })
}
