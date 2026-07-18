//! Video transcoding adapter backed by `avcodec-rs`.
//!
//! Only compiled when `media-processing-video` is enabled.

use bytes::Bytes;
use cheetah_codec::{
    video_payload_is_random_access, AVFrame, CodecExtradata, CodecId, FrameFlags, FrameFormat,
    MediaKind, ParameterSetCache, Rational32, Timebase, TrackId, TrackInfo, TrackReadiness,
};
use cheetah_media_api::{error::Result, MediaError};

use crate::config::MediaProcessingModuleConfig;
use crate::provider::avcodec_registry::build_registry;

/// One-shot video transcode result: zero or more output frames plus the
/// updated track description.
#[derive(Debug, Clone)]
pub struct VideoTranscodeResult {
    pub frames: Vec<AVFrame>,
    pub track: TrackInfo,
}

/// Output specification for a one-shot video transcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoTranscodeSpec {
    /// Target codec. Supported: H.264, H.265 and MJPEG.
    pub codec: CodecId,
    /// Target width in pixels. `0` means "keep the source width".
    pub width: u32,
    /// Target height in pixels. `0` means "keep the source height".
    pub height: u32,
    /// Target frame rate as a rational number. `den == 0` falls back to the
    /// source frame rate or 30 fps.
    pub frame_rate: Rational32,
    /// Target bitrate in bits per second.
    pub bitrate: u32,
}

impl VideoTranscodeSpec {
    /// Creates a spec that preserves source geometry and frame rate.
    pub fn new(codec: CodecId) -> Self {
        Self {
            codec,
            width: 0,
            height: 0,
            frame_rate: Rational32::new(0, 0),
            bitrate: 0,
        }
    }

    /// Sets explicit output dimensions and returns self.
    pub const fn with_dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Sets the target frame rate and returns self.
    pub const fn with_frame_rate(mut self, num: u32, den: u32) -> Self {
        self.frame_rate = Rational32::new(num, den);
        self
    }

    /// Sets the target bitrate and returns self.
    pub const fn with_bitrate(mut self, bitrate: u32) -> Self {
        self.bitrate = bitrate;
        self
    }
}

