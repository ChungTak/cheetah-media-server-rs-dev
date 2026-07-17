//! `ImageProcessApi` provider backed by `avcodec-rs`.
//!
//! Only compiled when `media-processing-image` is enabled.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_codec::{video_payload_is_random_access, ParameterSetCache};
use cheetah_codec::{CodecId as CheetahCodecId, MediaKind, TrackInfo};
use cheetah_media_api::{
    error::Result, ImageArtifact, ImageFormat, ImageInput, ImageOperation, ImageProcessApi,
    ImageProcessRequest, MediaError, MediaRequestContext,
};
use cheetah_runtime_api::RuntimeApi;
use futures::channel::oneshot;
use tracing::instrument;

use crate::config::MediaProcessingModuleConfig;
use crate::provider::semaphore::Semaphore;

/// Image processing provider using an avcodec `Registry`.
pub struct ImageProcessProvider {
    runtime: Arc<dyn RuntimeApi>,
    config: MediaProcessingModuleConfig,
    semaphore: Semaphore,
}

impl ImageProcessProvider {
    pub fn new(runtime: Arc<dyn RuntimeApi>, config: MediaProcessingModuleConfig) -> Self {
        let max_jobs = config.max_concurrent_jobs as usize;
        Self {
            runtime,
            config,
            semaphore: Semaphore::new(max_jobs),
        }
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
        // Acquire a concurrency permit before scheduling blocking work.
        let permit = self.semaphore.acquire().await;

        let runtime = Arc::clone(&self.runtime);
        let config = self.config.clone();

        let (tx, rx) = oneshot::channel::<Result<ImageArtifact>>();
        runtime
            .spawn_blocking(
                "image-process",
                Box::new(move || {
                    // Hold the permit for the lifetime of the blocking task.
                    let _permit = permit;
                    let result = process_blocking(request, &config);
                    let _ = tx.send(result);
                }),
            )
            .map_err(|e| MediaError::internal(format!("spawn blocking failed: {e}")))?;

        rx.await
            .map_err(|_| MediaError::internal("image process task canceled"))?
    }
}

fn build_registry(config: &MediaProcessingModuleConfig) -> Result<avcodec::core::Registry> {
    match config.profile.as_str() {
        "native-free" => Ok(filter_native_free_registry()),
        "software" if cfg!(feature = "avcodec-profile-software") => {
            Ok(avcodec::default_registry_builder().build())
        }
        "software" => Err(MediaError::unsupported(
            "software profile requires the avcodec-profile-software feature",
        )),
        _ => Err(MediaError::invalid_argument(format!(
            "unsupported avcodec profile: {}",
            config.profile
        ))),
    }
}

/// Builds a `Registry` from the default avcodec backend set but restricted to
/// the audited native-free software backend ids plus `libyuv` for CSC/resize.
fn filter_native_free_registry() -> avcodec::core::Registry {
    const ALLOWED: &[&str] = &["jpeg", "zune", "rust-h264", "rust-h265", "libyuv"];

    let all = avcodec::default_registry_builder();
    let mut filtered = avcodec::core::RegistryBuilder::new();
    for backend in all.backends() {
        if ALLOWED.contains(&backend.id()) {
            filtered = filtered.with_backend(*backend);
        }
    }
    filtered.build()
}

