//! Image encoding and processing API and request/result types.
//!
//! 图片编码与处理 API 及请求/结果类型。

use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_codec::{AVFrame, TrackInfo};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::port::MediaRequestContext;

pub use crate::processing::{ImageInput, ImageOperation, ImageProcessRequest};

/// Supported output image formats.
///
/// 支持的输出图片格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    Jpeg,
    Png,
}

impl ImageFormat {
    /// Return the MIME content type for the format.
    ///
    /// 返回该格式对应的 MIME content type。
    pub fn content_type(&self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
        }
    }

    /// Check whether `data` starts with the expected magic bytes.
    ///
    /// 检查 `data` 是否以该格式期望的 magic 开头。
    pub fn matches_magic(&self, data: &[u8]) -> bool {
        match self {
            ImageFormat::Jpeg => data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8,
            ImageFormat::Png => data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
        }
    }

    /// Parse width and height from the image header.
    ///
    /// 从图片头部解析宽高。
    pub fn parse_dimensions(&self, data: &[u8]) -> Option<(u32, u32)> {
        use cheetah_codec::image::ImageDimensions;
        match self {
            ImageFormat::Jpeg => cheetah_codec::image::jpeg::parse_dimensions(data)
                .map(|ImageDimensions { width, height }| (width, height)),
            ImageFormat::Png => cheetah_codec::image::png::parse_dimensions(data)
                .map(|ImageDimensions { width, height }| (width, height)),
        }
    }
}

impl FromStr for ImageFormat {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Ok(ImageFormat::Jpeg),
            "png" => Ok(ImageFormat::Png),
            _ => Err(format!("unknown image format: {s}")),
        }
    }
}

/// Request to encode a video keyframe into a still image.
///
/// 将视频关键帧编码为静态图片的请求。
#[derive(Debug, Clone)]
pub struct ImageEncodeRequest {
    pub frame: Arc<AVFrame>,
    pub track_info: TrackInfo,
    pub format: ImageFormat,
    /// JPEG quality (1–100). Ignored for PNG.
    ///
    /// JPEG 质量（1–100）。PNG 忽略。
    pub quality: u8,
    /// Optional maximum width; the encoded image is scaled down to fit while
    /// preserving aspect ratio.
    ///
    /// 可选最大宽度；编码图片将按比例缩放以适应。
    pub max_width: Option<u32>,
    /// Optional maximum height; the encoded image is scaled down to fit while
    /// preserving aspect ratio.
    ///
    /// 可选最大高度；编码图片将按比例缩放以适应。
    pub max_height: Option<u32>,
}

/// Result of an image encode operation.
///
/// 图片编码操作的结果。
#[derive(Debug, Clone)]
pub struct ImageArtifact {
    pub payload: Bytes,
    pub content_type: String,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
}

/// Runtime-neutral image encoding backend.
///
/// 运行时无关的图片编码后端。
#[async_trait]
pub trait ImageEncodeApi: Send + Sync {
    /// Encode the supplied frame into the requested image format.
    ///
    /// 将提供的帧编码为请求的图片格式。
    async fn encode(
        &self,
        ctx: &MediaRequestContext,
        request: ImageEncodeRequest,
    ) -> Result<ImageArtifact>;
}

/// Runtime-neutral image processing backend.
///
/// 运行时无关的图片处理后端。
#[async_trait]
pub trait ImageProcessApi: Send + Sync {
    /// Process the supplied image through the requested operation pipeline.
    ///
    /// 通过请求的算子管道处理所提供的图片。
    async fn process(
        &self,
        ctx: &MediaRequestContext,
        request: ImageProcessRequest,
    ) -> Result<ImageArtifact>;
}
