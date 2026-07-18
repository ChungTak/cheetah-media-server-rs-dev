//! Video mosaic compositor running in a blocking worker thread.
//!
//! Decodes 2-9 source video streams, scales each source to a fixed tile,
//! composites tiles onto a canvas, and encodes a single H.264/H.265 output stream.

use cheetah_codec::{
    frame::{FrameFlags as CheetahFrameFlags, FrameOrigin},
    track::{MediaKind, TrackReadiness},
    AVFrame, CodecId as CheetahCodecId, FrameFormat, ParameterSetCache, Rational32, Timebase,
    TrackId, TrackInfo,
};
use cheetah_media_api::{
    error::{MediaError, Result},
    processing::{MosaicCell, MosaicFit, MosaicLayout, VideoCodec, VideoMosaicInput},
};
use tracing::warn;

use crate::config::MediaProcessingModuleConfig;
use crate::provider::avcodec_registry::build_registry;
use crate::provider::video::{
    av_frame_from_packet, default_video_bitrate, map_input_format, map_output_codec,
};

use avcodec::core::FitMode;
use avcodec::core::{
    BufferHandle, Decoder, DecoderConfig, Encoder, EncoderConfig, Image, ImageFlags, ImageInfo,
    ImageOp, ImageOpKind, ImagePlane, ImageProcessRequest, ImageProcessor, ImageProcessorConfig,
    MemoryDomain, PacketFlags, Poll, Rect, SampleLayout, TimeBase,
};

const MIN_SOURCES: usize = 2;
const MAX_SOURCES: usize = 9;

pub(crate) struct VideoMosaicker {
    registry: avcodec::core::Registry,
    output_codec: CheetahCodecId,
    output_width: u32,
    output_height: u32,
    output_timebase: Timebase,
    output_frame_format: FrameFormat,
    output_frame_duration: i64,
    output_track: TrackInfo,
    encoder: Box<dyn Encoder>,
    sources: Vec<SourceState>,
    pts: i64,
    output_param_cache: ParameterSetCache,
    gop_size: u32,
    frame_count: u64,
}

struct SourceState {
    input_track: TrackInfo,
    decoder: Option<Box<dyn Decoder>>,
    processor: Box<dyn ImageProcessor>,
    cell: MosaicCell,
    cell_width: u32,
    cell_height: u32,
    fit: FitMode,
    latest_tile: Option<Image>,
    param_cache: ParameterSetCache,
    eos: bool,
}