fn process_blocking(
    request: ImageProcessRequest,
    config: &MediaProcessingModuleConfig,
) -> Result<ImageArtifact> {
    use avcodec::core::{
        ImageProcessRequest as AvImageProcessRequest, ImageProcessor, ImageProcessorConfig, Poll,
    };

    let registry = build_registry(config)?;

    // Pre-validate declared encoded dimensions so the decoder does not have to
    // allocate a huge pixel buffer for an oversized input.
    if let ImageInput::Encoded { data, format } = &request.input {
        if let Some((w, h)) = encoded_dimensions(data, *format) {
            if w > config.max_image_width || h > config.max_image_height {
                return Err(MediaError::invalid_argument(format!(
                    "encoded image {w}x{h} exceeds configured limit {}x{}",
                    config.max_image_width, config.max_image_height
                )));
            }
        }
    }

    let mut image = match request.input {
        ImageInput::Encoded { data, format } => decode_encoded_image(&registry, &data, format)?,
        ImageInput::Frame { frame, track } => {
            decode_video_frame(&registry, &frame, &track, config)?
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
        validate_operation_dimensions(op, &image, config, index)?;
        let av_op = map_image_operation(op, &image)
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

fn decode_video_frame(
    registry: &avcodec::core::Registry,
    frame: &cheetah_codec::AVFrame,
    track: &TrackInfo,
    config: &MediaProcessingModuleConfig,
) -> Result<avcodec::core::Image> {
    use avcodec::core::{
        BitstreamFormat, Decoder, DecoderConfig, Packet, PacketFlags, Poll, TimeBase,
    };

    if frame.media_kind != MediaKind::Video {
        return Err(MediaError::invalid_argument(
            "image process frame input must be a video frame",
        ));
    }

    // MJPEG payloads are simply independent JPEG images.
    if frame.codec == CheetahCodecId::MJPEG {
        return decode_encoded_image(registry, &frame.payload, ImageFormat::Jpeg);
    }

    let av_codec = avcodec_codec_id(frame.codec).ok_or_else(|| {
        MediaError::unsupported(format!(
            "video frame codec {:?} is not supported",
            frame.codec
        ))
    })?;

    if !video_payload_is_random_access(frame.codec, frame.format, &frame.payload) {
        return Err(MediaError::invalid_argument(
            "video frame input must be a random-access (key) frame",
        ));
    }

    // Validate declared track dimensions before decoding if available.
    if let (Some(w), Some(h)) = (track.width, track.height) {
        if w > config.max_image_width || h > config.max_image_height {
            return Err(MediaError::invalid_argument(format!(
                "video frame {}x{} exceeds configured limit {}x{}",
                w, h, config.max_image_width, config.max_image_height
            )));
        }
    }

    let time_base = track.media_timebase().map_err(|e| {
        MediaError::invalid_argument(format!("invalid track timebase for image decode: {e}"))
    })?;
    let av_time_base = TimeBase::new(time_base.num, time_base.den);

    let decoder_cfg = DecoderConfig::new(av_codec, av_time_base)
        .with_memory_domain(avcodec::core::MemoryDomain::Host)
        .with_allow_staging(true);

    // Prepend cached parameter sets so the decoder always sees a complete
    // random-access unit.
    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&track.extradata);
    let annexb_payload = cache.prepend_to_annexb_access_unit(frame.codec, &frame.payload);

    let bitstream_format = match av_codec {
        avcodec::core::CodecId::H264 => BitstreamFormat::H264AnnexB,
        avcodec::core::CodecId::H265 => BitstreamFormat::H265AnnexB,
        _ => BitstreamFormat::Unknown,
    };

    let mut packet = Packet::from_host_bytes(
        avcodec::core::utils::next_buffer_id(),
        av_codec,
        bitstream_format,
        annexb_payload.to_vec(),
    );
    packet.pts = Some(frame.pts);
    packet.dts = Some(frame.dts);
    packet.time_base = Some(av_time_base);
    packet.flags = PacketFlags::KEY;

    let mut decoder: Box<dyn Decoder> = registry.create_decoder(&decoder_cfg).map_err(|e| {
        if e.kind() == avcodec::core::AvErrorKind::Unsupported {
            MediaError::unsupported(format!("video decoder unavailable for {av_codec:?}"))
        } else {
            MediaError::internal(format!("create video decoder: {e}"))
        }
    })?;

    decoder
        .submit_packet(packet)
        .map_err(|e| MediaError::invalid_argument(format!("submit video packet: {e}")))?;
    decoder
        .flush()
        .map_err(|e| MediaError::internal(format!("flush video decoder: {e}")))?;

    loop {
        match decoder
            .poll_frame()
            .map_err(|e| MediaError::internal(format!("poll decoded frame: {e}")))?
        {
            Poll::Ready(img) => return Ok(img),
            Poll::EndOfStream => {
                return Err(MediaError::internal("video decoder flushed without output"));
            }
            Poll::Pending => continue,
        }
    }
}

fn avcodec_codec_id(codec: CheetahCodecId) -> Option<avcodec::core::CodecId> {
    Some(match codec {
        CheetahCodecId::H264 => avcodec::core::CodecId::H264,
        CheetahCodecId::H265 => avcodec::core::CodecId::H265,
        CheetahCodecId::MJPEG => avcodec::core::CodecId::Jpeg,
        _ => return None,
    })
}

/// Converts `image` to `Rgb24` if necessary so downstream JPEG encoders can assume
/// a host RGB buffer.
fn ensure_rgb24(
    registry: &avcodec::core::Registry,
    image: avcodec::core::Image,
) -> Result<avcodec::core::Image> {
    use avcodec::core::{
        ImageInfo, ImageOp, ImageOpKind, ImageProcessRequest as AvImageProcessRequest,
        ImageProcessor, ImageProcessorConfig, Poll,
    };

    if image.format == ImageInfo::Rgb24 {
        return Ok(image);
    }

    let mut cfg = ImageProcessorConfig::new();
    cfg.allow_staging = true;
    cfg.memory_domain = avcodec::core::MemoryDomain::Host;
    cfg.target_op = Some(ImageOpKind::Csc);
    cfg.output_format = Some(ImageInfo::Rgb24);

    let mut processor: Box<dyn ImageProcessor> = registry
        .create_image_processor(&cfg)
        .map_err(|e| MediaError::internal(format!("create rgb24 converter: {e}")))?;

    let request = AvImageProcessRequest {
        src: image,
        op: ImageOp::Csc {
            dst_format: ImageInfo::Rgb24,
        },
        aux: None,
        target_domain: None,
    };

    processor
        .submit(request)
        .map_err(|e| MediaError::internal(format!("submit rgb24 conversion: {e}")))?;

    match processor
        .poll_image()
        .map_err(|e| MediaError::internal(format!("poll rgb24 conversion: {e}")))?
    {
        Poll::Ready(img) => Ok(img),
        Poll::Pending | Poll::EndOfStream => Err(MediaError::internal(
            "rgb24 converter did not produce output",
        )),
    }
}

fn encode_jpeg(
    registry: &avcodec::core::Registry,
    image: avcodec::core::Image,
    quality: u8,
) -> Result<ImageArtifact> {
    use avcodec::core::{CodecId, JpegEncoder, JpegEncoderConfig, Poll};

    let image = ensure_rgb24(registry, image)?;
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

/// Computes a thumbnail-style fit: scales down if the source is larger than the
/// supplied bounds, preserving aspect ratio. A bound of `0` means unconstrained
/// along that axis.
fn fit_dimensions(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let bound_w = if max_w == 0 { src_w } else { max_w };
    let bound_h = if max_h == 0 { src_h } else { max_h };
    let scale = f64::min(
        f64::min(bound_w as f64 / src_w as f64, bound_h as f64 / src_h as f64),
        1.0,
    );
    let w = (src_w as f64 * scale).round() as u32;
    let h = (src_h as f64 * scale).round() as u32;
    (w.max(1), h.max(1))
}

fn validate_operation_dimensions(
    op: &ImageOperation,
    image: &avcodec::core::Image,
    config: &MediaProcessingModuleConfig,
    index: usize,
) -> Result<()> {
    let exceeds = |w: u32, h: u32| w > config.max_image_width || h > config.max_image_height;

    let (target_w, target_h) = match op {
        ImageOperation::Resize { width, height } => (*width, *height),
        ImageOperation::Fit { width, height } => {
            if *width == 0 && *height == 0 {
                return Ok(());
            }
            fit_dimensions(image.coded_width, image.coded_height, *width, *height)
        }
        ImageOperation::ResizePad { width, height, .. } => (*width, *height),
        ImageOperation::Crop { width, height, .. } => (*width, *height),
        ImageOperation::Pad {
            top,
            bottom,
            left,
            right,
            ..
        } => (
            image
                .coded_width
                .saturating_add(*left)
                .saturating_add(*right),
            image
                .coded_height
                .saturating_add(*top)
                .saturating_add(*bottom),
        ),
        ImageOperation::Rotate { degrees } => {
            if degrees.rem_euclid(180) == 0 {
                return Ok(());
            }
            if degrees.rem_euclid(90) == 0 {
                (image.coded_height, image.coded_width)
            } else {
                // Non-90-degree rotations are rejected by map_image_operation.
                return Ok(());
            }
        }
        ImageOperation::Flip { .. }
        | ImageOperation::Csc { .. }
        | ImageOperation::Blend { .. }
        | ImageOperation::Text { .. } => return Ok(()),
    };

    if exceeds(target_w, target_h) {
        return Err(MediaError::invalid_argument(format!(
            "operation #{index} target size {target_w}x{target_h} exceeds configured limit {}x{}",
            config.max_image_width, config.max_image_height
        )));
    }

    Ok(())
}

fn map_image_operation(
    op: &ImageOperation,
    image: &avcodec::core::Image,
) -> std::result::Result<avcodec::core::ImageOp, String> {
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
        ImageOperation::Fit { width, height } => {
            if *width == 0 && *height == 0 {
                return Ok(ImageOp::Copy);
            }
            let (dst_w, dst_h) =
                fit_dimensions(image.coded_width, image.coded_height, *width, *height);
            Ok(ImageOp::ResizePad {
                dst_width: dst_w,
                dst_height: dst_h,
                fit: avcodec::core::FitMode::Contain,
                align: avcodec::core::PadAlign::Center,
                fill: PadColor::BLACK,
                filter: ScaleFilter::Bilinear,
            })
        }
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

/// Parses width/height from an encoded image header without decoding pixels.
fn encoded_dimensions(data: &[u8], format: ImageFormat) -> Option<(u32, u32)> {
    match format {
        ImageFormat::Jpeg => jpeg_dimensions(data),
        ImageFormat::Png => png_dimensions(data),
    }
}

/// Parses width/height from a PNG IHDR chunk.
fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    if data.len() < 24 || data[..8] != PNG_SIG {
        return None;
    }
    // Bytes 8-11: chunk length (always 13 for IHDR).
    // Bytes 12-15: chunk type.
    if &data[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((width, height))
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
        if marker == 0xFF {
            // Fill byte: advance by one so the following `0xFF <marker>` is parsed.
            i += 1;
            continue;
        }
        if marker == 0xD8 || marker == 0x00 {
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
    use avcodec::native_free_software_registry_builder;
    use cheetah_codec::{
        AVFrame, CodecId as CheetahCodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId,
        TrackInfo,
    };
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

    fn decode_jpeg_output(data: &[u8]) -> (u32, u32) {
        use avcodec::core::{
            BufferHandle, BufferSlice, EncodedImage, EncodedImageFormat, ImageDecoder,
            ImageDecoderConfig, ImageInfo, Poll,
        };

        let registry = native_free_software_registry_builder().build();
        let mut decoder: Box<dyn ImageDecoder> = registry
            .create_image_decoder(
                &ImageDecoderConfig::new(ImageInfo::Rgb24).with_format(EncodedImageFormat::Jpeg),
            )
            .expect("jpeg decoder");
        let handle = BufferHandle::from_host_bytes(0, data.to_vec());
        let slice = BufferSlice::new(handle, 0, data.len());
        let encoded = EncodedImage::new(slice, EncodedImageFormat::Jpeg);
        decoder.submit(encoded).expect("submit");
        let image = match decoder.poll_image().expect("poll") {
            Poll::Ready(img) => img,
            _ => panic!("decoder did not produce output"),
        };
        (image.coded_width, image.coded_height)
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

        // Decode the produced JPEG to verify it is a valid image of the
        // reported size, not just a correctly-sized blob.
        let (decoded_w, decoded_h) = decode_jpeg_output(&artifact.payload);
        assert_eq!(decoded_w, artifact.width);
        assert_eq!(decoded_h, artifact.height);
    }

    #[tokio::test]
    async fn repeated_process_runs_are_stable() {
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

        for i in 0..50 {
            let artifact = provider
                .process(&MediaRequestContext::default(), request.clone())
                .await
                .expect(&format!("process iteration {i} should succeed"));
            assert!(!artifact.payload.is_empty());
        }
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

    #[tokio::test]
    async fn rejects_oversized_resize_before_allocating() {
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let mut config = MediaProcessingModuleConfig::default();
        config.max_image_width = 64;
        config.max_image_height = 64;

        let provider = ImageProcessProvider::new(runtime, config);
        let request = ImageProcessRequest::new(
            ImageInput::Encoded {
                data: make_jpeg_fixture(),
                format: ImageFormat::Jpeg,
            },
            ImageFormat::Jpeg,
        )
        .with_operations(vec![ImageOperation::Resize {
            width: 100_000,
            height: 100_000,
        }]);

        let err = provider
            .process(&MediaRequestContext::default(), request)
            .await
            .expect_err("oversized resize should be rejected");
        assert_eq!(err.code, MediaErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn rejects_rotation_that_swaps_dimensions_over_limit() {
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let mut config = MediaProcessingModuleConfig::default();
        // Fixture is 4x4; after 90° rotation it is still 4x4, so use limits
        // smaller than one axis to force the swapped axis over the limit.
        config.max_image_width = 8;
        config.max_image_height = 3;

        let provider = ImageProcessProvider::new(runtime, config);
        let request = ImageProcessRequest::new(
            ImageInput::Encoded {
                data: make_jpeg_fixture(),
                format: ImageFormat::Jpeg,
            },
            ImageFormat::Jpeg,
        )
        .with_operations(vec![ImageOperation::Rotate { degrees: 90 }]);

        let err = provider
            .process(&MediaRequestContext::default(), request)
            .await
            .expect_err("rotation swapping to over-limit height should be rejected");
        assert_eq!(err.code, MediaErrorCode::InvalidArgument);
    }

    #[test]
    fn jpeg_dimensions_detects_fixture() {
        let jpeg = make_jpeg_fixture();
        assert_eq!(jpeg_dimensions(&jpeg), Some((4, 4)));
    }

    #[test]
    fn png_dimensions_parses_ihdr() {
        let mut data = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        // IHDR chunk length (13) and type.
        data.extend_from_slice(&13u32.to_be_bytes());
        data.extend_from_slice(b"IHDR");
        // 1000x2000 image, 8-bit RGB, no interlace, then dummy CRC.
        data.extend_from_slice(&1000u32.to_be_bytes());
        data.extend_from_slice(&2000u32.to_be_bytes());
        data.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(png_dimensions(&data), Some((1000, 2000)));
    }

    #[tokio::test]
    async fn rejects_oversized_encoded_input_before_decode() {
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let mut config = MediaProcessingModuleConfig::default();
        config.max_image_width = 8;
        config.max_image_height = 8;

        // Craft a PNG IHDR that declares 100x100, which is over the 8x8 limit.
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&100u32.to_be_bytes());
        png.extend_from_slice(&100u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);

        let provider = ImageProcessProvider::new(runtime, config);
        let request = ImageProcessRequest::new(
            ImageInput::Encoded {
                data: Bytes::from(png),
                format: ImageFormat::Png,
            },
            ImageFormat::Jpeg,
        );

        let err = provider
            .process(&MediaRequestContext::default(), request)
            .await
            .expect_err("oversized encoded input should be rejected before decode");
        assert_eq!(err.code, MediaErrorCode::InvalidArgument);
    }

    #[test]
    fn parse_pad_color_understands_hex_and_names() {
        let c = parse_pad_color("#ff00aa").unwrap();
        assert_eq!(c.0, [255, 0, 170, 255]);
        assert_eq!(parse_pad_color("black").unwrap().0, [0, 0, 0, 255]);
        assert_eq!(parse_pad_color("white").unwrap().0, [255, 255, 255, 255]);
        assert!(parse_pad_color("not-a-color").is_none());
    }

    fn make_h264_keyframe_fixture(width: u32, height: u32) -> Bytes {
        use avcodec::core::{Encoder, EncoderConfig, Image, ImageInfo, Packet, Poll, TimeBase};

        let registry = native_free_software_registry_builder().build();
        let enc_cfg = EncoderConfig::new(
            avcodec::core::CodecId::H264,
            width,
            height,
            ImageInfo::Yuv420p,
            TimeBase::new(1, 30),
            1_000_000,
        );
        let mut encoder: Box<dyn Encoder> = registry
            .create_encoder(&enc_cfg)
            .expect("rust-h264 encoder available");

        let y = vec![128u8; width as usize * height as usize];
        let u = vec![128u8; (width as usize / 2) * (height as usize / 2)];
        let v = vec![128u8; (width as usize / 2) * (height as usize / 2)];
        let mut image = Image::from_host_i420(
            width,
            height,
            &y,
            width as usize,
            &u,
            width as usize / 2,
            &v,
            width as usize / 2,
        )
        .expect("from_host_i420");
        image.pts = Some(0);
        image.dts = Some(0);

        encoder.submit_frame(image).expect("submit frame");
        let packet: Packet = match encoder.poll_packet().expect("poll packet") {
            Poll::Ready(p) => p,
            other => panic!("expected packet immediately, got {other:?}"),
        };

        Bytes::copy_from_slice(
            packet
                .data
                .host_bytes()
                .expect("host bytes")
                .expect("payload present"),
        )
    }

    #[tokio::test]
    async fn decodes_h264_video_frame_to_jpeg() {
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let provider = ImageProcessProvider::new(runtime, MediaProcessingModuleConfig::default());

        let (width, height) = (64u32, 48u32);
        let payload = make_h264_keyframe_fixture(width, height);
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CheetahCodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 30),
            payload,
        );
        frame.flags |= FrameFlags::KEY;

        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CheetahCodecId::H264, 90_000);
        track.width = Some(width);
        track.height = Some(height);

        let request = ImageProcessRequest::new(
            ImageInput::Frame {
                frame: Arc::new(frame),
                track,
            },
            ImageFormat::Jpeg,
        );

        let artifact = provider
            .process(&MediaRequestContext::default(), request)
            .await
            .expect("h264 frame should decode and encode to jpeg");

        assert_eq!(artifact.format, ImageFormat::Jpeg);
        assert_eq!(artifact.width, width);
        assert_eq!(artifact.height, height);
        assert!(!artifact.payload.is_empty());
    }
}
