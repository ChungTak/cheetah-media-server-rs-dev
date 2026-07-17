//! `ImageProcessApi` provider backed by `avcodec-rs`.
//!
//! Only compiled when `media-processing-image` is enabled.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_media_api::{
    error::Result, ImageArtifact, ImageFormat, ImageInput, ImageOperation, ImageProcessApi,
    ImageProcessRequest, MediaError, MediaRequestContext,
};
use cheetah_runtime_api::RuntimeApi;
use futures::channel::oneshot;
use tracing::instrument;

use crate::config::MediaProcessingModuleConfig;

/// Image processing provider using an avcodec `Registry`.
pub struct ImageProcessProvider {
    runtime: Arc<dyn RuntimeApi>,
    config: MediaProcessingModuleConfig,
}

impl ImageProcessProvider {
    pub fn new(runtime: Arc<dyn RuntimeApi>, config: MediaProcessingModuleConfig) -> Self {
        Self { runtime, config }
    }
}

#[async_trait]
impl ImageProcessApi for ImageProcessProvider {
    #[instrument(skip(self, _ctx, request), fields(request_id = ?_ctx.request_id))]
    async fn process(
        &self,
        _ctx: &MediaRequestContext,
        request: ImageProcessRequest,
    ) -> Result<ImageArtifact> {
        let runtime = Arc::clone(&self.runtime);
        let config = self.config.clone();

        let (tx, rx) = oneshot::channel::<Result<ImageArtifact>>();
        runtime
            .spawn_blocking(
                "image-process",
                Box::new(move || {
                    let result = process_blocking(request, &config);
                    let _ = tx.send(result);
                }),
            )
            .map_err(|e| MediaError::internal(format!("spawn blocking failed: {e}")))?;

        rx.await
            .map_err(|_| MediaError::internal("image process task canceled"))?
    }
}

fn process_blocking(
    request: ImageProcessRequest,
    config: &MediaProcessingModuleConfig,
) -> Result<ImageArtifact> {
    use avcodec::core::{
        ImageProcessRequest as AvImageProcessRequest, ImageProcessor, ImageProcessorConfig, Poll,
    };
    use avcodec::native_free_software_registry_builder;

    let registry = native_free_software_registry_builder().build();

    let mut image = match request.input {
        ImageInput::Encoded { data, format } => decode_encoded_image(&registry, &data, format)?,
        ImageInput::Frame { .. } => {
            return Err(MediaError::unsupported(
                "video frame input is not yet supported",
            ));
        }
    };

    // Validate decoded dimensions against configured hard limits.
    if image.coded_width > config.max_image_width || image.coded_height > config.max_image_height {
        return Err(MediaError::invalid_argument(format!(
            "decoded image {}x{} exceeds configured limit {}x{}",
            image.coded_width, image.coded_height, config.max_image_width, config.max_image_height
        )));
    }

    for (index, op) in request.operations.iter().enumerate() {
        let av_op = map_image_operation(op)
            .map_err(|e| MediaError::unsupported(format!("operation #{index}: {e}")))?;
        let cfg = ImageProcessorConfig::new().with_target_op(discriminant_of(&av_op));
        let mut processor: Box<dyn ImageProcessor> = registry
            .create_image_processor(&cfg)
            .map_err(|e| MediaError::internal(format!("create image processor: {e}")))?;
        processor
            .submit(AvImageProcessRequest::new(image, av_op))
            .map_err(|e| MediaError::internal(format!("submit image operation: {e}")))?;
        image = match processor
            .poll_image()
            .map_err(|e| MediaError::internal(format!("poll image: {e}")))?
        {
            Poll::Ready(img) => img,
            Poll::Pending => {
                return Err(MediaError::internal("image processor returned pending"));
            }
            Poll::EndOfStream => {
                return Err(MediaError::internal("image processor ended without output"));
            }
        };
    }

    // Validate output dimensions.
    if image.coded_width > config.max_image_width || image.coded_height > config.max_image_height {
        return Err(MediaError::invalid_argument(format!(
            "output image {}x{} exceeds configured limit {}x{}",
            image.coded_width, image.coded_height, config.max_image_width, config.max_image_height
        )));
    }

    let quality = if request.quality == 0 || request.quality > 100 {
        config.default_jpeg_quality
    } else {
        request.quality
    };

    match request.output_format {
        ImageFormat::Jpeg => encode_jpeg(&registry, image, quality),
        ImageFormat::Png => Err(MediaError::unsupported("PNG output is not supported")),
    }
}