impl VideoMosaicker {
    pub fn new(
        config: &MediaProcessingModuleConfig,
        inputs: &[VideoMosaicInput],
        layout: &MosaicLayout,
        source_tracks: &[TrackInfo],
    ) -> Result<Self> {
        if inputs.len() < MIN_SOURCES {
            return Err(MediaError::invalid_argument(format!(
                "video mosaic requires at least {MIN_SOURCES} sources, got {}",
                inputs.len()
            )));
        }
        if inputs.len() > MAX_SOURCES {
            return Err(MediaError::invalid_argument(format!(
                "video mosaic supports at most {MAX_SOURCES} sources, got {}",
                inputs.len()
            )));
        }
        if source_tracks.len() != inputs.len() {
            return Err(MediaError::invalid_argument(
                "video mosaic source track count does not match input count",
            ));
        }
        if layout.columns == 0
            || layout.rows == 0
            || layout.cell_width == 0
            || layout.cell_height == 0
        {
            return Err(MediaError::invalid_argument(
                "video mosaic layout dimensions must be non-zero",
            ));
        }

        let output_width = layout
            .columns
            .checked_mul(layout.cell_width)
            .ok_or_else(|| MediaError::invalid_argument("video mosaic output width overflow"))?;
        let output_height = layout
            .rows
            .checked_mul(layout.cell_height)
            .ok_or_else(|| MediaError::invalid_argument("video mosaic output height overflow"))?;
        if output_width > config.max_image_width || output_height > config.max_image_height {
            return Err(MediaError::invalid_argument(format!(
                "video mosaic output {output_width}x{output_height} exceeds configured limit {}x{}",
                config.max_image_width, config.max_image_height
            )));
        }
        if !layout.cell_width.is_multiple_of(2) || !layout.cell_height.is_multiple_of(2) {
            return Err(MediaError::invalid_argument(
                "video mosaic cell width and height must be even",
            ));
        }

        let output_codec = video_codec_to_cheetah(layout.video_codec.unwrap_or(VideoCodec::H264));
        let (av_output_codec, output_frame_format, _output_bitstream, output_pixel_format) =
            map_output_codec(output_codec).ok_or_else(|| {
                MediaError::unsupported(format!("video mosaic output codec {output_codec:?}"))
            })?;
        if output_pixel_format != ImageInfo::Yuv420p {
            return Err(MediaError::unsupported(
                "video mosaic only supports YUV 4:2:0 output",
            ));
        }

        let output_fps = resolve_output_fps(layout);
        let output_timebase = Timebase::new(output_fps.den, output_fps.num);
        let output_bitrate = if let Some(br) = layout.bit_rate {
            if br > u32::MAX as u64 {
                return Err(MediaError::invalid_argument(
                    "video mosaic bit_rate exceeds u32 range",
                ));
            }
            br as u32
        } else {
            default_video_bitrate(output_codec, output_width, output_height)
        };

        let registry = build_registry(config)?;
        let encoder_cfg = EncoderConfig::new(
            av_output_codec,
            output_width,
            output_height,
            output_pixel_format,
            TimeBase::new(output_timebase.num, output_timebase.den),
            output_bitrate,
        )
        .with_memory_domain(MemoryDomain::Host)
        .with_allow_staging(false);
        let encoder = registry
            .create_encoder(&encoder_cfg)
            .map_err(|e| MediaError::unsupported(format!("create mosaic encoder: {e}")))?;

        let default_fit = map_mosaic_fit(layout.fit.unwrap_or(MosaicFit::Cover));
        let mut sources = Vec::with_capacity(inputs.len());
        for (i, input) in inputs.iter().enumerate() {
            let track = &source_tracks[i];
            if track.media_kind != MediaKind::Video {
                return Err(MediaError::invalid_argument(format!(
                    "video mosaic source {i} is not a video track"
                )));
            }
            if input.cell.column >= layout.columns || input.cell.row >= layout.rows {
                return Err(MediaError::invalid_argument(format!(
                    "video mosaic source {i} cell is outside the layout grid"
                )));
            }

            let fit = input.fit.map(map_mosaic_fit).unwrap_or(default_fit);
            let mut proc_cfg = ImageProcessorConfig::new();
            proc_cfg.memory_domain = MemoryDomain::Host;
            proc_cfg.allow_staging = false;
            proc_cfg.output_format = Some(ImageInfo::Yuv420p);
            proc_cfg.target_op = Some(ImageOpKind::CropResize);
            let processor = registry
                .create_image_processor(&proc_cfg)
                .map_err(|e| MediaError::unsupported(format!("create image processor: {e}")))?;

            sources.push(SourceState {
                input_track: track.clone(),
                decoder: None,
                processor,
                cell: input.cell.clone(),
                cell_width: layout.cell_width,
                cell_height: layout.cell_height,
                fit,
                latest_tile: None,
                param_cache: ParameterSetCache::default(),
                eos: false,
            });
        }

        let mut output_track = TrackInfo::new(
            TrackId(0),
            MediaKind::Video,
            output_codec,
            output_timebase.den as u32,
        );
        output_track.width = Some(output_width);
        output_track.height = Some(output_height);
        output_track.fps = Some(output_fps);
        output_track.bitrate = Some(output_bitrate);
        output_track.readiness = TrackReadiness::Ready;

        let gop_size = layout.gop_size.unwrap_or(30);

        Ok(Self {
            registry,
            output_codec,
            output_width,
            output_height,
            output_timebase,
            output_frame_format,
            output_frame_duration: 1,
            output_track,
            encoder,
            sources,
            pts: 0,
            output_param_cache: ParameterSetCache::default(),
            gop_size,
            frame_count: 0,
        })
    }

    pub fn output_track(&self) -> &TrackInfo {
        &self.output_track
    }