/// Transcodes a single compressed video `AVFrame` to the requested codec and
/// dimensions.
///
/// This is a stateless, one-shot helper intended for unit tests and for
/// building higher-level streaming sessions. It creates a fresh decoder +
/// encoder pair, submits the input packet, flushes, and returns all produced
/// output packets.
pub fn transcode_video_frame(
    input: &AVFrame,
    input_track: &TrackInfo,
    spec: &VideoTranscodeSpec,
    config: &MediaProcessingModuleConfig,
) -> Result<VideoTranscodeResult> {
    if input.media_kind != MediaKind::Video {
        return Err(MediaError::invalid_argument("input frame is not video"));
    }
    if input_track.media_kind != MediaKind::Video {
        return Err(MediaError::invalid_argument("input track is not video"));
    }
    if !video_payload_is_random_access(input.codec, input.format, &input.payload) {
        return Err(MediaError::invalid_argument(
            "input video frame is not a random access point",
        ));
    }

    let (src_av_codec, src_bitstream) =
        map_input_format(input.format, input.codec).ok_or_else(|| {
            MediaError::unsupported(format!(
                "unsupported input video codec/format: {input_codec:?}/{:?}",
                input.format,
                input_codec = input.codec
            ))
        })?;

    let (dst_av_codec, dst_frame_format, _dst_bitstream, dst_pixel_format) =
        map_output_codec(spec.codec).ok_or_else(|| {
            MediaError::unsupported(format!(
                "unsupported output video codec: {codec:?}",
                codec = spec.codec
            ))
        })?;

    let output_width = resolve_dimension(spec.width, input_track.width)?;
    let output_height = resolve_dimension(spec.height, input_track.height)?;
    let output_fps = resolve_frame_rate(spec.frame_rate, input_track.fps);
    let output_bitrate = resolve_bitrate(spec.bitrate, spec.codec, output_width, output_height);

    let registry = build_registry(config)?;

    let src_time_base = avcodec::core::TimeBase::new(input.timebase.num, input.timebase.den);
    let decoder_cfg = avcodec::core::DecoderConfig::new(src_av_codec, src_time_base)
        .with_allow_staging(false)
        .with_memory_domain(avcodec::core::MemoryDomain::Host);

    let dst_time_base = avcodec::core::TimeBase::new(output_fps.den, output_fps.num);
    let encoder_cfg = avcodec::core::EncoderConfig::new(
        dst_av_codec,
        output_width,
        output_height,
        dst_pixel_format,
        dst_time_base,
        output_bitrate,
    )
    .with_allow_staging(false)
    .with_memory_domain(avcodec::core::MemoryDomain::Host);

    let mut transcoder = avcodec::VideoTranscoder::new(&registry, &decoder_cfg, &encoder_cfg)
        .map_err(|e| MediaError::unsupported(format!("create video transcoder: {e}")))?;

    let mut packet = avcodec::core::Packet::from_host_bytes(
        avcodec::core::utils::next_buffer_id(),
        src_av_codec,
        src_bitstream,
        input.payload.to_vec(),
    );
    packet.pts = Some(input.pts);
    packet.dts = Some(input.dts);
    packet.time_base = Some(src_time_base);
    if input.duration > 0 {
        packet.duration = Some(input.duration);
    }

    transcoder
        .submit_packet(packet)
        .map_err(|e| MediaError::invalid_argument(format!("submit video packet: {e}")))?;

    let mut output_frames = Vec::new();
    let mut param_cache = ParameterSetCache::default();
    let output_timebase = Timebase::new(output_fps.den, output_fps.num);

    drain_transcoder(
        &mut transcoder,
        input.track_id,
        spec.codec,
        dst_frame_format,
        output_timebase,
        &mut output_frames,
    )?;

    transcoder
        .flush()
        .map_err(|e| MediaError::invalid_argument(format!("flush video transcoder: {e}")))?;

    drain_transcoder(
        &mut transcoder,
        input.track_id,
        spec.codec,
        dst_frame_format,
        output_timebase,
        &mut output_frames,
    )?;

    for frame in &output_frames {
        if matches!(frame.format, FrameFormat::CanonicalH26x) {
            param_cache.update_from_annexb(spec.codec, &frame.payload);
        }
    }

    let mut output_track = TrackInfo::new(
        input.track_id,
        MediaKind::Video,
        spec.codec,
        input_track.clock_rate,
    );
    output_track.width = Some(output_width);
    output_track.height = Some(output_height);
    output_track.fps = Some(output_fps);
    output_track.bitrate = Some(output_bitrate);
    output_track.readiness = TrackReadiness::Ready;
    output_track.extradata = param_cache
        .extradata_for_codec(spec.codec)
        .unwrap_or(CodecExtradata::None);

    Ok(VideoTranscodeResult {
        frames: output_frames,
        track: output_track,
    })
}

/// Stateful streaming video transcode session.
///
/// Created from the first source keyframe and then fed successive compressed
/// video frames. Output frames are produced on `submit` and `flush`.
///
/// 有状态的流式视频转码 session。从第一个源关键帧创建，之后逐帧送入压缩
/// 视频帧，通过 `submit` 与 `flush` 产出输出帧。
pub struct VideoTranscodeSession {
    config: MediaProcessingModuleConfig,
    source_track: TrackInfo,
    transcoder: Option<avcodec::VideoTranscoder>,
    param_cache: ParameterSetCache,
    output_timebase: Timebase,
    output_frame_format: FrameFormat,
    output_codec: CodecId,
    output_track: TrackInfo,
}