fn decode_encoded_image(
    registry: &avcodec::core::Registry,
    data: &Bytes,
    format: ImageFormat,
) -> Result<avcodec::core::Image> {
    use avcodec::core::{
        BufferHandle, BufferSlice, EncodedImage, EncodedImageFormat, ImageDecoder,
        ImageDecoderConfig, ImageInfo, Poll,
    };

    let encoded_format = match format {
        ImageFormat::Jpeg => EncodedImageFormat::Jpeg,
        ImageFormat::Png => EncodedImageFormat::Png,
    };

    let cfg = ImageDecoderConfig::new(ImageInfo::Rgb24).with_format(encoded_format);
    let mut decoder: Box<dyn ImageDecoder> = registry
        .create_image_decoder(&cfg)
        .map_err(|e| MediaError::internal(format!("create image decoder: {e}")))?;

    let handle = BufferHandle::from_host_bytes(0, data.to_vec());
    let slice = BufferSlice::new(handle, 0, data.len());
    let encoded = EncodedImage::new(slice, EncodedImageFormat::Auto);
    decoder
        .submit(encoded)
        .map_err(|e| MediaError::internal(format!("submit encoded image: {e}")))?;

    match decoder
        .poll_image()
        .map_err(|e| MediaError::internal(format!("poll decoded image: {e}")))?
    {
        Poll::Ready(img) => Ok(img),
        Poll::Pending | Poll::EndOfStream => {
            Err(MediaError::internal("image decoder did not produce output"))
        }
    }
}

fn encode_jpeg(
    registry: &avcodec::core::Registry,
    image: avcodec::core::Image,
    quality: u8,
) -> Result<ImageArtifact> {
    use avcodec::core::{CodecId, JpegEncoder, JpegEncoderConfig, Poll};

    let coded_width = image.coded_width;
    let coded_height = image.coded_height;

    let mut encoder: Box<dyn JpegEncoder> = registry
        .create_jpeg_encoder(&JpegEncoderConfig::new(quality, CodecId::Jpeg))
        .map_err(|e| MediaError::internal(format!("create jpeg encoder: {e}")))?;
    encoder
        .submit_frame(image)
        .map_err(|e| MediaError::internal(format!("submit frame to jpeg encoder: {e}")))?;
    let packet = match encoder
        .poll_packet()
        .map_err(|e| MediaError::internal(format!("poll jpeg packet: {e}")))?
    {
        Poll::Ready(p) => p,
        Poll::Pending | Poll::EndOfStream => {
            return Err(MediaError::internal("jpeg encoder did not produce output"))
        }
    };

    let bytes = packet
        .data
        .host_bytes()
        .map_err(|e| MediaError::internal(format!("read jpeg packet: {e}")))?
        .ok_or_else(|| MediaError::internal("jpeg packet is not host-backed"))?
        .to_vec();

    let (width, height) = jpeg_dimensions(&bytes).unwrap_or((coded_width, coded_height));

    Ok(ImageArtifact {
        payload: Bytes::from(bytes),
        content_type: "image/jpeg".to_string(),
        format: ImageFormat::Jpeg,
        width,
        height,
    })
}