    pub fn submit_source_frame(&mut self, source_index: usize, frame: &AVFrame) -> Result<()> {
        if source_index >= self.sources.len() {
            return Err(MediaError::invalid_argument(format!(
                "video mosaic source index {source_index} out of range"
            )));
        }
        let state = &mut self.sources[source_index];
        if state.eos {
            return Ok(());
        }
        if frame.media_kind != MediaKind::Video {
            return Ok(());
        }

        if state.decoder.is_none() {
            if !frame.flags.contains(CheetahFrameFlags::KEY)
                && !state.param_cache.has_required_sets(frame.codec)
            {
                warn!(
                    source = source_index,
                    "dropping non-key frame before mosaic decoder initialization"
                );
                return Ok(());
            }
            let registry = self.registry.clone();
            state.decoder = Some(build_decoder(&registry, frame)?);
        }

        state
            .param_cache
            .update_from_extradata(&state.input_track.extradata);
        state
            .param_cache
            .update_from_annexb(frame.codec, &frame.payload);
        let payload = state
            .param_cache
            .prepend_to_annexb_access_unit(frame.codec, &frame.payload);

        let (av_codec, bitstream_format) =
            map_input_format(frame.format, frame.codec).ok_or_else(|| {
                MediaError::unsupported(format!(
                    "unsupported source video codec/format: {:?}/{:?}",
                    frame.codec, frame.format
                ))
            })?;

        let mut packet = avcodec::core::Packet::from_host_bytes(
            avcodec::core::utils::next_buffer_id(),
            av_codec,
            bitstream_format,
            payload.to_vec(),
        );
        packet.pts = Some(frame.pts);
        packet.dts = Some(frame.dts);
        packet.time_base = Some(TimeBase::new(frame.timebase.num, frame.timebase.den));
        if frame.flags.contains(CheetahFrameFlags::KEY) {
            packet.flags = PacketFlags::KEY;
        }

        state
            .decoder
            .as_mut()
            .unwrap()
            .submit_packet(packet)
            .map_err(|e| MediaError::invalid_argument(format!("submit source packet: {e}")))?;

        let mut decoder = state.decoder.take().unwrap();
        loop {
            match decoder.poll_frame() {
                Ok(Poll::Ready(img)) => {
                    let tile = process_tile(img, state)?;
                    state.latest_tile = Some(tile);
                }
                Ok(Poll::Pending) => break,
                Ok(Poll::EndOfStream) => break,
                Err(e) => {
                    warn!(source = source_index, "mosaic decoder poll error: {e}");
                    break;
                }
            }
        }
        state.decoder = Some(decoder);

        Ok(())
    }

    pub fn mark_source_eos(&mut self, source_index: usize) -> Result<bool> {
        if source_index >= self.sources.len() {
            return Err(MediaError::invalid_argument(format!(
                "video mosaic source index {source_index} out of range"
            )));
        }
        let state = &mut self.sources[source_index];
        if state.eos {
            return Ok(false);
        }
        state.eos = true;
        if let Some(mut decoder) = state.decoder.take() {
            decoder
                .flush()
                .map_err(|e| MediaError::invalid_argument(format!("flush decoder: {e}")))?;
            loop {
                match decoder.poll_frame() {
                    Ok(Poll::Ready(img)) => {
                        if let Ok(tile) = process_tile(img, state) {
                            state.latest_tile = Some(tile);
                        }
                    }
                    Ok(Poll::Pending) => break,
                    Ok(Poll::EndOfStream) => break,
                    Err(_) => break,
                }
            }
            state.decoder = Some(decoder);
        }
        Ok(true)
    }

    pub fn tick(&mut self) -> Result<Vec<AVFrame>> {
        let (mut canvas_buf, y_stride, uv_stride) =
            allocate_canvas_buffer(self.output_width, self.output_height);
        fill_yuv420p_black(&mut canvas_buf, self.output_width, self.output_height);

        let mut order: Vec<usize> = (0..self.sources.len()).collect();
        order.sort_by_key(|i| self.sources[*i].cell.z_order);

        for i in order {
            let state = &self.sources[i];
            if let Some(tile) = state.latest_tile.as_ref() {
                composite_tile(
                    &mut canvas_buf,
                    self.output_width,
                    self.output_height,
                    y_stride,
                    uv_stride,
                    tile,
                    &state.cell,
                    state.cell_width,
                    state.cell_height,
                )
                .map_err(|e| MediaError::internal(format!("composite tile: {e}")))?;
            }
        }

        let mut canvas = build_yuv420p_image(
            canvas_buf,
            self.output_width,
            self.output_height,
            y_stride,
            uv_stride,
        );
        canvas.pts = Some(self.pts);
        canvas.dts = Some(self.pts);
        let force_key = self.frame_count == 0
            || (self.gop_size > 0 && self.frame_count.is_multiple_of(self.gop_size as u64));
        if force_key {
            canvas.flags = ImageFlags::KEY;
        }

        self.encoder
            .submit_frame(canvas)
            .map_err(|e| MediaError::invalid_argument(format!("submit mosaic frame: {e}")))?;

        let mut out = Vec::new();
        drain_encoder(self, &mut out)?;
        self.pts += 1;
        self.frame_count += 1;
        Ok(out)
    }

    pub fn flush(&mut self) -> Result<Vec<AVFrame>> {
        self.encoder
            .flush()
            .map_err(|e| MediaError::invalid_argument(format!("flush mosaic encoder: {e}")))?;
        let mut out = Vec::new();
        drain_encoder(self, &mut out)?;
        Ok(out)
    }

    pub fn all_sources_eos(&self) -> bool {
        self.sources.iter().all(|s| s.eos)
    }
}