impl VideoTranscodeSession {
    /// Create a new streaming transcode session for a source video track.
    ///
    /// `input_track` must describe the source video (codec, dimensions, fps).
    /// The actual transcoder is built lazily on the first `submit` so that the
    /// source frame timebase is available to the decoder.
    pub fn new(
        input_track: &TrackInfo,
        spec: &VideoTranscodeSpec,
        config: &MediaProcessingModuleConfig,
    ) -> Result<Self> {
        if input_track.media_kind != MediaKind::Video {
            return Err(MediaError::invalid_argument("input track is not video"));
        }

        let (_, dst_frame_format, _, _) = map_output_codec(spec.codec).ok_or_else(|| {
            MediaError::unsupported(format!(
                "unsupported output video codec: {codec:?}",
                codec = spec.codec
            ))
        })?;

        let output_width = resolve_dimension(spec.width, input_track.width)?;
        let output_height = resolve_dimension(spec.height, input_track.height)?;
        let output_fps = resolve_frame_rate(spec.frame_rate, input_track.fps);
        let output_bitrate = resolve_bitrate(spec.bitrate, spec.codec, output_width, output_height);

        let mut output_track = TrackInfo::new(
            input_track.track_id,
            MediaKind::Video,
            spec.codec,
            input_track.clock_rate,
        );
        output_track.width = Some(output_width);
        output_track.height = Some(output_height);
        output_track.fps = Some(output_fps);
        output_track.bitrate = Some(output_bitrate);
        output_track.readiness = TrackReadiness::Ready;

        Ok(Self {
            config: config.clone(),
            source_track: input_track.clone(),
            transcoder: None,
            param_cache: ParameterSetCache::default(),
            output_timebase: Timebase::new(output_fps.den, output_fps.num),
            output_frame_format: dst_frame_format,
            output_codec: spec.codec,
            output_track,
        })
    }

    /// Submit one compressed source frame and return all produced output frames.
    ///
    /// The first call must be a random-access point; subsequent delta frames are
    /// decoded using the session's internal decoder state.
    pub fn submit(&mut self, input: &AVFrame) -> Result<Vec<AVFrame>> {
        if input.media_kind != MediaKind::Video {
            return Err(MediaError::invalid_argument("input frame is not video"));
        }
        if self.transcoder.is_none() {
            self.transcoder = Some(self.build_transcoder(input)?);
        }
        let transcoder = self.transcoder.as_mut().unwrap();

        let (src_av_codec, src_bitstream) = map_input_format(input.format, input.codec)
            .ok_or_else(|| {
                MediaError::unsupported(format!(
                    "unsupported input video codec/format: {input_codec:?}/{:?}",
                    input.format,
                    input_codec = input.codec
                ))
            })?;

        let mut packet = avcodec::core::Packet::from_host_bytes(
            avcodec::core::utils::next_buffer_id(),
            src_av_codec,
            src_bitstream,
            input.payload.to_vec(),
        );
        packet.pts = Some(input.pts);
        packet.dts = Some(input.dts);
        packet.time_base = Some(avcodec::core::TimeBase::new(
            input.timebase.num,
            input.timebase.den,
        ));
        if input.duration > 0 {
            packet.duration = Some(input.duration);
        }

        transcoder
            .submit_packet(packet)
            .map_err(|e| MediaError::invalid_argument(format!("submit video packet: {e}")))?;

        let mut output_frames = Vec::new();
        drain_transcoder(
            transcoder,
            input.track_id,
            self.output_codec,
            self.output_frame_format,
            self.output_timebase,
            &mut output_frames,
        )?;

        self.update_track_extradata(&output_frames);
        Ok(output_frames)
    }

    /// Flush the encoder/decoder, return any remaining output frames, and reset
    /// the underlying transcoder so it can accept the next group of pictures.
    pub fn flush(&mut self) -> Result<Vec<AVFrame>> {
        let Some(transcoder) = self.transcoder.as_mut() else {
            return Ok(Vec::new());
        };
        transcoder
            .flush()
            .map_err(|e| MediaError::invalid_argument(format!("flush video transcoder: {e}")))?;

        let mut output_frames = Vec::new();
        drain_transcoder(
            transcoder,
            self.source_track.track_id,
            self.output_codec,
            self.output_frame_format,
            self.output_timebase,
            &mut output_frames,
        )?;

        transcoder
            .reset()
            .map_err(|e| MediaError::invalid_argument(format!("reset video transcoder: {e}")))?;

        self.update_track_extradata(&output_frames);
        Ok(output_frames)
    }