fn map_image_operation(op: &ImageOperation) -> std::result::Result<avcodec::core::ImageOp, String> {
    use avcodec::core::{ImageOp, PadColor, Rect, Rotation, ScaleFilter};

    match op {
        ImageOperation::Crop {
            x,
            y,
            width,
            height,
        } => Ok(ImageOp::Crop(Rect::new(*x, *y, *width, *height))),
        ImageOperation::Resize { width, height } => Ok(ImageOp::Resize {
            width: *width,
            height: *height,
        }),
        ImageOperation::Fit { width, height } => Ok(ImageOp::ResizePad {
            dst_width: *width,
            dst_height: *height,
            fit: avcodec::core::FitMode::Contain,
            align: avcodec::core::PadAlign::Center,
            fill: PadColor::BLACK,
            filter: ScaleFilter::Bilinear,
        }),
        ImageOperation::Rotate { degrees } => {
            let rotation = match *degrees {
                90 => Rotation::R90,
                180 => Rotation::R180,
                270 => Rotation::R270,
                _ => return Err(format!("unsupported rotation angle: {degrees}")),
            };
            Ok(ImageOp::Rotate(rotation))
        }
        ImageOperation::Flip {
            horizontal,
            vertical,
        } => match (horizontal, vertical) {
            (true, false) => Ok(ImageOp::Flip(avcodec::core::FlipAxis::Horizontal)),
            (false, true) => Ok(ImageOp::Flip(avcodec::core::FlipAxis::Vertical)),
            (true, true) => {
                Err("simultaneous horizontal and vertical flip is not supported".to_string())
            }
            (false, false) => Ok(ImageOp::Copy),
        },
        ImageOperation::Pad {
            top,
            bottom,
            left,
            right,
            color,
        } => {
            let pad_color = color
                .as_ref()
                .and_then(|s| parse_pad_color(s))
                .unwrap_or(PadColor::BLACK);
            Ok(ImageOp::Pad {
                left: *left,
                right: *right,
                top: *top,
                bottom: *bottom,
                color: pad_color,
            })
        }
        ImageOperation::Csc { format } => {
            let info = parse_image_info(format)?;
            Ok(ImageOp::Csc { dst_format: info })
        }
        ImageOperation::ResizePad {
            width,
            height,
            color,
        } => {
            let pad_color = color
                .as_ref()
                .and_then(|s| parse_pad_color(s))
                .unwrap_or(PadColor::BLACK);
            Ok(ImageOp::ResizePad {
                dst_width: *width,
                dst_height: *height,
                fit: avcodec::core::FitMode::Contain,
                align: avcodec::core::PadAlign::Center,
                fill: pad_color,
                filter: ScaleFilter::Bilinear,
            })
        }
        ImageOperation::Blend { .. } => Err("blend is not yet supported".to_string()),
        ImageOperation::Text { .. } => Err("text overlay is not yet supported".to_string()),
    }
}

fn discriminant_of(op: &avcodec::core::ImageOp) -> avcodec::core::ImageOpKind {
    use avcodec::core::{ImageOp, ImageOpKind};

    match op {
        ImageOp::Copy => ImageOpKind::Copy,
        ImageOp::Crop { .. } => ImageOpKind::Crop,
        ImageOp::Resize { .. } => ImageOpKind::Resize,
        ImageOp::CropResize { .. } => ImageOpKind::CropResize,
        ImageOp::Csc { .. } => ImageOpKind::Csc,
        ImageOp::Rotate { .. } => ImageOpKind::Rotate,
        ImageOp::Flip { .. } => ImageOpKind::Flip,
        ImageOp::Pad { .. } => ImageOpKind::Pad,
        ImageOp::Blend { .. } => ImageOpKind::Blend,
        ImageOp::Osd { .. } | ImageOp::OsdPolygon { .. } | ImageOp::OsdText { .. } => {
            ImageOpKind::Osd
        }
        ImageOp::ResizePad { .. } => ImageOpKind::ResizePad,
        ImageOp::Normalize { .. } => ImageOpKind::Normalize,
    }
}

fn parse_image_info(s: &str) -> std::result::Result<avcodec::core::ImageInfo, String> {
    match s.to_ascii_lowercase().as_str() {
        "yuv420p" => Ok(avcodec::core::ImageInfo::Yuv420p),
        "yuv422p" => Ok(avcodec::core::ImageInfo::Yuv422p),
        "yuv444p" => Ok(avcodec::core::ImageInfo::Yuv444p),
        "nv12" => Ok(avcodec::core::ImageInfo::Nv12),
        "nv21" => Ok(avcodec::core::ImageInfo::Nv21),
        "rgb24" | "rgb" => Ok(avcodec::core::ImageInfo::Rgb24),
        "rgba" => Ok(avcodec::core::ImageInfo::Rgba),
        "gray8" | "gray" => Ok(avcodec::core::ImageInfo::Gray8),
        "bgr24" | "bgr" => Ok(avcodec::core::ImageInfo::Bgr24),
        "bgra" => Ok(avcodec::core::ImageInfo::Bgra),
        _ => Err(format!("unknown pixel format: {s}")),
    }
}

fn parse_pad_color(s: &str) -> Option<avcodec::core::PadColor> {
    let s = s.trim();
    if s.starts_with('#') && s.len() == 7 && s.is_ascii() {
        let r = u8::from_str_radix(&s[1..3], 16).ok()?;
        let g = u8::from_str_radix(&s[3..5], 16).ok()?;
        let b = u8::from_str_radix(&s[5..7], 16).ok()?;
        Some(avcodec::core::PadColor([r, g, b, 255]))
    } else if s.eq_ignore_ascii_case("black") {
        Some(avcodec::core::PadColor::BLACK)
    } else if s.eq_ignore_ascii_case("white") {
        Some(avcodec::core::PadColor([255, 255, 255, 255]))
    } else if s.eq_ignore_ascii_case("gray") || s.eq_ignore_ascii_case("grey") {
        Some(avcodec::core::PadColor::gray(128))
    } else {
        None
    }
}