fn build_decoder(registry: &avcodec::core::Registry, frame: &AVFrame) -> Result<Box<dyn Decoder>> {
    let (av_codec, _bitstream) = map_input_format(frame.format, frame.codec).ok_or_else(|| {
        MediaError::unsupported(format!(
            "unsupported source video codec/format: {:?}/{:?}",
            frame.codec, frame.format
        ))
    })?;
    let time_base = TimeBase::new(frame.timebase.num, frame.timebase.den);
    let cfg = DecoderConfig::new(av_codec, time_base)
        .with_memory_domain(MemoryDomain::Host)
        .with_allow_staging(false);
    registry
        .create_decoder(&cfg)
        .map_err(|e| MediaError::unsupported(format!("create decoder: {e}")))
}

fn process_tile(image: Image, state: &mut SourceState) -> Result<Image> {
    let (crop, dst_w, dst_h) =
        compute_tile_geometry(&image, state.cell_width, state.cell_height, state.fit);

    let req = ImageProcessRequest::new(
        image,
        ImageOp::CropResize {
            src: crop,
            dst_width: dst_w,
            dst_height: dst_h,
        },
    );
    state
        .processor
        .submit(req)
        .map_err(|e| MediaError::internal(format!("submit tile processor: {e}")))?;
    let scaled = match state.processor.poll_image() {
        Ok(Poll::Ready(img)) => img,
        Ok(Poll::Pending) => return Err(MediaError::internal("tile processor returned pending")),
        Ok(Poll::EndOfStream) => {
            return Err(MediaError::internal("tile processor ended without output"))
        }
        Err(e) => return Err(MediaError::internal(format!("poll tile image: {e}"))),
    };

    if state.fit == FitMode::Contain && (dst_w < state.cell_width || dst_h < state.cell_height) {
        pad_yuv420p_to_cell(scaled, state.cell_width, state.cell_height)
            .map_err(|e| MediaError::internal(format!("pad mosaic tile: {e}")))
    } else {
        Ok(scaled)
    }
}

fn even_dim(v: u32) -> u32 {
    v & !1
}

fn centered_even_rect(x: u32, y: u32, mut w: u32, mut h: u32, max_w: u32, max_h: u32) -> Rect {
    w = even_dim(w).max(2);
    h = even_dim(h).max(2);
    if w > max_w {
        w = even_dim(max_w).max(2);
    }
    if h > max_h {
        h = even_dim(max_h).max(2);
    }
    let mut cx = x + (max_w.saturating_sub(w)) / 2;
    let mut cy = y + (max_h.saturating_sub(h)) / 2;
    if cx + w > max_w {
        cx = max_w.saturating_sub(w);
    }
    if cy + h > max_h {
        cy = max_h.saturating_sub(h);
    }
    cx &= !1;
    cy &= !1;
    Rect {
        x: cx,
        y: cy,
        width: w,
        height: h,
    }
}

fn compute_tile_geometry(
    image: &Image,
    cell_w: u32,
    cell_h: u32,
    fit: FitMode,
) -> (Rect, u32, u32) {
    let src_x = image.visible.x;
    let src_y = image.visible.y;
    let src_w = image.visible.width;
    let src_h = image.visible.height;
    let max_w = image.coded_width;
    let max_h = image.coded_height;

    match fit {
        FitMode::Stretch => {
            let crop = centered_even_rect(src_x, src_y, src_w, src_h, max_w, max_h);
            (crop, even_dim(cell_w).max(2), even_dim(cell_h).max(2))
        }
        FitMode::Cover => {
            let cell_aspect = cell_w as f64 / cell_h.max(1) as f64;
            let src_aspect = src_w as f64 / src_h.max(1) as f64;
            let (crop_w, crop_h) = if src_aspect > cell_aspect {
                let h = src_h;
                let w = ((src_h as f64 * cell_aspect) as u32).max(1);
                (w, h)
            } else {
                let w = src_w;
                let h = ((src_w as f64 / cell_aspect) as u32).max(1);
                (w, h)
            };
            let crop = centered_even_rect(src_x, src_y, crop_w, crop_h, max_w, max_h);
            (crop, even_dim(cell_w).max(2), even_dim(cell_h).max(2))
        }
        FitMode::Contain => {
            let scale =
                (cell_w as f64 / src_w.max(1) as f64).min(cell_h as f64 / src_h.max(1) as f64);
            let content_w = ((src_w as f64 * scale) as u32).max(1);
            let content_h = ((src_h as f64 * scale) as u32).max(1);
            let crop = centered_even_rect(src_x, src_y, src_w, src_h, max_w, max_h);
            (crop, even_dim(content_w).max(2), even_dim(content_h).max(2))
        }
    }
}