    /// Return the current output track description.
    pub fn output_track(&self) -> &TrackInfo {
        &self.output_track
    }

    fn build_transcoder(&self, input: &AVFrame) -> Result<avcodec::VideoTranscoder> {
        let (src_av_codec, _) = map_input_format(input.format, input.codec).ok_or_else(|| {
            MediaError::unsupported(format!(
                "unsupported source video codec/format: {:?}/{:?}",
                input.codec, input.format
            ))
        })?;

        let (dst_av_codec, _dst_frame_format, _dst_bitstream, dst_pixel_format) =
            map_output_codec(self.output_codec).ok_or_else(|| {
                MediaError::unsupported(format!(
                    "unsupported output video codec: {codec:?}",
                    codec = self.output_codec
                ))
            })?;

        let registry = build_registry(&self.config)?;

        let src_time_base = avcodec::core::TimeBase::new(input.timebase.num, input.timebase.den);
        let decoder_cfg = avcodec::core::DecoderConfig::new(src_av_codec, src_time_base)
            .with_allow_staging(false)
            .with_memory_domain(avcodec::core::MemoryDomain::Host);

        let dst_time_base =
            avcodec::core::TimeBase::new(self.output_timebase.num, self.output_timebase.den);
        let encoder_cfg = avcodec::core::EncoderConfig::new(
            dst_av_codec,
            self.output_track.width.unwrap_or(64),
            self.output_track.height.unwrap_or(64),
            dst_pixel_format,
            dst_time_base,
            self.output_track.bitrate.unwrap_or(0),
        )
        .with_allow_staging(false)
        .with_memory_domain(avcodec::core::MemoryDomain::Host);

        avcodec::VideoTranscoder::new(&registry, &decoder_cfg, &encoder_cfg)
            .map_err(|e| MediaError::unsupported(format!("create video transcoder: {e}")))
    }

    fn update_track_extradata(&mut self, frames: &[AVFrame]) {
        for frame in frames {
            if matches!(frame.format, FrameFormat::CanonicalH26x) {
                self.param_cache
                    .update_from_annexb(self.output_codec, &frame.payload);
            }
        }
        self.output_track.extradata = self
            .param_cache
            .extradata_for_codec(self.output_codec)
            .unwrap_or(CodecExtradata::None);
    }
}

fn map_input_format(
    format: FrameFormat,
    codec: CodecId,
) -> Option<(avcodec::core::CodecId, avcodec::core::BitstreamFormat)> {
    match (format, codec) {
        (FrameFormat::CanonicalH26x, CodecId::H264) => Some((
            avcodec::core::CodecId::H264,
            avcodec::core::BitstreamFormat::H264AnnexB,
        )),
        (FrameFormat::CanonicalH26x, CodecId::H265) => Some((
            avcodec::core::CodecId::H265,
            avcodec::core::BitstreamFormat::H265AnnexB,
        )),
        (FrameFormat::MjpegFrame, CodecId::MJPEG) => Some((
            avcodec::core::CodecId::Mjpeg,
            avcodec::core::BitstreamFormat::JpegInterchange,
        )),
        _ => None,
    }
}

fn map_output_codec(
    codec: CodecId,
) -> Option<(
    avcodec::core::CodecId,
    FrameFormat,
    avcodec::core::BitstreamFormat,
    avcodec::core::ImageInfo,
)> {
    match codec {
        CodecId::H264 => Some((
            avcodec::core::CodecId::H264,
            FrameFormat::CanonicalH26x,
            avcodec::core::BitstreamFormat::H264AnnexB,
            avcodec::core::ImageInfo::Yuv420p,
        )),
        CodecId::H265 => Some((
            avcodec::core::CodecId::H265,
            FrameFormat::CanonicalH26x,
            avcodec::core::BitstreamFormat::H265AnnexB,
            avcodec::core::ImageInfo::Yuv420p,
        )),
        CodecId::MJPEG => Some((
            avcodec::core::CodecId::Mjpeg,
            FrameFormat::MjpegFrame,
            avcodec::core::BitstreamFormat::JpegInterchange,
            avcodec::core::ImageInfo::Rgb24,
        )),
        _ => None,
    }
}

