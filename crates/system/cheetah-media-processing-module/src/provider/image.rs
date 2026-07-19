//! `ImageProcessApi` provider backed by `avcodec-rs`.
//!
//! Only compiled when `media-processing-image` is enabled.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_codec::{video_payload_is_random_access, ParameterSetCache};
use cheetah_codec::{CodecId as CheetahCodecId, MediaKind, TrackInfo};
use cheetah_media_api::{
    error::Result, ids::FileHandle, ImageArtifact, ImageFormat, ImageInput, ImageOperation,
    ImageProcessApi, ImageProcessRequest, MediaError, MediaFileStoreApi, MediaRequestContext,
};
use cheetah_runtime_api::RuntimeApi;
use futures::channel::oneshot;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{instrument, warn};

use crate::config::MediaProcessingModuleConfig;
use crate::provider::avcodec_registry::build_registry;
use crate::provider::semaphore::Semaphore;

/// Image processing provider using an avcodec `Registry`.
pub struct ImageProcessProvider {
    runtime: Arc<dyn RuntimeApi>,
    file_store: Option<Arc<dyn MediaFileStoreApi>>,
    config: Arc<Mutex<MediaProcessingModuleConfig>>,
    semaphore: Semaphore,
}

impl ImageProcessProvider {
    pub fn new(
        runtime: Arc<dyn RuntimeApi>,
        file_store: Option<Arc<dyn MediaFileStoreApi>>,
        config: MediaProcessingModuleConfig,
    ) -> Self {
        let config = Arc::new(Mutex::new(config));
        Self {
            runtime,
            file_store,
            config: config.clone(),
            semaphore: Semaphore::with_config(config),
        }
    }

    /// Atomically replace the running configuration.
    pub fn update_config(&self, config: MediaProcessingModuleConfig) {
        *self.config.lock().unwrap_or_else(|e| e.into_inner()) = config;
        self.semaphore.notify_waiters();
    }