fn pad_yuv420p_to_cell(content: Image, cell_w: u32, cell_h: u32) -> avcodec::core::AvResult<Image> {
    let content_w = content.visible.width;
    let content_h = content.visible.height;
    if content_w == cell_w && content_h == cell_h {
        return Ok(content);
    }

    let cw = cell_w as usize;
    let ch = cell_h as usize;
    let uv_h = ch.div_ceil(2);
    let y_len = cw * ch;
    let uv_len = (cw / 2) * uv_h;
    let total = y_len + uv_len * 2;
    let mut buf = avcodec::core::buffer::allocate_host_vec(total);

    for y in &mut buf[0..y_len] {
        *y = 16;
    }
    for uv in &mut buf[y_len..y_len + uv_len * 2] {
        *uv = 128;
    }

    let off_x = ((cell_w - content_w) / 2) & !1;
    let off_y = ((cell_h - content_h) / 2) & !1;

    let y_plane = content.planes[0].ok_or(avcodec::core::AvError::InvalidArgument)?;
    let u_plane = content.planes[1].ok_or(avcodec::core::AvError::InvalidArgument)?;
    let v_plane = content.planes[2].ok_or(avcodec::core::AvError::InvalidArgument)?;

    let tile_y = content
        .plane_host_bytes(0)?
        .ok_or(avcodec::core::AvError::InvalidArgument)?;
    let tile_u = content
        .plane_host_bytes(1)?
        .ok_or(avcodec::core::AvError::InvalidArgument)?;
    let tile_v = content
        .plane_host_bytes(2)?
        .ok_or(avcodec::core::AvError::InvalidArgument)?;

    let y_src_start = content.visible.y as usize * y_plane.stride + content.visible.x as usize;
    for row in 0..content_h as usize {
        let src = y_src_start + row * y_plane.stride;
        let dst = (off_y as usize + row) * cw + off_x as usize;
        buf[dst..dst + content_w as usize].copy_from_slice(&tile_y[src..src + content_w as usize]);
    }

    let content_uv_w = (content_w / 2) as usize;
    let content_uv_h = (content_h / 2) as usize;
    let uv_off_x = (off_x / 2) as usize;
    let uv_off_y = (off_y / 2) as usize;
    let uv_stride = cw / 2;

    let uv_src_start =
        (content.visible.y as usize / 2) * u_plane.stride + (content.visible.x as usize / 2);
    for row in 0..content_uv_h {
        let src = uv_src_start + row * u_plane.stride;
        let dst_u = y_len + (uv_off_y + row) * uv_stride + uv_off_x;
        buf[dst_u..dst_u + content_uv_w].copy_from_slice(&tile_u[src..src + content_uv_w]);
        let dst_v = y_len + uv_len + (uv_off_y + row) * uv_stride + uv_off_x;
        let v_src = (content.visible.y as usize / 2) * v_plane.stride
            + (content.visible.x as usize / 2)
            + row * v_plane.stride;
        buf[dst_v..dst_v + content_uv_w].copy_from_slice(&tile_v[v_src..v_src + content_uv_w]);
    }

    let handle = BufferHandle::from_host_bytes(avcodec::core::utils::next_buffer_id(), buf);
    let mut image =
        Image::new(ImageInfo::Yuv420p, cell_w, cell_h, handle).with_layout(SampleLayout::Planar);
    image.set_plane(
        0,
        ImagePlane {
            offset: 0,
            stride: cw,
            len: y_len,
        },
    );
    image.set_plane(
        1,
        ImagePlane {
            offset: y_len,
            stride: uv_stride,
            len: uv_len,
        },
    );
    image.set_plane(
        2,
        ImagePlane {
            offset: y_len + uv_len,
            stride: uv_stride,
            len: uv_len,
        },
    );
    Ok(image)
}

fn drain_encoder(mosaicker: &mut VideoMosaicker, out: &mut Vec<AVFrame>) -> Result<()> {
    loop {
        match mosaicker.encoder.poll_packet() {
            Ok(Poll::Ready(packet)) => {
                let mut frame = av_frame_from_packet(
                    TrackId(0),
                    mosaicker.output_codec,
                    mosaicker.output_frame_format,
                    mosaicker.output_timebase,
                    &packet,
                )?;
                frame.origin = FrameOrigin::Generated;
                let _ = frame.set_duration(mosaicker.output_frame_duration);
                if mosaicker.output_frame_format == FrameFormat::CanonicalH26x {
                    mosaicker
                        .output_param_cache
                        .update_from_annexb(mosaicker.output_codec, &frame.payload);
                    mosaicker.output_track.extradata = mosaicker
                        .output_param_cache
                        .extradata_for_codec(mosaicker.output_codec)
                        .unwrap_or(cheetah_codec::CodecExtradata::None);
                }
                out.push(frame);
            }
            Ok(Poll::Pending) => break,
            Ok(Poll::EndOfStream) => break,
            Err(e) => {
                return Err(MediaError::invalid_argument(format!(
                    "poll mosaic packet: {e}"
                )))
            }
        }
    }
    Ok(())
}

