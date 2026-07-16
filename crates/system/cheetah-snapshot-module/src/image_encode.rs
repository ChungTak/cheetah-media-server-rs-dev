//! Image encoding backend for the snapshot module.
//!
//! 快照模块的图片编码后端。

use std::io::Cursor;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_codec::{CodecId, MediaKind};
use cheetah_media_api::error::{MediaError, Result};
use cheetah_media_api::image::{ImageArtifact, ImageEncodeApi, ImageEncodeRequest, ImageFormat};
use cheetah_media_api::port::MediaRequestContext;
use image::codecs::jpeg::JpegEncoder;
use image::ImageFormat as ImageCrateFormat;

/// A real image encode backend that decodes MJPEG payloads and re-encodes them
/// as JPEG or PNG with optional down-scaling.
///
/// 真实的图片编码后端，将 MJPEG 负载解码后重新编码为 JPEG 或 PNG，
/// 并支持可选的缩放。
#[derive(Debug, Clone, Default)]
pub struct ImageEncoderBackend;

impl ImageEncoderBackend {
    /// Create a new image encoder backend.
    pub fn new() -> Self {
        Self
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
        if frame.codec != CodecId::MJPEG {
            return Err(MediaError::unsupported(format!(
                "image encode does not support codec {:?}",
                frame.codec
            )));
        }

        let img = image::load_from_memory(&frame.payload).map_err(|e| {
            MediaError::invalid_argument(format!("failed to decode mjpeg payload: {e}"))
        })?;

        let img = match (request.max_width, request.max_height) {
            (None, None) => img,
            _ => {
                let max_w = request.max_width.unwrap_or(u32::MAX);
                let max_h = request.max_height.unwrap_or(u32::MAX);
                img.thumbnail(max_w, max_h)
            }
        };

        let width = img.width();
        let height = img.height();
        let mut buf = Cursor::new(Vec::new());

        match request.format {
            ImageFormat::Jpeg => {
                let quality = request.quality.clamp(1, 100);
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
            content_type: request.format.content_type().to_string(),
            format: request.format,
            width,
            height,
        })
    }
}