fn resolve_dimension(spec_value: u32, track_value: Option<u32>) -> Result<u32> {
    if spec_value != 0 {
        Ok(spec_value)
    } else {
        track_value.ok_or_else(|| MediaError::invalid_argument("missing source video dimension"))
    }
}

fn resolve_frame_rate(spec_value: Rational32, track_value: Option<Rational32>) -> Rational32 {
    if spec_value.den != 0 {
        spec_value
    } else if let Some(fps) = track_value {
        fps
    } else {
        Rational32::new(30, 1)
    }
}

fn resolve_bitrate(spec_value: u32, codec: CodecId, width: u32, height: u32) -> u32 {
    if spec_value != 0 {
        spec_value
    } else {
        default_video_bitrate(codec, width, height)
    }
}

fn default_video_bitrate(codec: CodecId, width: u32, height: u32) -> u32 {
    let pixels = u64::from(width) * u64::from(height);
    let base = match codec {
        CodecId::MJPEG => pixels * 8,
        _ => pixels * 2,
    };
    base.clamp(128_000, 20_000_000) as u32
}

fn drain_transcoder(
    transcoder: &mut avcodec::VideoTranscoder,
    track_id: TrackId,
    codec: CodecId,
    frame_format: FrameFormat,
    timebase: Timebase,
    out: &mut Vec<AVFrame>,
) -> Result<()> {
    loop {
        match transcoder.poll_packet() {
            Ok(avcodec::core::Poll::Ready(packet)) => {
                out.push(av_frame_from_packet(
                    track_id,
                    codec,
                    frame_format,
                    timebase,
                    &packet,
                )?);
            }
            Ok(avcodec::core::Poll::Pending) => break,
            Ok(avcodec::core::Poll::EndOfStream) => break,
            Err(e) => {
                return Err(MediaError::invalid_argument(format!(
                    "poll video packet: {e}"
                )))
            }
        }
    }
    Ok(())
}