const STRIDE_ALIGNMENT: usize = 64;

fn align_up(n: usize, a: usize) -> usize {
    n.div_ceil(a) * a
}

fn allocate_canvas_buffer(width: u32, height: u32) -> (Vec<u8>, usize, usize) {
    let w = width as usize;
    let h = height as usize;
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let y_stride = align_up(w, STRIDE_ALIGNMENT);
    let uv_stride = align_up(cw, STRIDE_ALIGNMENT);
    let y_size = y_stride * h;
    let uv_size = uv_stride * ch;
    let total = y_size + uv_size * 2;
    (vec![0; total], y_stride, uv_stride)
}

fn fill_yuv420p_black(buf: &mut [u8], width: u32, height: u32) {
    let w = width as usize;
    let h = height as usize;
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let y_stride = align_up(w, STRIDE_ALIGNMENT);
    let uv_stride = align_up(cw, STRIDE_ALIGNMENT);
    let y_size = y_stride * h;
    let uv_size = uv_stride * ch;
    buf[..y_size].fill(0);
    buf[y_size..y_size + uv_size].fill(128);
    buf[y_size + uv_size..y_size + uv_size * 2].fill(128);
}

fn build_yuv420p_image(
    buf: Vec<u8>,
    width: u32,
    height: u32,
    y_stride: usize,
    uv_stride: usize,
) -> Image {
    let h = height as usize;
    let ch = h.div_ceil(2);
    let y_size = y_stride * h;
    let uv_size = uv_stride * ch;
    let handle = BufferHandle::from_host_bytes(avcodec::core::utils::next_buffer_id(), buf);
    let mut image = Image::new(ImageInfo::Yuv420p, width, height, handle);
    image.layout = SampleLayout::Planar;
    image.set_plane(
        0,
        ImagePlane {
            offset: 0,
            stride: y_stride,
            len: y_size,
        },
    );
    image.set_plane(
        1,
        ImagePlane {
            offset: y_size,
            stride: uv_stride,
            len: uv_size,
        },
    );
    image.set_plane(
        2,
        ImagePlane {
            offset: y_size + uv_size,
            stride: uv_stride,
            len: uv_size,
        },
    );
    image
}

#[allow(clippy::too_many_arguments)]
fn composite_tile(
    canvas_buf: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    y_stride: usize,
    uv_stride: usize,
    tile: &Image,
    cell: &MosaicCell,
    cell_width: u32,
    cell_height: u32,
) -> avcodec::core::AvResult<()> {
    let dst_x = cell.column * cell_width;
    let dst_y = cell.row * cell_height;
    if dst_x + cell_width > canvas_width || dst_y + cell_height > canvas_height {
        return Ok(());
    }

    let visible = tile.visible;
    let copy_w = visible.width.min(cell_width);
    let copy_h = visible.height.min(cell_height);
    if copy_w == 0 || copy_h == 0 {
        return Ok(());
    }

    let tile_y = tile
        .plane_host_bytes(0)?
        .ok_or(avcodec::core::AvError::InvalidArgument)?;
    let tile_u = tile
        .plane_host_bytes(1)?
        .ok_or(avcodec::core::AvError::InvalidArgument)?;
    let tile_v = tile
        .plane_host_bytes(2)?
        .ok_or(avcodec::core::AvError::InvalidArgument)?;

    let y_plane = tile.planes[0].ok_or(avcodec::core::AvError::InvalidArgument)?;
    let u_plane = tile.planes[1].ok_or(avcodec::core::AvError::InvalidArgument)?;
    let v_plane = tile.planes[2].ok_or(avcodec::core::AvError::InvalidArgument)?;

    let y_src_start = visible.y as usize * y_plane.stride + visible.x as usize;
    let y_dst_start = dst_y as usize * y_stride + dst_x as usize;
    for row in 0..copy_h as usize {
        let src = y_src_start + row * y_plane.stride;
        let dst = y_dst_start + row * y_stride;
        canvas_buf[dst..dst + copy_w as usize].copy_from_slice(&tile_y[src..src + copy_w as usize]);
    }

    let copy_cw = (copy_w as usize).div_ceil(2);
    let copy_ch = (copy_h as usize).div_ceil(2);
    let canvas_h = canvas_height as usize;
    let ch = canvas_h.div_ceil(2);
    let y_size = y_stride * canvas_h;
    let uv_size = uv_stride * ch;

    let u_src_start = (visible.y as usize / 2) * u_plane.stride + (visible.x as usize / 2);
    let u_dst_start = y_size + (dst_y as usize / 2) * uv_stride + (dst_x as usize / 2);
    for row in 0..copy_ch {
        let src = u_src_start + row * u_plane.stride;
        let dst = u_dst_start + row * uv_stride;
        canvas_buf[dst..dst + copy_cw].copy_from_slice(&tile_u[src..src + copy_cw]);
    }

    let v_src_start = (visible.y as usize / 2) * v_plane.stride + (visible.x as usize / 2);
    let v_dst_start = y_size + uv_size + (dst_y as usize / 2) * uv_stride + (dst_x as usize / 2);
    for row in 0..copy_ch {
        let src = v_src_start + row * v_plane.stride;
        let dst = v_dst_start + row * uv_stride;
        canvas_buf[dst..dst + copy_cw].copy_from_slice(&tile_v[src..src + copy_cw]);
    }

    Ok(())
}