/// Parses width/height from a JPEG SOF0/SOF2 segment.
fn jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2; // skip SOI
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        if marker == 0xD9 {
            break; // EOI
        }
        if marker == 0xD8 || marker == 0x00 || marker == 0xFF {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if i + 2 + len > data.len() {
            break;
        }
        // SOF0..SOF15 except DHT/DAC/APP markers that share range.
        if (0xC0..=0xCF).contains(&marker)
            && marker != 0xC4
            && marker != 0xC8
            && marker != 0xCC
            && len >= 7
        {
            let height = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
            let width = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
            return Some((width, height));
        }
        i += 2 + len;
    }
    None
}

#[cfg(all(test, feature = "media-processing-image"))]
mod tests {
    use super::*;
    use cheetah_media_api::{ImageFormat, ImageInput, ImageProcessRequest, MediaErrorCode};
    use cheetah_runtime_api::RuntimeApi;
    use cheetah_runtime_tokio::TokioRuntime;

    fn make_jpeg_fixture() -> Bytes {
        use avcodec::core::{CodecId, Image, ImageInfo, JpegEncoder, JpegEncoderConfig, Poll};
        use avcodec::native_free_software_registry_builder;

        let registry = native_free_software_registry_builder().build();
        let mut encoder: Box<dyn JpegEncoder> = registry
            .create_jpeg_encoder(&JpegEncoderConfig::new(80, CodecId::Jpeg))
            .expect("jpeg encoder");
        let rgb = vec![128u8; 4 * 4 * 3];
        let frame =
            Image::new_host_packed(ImageInfo::Rgb24, 4, 4, 0, 4 * 3, rgb, 0).expect("host image");
        encoder.submit_frame(frame).expect("submit");
        let packet = match encoder.poll_packet().expect("poll") {
            Poll::Ready(p) => p,
            _ => panic!("expected jpeg packet"),
        };
        Bytes::copy_from_slice(
            packet
                .data
                .host_bytes()
                .expect("host bytes")
                .expect("not empty"),
        )
    }

    #[tokio::test]
    async fn decodes_and_resizes_jpeg() {
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let provider = ImageProcessProvider::new(runtime, MediaProcessingModuleConfig::default());
        let request = ImageProcessRequest::new(
            ImageInput::Encoded {
                data: make_jpeg_fixture(),
                format: ImageFormat::Jpeg,
            },
            ImageFormat::Jpeg,
        )
        .with_operations(vec![ImageOperation::Resize {
            width: 2,
            height: 2,
        }]);

        let artifact = provider
            .process(&MediaRequestContext::default(), request)
            .await
            .expect("process should succeed");

        assert_eq!(artifact.format, ImageFormat::Jpeg);
        assert_eq!(artifact.content_type, "image/jpeg");
        assert_eq!(artifact.width, 2);
        assert_eq!(artifact.height, 2);
        assert!(!artifact.payload.is_empty());
    }

    #[tokio::test]
    async fn rejects_png_output() {
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let provider = ImageProcessProvider::new(runtime, MediaProcessingModuleConfig::default());
        let request = ImageProcessRequest::new(
            ImageInput::Encoded {
                data: make_jpeg_fixture(),
                format: ImageFormat::Jpeg,
            },
            ImageFormat::Png,
        );

        let err = provider
            .process(&MediaRequestContext::default(), request)
            .await
            .expect_err("png output should be unsupported");
        assert_eq!(err.code, MediaErrorCode::Unsupported);
    }

    #[test]
    fn jpeg_dimensions_detects_fixture() {
        let jpeg = make_jpeg_fixture();
        assert_eq!(jpeg_dimensions(&jpeg), Some((4, 4)));
    }

    #[test]
    fn parse_pad_color_understands_hex_and_names() {
        let c = parse_pad_color("#ff00aa").unwrap();
        assert_eq!(c.0, [255, 0, 170, 255]);
        assert_eq!(parse_pad_color("black").unwrap().0, [0, 0, 0, 255]);
        assert_eq!(parse_pad_color("white").unwrap().0, [255, 255, 255, 255]);
        assert!(parse_pad_color("not-a-color").is_none());
    }
}