fn av_frame_from_packet(
    track_id: TrackId,
    codec: CodecId,
    frame_format: FrameFormat,
    timebase: Timebase,
    packet: &avcodec::core::Packet,
) -> Result<AVFrame> {
    let payload_bytes = packet
        .data
        .host_bytes()
        .map_err(|e| MediaError::internal(format!("read video packet payload: {e}")))?
        .ok_or_else(|| MediaError::internal("video packet payload not in host memory"))?;

    let pts = packet.pts.unwrap_or(0);
    let dts = packet.dts.unwrap_or(pts);

    let mut frame = AVFrame::new(
        track_id,
        MediaKind::Video,
        codec,
        frame_format,
        pts,
        dts,
        timebase,
        Bytes::copy_from_slice(payload_bytes),
    );
    if let Some(duration) = packet.duration {
        let _ = frame.set_duration(duration);
    }
    if packet.flags.contains(avcodec::core::PacketFlags::KEY) {
        frame.flags.insert(FrameFlags::KEY);
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use avcodec::core::{EncoderConfig, Image, ImageInfo, Poll, TimeBase};
    use bytes::Bytes;

    fn build_test_image(format: ImageInfo, width: u32, height: u32) -> Image {
        match format {
            ImageInfo::Yuv420p => {
                let w = width as usize;
                let h = height as usize;
                let cw = w.div_ceil(2);
                let ch = h.div_ceil(2);
                let y = vec![128u8; w * h];
                let u = vec![128u8; cw * ch];
                let v = vec![128u8; cw * ch];
                Image::from_host_i420(width, height, &y, w, &u, cw, &v, cw).expect("i420 image")
            }
            ImageInfo::Rgb24 => {
                let w = width as usize;
                let h = height as usize;
                let stride = w * 3;
                let bytes = vec![128u8; stride * h];
                Image::new_host_packed(ImageInfo::Rgb24, width, height, 0, stride, bytes, 0)
                    .expect("rgb24 image")
            }
            _ => panic!("unsupported test image format"),
        }
    }

    fn encode_input(codec: CodecId, width: u32, height: u32) -> (AVFrame, TrackInfo) {
        let registry = build_registry(&MediaProcessingModuleConfig::default()).unwrap();
        let av_codec = match codec {
            CodecId::H264 => avcodec::core::CodecId::H264,
            CodecId::H265 => avcodec::core::CodecId::H265,
            CodecId::MJPEG => avcodec::core::CodecId::Mjpeg,
            _ => panic!("unsupported test input codec"),
        };
        let pixel_format = match codec {
            CodecId::MJPEG => ImageInfo::Rgb24,
            _ => ImageInfo::Yuv420p,
        };
        let cfg = EncoderConfig::new(
            av_codec,
            width,
            height,
            pixel_format,
            TimeBase::new(1, 30),
            300_000,
        )
        .with_allow_staging(false)
        .with_memory_domain(avcodec::core::MemoryDomain::Host);

        let mut encoder = registry.create_encoder(&cfg).expect("create encoder");

        let mut image = build_test_image(pixel_format, width, height);
        image.pts = Some(0);
        image.dts = Some(0);
        encoder.submit_frame(image).expect("submit frame");

        let packet = loop {
            match encoder.poll_packet().expect("poll packet") {
                Poll::Ready(p) => break p,
                Poll::Pending => {}
                Poll::EndOfStream => panic!("encoder returned EOS before packet"),
            }
        };

        let payload = packet
            .data
            .host_bytes()
            .expect("host bytes")
            .expect("payload present");

        let frame_format = match codec {
            CodecId::MJPEG => FrameFormat::MjpegFrame,
            _ => FrameFormat::CanonicalH26x,
        };
        let mut frame = AVFrame::new(
            TrackId(0),
            MediaKind::Video,
            codec,
            frame_format,
            0,
            0,
            Timebase::new(1, 30),
            Bytes::copy_from_slice(payload),
        );
        frame.flags.insert(FrameFlags::KEY);

        let mut track = TrackInfo::new(TrackId(0), MediaKind::Video, codec, 90_000);
        track.width = Some(width);
        track.height = Some(height);
        track.fps = Some(Rational32::new(30, 1));
        track.readiness = TrackReadiness::Ready;

        (frame, track)
    }

    fn assert_output(
        result: &VideoTranscodeResult,
        expected_codec: CodecId,
        expected_width: u32,
        expected_height: u32,
    ) {
        assert!(
            !result.frames.is_empty(),
            "transcoder produced no output frames"
        );
        for frame in &result.frames {
            assert_eq!(frame.codec, expected_codec);
        }
        assert_eq!(result.track.codec, expected_codec);
        assert_eq!(result.track.width, Some(expected_width));
        assert_eq!(result.track.height, Some(expected_height));
        assert_eq!(result.track.readiness, TrackReadiness::Ready);
    }

    #[test]
    fn h264_to_h264_same_dimensions() {
        let cfg = MediaProcessingModuleConfig::default();
        let (frame, track) = encode_input(CodecId::H264, 64, 64);
        let spec = VideoTranscodeSpec::new(CodecId::H264);

        let result = transcode_video_frame(&frame, &track, &spec, &cfg).expect("transcode");
        assert_output(&result, CodecId::H264, 64, 64);
        assert!(result.frames[0].is_key_frame());
    }

    #[test]
    fn h264_to_h265() {
        let cfg = MediaProcessingModuleConfig::default();
        let (frame, track) = encode_input(CodecId::H264, 64, 64);
        let spec = VideoTranscodeSpec::new(CodecId::H265);

        let result = transcode_video_frame(&frame, &track, &spec, &cfg).expect("transcode");
        assert_output(&result, CodecId::H265, 64, 64);
        assert!(result.frames[0].is_key_frame());
    }

    #[test]
    fn h265_to_h264() {
        let cfg = MediaProcessingModuleConfig::default();
        let (frame, track) = encode_input(CodecId::H265, 64, 64);
        let spec = VideoTranscodeSpec::new(CodecId::H264);

        let result = transcode_video_frame(&frame, &track, &spec, &cfg).expect("transcode");
        assert_output(&result, CodecId::H264, 64, 64);
        assert!(result.frames[0].is_key_frame());
    }

    #[test]
    fn mjpeg_to_h264() {
        let cfg = MediaProcessingModuleConfig::default();
        let (frame, track) = encode_input(CodecId::MJPEG, 64, 64);
        let spec = VideoTranscodeSpec::new(CodecId::H264);

        let result = transcode_video_frame(&frame, &track, &spec, &cfg).expect("transcode");
        assert_output(&result, CodecId::H264, 64, 64);
        assert!(result.frames[0].is_key_frame());
    }

    #[test]
    fn h264_to_mjpeg() {
        let cfg = MediaProcessingModuleConfig::default();
        let (frame, track) = encode_input(CodecId::H264, 64, 64);
        let spec = VideoTranscodeSpec::new(CodecId::MJPEG);

        let result = transcode_video_frame(&frame, &track, &spec, &cfg).expect("transcode");
        assert_output(&result, CodecId::MJPEG, 64, 64);
    }

    #[test]
    fn h264_rescale_to_32x32() {
        let cfg = MediaProcessingModuleConfig::default();
        let (frame, track) = encode_input(CodecId::H264, 64, 64);
        let spec = VideoTranscodeSpec::new(CodecId::H264).with_dimensions(32, 32);

        let result = transcode_video_frame(&frame, &track, &spec, &cfg).expect("transcode");
        assert_output(&result, CodecId::H264, 32, 32);
    }

    #[test]
    fn output_timebase_is_reciprocal_of_fps() {
        let cfg = MediaProcessingModuleConfig::default();
        let (frame, track) = encode_input(CodecId::H264, 64, 64);
        let spec = VideoTranscodeSpec::new(CodecId::H264);

        let result = transcode_video_frame(&frame, &track, &spec, &cfg).expect("transcode");
        assert_output(&result, CodecId::H264, 64, 64);
        // 30 fps -> time base 1/30 seconds per tick, not 30/1.
        assert_eq!(result.frames[0].timebase, Timebase::new(1, 30));
    }

    #[test]
    fn streaming_session_produces_video_output() {
        let cfg = MediaProcessingModuleConfig::default();
        let (frame1, track) = encode_input(CodecId::H264, 64, 64);
        let spec = VideoTranscodeSpec::new(CodecId::H264);

        let mut session = VideoTranscodeSession::new(&track, &spec, &cfg)
            .expect("create video transcode session");

        // The encoder may buffer frames; flush the GOP to get output, then
        // continue with the next GOP after reset.
        let mut frame2 = frame1.clone();
        frame2.pts = 1;
        frame2.dts = 1;
        frame2.pts_us = 33_333;
        frame2.dts_us = 33_333;

        session.submit(&frame1).expect("submit first keyframe");
        let mut total = session.flush().expect("flush first GOP");
        assert!(
            !total.is_empty(),
            "flush should produce output for the first GOP"
        );

        total.extend(session.submit(&frame2).expect("submit second keyframe"));
        total.extend(session.flush().expect("flush second GOP"));

        assert!(!total.is_empty());
        assert!(total.iter().any(|f| f.is_key_frame()));
        assert_eq!(total[0].timebase, Timebase::new(1, 30));
        assert_eq!(session.output_track().codec, CodecId::H264);
        assert_eq!(session.output_track().width, Some(64));
        assert_eq!(session.output_track().height, Some(64));
    }
}