fn resolve_output_fps(layout: &MosaicLayout) -> Rational32 {
    let num = layout.frame_rate_num.unwrap_or(30);
    let den = layout.frame_rate_den.unwrap_or(1);
    if num == 0 || den == 0 {
        Rational32::new(30, 1)
    } else {
        Rational32::new(num, den)
    }
}

fn video_codec_to_cheetah(codec: VideoCodec) -> CheetahCodecId {
    match codec {
        VideoCodec::H264 => CheetahCodecId::H264,
        VideoCodec::H265 => CheetahCodecId::H265,
        VideoCodec::MJPEG => CheetahCodecId::MJPEG,
    }
}

fn map_mosaic_fit(fit: MosaicFit) -> FitMode {
    match fit {
        MosaicFit::Contain => FitMode::Contain,
        MosaicFit::Cover => FitMode::Cover,
        MosaicFit::Stretch => FitMode::Stretch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avcodec::core::{BitstreamFormat, CodecId as AvCodecId, Packet, Poll, TimeBase};
    use avcodec::{VideoDecoderRequest, VideoProfile, VideoSdk};

    #[test]
    fn mosaicker_rejects_too_few_sources() {
        let config = MediaProcessingModuleConfig {
            profile: "software".to_string(),
            ..Default::default()
        };
        let layout = MosaicLayout {
            columns: 1,
            rows: 1,
            cell_width: 320,
            cell_height: 240,
            background: None,
            frame_rate_num: None,
            frame_rate_den: None,
            bit_rate: None,
            gop_size: None,
            video_codec: None,
            fit: None,
        };
        let input = VideoMosaicInput {
            source: cheetah_media_api::ids::MediaKey::new("_", "app", "s1", None).unwrap(),
            cell: MosaicCell {
                column: 0,
                row: 0,
                z_order: 0,
            },
            audio_gain_db: None,
            fit: None,
            label: None,
        };
        let track = TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30);
        let result = VideoMosaicker::new(&config, &[input], &layout, &[track]);
        assert!(result.is_err());
    }

    #[test]
    fn mosaicker_rejects_odd_cell_dimensions() {
        let config = MediaProcessingModuleConfig {
            profile: "software".to_string(),
            ..Default::default()
        };
        let layout = MosaicLayout {
            columns: 2,
            rows: 1,
            cell_width: 321,
            cell_height: 240,
            background: None,
            frame_rate_num: None,
            frame_rate_den: None,
            bit_rate: None,
            gop_size: None,
            video_codec: None,
            fit: None,
        };
        let inputs = vec![
            VideoMosaicInput {
                source: cheetah_media_api::ids::MediaKey::new("_", "app", "s1", None).unwrap(),
                cell: MosaicCell {
                    column: 0,
                    row: 0,
                    z_order: 0,
                },
                audio_gain_db: None,
                fit: None,
                label: None,
            },
            VideoMosaicInput {
                source: cheetah_media_api::ids::MediaKey::new("_", "app", "s2", None).unwrap(),
                cell: MosaicCell {
                    column: 1,
                    row: 0,
                    z_order: 0,
                },
                audio_gain_db: None,
                fit: None,
                label: None,
            },
        ];
        let tracks = vec![
            TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30),
            TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30),
        ];
        let result = VideoMosaicker::new(&config, &inputs, &layout, &tracks);
        assert!(result.is_err());
    }

    #[test]
    #[cfg(feature = "media-processing-cpu")]
    fn mosaicker_produces_h264_output_from_black_canvas() {
        let config = MediaProcessingModuleConfig {
            profile: "software".to_string(),
            ..Default::default()
        };
        let layout = MosaicLayout {
            columns: 2,
            rows: 1,
            cell_width: 320,
            cell_height: 240,
            background: None,
            frame_rate_num: None,
            frame_rate_den: None,
            bit_rate: None,
            gop_size: None,
            video_codec: None,
            fit: None,
        };
        let inputs = vec![
            VideoMosaicInput {
                source: cheetah_media_api::ids::MediaKey::new("_", "app", "s1", None).unwrap(),
                cell: MosaicCell {
                    column: 0,
                    row: 0,
                    z_order: 0,
                },
                audio_gain_db: None,
                fit: None,
                label: None,
            },
            VideoMosaicInput {
                source: cheetah_media_api::ids::MediaKey::new("_", "app", "s2", None).unwrap(),
                cell: MosaicCell {
                    column: 1,
                    row: 0,
                    z_order: 0,
                },
                audio_gain_db: None,
                fit: None,
                label: None,
            },
        ];
        let mut tracks = vec![
            TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30),
            TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30),
        ];
        for track in &mut tracks {
            track.readiness = TrackReadiness::Ready;
            track.width = Some(640);
            track.height = Some(480);
            track.fps = Some(Rational32::new(30, 1));
        }

        let mut mosaicker = VideoMosaicker::new(&config, &inputs, &layout, &tracks).unwrap();
        let mut all_frames = Vec::new();
        for _ in 0..3 {
            all_frames.extend(mosaicker.tick().unwrap());
        }
        all_frames.extend(mosaicker.flush().unwrap());

        assert!(
            !all_frames.is_empty(),
            "mosaic should produce at least one encoded frame"
        );
        for frame in &all_frames {
            assert_eq!(frame.codec, CheetahCodecId::H264);
            assert_eq!(frame.media_kind, MediaKind::Video);
            assert_eq!(frame.format, FrameFormat::CanonicalH26x);
        }
    }

    #[test]
    #[cfg(feature = "media-processing-cpu")]
    fn mosaicker_output_decodes_back_to_image() {
        let config = MediaProcessingModuleConfig {
            profile: "software".to_string(),
            ..Default::default()
        };
        let layout = MosaicLayout {
            columns: 1,
            rows: 2,
            cell_width: 160,
            cell_height: 120,
            background: None,
            frame_rate_num: Some(30),
            frame_rate_den: Some(1),
            bit_rate: None,
            gop_size: None,
            video_codec: None,
            fit: None,
        };
        let inputs = vec![
            VideoMosaicInput {
                source: cheetah_media_api::ids::MediaKey::new("_", "app", "s1", None).unwrap(),
                cell: MosaicCell {
                    column: 0,
                    row: 0,
                    z_order: 0,
                },
                audio_gain_db: None,
                fit: None,
                label: None,
            },
            VideoMosaicInput {
                source: cheetah_media_api::ids::MediaKey::new("_", "app", "s2", None).unwrap(),
                cell: MosaicCell {
                    column: 0,
                    row: 1,
                    z_order: 0,
                },
                audio_gain_db: None,
                fit: None,
                label: None,
            },
        ];
        let mut tracks = vec![
            TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30),
            TrackInfo::new(TrackId(0), MediaKind::Video, CheetahCodecId::H264, 30),
        ];
        for track in &mut tracks {
            track.readiness = TrackReadiness::Ready;
            track.width = Some(160);
            track.height = Some(120);
            track.fps = Some(Rational32::new(30, 1));
        }

        let mut mosaicker = VideoMosaicker::new(&config, &inputs, &layout, &tracks).unwrap();
        let mut all_frames = Vec::new();
        for _ in 0..3 {
            all_frames.extend(mosaicker.tick().unwrap());
        }
        all_frames.extend(mosaicker.flush().unwrap());

        assert!(
            !all_frames.is_empty(),
            "mosaic should produce encoded frames"
        );

        let sdk = VideoSdk::new().expect("video sdk");
        let mut decoder = sdk
            .create_decoder(
                VideoProfile::Software,
                VideoDecoderRequest::new(AvCodecId::H264, TimeBase::new(1, 30)).unwrap(),
            )
            .expect("create h264 decoder")
            .into_session();

        for frame in &all_frames {
            let mut packet = Packet::from_host_bytes(
                avcodec::core::utils::next_buffer_id(),
                AvCodecId::H264,
                BitstreamFormat::H264AnnexB,
                frame.payload.to_vec(),
            );
            packet.pts = Some(frame.pts);
            packet.dts = Some(frame.dts);
            packet.time_base = Some(TimeBase::new(frame.timebase.num, frame.timebase.den));
            decoder.submit_packet(packet).expect("submit mosaic packet");

            for _ in 0..5 {
                match decoder.poll_image().expect("poll decoded image") {
                    Poll::Ready(img) => {
                        assert_eq!(img.format, ImageInfo::Yuv420p);
                        assert_eq!(img.visible.width, mosaicker.output_width);
                        assert_eq!(img.visible.height, mosaicker.output_height);
                        return;
                    }
                    Poll::Pending => {}
                    Poll::EndOfStream => break,
                }
            }
        }

        panic!("did not decode any mosaic output frame");
    }
}