    /// Read the current configuration snapshot.
    fn config(&self) -> MediaProcessingModuleConfig {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[async_trait]
impl ImageProcessApi for ImageProcessProvider {
    #[instrument(skip(self, ctx, request), fields(request_id = ?ctx.request_id))]
    async fn process(
        &self,
        ctx: &MediaRequestContext,
        request: ImageProcessRequest,
    ) -> Result<ImageArtifact> {
        // Acquire a concurrency permit before scheduling blocking work.
        let permit = self.semaphore.acquire().await;

        let runtime = Arc::clone(&self.runtime);
        let file_store = self.file_store.clone();
        let config = self.config();
        let ctx = ctx.clone();

        let (tx, rx) = oneshot::channel::<Result<ImageArtifact>>();
        runtime
            .spawn_blocking(
                "image-process",
                Box::new(move || {
                    // Hold the permit for the lifetime of the blocking task.
                    let _permit = permit;
                    let result = process_blocking(request, &config, &ctx, file_store.as_deref());
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
    ctx: &MediaRequestContext,
    file_store: Option<&dyn MediaFileStoreApi>,
) -> Result<ImageArtifact> {
    use avcodec::core::{
        ImageProcessRequest as AvImageProcessRequest, ImageProcessor, ImageProcessorConfig, Poll,
    };

    let registry = build_registry(config)?;

    // Pre-validate encoded frame size and declared dimensions so the decoder does
    // not have to allocate a huge buffer for an oversized input.
    if let ImageInput::Encoded { data, format } = &request.input {
        if data.len() as u64 > config.max_encoded_frame_bytes {
            return Err(MediaError::invalid_argument(format!(
                "encoded image {} bytes exceeds configured limit {}",
                data.len(),
                config.max_encoded_frame_bytes
            )));
        }
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
        ImageInput::Encoded { data, format } => {
            decode_encoded_image(&registry, &data, format, avcodec::core::ImageInfo::Rgb24)?
        }
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

        let mapped = map_image_operation(op, &image, &registry, config, ctx, file_store)
            .map_err(|e| MediaError::invalid_argument(format!("operation #{index}: {e}")))?;
        let cfg = ImageProcessorConfig::new().with_target_op(discriminant_of(&mapped.op));
        let mut processor: Box<dyn ImageProcessor> = registry
            .create_image_processor(&cfg)
            .map_err(|e| map_image_processor_error(&e))?;
        let mut av_req = AvImageProcessRequest::new(image, mapped.op);
        if let Some(aux) = mapped.aux {
            av_req = av_req.with_aux(aux);
        }
        processor
            .submit(av_req)
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

fn map_image_processor_error(e: &avcodec::core::AvError) -> MediaError {
    use avcodec::core::AvErrorKind;
    let message = format!("create image processor: {e}");
    match e.kind() {
        AvErrorKind::SelectionFailed | AvErrorKind::Unsupported => {
            MediaError::unsupported(format!("image processor unavailable: {e}"))
        }
        _ => MediaError::internal(message),
    }
}

fn decode_encoded_image(
    registry: &avcodec::core::Registry,
    data: &Bytes,
    format: ImageFormat,
    target_info: avcodec::core::ImageInfo,
) -> Result<avcodec::core::Image> {
    use avcodec::core::{
        BufferHandle, BufferSlice, EncodedImage, EncodedImageFormat, ImageDecoder,
        ImageDecoderConfig, Poll,
    };

    let encoded_format = match format {
        ImageFormat::Jpeg => EncodedImageFormat::Jpeg,
        ImageFormat::Png => EncodedImageFormat::Png,
    };

    let cfg = ImageDecoderConfig::new(target_info).with_format(encoded_format);
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
        return decode_encoded_image(
            registry,
            &frame.payload,
            ImageFormat::Jpeg,
            avcodec::core::ImageInfo::Rgb24,
        );
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

    let av_time_base = TimeBase::new(frame.timebase.num, frame.timebase.den);

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
        ImageOperation::Blend { overlay, .. } => {
            if let Some(format) = detect_overlay_format(overlay) {
                if let Some((w, h)) = encoded_dimensions(overlay, format) {
                    if exceeds(w, h) {
                        return Err(MediaError::invalid_argument(format!(
                            "operation #{index} blend overlay {w}x{h} exceeds configured limit {}x{}",
                            config.max_image_width, config.max_image_height
                        )));
                    }
                }
            }
            return Ok(());
        }
        ImageOperation::Flip { .. } | ImageOperation::Csc { .. } | ImageOperation::Text { .. } => {
            return Ok(())
        }
    };

    if exceeds(target_w, target_h) {
        return Err(MediaError::invalid_argument(format!(
            "operation #{index} target size {target_w}x{target_h} exceeds configured limit {}x{}",
            config.max_image_width, config.max_image_height
        )));
    }

    Ok(())
}

struct MappedOperation {
    op: avcodec::core::ImageOp,
    aux: Option<avcodec::core::Image>,
}

fn map_image_operation(
    op: &ImageOperation,
    image: &avcodec::core::Image,
    registry: &avcodec::core::Registry,
    config: &MediaProcessingModuleConfig,
    ctx: &MediaRequestContext,
    file_store: Option<&dyn MediaFileStoreApi>,
) -> std::result::Result<MappedOperation, String> {
    use avcodec::core::{ImageOp, OsdFontData, PadColor, Rect, Rotation, ScaleFilter};

    match op {
        ImageOperation::Crop {
            x,
            y,
            width,
            height,
        } => Ok(MappedOperation {
            op: ImageOp::Crop(Rect::new(*x, *y, *width, *height)),
            aux: None,
        }),
        ImageOperation::Resize { width, height } => Ok(MappedOperation {
            op: ImageOp::Resize {
                width: *width,
                height: *height,
            },
            aux: None,
        }),
        ImageOperation::Fit { width, height } => {
            if *width == 0 && *height == 0 {
                return Ok(MappedOperation {
                    op: ImageOp::Copy,
                    aux: None,
                });
            }
            let (dst_w, dst_h) =
                fit_dimensions(image.coded_width, image.coded_height, *width, *height);
            Ok(MappedOperation {
                op: ImageOp::ResizePad {
                    dst_width: dst_w,
                    dst_height: dst_h,
                    fit: avcodec::core::FitMode::Contain,
                    align: avcodec::core::PadAlign::Center,
                    fill: PadColor::BLACK,
                    filter: ScaleFilter::Bilinear,
                },
                aux: None,
            })
        }
        ImageOperation::Rotate { degrees } => {
            let rotation = match *degrees {
                90 => Rotation::R90,
                180 => Rotation::R180,
                270 => Rotation::R270,
                _ => return Err(format!("unsupported rotation angle: {degrees}")),
            };
            Ok(MappedOperation {
                op: ImageOp::Rotate(rotation),
                aux: None,
            })
        }
        ImageOperation::Flip {
            horizontal,
            vertical,
        } => match (horizontal, vertical) {
            (true, false) => Ok(MappedOperation {
                op: ImageOp::Flip(avcodec::core::FlipAxis::Horizontal),
                aux: None,
            }),
            (false, true) => Ok(MappedOperation {
                op: ImageOp::Flip(avcodec::core::FlipAxis::Vertical),
                aux: None,
            }),
            (true, true) => {
                Err("simultaneous horizontal and vertical flip is not supported".to_string())
            }
            (false, false) => Ok(MappedOperation {
                op: ImageOp::Copy,
                aux: None,
            }),
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
            Ok(MappedOperation {
                op: ImageOp::Pad {
                    left: *left,
                    right: *right,
                    top: *top,
                    bottom: *bottom,
                    color: pad_color,
                },
                aux: None,
            })
        }
        ImageOperation::Csc { format } => {
            let info = parse_image_info(format)?;
            Ok(MappedOperation {
                op: ImageOp::Csc { dst_format: info },
                aux: None,
            })
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
            Ok(MappedOperation {
                op: ImageOp::ResizePad {
                    dst_width: *width,
                    dst_height: *height,
                    fit: avcodec::core::FitMode::Contain,
                    align: avcodec::core::PadAlign::Center,
                    fill: pad_color,
                    filter: ScaleFilter::Bilinear,
                },
                aux: None,
            })
        }
        ImageOperation::Blend {
            overlay,
            x,
            y,
            opacity,
        } => {
            let format = detect_overlay_format(overlay)
                .ok_or_else(|| "blend overlay is not a recognized JPEG/PNG image".to_string())?;
            let (ov_w, ov_h) = encoded_dimensions(overlay, format)
                .ok_or_else(|| "blend overlay has no parseable dimensions".to_string())?;
            if ov_w > config.max_image_width || ov_h > config.max_image_height {
                return Err(format!(
                    "blend overlay {ov_w}x{ov_h} exceeds configured limit {}x{}",
                    config.max_image_width, config.max_image_height
                ));
            }
            let aux = decode_overlay_image(registry, overlay, format, config)?;
            let x = u32::try_from(*x)
                .map_err(|_| "blend x coordinate must be non-negative".to_string())?;
            let y = u32::try_from(*y)
                .map_err(|_| "blend y coordinate must be non-negative".to_string())?;
            Ok(MappedOperation {
                op: ImageOp::Blend {
                    x,
                    y,
                    global_alpha: opacity.unwrap_or(255),
                },
                aux: Some(aux),
            })
        }
        ImageOperation::Text {
            text,
            font_handle,
            x,
            y,
            size,
            color,
        } => {
            if *size > config.max_overlay_font_size {
                return Err(format!(
                    "text font size {size} exceeds configured max_overlay_font_size {}",
                    config.max_overlay_font_size
                ));
            }
            if font_handle.0.is_empty() {
                return Err("text overlay requires a non-empty font_handle".to_string());
            }
            let file_store = file_store.ok_or_else(|| {
                "text overlay requires a MediaFileStore but none is configured".to_string()
            })?;
            let font_data = resolve_font(ctx, file_store, font_handle)?;
            let color = color
                .as_ref()
                .and_then(|s| parse_pad_color(s))
                .unwrap_or(PadColor([255, 255, 255, 255]));
            Ok(MappedOperation {
                op: ImageOp::OsdText {
                    x: *x,
                    y: *y,
                    text: text.clone(),
                    size_px: *size,
                    color,
                    font: OsdFontData::new(font_data),
                },
                aux: None,
            })
        }
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
    if s.starts_with('#') && s.is_ascii() {
        match s.len() {
            7 => {
                let r = u8::from_str_radix(&s[1..3], 16).ok()?;
                let g = u8::from_str_radix(&s[3..5], 16).ok()?;
                let b = u8::from_str_radix(&s[5..7], 16).ok()?;
                Some(avcodec::core::PadColor([r, g, b, 255]))
            }
            9 => {
                let r = u8::from_str_radix(&s[1..3], 16).ok()?;
                let g = u8::from_str_radix(&s[3..5], 16).ok()?;
                let b = u8::from_str_radix(&s[5..7], 16).ok()?;
                let a = u8::from_str_radix(&s[7..9], 16).ok()?;
                Some(avcodec::core::PadColor([r, g, b, a]))
            }
            5 => {
                let r = u8::from_str_radix(&s[1..2], 16).ok()? * 17;
                let g = u8::from_str_radix(&s[2..3], 16).ok()? * 17;
                let b = u8::from_str_radix(&s[3..4], 16).ok()? * 17;
                let a = u8::from_str_radix(&s[4..5], 16).ok()? * 17;
                Some(avcodec::core::PadColor([r, g, b, a]))
            }
            _ => None,
        }
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

/// Detects the encoded image format of an overlay payload from its magic bytes.
fn detect_overlay_format(data: &[u8]) -> Option<ImageFormat> {
    if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
        Some(ImageFormat::Jpeg)
    } else if data.len() >= 8 && data[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        Some(ImageFormat::Png)
    } else {
        None
    }
}

/// Decodes an overlay payload into a host-memory `Image` for blending.
fn decode_overlay_image(
    registry: &avcodec::core::Registry,
    data: &Bytes,
    format: ImageFormat,
    config: &MediaProcessingModuleConfig,
) -> std::result::Result<avcodec::core::Image, String> {
    use avcodec::core::ImageInfo;
    let target_info = match format {
        ImageFormat::Jpeg => ImageInfo::Rgb24,
        // Preserve per-pixel alpha for PNG overlays so OpenCV Blend can use it.
        ImageFormat::Png => ImageInfo::Rgba,
    };
    let image = decode_encoded_image(registry, data, format, target_info)
        .map_err(|e| format!("decode overlay image: {e}"))?;
    if image.coded_width > config.max_image_width || image.coded_height > config.max_image_height {
        return Err(format!(
            "decoded overlay {}x{} exceeds configured limit {}x{}",
            image.coded_width, image.coded_height, config.max_image_width, config.max_image_height
        ));
    }
    Ok(image)
}

/// Loads a font file referenced by an authorized `FileHandle`.
///
/// Errors are sanitized: the returned `String` never contains the server-side
/// absolute path, font payload, or store-layer error detail. Those details are
/// logged at warn level for operators while callers receive a generic message.
fn resolve_font(
    ctx: &MediaRequestContext,
    file_store: &dyn MediaFileStoreApi,
    handle: &FileHandle,
) -> std::result::Result<Vec<u8>, String> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let entry = file_store
        .resolve_for_read(ctx, handle, None, now_ms)
        .map_err(|e| {
            warn!(font_handle = %handle, "resolve font handle failed: {e}");
            format!("font handle {handle} is not authorized or not found")
        })?;
    std::fs::read(&entry.absolute_path).map_err(|e| {
        warn!(
            font_handle = %handle,
            path = %entry.absolute_path,
            "failed to read font file: {e}"
        );
        format!("failed to read font for handle {handle}")
    })
}

#[cfg(all(test, feature = "media-processing-image"))]
mod tests {
    use super::*;
    use avcodec::native_free_software_registry_builder;
    use cheetah_codec::{
        AVFrame, CodecId as CheetahCodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId,
        TrackInfo,
    };
    use cheetah_media_api::{
        ids::FileHandle, FileDownload, FileStoreEntry, ImageFormat, ImageInput,
        ImageProcessRequest, MediaErrorCode, MediaFileStoreApi,
    };
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
        let provider =
            ImageProcessProvider::new(runtime, None, MediaProcessingModuleConfig::default());
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
        let provider =
            ImageProcessProvider::new(runtime, None, MediaProcessingModuleConfig::default());
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
                .unwrap_or_else(|_| panic!("process iteration {i} should succeed"));
            assert!(!artifact.payload.is_empty());
        }
    }

    #[tokio::test]
    async fn rejects_png_output() {
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let provider =
            ImageProcessProvider::new(runtime, None, MediaProcessingModuleConfig::default());
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
        let config = MediaProcessingModuleConfig {
            max_image_width: 64,
            max_image_height: 64,
            ..Default::default()
        };

        let provider = ImageProcessProvider::new(runtime, None, config);
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
        // Fixture is 4x4; after 90° rotation it is still 4x4, so use limits
        // smaller than one axis to force the swapped axis over the limit.
        let config = MediaProcessingModuleConfig {
            max_image_width: 8,
            max_image_height: 3,
            ..Default::default()
        };

        let provider = ImageProcessProvider::new(runtime, None, config);
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
        let config = MediaProcessingModuleConfig {
            max_image_width: 8,
            max_image_height: 8,
            ..Default::default()
        };

        // Craft a PNG IHDR that declares 100x100, which is over the 8x8 limit.
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&100u32.to_be_bytes());
        png.extend_from_slice(&100u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);

        let provider = ImageProcessProvider::new(runtime, None, config);
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
        assert_eq!(parse_pad_color("#ff00aacc").unwrap().0, [255, 0, 170, 204]);
        assert_eq!(parse_pad_color("#f0ac").unwrap().0, [255, 0, 170, 204]);
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
        let provider =
            ImageProcessProvider::new(runtime, None, MediaProcessingModuleConfig::default());

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

    #[tokio::test]
    #[cfg(feature = "media-processing-image-overlay")]
    async fn blends_jpeg_overlay_onto_jpeg() {
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let provider =
            ImageProcessProvider::new(runtime, None, MediaProcessingModuleConfig::default());
        let overlay = make_jpeg_fixture();
        let request = ImageProcessRequest {
            input: ImageInput::Encoded {
                data: make_jpeg_fixture(),
                format: ImageFormat::Jpeg,
            },
            operations: vec![ImageOperation::Blend {
                overlay,
                x: 0,
                y: 0,
                opacity: Some(128),
            }],
            output_format: ImageFormat::Jpeg,
            quality: 80,
        };

        let artifact = provider
            .process(&MediaRequestContext::default(), request)
            .await
            .expect("blend should produce jpeg");

        assert_eq!(artifact.format, ImageFormat::Jpeg);
        assert_eq!(artifact.width, 4);
        assert_eq!(artifact.height, 4);
        assert!(!artifact.payload.is_empty());
    }

    #[tokio::test]
    async fn rejects_oversized_blend_overlay() {
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let config = MediaProcessingModuleConfig {
            max_image_width: 8,
            max_image_height: 8,
            ..Default::default()
        };

        // 100x100 PNG declaration.
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&100u32.to_be_bytes());
        png.extend_from_slice(&100u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);

        let provider = ImageProcessProvider::new(runtime, None, config);
        let request = ImageProcessRequest {
            input: ImageInput::Encoded {
                data: make_jpeg_fixture(),
                format: ImageFormat::Jpeg,
            },
            operations: vec![ImageOperation::Blend {
                overlay: Bytes::from(png),
                x: 0,
                y: 0,
                opacity: None,
            }],
            output_format: ImageFormat::Jpeg,
            quality: 80,
        };

        let err = provider
            .process(&MediaRequestContext::default(), request)
            .await
            .expect_err("oversized blend overlay should be rejected");
        assert_eq!(err.code, MediaErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn text_overlay_requires_file_store() {
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let provider =
            ImageProcessProvider::new(runtime, None, MediaProcessingModuleConfig::default());
        let request = ImageProcessRequest {
            input: ImageInput::Encoded {
                data: make_jpeg_fixture(),
                format: ImageFormat::Jpeg,
            },
            operations: vec![ImageOperation::Text {
                text: "hello".to_string(),
                font_handle: FileHandle("test-font".to_string()),
                x: 0,
                y: 0,
                size: 12,
                color: Some("#ffffffff".to_string()),
            }],
            output_format: ImageFormat::Jpeg,
            quality: 80,
        };

        let err = provider
            .process(&MediaRequestContext::default(), request)
            .await
            .expect_err("text overlay without file store should fail");
        assert_eq!(err.code, MediaErrorCode::InvalidArgument);
    }

    struct MockFontStore {
        path: String,
    }

    impl MediaFileStoreApi for MockFontStore {
        fn register_file(
            &self,
            _ctx: &MediaRequestContext,
            _entry: FileStoreEntry,
        ) -> cheetah_media_api::error::Result<FileHandle> {
            Err(MediaError::unsupported("register_file not implemented"))
        }

        fn resolve_for_read(
            &self,
            _ctx: &MediaRequestContext,
            handle: &FileHandle,
            _resource_scope: Option<&cheetah_media_api::MediaKey>,
            _now_ms: i64,
        ) -> cheetah_media_api::error::Result<FileStoreEntry> {
            if handle.0 == "serif" {
                Ok(FileStoreEntry {
                    absolute_path: self.path.clone(),
                    ..FileStoreEntry::default()
                })
            } else {
                Err(MediaError::invalid_argument(format!(
                    "unknown font handle: {}",
                    handle.0
                )))
            }
        }

        fn delete(
            &self,
            _ctx: &MediaRequestContext,
            _handle: &FileHandle,
            _now_ms: i64,
        ) -> cheetah_media_api::error::Result<()> {
            Err(MediaError::unsupported("delete not implemented"))
        }

        fn delete_batch(
            &self,
            _ctx: &MediaRequestContext,
            _query: cheetah_media_api::FileStoreQuery,
            _batch_limit: u32,
            _now_ms: i64,
        ) -> cheetah_media_api::error::Result<cheetah_media_api::DeleteBatchResult> {
            Err(MediaError::unsupported("delete_batch not implemented"))
        }

        fn resolve_download(
            &self,
            _ctx: &MediaRequestContext,
            _handle: &FileHandle,
            _range: Option<cheetah_media_api::FileRange>,
            _filename: Option<String>,
            _now_ms: i64,
        ) -> cheetah_media_api::error::Result<FileDownload> {
            Err(MediaError::unsupported("resolve_download not implemented"))
        }
    }

    fn is_font_path(p: &std::path::Path) -> bool {
        matches!(
            p.extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase())
                .as_deref(),
            Some("ttf") | Some("otf")
        )
    }

    fn find_test_font() -> Option<String> {
        let candidates = ["/usr/share/fonts", "/usr/local/share/fonts"];
        for root in candidates {
            let Ok(entries) = std::fs::read_dir(root) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Ok(sub) = std::fs::read_dir(&path) {
                        for e in sub.flatten() {
                            let p = e.path();
                            if is_font_path(&p) {
                                return Some(p.to_string_lossy().to_string());
                            }
                        }
                    }
                } else if is_font_path(&path) {
                    return Some(path.to_string_lossy().to_string());
                }
            }
        }
        None
    }

    #[tokio::test]
    async fn text_overlay_renders_with_font() {
        let Some(font_path) = find_test_font() else {
            return;
        };
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let store: Arc<dyn MediaFileStoreApi> = Arc::new(MockFontStore { path: font_path });
        let provider =
            ImageProcessProvider::new(runtime, Some(store), MediaProcessingModuleConfig::default());

        let request = ImageProcessRequest {
            input: ImageInput::Encoded {
                data: make_jpeg_fixture(),
                format: ImageFormat::Jpeg,
            },
            operations: vec![ImageOperation::Text {
                text: "A".to_string(),
                font_handle: FileHandle("serif".to_string()),
                x: 0,
                y: 0,
                size: 12,
                color: Some("#ffffffff".to_string()),
            }],
            output_format: ImageFormat::Jpeg,
            quality: 80,
        };

        let artifact = provider
            .process(&MediaRequestContext::default(), request)
            .await
            .expect("text overlay should render");

        assert_eq!(artifact.format, ImageFormat::Jpeg);
        assert_eq!(artifact.width, 4);
        assert_eq!(artifact.height, 4);
        assert!(!artifact.payload.is_empty());
    }
}
