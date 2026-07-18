//! Per-stream HLS muxer: receives AVFrames, produces TS/fMP4 segments, manages playlist.
//!
//! Switches to fMP4 for AV1/VP9, supports LL-HLS part generation, and maintains a
//! ring buffer of segments, cached playlists, and DVR rewind metadata.
//!
//! 每个流的 HLS 复用器：接收 AVFrame，生成 TS/fMP4 分段，并管理播放列表。
//!
//! 在检测到 AV1/VP9 时自动切换为 fMP4，支持 LL-HLS 分片生成，并维护分段环形缓冲区、
//! 缓存播放列表及 DVR 回退元数据。
//!

use std::collections::VecDeque;

use bytes::Bytes;
use cheetah_codec::{
    aac_channel_count_from_asc, adts_wrap, h26x_length_prefixed_from_payload, subtitle::WebVttCue,
    AVFrame, AacAudioSpecificConfig, CodecExtradata, CodecId, FrameFlags, MediaKind, TrackInfo,
};
use cheetah_hls_core::{
    Fmp4Muxer, Fmp4Sample, Fmp4TrackDesc, HlsContainer, HlsCoreError, HlsPart, LlHlsPackagingMode,
    LowLatencyState, PlaylistBuilder, SegmentRing, TrackLane, TsMuxer, VttMux, VttMuxConfig,
    VttSegment,
};

use crate::demuxed_muxer::{DemuxedMuxerConfig, DemuxedStreamMuxer};

/// Configuration for the per-stream HLS muxer.
///
/// Drives segment/part timing, container selection, LL-HLS packaging, and
/// publisher-ready behavior.
///
/// 每个流 HLS 复用器的配置。
///
/// 控制分段/分片时序、容器选择、LL-HLS 封装以及发布就绪行为。
#[derive(Debug, Clone)]
pub struct StreamMuxerConfig {
    pub segment_duration_ms: u64,
    pub segment_count: usize,
    pub ready_threshold: usize,
    pub force_segment_after_ms: u64,
    pub fast_register: bool,
    pub container: HlsContainer,
    pub ll_hls_enabled: bool,
    pub part_target_ms: u64,
    pub max_completed_segments: usize,
    pub ll_hls_packaging_mode: LlHlsPackagingMode,
    #[allow(dead_code)]
    pub origin_mode: bool,
    #[allow(dead_code)]
    pub stream_name: String,
    /// Optional WebVTT subtitle muxer configuration.
    ///
    /// 可选的 WebVTT 字幕复用器配置。
    pub vtt_config: Option<VttMuxConfig>,
}

/// Lightweight segment metadata kept for DVR/rewind playlist generation.
///
/// 为 DVR/回退播放列表生成保存的轻量分段元数据。
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SegmentMeta {
    pub name: String,
    pub duration_secs: f64,
    pub sequence: u64,
    pub program_date_time_ms: Option<i64>,
    pub markers: Vec<cheetah_hls_core::CueMarker>,
}

/// Output event produced by the stream muxer.
///
/// Either a completed segment, a finalized LL-HLS part, or a new WebVTT
/// subtitle segment.
///
/// 流复用器产生的事件。
///
/// 一个已完成的分段、最终化的 LL-HLS 分片或新的 WebVTT 字幕分段。
#[derive(Debug, Clone)]
#[allow(dead_code)]
#[allow(clippy::enum_variant_names)]
pub enum MuxerOutput {
    SegmentReady {
        name: String,
        duration_secs: f64,
        data: Bytes,
    },
    PartReady(HlsPart),
    VttSegmentReady(VttSegment),
}

/// Per-stream HLS muxer state.
///
/// Owns either a TS or fMP4 muxer, optional LL-HLS state, a segment ring, a
/// demuxed sub-muxer, and an optional WebVTT subtitle muxer. It normalizes AAC
/// and H.26x payload formats and keeps rewind metadata for DVR playlists.
///
/// 每个流的 HLS 复用器状态。
///
/// 拥有 TS 或 fMP4 复用器、可选 LL-HLS 状态、分段环和可选 demuxed 子复用器。
/// 它归一化 AAC 和 H.26x 负载格式，并保留 DVR 播放列表的回退元数据。
pub struct StreamMuxer {
    config: StreamMuxerConfig,
    ts_muxer: Option<TsMuxer>,
    fmp4_muxer: Option<Fmp4Muxer>,
    fmp4_init: Option<Bytes>,
    pending_fmp4_samples: Vec<Fmp4Sample>,
    pending_part_samples: Vec<Fmp4Sample>,
    pending_segment_part_data: Vec<Bytes>,
    ring: SegmentRing,
    ll_state: Option<LowLatencyState>,
    video_codec: CodecId,
    audio_codec: CodecId,
    has_video: bool,
    has_audio: bool,
    aac_config: Option<AacAudioSpecificConfig>,
    parameter_sets: Option<Bytes>,
    video_extradata: Bytes,
    audio_extradata: Bytes,
    video_width: u16,
    video_height: u16,
    audio_sample_rate: u32,
    audio_channels: u8,
    segment_start_dts: Option<u64>,
    segment_last_dts: u64,
    last_video_frame_interval_us: Option<u64>,
    prev_video_dts_us: Option<u64>,
    segment_has_keyframe: bool,
    segment_seq: u64,
    ready: bool,
    pub enabled: bool,
    cached_playlist: Option<String>,
    cached_playlist_gzip: Option<Bytes>,
    concluded: bool,
    wallclock_offset_ms: Option<i64>,
    stream_key: String,
    rewind_history: Vec<SegmentMeta>,
    pending_markers: Vec<cheetah_hls_core::CueMarker>,
    demuxed: Option<DemuxedStreamMuxer>,
    vtt_mux: Option<VttMux>,
    vtt_ready: bool,
    vtt_segments: VecDeque<VttSegment>,
    pending_vtt_outputs: Vec<MuxerOutput>,
}

#[allow(dead_code)]
impl StreamMuxer {
    pub fn new(config: StreamMuxerConfig) -> Self {
        let ll_state = if config.ll_hls_enabled && config.container == HlsContainer::Fmp4 {
            Some(LowLatencyState::new(
                config.part_target_ms,
                config.max_completed_segments,
            ))
        } else {
            None
        };
        let stream_key = generate_stream_validation_key();
        let vtt_mux = config.vtt_config.map(VttMux::new);
        Self {
            ring: SegmentRing::new(config.segment_count),
            config,
            ts_muxer: None,
            fmp4_muxer: None,
            fmp4_init: None,
            pending_fmp4_samples: Vec::new(),
            pending_part_samples: Vec::new(),
            pending_segment_part_data: Vec::new(),
            ll_state,
            video_codec: CodecId::H264,
            audio_codec: CodecId::AAC,
            has_video: false,
            has_audio: false,
            aac_config: None,
            parameter_sets: None,
            video_extradata: Bytes::new(),
            audio_extradata: Bytes::new(),
            video_width: 0,
            video_height: 0,
            audio_sample_rate: 0,
            audio_channels: 0,
            segment_start_dts: None,
            segment_last_dts: 0,
            last_video_frame_interval_us: None,
            prev_video_dts_us: None,
            segment_has_keyframe: false,
            segment_seq: 0,
            ready: false,
            enabled: true,
            cached_playlist: None,
            cached_playlist_gzip: None,
            concluded: false,
            wallclock_offset_ms: None,
            stream_key,
            rewind_history: Vec::new(),
            pending_markers: Vec::new(),
            demuxed: None,
            vtt_mux,
            vtt_ready: false,
            vtt_segments: VecDeque::new(),
            pending_vtt_outputs: Vec::new(),
        }
    }

    /// Initialize or update tracks and select the effective container.
    ///
    /// Auto-upgrades TS to fMP4 for AV1/VP9, resolves AAC channel layouts from the
    /// ASC, and instantiates the regular fMP4 muxer or the demuxed sub-muxer.
    ///
    /// 初始化或更新轨道并选择有效容器。
    ///
    /// 对 AV1/VP9 自动将 TS 升级为 fMP4，从 ASC 解析 AAC 声道布局，并实例化常规 fMP4 复用器或 demuxed 子复用器。
    pub fn set_tracks(&mut self, tracks: &[TrackInfo]) {
        self.has_video = tracks.iter().any(|t| t.media_kind == MediaKind::Video);
        self.has_audio = tracks.iter().any(|t| t.media_kind == MediaKind::Audio);
        if let Some(video) = tracks.iter().find(|t| t.media_kind == MediaKind::Video) {
            self.video_codec = video.codec;
            self.parameter_sets = extract_parameter_sets(&video.extradata);
            self.video_extradata = extract_raw_extradata(&video.extradata);
            self.video_width = video.width.unwrap_or(0).min(u16::MAX as u32) as u16;
            self.video_height = video.height.unwrap_or(0).min(u16::MAX as u32) as u16;
        }
        if let Some(audio) = tracks.iter().find(|t| t.media_kind == MediaKind::Audio) {
            self.audio_codec = audio.codec;
            if audio.codec == CodecId::AAC {
                self.aac_config = extract_aac_config(&audio.extradata);
            }
            self.audio_extradata = extract_raw_extradata(&audio.extradata);
            self.audio_sample_rate = audio.sample_rate.unwrap_or(44100);
            self.audio_channels = audio.channels.unwrap_or(2);
            // ASC may carry channel_configuration=0 with a PCE (e.g. FLV 5.1 sources). ADTS has
            // no PCE, so rewrite the channel_configuration with a value matching the track's
            // channel count (preferring the count parsed from the PCE itself) to avoid emitting
            // ch_cfg=0 ADTS that decoders interpret as "channels=0, sample_rate=0" and refuse to
            // play.
            if self.audio_codec == CodecId::AAC {
                let raw_asc = match &audio.extradata {
                    CodecExtradata::AAC { asc } => Some(asc.as_ref()),
                    _ => None,
                };
                let resolved_channels = raw_asc
                    .and_then(aac_channel_count_from_asc)
                    .unwrap_or(self.audio_channels);
                if resolved_channels > 0 {
                    self.audio_channels = resolved_channels;
                }
                self.aac_config = patch_aac_config_channels(self.aac_config, self.audio_channels);
            }
        }

        // Auto-upgrade to fMP4 for codecs with poor TS player support (AV1, VP9).
        //
        // AV1 in MPEG-TS uses stream_type=0x9F (de facto standard), but most HLS players
        // (ffplay, hls.js, Safari, ExoPlayer) do not support AV1 demuxing from TS containers.
        // They report "unknown codec" or misidentify it as mpeg4. In contrast, AV1 in fMP4
        // (using av01 sample entry in moov/stsd) is widely supported by the same players.
        // Therefore we automatically switch to fMP4 container when AV1 or VP9 is detected,
        // regardless of the user's container config. This matches the industry practice where
        // AV1 HLS content is exclusively delivered via fMP4 (CMAF).
        let effective_container = match self.config.container {
            HlsContainer::Ts if matches!(self.video_codec, CodecId::AV1 | CodecId::VP9) => {
                HlsContainer::Fmp4
            }
            other => other,
        };

        match effective_container {
            HlsContainer::Ts => {
                self.ts_muxer = Some(TsMuxer::new(
                    self.video_codec,
                    self.audio_codec,
                    self.has_audio,
                ));
            }
            HlsContainer::Fmp4 => {
                self.config.container = HlsContainer::Fmp4;
                // Initialize LL-HLS state if enabled but wasn't created (e.g., TS→fMP4 auto-upgrade)
                if self.config.ll_hls_enabled && self.ll_state.is_none() {
                    self.ll_state = Some(LowLatencyState::new(
                        self.config.part_target_ms,
                        self.config.max_completed_segments,
                    ));
                }
                // In demuxed mode, delegate to DemuxedStreamMuxer
                if self.config.ll_hls_enabled
                    && self.config.ll_hls_packaging_mode == LlHlsPackagingMode::DemuxedAv
                    && self.has_video
                    && self.has_audio
                {
                    let mut demuxed = DemuxedStreamMuxer::new(DemuxedMuxerConfig {
                        segment_duration_ms: self.config.segment_duration_ms,
                        segment_count: self.config.segment_count,
                        force_segment_after_ms: self.config.force_segment_after_ms,
                        part_target_ms: self.config.part_target_ms,
                        max_completed_segments: self.config.max_completed_segments,
                    });
                    demuxed.set_tracks(tracks);
                    self.demuxed = Some(demuxed);
                } else {
                    self.init_fmp4_muxer(tracks);
                }
            }
        }

        // Compute frame-aligned part duration and update LL-HLS state
        if let Some(ref mut ll) = self.ll_state {
            let optimal = Self::compute_optimal_part_duration_static(&self.config, tracks);
            if optimal != self.config.part_target_ms {
                ll.set_part_target_ms(optimal);
            }
        }
    }

    /// Build the `Fmp4Muxer` and init segment from the current track list.
    ///
    /// Skips audio in LLHLS video-only mode and drops non-AV tracks.
    ///
    /// 根据当前轨道列表构建 `Fmp4Muxer` 与 init segment。
    ///
    /// 在 LLHLS 仅视频模式下跳过音频，并丢弃非音视频轨道。
    fn init_fmp4_muxer(&mut self, tracks: &[TrackInfo]) {
        let mut fmp4_tracks = Vec::new();
        for t in tracks {
            if t.media_kind != MediaKind::Video && t.media_kind != MediaKind::Audio {
                continue;
            }
            // In LLHLS video-only mode, skip audio track to avoid muxed SourceBuffer
            // issues with hls.js. In demuxed mode, this function is not called.
            if self.ll_state.is_some()
                && self.config.ll_hls_packaging_mode == LlHlsPackagingMode::VideoOnly
                && t.media_kind == MediaKind::Audio
            {
                continue;
            }
            let track_id = match t.media_kind {
                MediaKind::Video => 1,
                MediaKind::Audio => 2,
                _ => unreachable!("non audio/video tracks are filtered above"),
            };
            let timescale = if t.media_kind == MediaKind::Video {
                90000
            } else {
                t.sample_rate.unwrap_or(44100)
            };
            fmp4_tracks.push(Fmp4TrackDesc {
                track_id,
                codec: t.codec,
                media_kind: t.media_kind,
                timescale,
                extradata: extract_raw_extradata(&t.extradata),
                width: t.width.unwrap_or(0) as u16,
                height: t.height.unwrap_or(0) as u16,
                sample_rate: t.sample_rate.unwrap_or(0),
                channels: t.channels.unwrap_or(0),
            });
        }
        let mut muxer = Fmp4Muxer::new(fmp4_tracks);
        self.fmp4_init = Some(muxer.init_segment());
        self.fmp4_muxer = Some(muxer);
    }

    /// Feed a frame into the muxer and produce segment/part events.
    ///
    /// Dispatches to the demuxed muxer when active, handles CONFIG frames and
    /// parameter sets, then routes to TS or fMP4 path depending on the container.
    ///
    /// 将帧送入复用器并生成分段/分片事件。
    ///
    /// 在 demuxed 复用器激活时转发，处理 CONFIG 帧和参数集，然后根据容器选择 TS 或 fMP4 路径。
    pub fn push_frame(&mut self, frame: &AVFrame) -> Vec<MuxerOutput> {
        // On-demand mode: skip muxing when disabled
        if !self.enabled || self.concluded {
            return Vec::new();
        }

        // Delegate to demuxed muxer when active
        if let Some(ref mut demuxed) = self.demuxed {
            let outputs = demuxed.push_frame(frame);
            if !outputs.is_empty() && !self.ready {
                self.ready = demuxed.is_ready();
            }
            return outputs;
        }

        // Extract AAC config from CONFIG frames (before NON_PICTURE skip)
        if frame.flags.contains(FrameFlags::CONFIG) {
            if frame.media_kind == MediaKind::Audio && frame.codec == CodecId::AAC {
                if self.aac_config.is_none() {
                    self.aac_config = AacAudioSpecificConfig::from_bytes(&frame.payload);
                }
                // Resolve channel count from the ASC (PCE-aware). FLV/RTMP sources commonly
                // arrive at the muxer with track.channels still defaulted to 1 or 2 because
                // the AMF metadata "stereo" flag predates the AAC config; fall back to whatever
                // we already learned from the track if PCE parsing fails.
                if let Some(resolved) = aac_channel_count_from_asc(&frame.payload) {
                    if resolved > 0 {
                        self.audio_channels = resolved;
                    }
                }
                // Backfill ch_cfg=0 ASC (PCE-only multichannel) so ADTS frames carry
                // a valid channel_configuration that decoders can use, even if the
                // original snapshot already produced an aac_config with ch_cfg=0.
                self.aac_config = patch_aac_config_channels(self.aac_config, self.audio_channels);
                self.audio_codec = CodecId::AAC;
            }
            // Extract video parameter sets (SPS/PPS/VPS) from CONFIG frames
            if frame.media_kind == MediaKind::Video && self.parameter_sets.is_none() {
                self.video_codec = frame.codec;
                if !frame.payload.is_empty() {
                    self.parameter_sets = Some(Bytes::from(to_annexb(&frame.payload)));
                    self.video_extradata = frame.payload.clone();
                }
            }
            return Vec::new();
        }

        // Skip SEI/metadata NALs
        if frame.flags.contains(FrameFlags::NON_PICTURE) {
            return Vec::new();
        }

        if self.waiting_for_initial_video_keyframe(frame) {
            return Vec::new();
        }

        let mut outputs = match self.config.container {
            HlsContainer::Ts => {
                let produced = self.push_frame_ts(frame);
                if produced {
                    if let Some(seg) = self.ring.latest() {
                        vec![MuxerOutput::SegmentReady {
                            name: seg.name.clone(),
                            duration_secs: seg.duration_secs,
                            data: seg.data.clone(),
                        }]
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            }
            HlsContainer::Fmp4 => self.push_frame_fmp4(frame),
        };
        outputs.extend(self.take_vtt_outputs());
        outputs
    }

    /// Push a frame through the fMP4 muxing path.
    ///
    /// Auto-initializes the fMP4 muxer from the first keyframe if needed, detects
    /// timestamp rollbacks, decides segment cuts, and emits LL-HLS parts.
    ///
    /// 通过 fMP4 复用路径推送帧。
    ///
    /// 必要时从首个关键帧自动初始化 fMP4 复用器，检测时间戳回退，决定分段切割并输出 LL-HLS 分片。
    fn push_frame_fmp4(&mut self, frame: &AVFrame) -> Vec<MuxerOutput> {
        let is_video = frame.media_kind == MediaKind::Video;
        let is_keyframe = frame.flags.contains(FrameFlags::KEY);

        // In LLHLS video-only mode, skip audio frames
        if self.ll_state.is_some()
            && self.config.ll_hls_packaging_mode == LlHlsPackagingMode::VideoOnly
            && !is_video
        {
            return Vec::new();
        }

        let frame_dts_us = frame.dts_us.max(0) as u64;
        let dts_ms = frame_dts_us / 1000;
        let pts_ms = (frame.pts_us.max(frame.dts_us) as u64) / 1000;

        // Auto-initialize fMP4 muxer from first keyframe if set_tracks wasn't called
        if self.fmp4_muxer.is_none() {
            if is_video && is_keyframe {
                self.video_codec = frame.codec;
                let tracks = vec![Fmp4TrackDesc {
                    track_id: 1,
                    codec: frame.codec,
                    media_kind: MediaKind::Video,
                    timescale: 90000,
                    extradata: self.video_extradata.clone(),
                    width: self.video_width,
                    height: self.video_height,
                    sample_rate: 0,
                    channels: 0,
                }];
                let mut muxer = Fmp4Muxer::new(tracks);
                self.fmp4_init = Some(muxer.init_segment());
                self.fmp4_muxer = Some(muxer);
            } else if frame.media_kind == MediaKind::Audio {
                self.has_audio = true;
                self.audio_codec = frame.codec;
                return Vec::new();
            } else {
                return Vec::new();
            }
        }

        // Timestamp rollback detection
        if let Some(start_dts) = self.segment_start_dts {
            if frame_dts_us < start_dts {
                self.segment_start_dts = Some(frame_dts_us);
                self.segment_last_dts = frame_dts_us;
            }
        }

        // Check if we should cut a segment
        let should_cut = if let Some(start_dts) = self.segment_start_dts {
            let elapsed_us = frame_dts_us.saturating_sub(start_dts);
            let duration_us = self.config.segment_duration_ms * 1000;
            let force_us = self.config.force_segment_after_ms * 1000;
            let normal_cut = is_video && is_keyframe && elapsed_us >= duration_us;
            let force_cut = elapsed_us >= force_us;
            let fast_cut =
                self.config.fast_register && self.segment_seq < 2 && is_video && is_keyframe;
            normal_cut || force_cut || fast_cut
        } else {
            false
        };

        let mut outputs = Vec::new();

        if should_cut {
            // Finalize any pending part before segment cut
            if let Some(part) = self.try_finalize_current_part() {
                outputs.push(MuxerOutput::PartReady(part));
            }
            // The cut-triggering frame's DTS is the *next* segment's first sample
            // for video. Pass it so EXTINF includes the last frame's display time
            // and matches the segment's actual coverage / next tfdt.
            let next_start_for_extinf = if is_video { Some(frame_dts_us) } else { None };
            self.finalize_segment(next_start_for_extinf);
            if let Some(seg) = self.ring.latest() {
                outputs.push(MuxerOutput::SegmentReady {
                    name: seg.name.clone(),
                    duration_secs: seg.duration_secs,
                    data: seg.data.clone(),
                });
            }
        }

        if self.segment_start_dts.is_none() {
            self.segment_start_dts = Some(frame_dts_us);
            self.segment_has_keyframe = false;
        }

        if is_video && is_keyframe {
            self.segment_has_keyframe = true;
        }
        self.segment_last_dts = frame_dts_us;

        if is_video {
            if let Some(prev) = self.prev_video_dts_us {
                if frame_dts_us > prev {
                    self.last_video_frame_interval_us = Some(frame_dts_us - prev);
                }
            }
            self.prev_video_dts_us = Some(frame_dts_us);
        }

        // Determine track_id (video=1, audio=2 by convention)
        let track_id = if is_video { 1 } else { 2 };

        let sample = Fmp4Sample {
            track_id,
            pts_ms,
            dts_ms,
            is_keyframe,
            data: fmp4_sample_payload(frame),
        };

        self.pending_fmp4_samples.push(sample.clone());

        // LL-HLS part cutting
        let should_cut_part = self
            .ll_state
            .as_ref()
            .map(|ll| ll.should_cut_part(dts_ms))
            .unwrap_or(false);

        if should_cut_part {
            if let Some(part) = self.finalize_part_inner_with_end(Some(dts_ms)) {
                outputs.push(MuxerOutput::PartReady(part));
            }
        }

        if let Some(ref mut ll) = self.ll_state {
            ll.note_sample(dts_ms, is_video && is_keyframe);
            self.pending_part_samples.push(sample);
        }

        outputs
    }

    /// Whether the muxer must drop frames until the first video keyframe.
    ///
    /// 复用器是否必须丢弃帧直到首个视频关键帧。
    fn waiting_for_initial_video_keyframe(&self, frame: &AVFrame) -> bool {
        self.has_video
            && self.segment_start_dts.is_none()
            && !(frame.media_kind == MediaKind::Video && frame.flags.contains(FrameFlags::KEY))
    }

    /// Push a frame through the TS muxing path.
    ///
    /// Initializes the `TsMuxer` on first keyframe, prepends parameter sets on
    /// segment-start keyframes, and wraps raw AAC into ADTS.
    ///
    /// 通过 TS 复用路径推送帧。
    ///
    /// 在首个关键帧初始化 `TsMuxer`，在分段起始关键帧前追加参数集，并将原始 AAC 包装为 ADTS。
    fn push_frame_ts(&mut self, frame: &AVFrame) -> bool {
        if self.ts_muxer.is_none() {
            if frame.media_kind == MediaKind::Video && frame.flags.contains(FrameFlags::KEY) {
                self.video_codec = frame.codec;
                self.ts_muxer = Some(TsMuxer::new(
                    self.video_codec,
                    self.audio_codec,
                    self.has_audio,
                ));
            } else if frame.media_kind == MediaKind::Audio {
                self.has_audio = true;
                self.audio_codec = frame.codec;
                return false;
            } else {
                return false;
            }
        }

        let is_video = frame.media_kind == MediaKind::Video;
        let is_keyframe = frame.flags.contains(FrameFlags::KEY);
        let frame_dts_us = frame.dts_us.max(0) as u64;
        let dts_90k = us_to_90k(frame_dts_us);
        // Ensure PTS >= DTS (required by MPEG-TS)
        let pts_90k = us_to_90k(frame.pts_us.max(frame.dts_us) as u64);

        // Timestamp rollback detection: reset segment timing
        if let Some(start_dts) = self.segment_start_dts {
            if frame_dts_us < start_dts {
                self.segment_start_dts = Some(frame_dts_us);
                self.segment_last_dts = frame_dts_us;
            }
        }

        // Check if we should cut a segment
        let should_cut = if let Some(start_dts) = self.segment_start_dts {
            let elapsed_us = frame_dts_us.saturating_sub(start_dts);
            let duration_us = self.config.segment_duration_ms * 1000;
            let force_us = self.config.force_segment_after_ms * 1000;
            let normal_cut = is_video && is_keyframe && elapsed_us >= duration_us;
            let force_cut = elapsed_us >= force_us;
            // Fast register: first 2 segments cut on any keyframe
            let fast_cut =
                self.config.fast_register && self.segment_seq < 2 && is_video && is_keyframe;
            normal_cut || force_cut || fast_cut
        } else {
            false
        };

        let mut produced_segment = false;
        if should_cut {
            let next_start_for_extinf = if is_video { Some(frame_dts_us) } else { None };
            self.finalize_segment(next_start_for_extinf);
            produced_segment = true;
        }

        // Start new segment if needed
        if self.segment_start_dts.is_none() {
            self.segment_start_dts = Some(frame_dts_us);
            self.segment_has_keyframe = false;
            self.ts_muxer.as_mut().unwrap().write_pat_pmt();
        }

        let had_keyframe_before_this_frame = self.segment_has_keyframe;
        if is_video && is_keyframe {
            self.segment_has_keyframe = true;
        }
        self.segment_last_dts = frame_dts_us;

        if is_video {
            if let Some(prev) = self.prev_video_dts_us {
                if frame_dts_us > prev {
                    self.last_video_frame_interval_us = Some(frame_dts_us - prev);
                }
            }
            self.prev_video_dts_us = Some(frame_dts_us);
        }

        let muxer = self.ts_muxer.as_mut().unwrap();
        if is_video {
            // Convert length-prefixed NALUs to Annex-B for TS container
            let annexb_payload = to_annexb(&frame.payload);

            // Prepend parameter sets on the first keyframe of a new segment
            let need_parameter_sets =
                is_keyframe && (!had_keyframe_before_this_frame || produced_segment);
            if need_parameter_sets {
                if let Some(ps) = &self.parameter_sets {
                    let mut combined = Vec::with_capacity(ps.len() + annexb_payload.len());
                    combined.extend_from_slice(ps);
                    combined.extend_from_slice(&annexb_payload);
                    muxer.write_video(&combined, pts_90k, dts_90k, true);
                    return produced_segment;
                }
            }
            muxer.write_video(&annexb_payload, pts_90k, dts_90k, is_keyframe);
        } else {
            // AAC: ensure ADTS header is present for TS container
            if self.audio_codec == CodecId::AAC {
                let payload = &frame.payload;
                // Check if frame already has ADTS header (sync word 0xFFF)
                let has_adts =
                    payload.len() >= 2 && payload[0] == 0xFF && (payload[1] & 0xF0) == 0xF0;
                if has_adts {
                    // Already has ADTS — pass through directly
                    muxer.write_audio(payload, pts_90k);
                } else if let Some(asc) = self.aac_config {
                    // Raw AAC AU — wrap with ADTS
                    let adts_frame = adts_wrap(payload, asc);
                    muxer.write_audio(&adts_frame, pts_90k);
                } else {
                    // No ASC and no ADTS — skip frame (can't mux without config)
                }
            } else {
                muxer.write_audio(&frame.payload, pts_90k);
            }
        }

        produced_segment
    }

    /// Force-finalize the current segment at end-of-stream.
    ///
    /// 在流结束时强制完成当前分段。
    pub fn flush(&mut self) {
        if self.segment_start_dts.is_some() {
            self.try_finalize_current_part();
            // No next-segment hint at flush time: finalize_segment will fall back
            // to last_dts + estimated frame interval so EXTINF still includes the
            // final frame's display duration.
            self.finalize_segment(None);
        }
    }

    /// Whether the stream has produced enough segments to be considered ready.
    ///
    /// 流是否已生成足够分段以被视为就绪。
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// Return the media playlist, using the cached version when no session token is needed.
    ///
    /// 返回媒体播放列表，当不需要会话令牌时使用缓存版本。
    pub fn playlist(&self, session_id: Option<u64>) -> String {
        // Use cached playlist when no special parameters
        if session_id.is_none() {
            if let Some(ref cached) = self.cached_playlist {
                return cached.clone();
            }
        }
        self.playlist_with_options(session_id, false)
    }

    /// Generate a playlist with legacy mode and optional stream-key validation token.
    ///
    /// 生成支持 legacy 模式与可选流密钥验证令牌的播放列表。
    pub fn playlist_with_options_and_token(
        &self,
        session_id: Option<u64>,
        legacy: bool,
        include_stream_key: bool,
    ) -> String {
        let playlist = self.playlist_with_options(session_id, legacy);
        if include_stream_key {
            append_stream_key_to_playlist_uris(&playlist, &self.stream_key)
        } else {
            playlist
        }
    }

    /// Return the pre-compressed gzip playlist, if available.
    ///
    /// 返回预压缩的 gzip 播放列表（如有）。
    pub fn cached_playlist_gzip(&self) -> Option<Bytes> {
        self.cached_playlist_gzip.clone()
    }

    /// Generate a media playlist with the requested legacy and session options.
    ///
    /// 生成带有指定 legacy 与会话选项的媒体播放列表。
    pub fn playlist_with_options(&self, session_id: Option<u64>, legacy: bool) -> String {
        // In demuxed mode, return video lane chunklist for legacy compat
        if let Some(ref demuxed) = self.demuxed {
            return demuxed
                .track_playlist(TrackLane::Video, session_id, false, &self.stream_key)
                .unwrap_or_default();
        }
        if let Some(ref ll) = self.ll_state {
            PlaylistBuilder::build_media_ll(&self.ring, ll, session_id, "", legacy, self.concluded)
        } else {
            let mut out = PlaylistBuilder::build_media_with_container(
                &self.ring,
                session_id,
                self.config.container,
            );
            if self.concluded {
                out.push_str("#EXT-X-ENDLIST\n");
            }
            out
        }
    }

    /// Rebuild the cached default playlist and gzip version.
    ///
    /// Call after each part/segment boundary to keep the hot-path response fast.
    ///
    /// 重建默认缓存播放列表与 gzip 版本。
    ///
    /// 在每次分片/分段边界后调用，以保持热路径响应快速。
    fn rebuild_playlist_cache(&mut self) {
        if self.ready {
            let content = self.playlist_with_options(None, false);
            // Pre-generate gzip version
            self.cached_playlist_gzip = Some(gzip_compress(content.as_bytes()));
            self.cached_playlist = Some(content);
        }
    }

    /// Compute a frame-aligned part target from video FPS or audio frame size.
    ///
    /// Aligning part boundaries to frame boundaries avoids partial frames and
    /// reduces player drift.
    ///
    /// 根据视频 FPS 或音频帧大小计算帧对齐的分片目标。
    ///
    /// 将分片边界对齐到帧边界可避免不完整帧并减少播放器漂移。
    fn compute_optimal_part_duration_static(
        config: &StreamMuxerConfig,
        tracks: &[TrackInfo],
    ) -> u64 {
        let target_ms = config.part_target_ms as f64;

        // Prefer video frame rate alignment
        if let Some(video) = tracks.iter().find(|t| t.media_kind == MediaKind::Video) {
            if let Some(fps) = video.fps {
                let fps_f = fps.num as f64 / fps.den as f64;
                if fps_f > 0.0 {
                    let frame_ms = 1000.0 / fps_f;
                    let frames = (target_ms / frame_ms).round().max(1.0);
                    return (frames * frame_ms).round() as u64;
                }
            }
        }

        // Fallback to audio frame alignment
        if let Some(audio) = tracks.iter().find(|t| t.media_kind == MediaKind::Audio) {
            let sr = audio.sample_rate.unwrap_or(44100) as f64;
            if sr > 0.0 {
                let samples_per_frame = if audio.codec == CodecId::AAC {
                    1024.0
                } else {
                    960.0
                };
                let frame_ms = samples_per_frame / sr * 1000.0;
                let frames = (target_ms / frame_ms).round().max(1.0);
                return (frames * frame_ms).round() as u64;
            }
        }

        config.part_target_ms
    }

    /// Conclude the live stream and append `EXT-X-ENDLIST`.
    ///
    /// No more frames are accepted after this call.
    ///
    /// 结束直播流并追加 `EXT-X-ENDLIST`。
    ///
    /// 调用后不再接受新帧。
    pub fn conclude(&mut self) {
        if let Some(ref mut demuxed) = self.demuxed {
            demuxed.conclude();
        } else {
            self.flush();
        }
        self.concluded = true;
        self.rebuild_playlist_cache();
    }

    /// Whether the stream has been concluded.
    ///
    /// 流是否已结束。
    #[allow(dead_code)]
    pub fn is_concluded(&self) -> bool {
        self.concluded
    }

    /// Insert a CUE marker to be attached to the next finalized segment.
    ///
    /// 插入一个 CUE 标记，附加到下一个完成的分段。
    #[allow(dead_code)]
    pub fn insert_marker(&mut self, marker: cheetah_hls_core::CueMarker) {
        self.pending_markers.push(marker);
    }

    /// Return the stream validation key used to sign playlist/segment URIs.
    ///
    /// 返回用于签名播放列表/分段 URI 的流验证密钥。
    pub fn stream_key(&self) -> &str {
        &self.stream_key
    }

    /// Generate a rewind playlist covering all retrievable segments.
    ///
    /// Used for DVR/timeshift by listing segments still present in the ring.
    ///
    /// 生成覆盖所有可获取分段的回退播放列表。
    ///
    /// 通过列出仍在环形缓冲区中的分段来支持 DVR/时移。
    pub fn playlist_rewind(&self, session_id: Option<u64>) -> String {
        if self.rewind_history.is_empty() {
            return self.playlist_with_options(session_id, false);
        }

        let retrievable = self
            .rewind_history
            .iter()
            .filter(|meta| self.ring.get(&meta.name).is_some())
            .collect::<Vec<_>>();
        if retrievable.is_empty() {
            return self.playlist_with_options(session_id, false);
        }

        let target_duration = self
            .ring
            .iter()
            .map(|segment| segment.duration_secs.ceil() as u64)
            .max()
            .unwrap_or(4);
        let first_seq = retrievable.first().map(|m| m.sequence).unwrap_or(0);
        let ext = match self.config.container {
            HlsContainer::Fmp4 => ".m4s",
            HlsContainer::Ts => ".ts",
        };
        let version = match self.config.container {
            HlsContainer::Fmp4 => 7,
            HlsContainer::Ts => 3,
        };

        let mut out = String::with_capacity(retrievable.len() * 80);
        out.push_str("#EXTM3U\n");
        out.push_str(&format!("#EXT-X-VERSION:{version}\n"));
        out.push_str(&format!("#EXT-X-TARGETDURATION:{target_duration}\n"));
        out.push_str(&format!("#EXT-X-MEDIA-SEQUENCE:{first_seq}\n"));

        if self.config.container == HlsContainer::Fmp4 {
            match session_id {
                Some(uid) => out.push_str(&format!("#EXT-X-MAP:URI=\"init.mp4?uid={uid}\"\n")),
                None => out.push_str("#EXT-X-MAP:URI=\"init.mp4\"\n"),
            }
        }

        for meta in retrievable {
            if let Some(pdt_ms) = meta.program_date_time_ms {
                out.push_str(&format!(
                    "#EXT-X-PROGRAM-DATE-TIME:{}\n",
                    cheetah_hls_core::format_iso8601(pdt_ms)
                ));
            }
            out.push_str(&format!("#EXTINF:{:.3},\n", meta.duration_secs));
            match session_id {
                Some(uid) => out.push_str(&format!("{}{ext}?uid={uid}\n", meta.name)),
                None => out.push_str(&format!("{}{ext}\n", meta.name)),
            }
        }

        if self.concluded {
            out.push_str("#EXT-X-ENDLIST\n");
        }
        out
    }

    /// Generate a rewind playlist with optional stream-key validation token.
    ///
    /// 生成带可选流密钥验证令牌的回退播放列表。
    pub fn playlist_rewind_with_token(
        &self,
        session_id: Option<u64>,
        include_stream_key: bool,
    ) -> String {
        let playlist = self.playlist_rewind(session_id);
        if include_stream_key {
            append_stream_key_to_playlist_uris(&playlist, &self.stream_key)
        } else {
            playlist
        }
    }

    /// Return a completed segment by name, with demuxed lane fallback.
    ///
    /// 按名称返回已完成的分段，支持 demuxed 轨道回退。
    pub fn get_segment(&self, name: &str) -> Option<Bytes> {
        if let Some(ref demuxed) = self.demuxed {
            // Try exact name first, then with video_ prefix for legacy URLs
            return demuxed
                .track_segment(TrackLane::Video, name)
                .or_else(|| demuxed.track_segment(TrackLane::Video, &format!("video_{name}")));
        }
        self.ring.get(name).map(|s| s.data.clone())
    }

    /// Return a finalized LL-HLS part by global sequence.
    ///
    /// 根据全局序列号返回已完成化的 LL-HLS 分片。
    pub fn get_part(&self, part_seq: u64) -> Option<Bytes> {
        if let Some(ref demuxed) = self.demuxed {
            return demuxed.track_part(TrackLane::Video, part_seq);
        }
        self.ll_state
            .as_ref()
            .and_then(|ll| ll.get_part(part_seq))
            .map(|p| p.data.clone())
    }

    /// Return the fMP4 init segment.
    ///
    /// 返回 fMP4 init segment。
    pub fn init_segment(&self) -> Option<Bytes> {
        if let Some(ref demuxed) = self.demuxed {
            return demuxed.track_init_segment(TrackLane::Video);
        }
        self.fmp4_init.clone()
    }

    /// Whether this muxer is in demuxed mode.
    ///
    /// 该复用器是否处于 demuxed 模式。
    pub fn is_demuxed(&self) -> bool {
        self.demuxed.is_some()
    }

    /// Return the video codec.
    ///
    /// 返回视频编解码器。
    pub fn video_codec(&self) -> CodecId {
        self.video_codec
    }

    /// Return the video width and height.
    ///
    /// 返回视频宽高。
    pub fn video_dimensions(&self) -> (u16, u16) {
        (self.video_width, self.video_height)
    }

    /// Return the audio codec.
    ///
    /// 返回音频编解码器。
    pub fn audio_codec(&self) -> CodecId {
        self.audio_codec
    }

    /// Return the audio channel count.
    ///
    /// 返回音频声道数。
    pub fn audio_channels(&self) -> u8 {
        self.audio_channels
    }

    /// Whether the muxer still needs the AAC AudioSpecificConfig.
    ///
    /// The module may re-fetch the stream snapshot if the config arrived late.
    ///
    /// 复用器是否仍需要 AAC AudioSpecificConfig。
    ///
    /// 如果配置到达较晚，模块可能重新获取流快照。
    pub fn needs_aac_config_refresh(&self) -> bool {
        self.has_audio && self.audio_codec == CodecId::AAC && self.aac_config.is_none()
    }

    /// Return raw video extradata for codec string generation.
    ///
    /// 返回用于 codec 字符串生成的原始视频 extradata。
    pub fn video_extradata(&self) -> &[u8] {
        &self.video_extradata
    }

    /// Return raw audio extradata for codec string generation.
    ///
    /// 返回用于 codec 字符串生成的原始音频 extradata。
    pub fn audio_extradata(&self) -> &[u8] {
        &self.audio_extradata
    }

    /// Return the init segment for a demuxed lane.
    ///
    /// 返回 demuxed 轨道的 init segment。
    pub fn track_init_segment(&self, lane: TrackLane) -> Option<Bytes> {
        self.demuxed.as_ref()?.track_init_segment(lane)
    }

    /// Return a part for a demuxed lane.
    ///
    /// 返回 demuxed 轨道的分片。
    pub fn track_part(&self, lane: TrackLane, seq: u64) -> Option<Bytes> {
        self.demuxed.as_ref()?.track_part(lane, seq)
    }

    /// Return a segment for a demuxed lane.
    ///
    /// 返回 demuxed 轨道的分段。
    pub fn track_segment(&self, lane: TrackLane, name: &str) -> Option<Bytes> {
        self.demuxed.as_ref()?.track_segment(lane, name)
    }

    /// Generate a per-track chunklist for a demuxed lane.
    ///
    /// 生成 demuxed 轨道的每轨分片列表。
    pub fn track_playlist(
        &self,
        lane: TrackLane,
        session_id: Option<u64>,
        include_stream_key: bool,
    ) -> Option<String> {
        self.demuxed.as_ref()?.track_playlist(
            lane,
            session_id,
            include_stream_key,
            &self.stream_key,
        )
    }

    /// Return the (last_msn, last_part_index) for a demuxed lane.
    ///
    /// 返回 demuxed 轨道的 (last_msn, last_part_index)。
    pub fn rendition_state(&self, lane: TrackLane) -> Option<(u64, u64)> {
        self.demuxed.as_ref()?.rendition_state(lane)
    }

    /// Check whether a blocking request for a demuxed lane is satisfied.
    ///
    /// 检查 demuxed 轨道的阻塞请求是否满足。
    pub fn is_track_blocking_satisfied(
        &self,
        lane: TrackLane,
        target_msn: u64,
        target_part: Option<u64>,
    ) -> bool {
        if self.concluded {
            return true;
        }
        if let Some(ref demuxed) = self.demuxed {
            demuxed
                .lane(lane)
                .map(|t| t.is_blocking_satisfied(target_msn, target_part))
                .unwrap_or(true)
        } else {
            self.is_blocking_satisfied(target_msn, target_part)
        }
    }

    /// Return the most recently added segment name and data.
    ///
    /// 返回最近添加的分段名称与数据。
    pub fn latest_segment(&self) -> Option<(String, Bytes)> {
        self.ring.latest().map(|s| (s.name.clone(), s.data.clone()))
    }

    /// Return the effective container format.
    ///
    /// 返回有效容器格式。
    pub fn container(&self) -> HlsContainer {
        self.config.container
    }

    /// Whether LL-HLS mode is active.
    ///
    /// 是否启用 LL-HLS 模式。
    #[allow(dead_code)]
    pub fn is_ll_hls(&self) -> bool {
        self.ll_state.is_some()
    }

    /// Current segment media sequence number.
    ///
    /// 当前分段媒体序列号。
    #[allow(dead_code)]
    pub fn current_msn(&self) -> u64 {
        self.segment_seq
    }

    /// Next part sequence number to be produced.
    ///
    /// 下一个待生成分片的序列号。
    pub fn next_part_seq(&self) -> u64 {
        if let Some(ref demuxed) = self.demuxed {
            return demuxed.video().map(|v| v.next_part_seq()).unwrap_or(0);
        }
        self.ll_state
            .as_ref()
            .map(|ll| ll.next_part_seq())
            .unwrap_or(0)
    }

    /// Next part sequence number for a specific demuxed lane.
    ///
    /// 指定 demuxed 轨道的下一个待生成分片序列号。
    pub fn track_next_part_seq(&self, lane: TrackLane) -> u64 {
        self.demuxed
            .as_ref()
            .and_then(|d| d.lane(lane))
            .map(|t| t.next_part_seq())
            .unwrap_or(0)
    }

    /// Check whether a blocking playlist request is satisfied.
    ///
    /// A request for (MSN, part) is satisfied when the current state has progressed
    /// past it, or the stream has concluded.
    ///
    /// 检查阻塞播放列表请求是否满足。
    ///
    /// 当当前状态已超过该 (MSN, part) 或流已结束时，请求视为满足。
    pub fn is_blocking_satisfied(&self, target_msn: u64, target_part: Option<u64>) -> bool {
        if self.concluded {
            return true;
        }
        // In demuxed mode, delegate to video lane
        if let Some(ref demuxed) = self.demuxed {
            return demuxed
                .lane(TrackLane::Video)
                .map(|t| t.is_blocking_satisfied(target_msn, target_part))
                .unwrap_or(true);
        }
        let Some(ref ll) = self.ll_state else {
            return true;
        };
        let current_msn = ll.parent_segment_seq();
        match target_part {
            Some(tp) => {
                if current_msn > target_msn {
                    return true;
                }
                if current_msn == target_msn {
                    // Part count in current segment
                    let parts_in_segment = ll.current_parts().len() as u64;
                    return parts_in_segment > tp;
                }
                false
            }
            None => current_msn > target_msn,
        }
    }

    /// Finalize the current part if there are pending samples.
    ///
    /// 如果存在待处理采样，则完成当前分片。
    fn try_finalize_current_part(&mut self) -> Option<HlsPart> {
        self.finalize_part_inner_with_end(None)
    }

    /// Finalize the pending LL-HLS part from `pending_part_samples`.
    ///
    /// Writes the part via `Fmp4Muxer`, queues its data for the segment, and updates
    /// the cached playlist.
    ///
    /// 从 `pending_part_samples` 完成待处理 LL-HLS 分片。
    ///
    /// 通过 `Fmp4Muxer` 写入分片，将其数据排队用于分段，并更新缓存播放列表。
    fn finalize_part_inner_with_end(&mut self, end_dts_ms: Option<u64>) -> Option<HlsPart> {
        if self.pending_part_samples.is_empty() {
            return None;
        }
        let muxer = self.fmp4_muxer.as_mut()?;
        let samples = std::mem::take(&mut self.pending_part_samples);
        let first_dts_ms = samples.first().map(|s| s.dts_ms).unwrap_or(0);
        let duration_secs = match end_dts_ms {
            Some(end) => end.saturating_sub(first_dts_ms) as f64 / 1000.0,
            None => {
                let last_dts_ms = samples.last().map(|s| s.dts_ms).unwrap_or(first_dts_ms);
                let d = last_dts_ms.saturating_sub(first_dts_ms) as f64 / 1000.0;
                if d <= 0.0 {
                    self.config.part_target_ms as f64 / 1000.0
                } else {
                    d
                }
            }
        };
        let data = muxer.write_part(&samples);
        // Store part data for segment concatenation (segment = concat of parts)
        self.pending_segment_part_data.push(data.clone());
        let ll = self.ll_state.as_mut()?;
        let part = ll.finalize_part(data, duration_secs);
        self.rebuild_playlist_cache();
        Some(part)
    }

    /// Finalize the current segment and push it into the ring.
    ///
    /// Computes EXTINF from the next segment's start DTS when available, otherwise
    /// estimates the last frame's duration to avoid player drift. Also sets CUE markers
    /// and rewind history.
    ///
    /// 完成当前分段并推入环形缓冲区。
    ///
    /// 在可用时根据下一个分段起始 DTS 计算 EXTINF，否则估算最后一帧时长以避免播放器漂移。
    /// 同时设置 CUE 标记与回退历史。
    fn finalize_segment(&mut self, next_video_start_dts_us: Option<u64>) {
        let Some(start_dts) = self.segment_start_dts.take() else {
            return;
        };

        let data = match self.config.container {
            HlsContainer::Ts => {
                let Some(muxer) = self.ts_muxer.as_mut() else {
                    return;
                };
                muxer.take_segment()
            }
            HlsContainer::Fmp4 => {
                let Some(_muxer) = self.fmp4_muxer.as_mut() else {
                    return;
                };
                // In LLHLS mode, segment = concatenation of all its parts.
                // In non-LLHLS fMP4 mode, re-mux from pending samples.
                if self.ll_state.is_some() && !self.pending_segment_part_data.is_empty() {
                    // Also mux any remaining samples not yet finalized as a part
                    if !self.pending_part_samples.is_empty() {
                        let muxer = self.fmp4_muxer.as_mut().unwrap();
                        let samples = std::mem::take(&mut self.pending_part_samples);
                        let tail = muxer.write_part(&samples);
                        self.pending_segment_part_data.push(tail);
                    }
                    let parts = std::mem::take(&mut self.pending_segment_part_data);
                    let total_len: usize = parts.iter().map(|p| p.len()).sum();
                    let mut combined = bytes::BytesMut::with_capacity(total_len);
                    for p in &parts {
                        combined.extend_from_slice(p);
                    }
                    self.pending_fmp4_samples.clear();
                    combined.freeze()
                } else {
                    let muxer = self.fmp4_muxer.as_mut().unwrap();
                    let samples = std::mem::take(&mut self.pending_fmp4_samples);
                    if samples.is_empty() {
                        return;
                    }
                    self.pending_segment_part_data.clear();
                    muxer.write_segment(&samples)
                }
            }
        };

        if data.is_empty() {
            return;
        }

        let name = format!("seg_{}", self.segment_seq);
        let seg_seq = self.segment_seq;
        self.segment_seq += 1;

        // EXTINF must cover the segment up to the next segment's first sample DTS.
        // Otherwise the last frame's display duration is dropped and the player
        // accumulates drift (50 ms per segment at 20 fps), which surfaces as
        // periodic 1 s stalls when the drift overlaps a playlist-reload window.
        let duration_us = match next_video_start_dts_us {
            Some(end) if end > start_dts => end - start_dts,
            _ if self.segment_last_dts > start_dts => {
                let span = self.segment_last_dts - start_dts;
                // Estimate one extra frame's display time so we do not under-report.
                let est_frame_us = self
                    .last_video_frame_interval_us
                    .filter(|i| *i > 0)
                    .unwrap_or(33_000);
                span + est_frame_us
            }
            _ => self.config.segment_duration_ms * 1000,
        };
        let duration_secs = duration_us as f64 / 1_000_000.0;

        // Close the aligned WebVTT subtitle segment when present.
        let end_us = next_video_start_dts_us.unwrap_or(start_dts.saturating_add(duration_us));
        self.close_vtt_segment(end_us / 1000);

        // Compute wallclock offset on first segment
        let segment_start_dts_ms = (start_dts / 1000) as i64;
        if self.wallclock_offset_ms.is_none() {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            self.wallclock_offset_ms = Some(now_ms - segment_start_dts_ms);
        }
        let pdt_ms = segment_start_dts_ms + self.wallclock_offset_ms.unwrap_or(0);

        self.ring.push_with_pdt(
            name,
            duration_secs,
            data,
            self.segment_has_keyframe,
            Some(pdt_ms),
        );

        // Set CUE markers on the newly pushed segment
        if !self.pending_markers.is_empty() {
            let cue_str = cheetah_hls_core::format_cue_tags(&self.pending_markers);
            if let Some(seg) = self.ring.latest_mut() {
                seg.cue_tags = Some(cue_str);
            }
        }

        // Save segment metadata for DVR/rewind playlist generation.
        // rewind_history contains ALL segments ever produced (ring only keeps the live window).
        self.rewind_history.push(SegmentMeta {
            name: format!("seg_{seg_seq}"),
            duration_secs,
            sequence: seg_seq,
            program_date_time_ms: Some(pdt_ms),
            markers: std::mem::take(&mut self.pending_markers),
        });
        // Cap rewind history to prevent unbounded memory growth (~4h at 4s segments)
        const MAX_REWIND_HISTORY: usize = 3600;
        if self.rewind_history.len() > MAX_REWIND_HISTORY {
            self.rewind_history
                .drain(..self.rewind_history.len() - MAX_REWIND_HISTORY);
        }

        // Notify LL-HLS state of segment boundary
        if let Some(ref mut ll) = self.ll_state {
            ll.on_segment_boundary(seg_seq + 1);
        }
        self.pending_part_samples.clear();
        self.pending_segment_part_data.clear();

        if !self.ready && self.ring.len() >= self.config.ready_threshold {
            self.ready = true;
        }
        self.rebuild_playlist_cache();
    }

    /// Close a WebVTT subtitle segment aligned with the video segment boundary.
    ///
    /// 在与视频分段边界对齐处关闭 WebVTT 字幕分段。
    fn close_vtt_segment(&mut self, end_ms: u64) {
        let Some(mux) = self.vtt_mux.as_mut() else {
            return;
        };
        if mux.close_segment(end_ms).is_err() {
            return;
        }
        // `close_segment` appends exactly one segment; take it directly so we do
        // not miss it once the muxer's internal ring starts evicting old ones.
        if let Some(segment) = mux.segments().back().cloned() {
            self.pending_vtt_outputs
                .push(MuxerOutput::VttSegmentReady(segment.clone()));
            if self.vtt_segments.len() >= self.config.segment_count.max(1) {
                self.vtt_segments.pop_front();
            }
            self.vtt_segments.push_back(segment);
        }
        if !self.vtt_segments.is_empty() {
            self.vtt_ready = true;
        }
    }

    /// Push a WebVTT cue into the subtitle muxer.
    ///
    /// Cues are retained until the next video segment boundary closes the VTT segment.
    ///
    /// 向字幕复用器推入一条 WebVTT cue。
    ///
    /// cue 会被保留到下一个视频分段边界关闭 VTT 分段。
    pub fn push_cue(&mut self, cue: WebVttCue) -> Result<(), HlsCoreError> {
        if let Some(mux) = self.vtt_mux.as_mut() {
            mux.push_cue(cue)?;
        }
        Ok(())
    }

    /// Return the configured subtitle muxer, if any.
    ///
    /// 返回配置的字幕复用器（如有）。
    pub fn vtt_mux(&self) -> Option<&VttMux> {
        self.vtt_mux.as_ref()
    }

    /// Return completed WebVTT subtitle segments, oldest first.
    ///
    /// 返回已完成的 WebVTT 字幕分段，最旧的在前。
    pub fn vtt_segments(&self) -> &VecDeque<VttSegment> {
        &self.vtt_segments
    }

    /// Whether the subtitle track has produced at least one segment.
    ///
    /// 字幕轨道是否已产出至少一个分段。
    pub fn vtt_ready(&self) -> bool {
        self.vtt_ready
    }

    fn take_vtt_outputs(&mut self) -> Vec<MuxerOutput> {
        std::mem::take(&mut self.pending_vtt_outputs)
    }
}

/// Generate a high-entropy hex stream validation key.
///
/// Appended to segment/part URIs when `stream_key_validation` is enabled so
/// random URL guessing is ineffective.
///
/// 生成高熵十六进制流验证密钥。
///
/// 在启用 `stream_key_validation` 时附加到分段/分片 URI，使随机 URL 猜测无效。
pub(crate) fn generate_stream_validation_key() -> String {
    let mut random = [0_u8; 16];
    getrandom::getrandom(&mut random).expect("secure random stream validation key");

    let mut out = String::with_capacity(32);
    for byte in random {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Gzip-compress data with fast compression.
///
/// Falls back to the original bytes if compression fails.
///
/// 使用快速压缩对数据进行 gzip 压缩。
///
/// 压缩失败时回退到原始字节。
fn gzip_compress(data: &[u8]) -> Bytes {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut encoder = GzEncoder::new(Vec::with_capacity(data.len() / 2), Compression::fast());
    if encoder.write_all(data).is_ok() {
        if let Ok(compressed) = encoder.finish() {
            return Bytes::from(compressed);
        }
    }
    Bytes::copy_from_slice(data)
}

/// Append the stream-key token to all media URIs in a playlist.
///
/// Rewrites init, part, preload-hint, and segment URIs to include `?k=<token>`.
///
/// 将流密钥令牌追加到播放列表中的所有媒体 URI。
///
/// 重写 init、分片、预加载提示和分段 URI 以包含 `?k=<token>`。
fn append_stream_key_to_playlist_uris(playlist: &str, stream_key: &str) -> String {
    let mut out = String::with_capacity(playlist.len() + stream_key.len() * 4);
    for line in playlist.lines() {
        if line.starts_with("#EXT-X-MAP:")
            || line.starts_with("#EXT-X-PART:")
            || line.starts_with("#EXT-X-PRELOAD-HINT:")
        {
            out.push_str(&append_query_to_quoted_uri(line, "k", stream_key));
        } else if !line.is_empty() && !line.starts_with('#') {
            out.push_str(&append_query_to_uri(line, "k", stream_key));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

fn append_query_to_quoted_uri(line: &str, key: &str, value: &str) -> String {
    let Some(uri_pos) = line.find("URI=\"") else {
        return line.to_string();
    };
    let value_start = uri_pos + 5;
    let Some(value_end_offset) = line[value_start..].find('"') else {
        return line.to_string();
    };
    let value_end = value_start + value_end_offset;
    let mut out = String::with_capacity(line.len() + key.len() + value.len() + 2);
    out.push_str(&line[..value_start]);
    out.push_str(&append_query_to_uri(
        &line[value_start..value_end],
        key,
        value,
    ));
    out.push_str(&line[value_end..]);
    out
}

fn append_query_to_uri(uri: &str, key: &str, value: &str) -> String {
    let separator = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{separator}{key}={value}")
}

/// Extract SPS/PPS/VPS parameter sets as Annex-B start-code bytes.
///
/// These are prepended to TS keyframes and used for codec string generation.
///
/// 以 Annex-B 起始码字节形式提取 SPS/PPS/VPS 参数集。
///
/// 将其前置到 TS 关键帧并用于 codec 字符串生成。
fn extract_parameter_sets(extradata: &CodecExtradata) -> Option<Bytes> {
    match extradata {
        CodecExtradata::H264 { sps, pps, .. } => {
            let mut buf = Vec::new();
            for s in sps {
                buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                buf.extend_from_slice(s);
            }
            for p in pps {
                buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                buf.extend_from_slice(p);
            }
            if buf.is_empty() {
                None
            } else {
                Some(Bytes::from(buf))
            }
        }
        CodecExtradata::H265 { vps, sps, pps, .. } => {
            let mut buf = Vec::new();
            for v in vps {
                buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                buf.extend_from_slice(v);
            }
            for s in sps {
                buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                buf.extend_from_slice(s);
            }
            for p in pps {
                buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                buf.extend_from_slice(p);
            }
            if buf.is_empty() {
                None
            } else {
                Some(Bytes::from(buf))
            }
        }
        _ => None,
    }
}

/// Extract the AAC AudioSpecificConfig from track extradata.
///
/// 从轨道 extradata 提取 AAC AudioSpecificConfig。
fn extract_aac_config(extradata: &CodecExtradata) -> Option<AacAudioSpecificConfig> {
    match extradata {
        CodecExtradata::AAC { asc } => AacAudioSpecificConfig::from_bytes(asc),
        _ => None,
    }
}

/// Map a channel count to the ADTS `channel_configuration` enum.
///
/// ADTS reserves only 3 bits, so some layouts require a stereo fallback.
///
/// 将声道数映射到 ADTS 的 `channel_configuration` 枚举。
///
/// ADTS 仅保留 3 位，因此某些声道布局需要回退到立体声。
fn channels_to_aac_channel_configuration(channels: u8) -> Option<u8> {
    match channels {
        1 => Some(1),
        2 => Some(2),
        3 => Some(3),
        4 => Some(4),
        5 => Some(5),
        6 => Some(6),
        // 7-channel layouts (ASC ch_cfg=11) cannot be represented in ADTS' 3-bit field;
        // signal None so the caller picks a safe stereo fallback rather than emitting
        // garbage that decoders would reject.
        8 => Some(7),
        _ => None,
    }
}

/// Patch an AAC config so `channel_configuration` is non-zero.
///
/// Preserves PCE-based multichannel layouts when present and rewrites `ch_cfg=0`
/// for ADTS consumers.
///
/// 修补 AAC 配置，使 `channel_configuration` 非零。
///
/// 在存在 PCE 时保留多声道布局，并为 ADTS 消费者重写 `ch_cfg=0`。
fn patch_aac_config_channels(
    asc: Option<AacAudioSpecificConfig>,
    track_channels: u8,
) -> Option<AacAudioSpecificConfig> {
    let mut cfg = asc?;
    if cfg.channel_configuration == 0 {
        cfg.channel_configuration =
            channels_to_aac_channel_configuration(track_channels).unwrap_or(2);
    }
    Some(cfg)
}

/// Extract raw extradata suitable for fMP4 codec config boxes.
///
/// Builds avcC/hvcC from SPS/PPS/VPS when an explicit config is not present.
///
/// 提取适用于 fMP4 编解码器配置盒的原始 extradata。
///
/// 当没有显式配置时，根据 SPS/PPS/VPS 构建 avcC/hvcC。
pub(crate) fn extract_raw_extradata(extradata: &CodecExtradata) -> Bytes {
    match extradata {
        CodecExtradata::H264 { sps, pps, avcc } => avcc
            .clone()
            .unwrap_or_else(|| build_h264_avcc(sps.as_slice(), pps.as_slice())),
        CodecExtradata::H265 {
            vps,
            sps,
            pps,
            hvcc,
        } => hvcc
            .clone()
            .unwrap_or_else(|| build_h265_hvcc(vps.as_slice(), sps.as_slice(), pps.as_slice())),
        CodecExtradata::AAC { asc } => asc.clone(),
        CodecExtradata::AV1 { codec_config, .. } => codec_config.clone().unwrap_or_default(),
        CodecExtradata::VP9 { config, .. } => config.clone().unwrap_or_default(),
        CodecExtradata::Opus {
            channel_mapping, ..
        } => channel_mapping.clone().unwrap_or_default(),
        CodecExtradata::Raw(data) => data.clone(),
        _ => Bytes::new(),
    }
}

/// Build an H.264 avcC decoder configuration record.
///
/// Assembles profile/compat/level, SPS count and length, and PPS count and length.
///
/// 构建 H.264 avcC 解码器配置记录。
///
/// 组装 profile/compat/level、SPS 数量与长度以及 PPS 数量与长度。
fn build_h264_avcc(sps: &[Bytes], pps: &[Bytes]) -> Bytes {
    let Some(first_sps) = sps.first() else {
        return Bytes::new();
    };
    if first_sps.len() < 4 || pps.is_empty() {
        return Bytes::new();
    }

    let sps_count = sps.len().min(31);
    let pps_count = pps.len().min(255);
    let total_len = 6
        + sps
            .iter()
            .take(sps_count)
            .map(|unit| 2 + unit.len().min(u16::MAX as usize))
            .sum::<usize>()
        + 1
        + pps
            .iter()
            .take(pps_count)
            .map(|unit| 2 + unit.len().min(u16::MAX as usize))
            .sum::<usize>();
    let mut out = Vec::with_capacity(total_len);
    out.push(1);
    out.push(first_sps[1]);
    out.push(first_sps[2]);
    out.push(first_sps[3]);
    out.push(0xff);
    out.push(0xe0 | sps_count as u8);
    for unit in sps.iter().take(sps_count) {
        let unit = &unit[..unit.len().min(u16::MAX as usize)];
        out.extend_from_slice(&(unit.len() as u16).to_be_bytes());
        out.extend_from_slice(unit);
    }
    out.push(pps_count as u8);
    for unit in pps.iter().take(pps_count) {
        let unit = &unit[..unit.len().min(u16::MAX as usize)];
        out.extend_from_slice(&(unit.len() as u16).to_be_bytes());
        out.extend_from_slice(unit);
    }
    Bytes::from(out)
}

/// Build an H.265 hvcC decoder configuration record.
///
/// Parses profile_tier_level from the SPS and emits the VPS/SPS/PPS arrays.
///
/// 构建 H.265 hvcC 解码器配置记录。
///
/// 从 SPS 解析 profile_tier_level 并输出 VPS/SPS/PPS 数组。
fn build_h265_hvcc(vps: &[Bytes], sps: &[Bytes], pps: &[Bytes]) -> Bytes {
    if vps.is_empty() || sps.is_empty() || pps.is_empty() {
        return Bytes::new();
    }

    // Parse profile_tier_level from SPS.
    // SPS layout: [2-byte NAL header][4-bit vps_id + 3-bit max_sub_layers_minus1 + 1-bit nesting]
    //             [profile_tier_level: 1+4+6+1 = 12 bytes]
    // profile_tier_level bytes (starting at SPS byte offset 2, bit offset 8):
    //   byte 0: general_profile_space(2) | general_tier_flag(1) | general_profile_idc(5)
    //   bytes 1-4: general_profile_compatibility_flags (32 bits)
    //   bytes 5-10: general_constraint_indicator_flags (48 bits)
    //   byte 11: general_level_idc
    let first_sps = &sps[0];
    let (profile_byte, compat_flags, constraint_flags, level_idc) = if first_sps.len() >= 2 + 1 + 12
    {
        // SPS byte 2 = vps_id(4) + max_sub_layers_minus1(3) + temporal_id_nesting(1)
        // profile_tier_level starts at byte 3
        let ptl = &first_sps[3..];
        let pb = ptl[0]; // profile_space(2) + tier_flag(1) + profile_idc(5)
        let cf = u32::from_be_bytes([ptl[1], ptl[2], ptl[3], ptl[4]]);
        let mut cons = [0u8; 6];
        cons.copy_from_slice(&ptl[5..11]);
        let level = ptl[11];
        (pb, cf, cons, level)
    } else {
        // Fallback: Main profile, Level 4.0, set compatibility bit for Main
        (0x01, 0x60000000_u32, [0x90u8, 0, 0, 0, 0, 0], 120)
    };

    let mut out = Vec::new();
    out.push(1); // configurationVersion
    out.push(profile_byte); // general_profile_space + general_tier_flag + general_profile_idc
    out.extend_from_slice(&compat_flags.to_be_bytes()); // general_profile_compatibility_flags
    out.extend_from_slice(&constraint_flags); // general_constraint_indicator_flags
    out.push(level_idc); // general_level_idc
    out.extend_from_slice(&0xf000_u16.to_be_bytes()); // min_spatial_segmentation_idc
    out.push(0xfc); // parallelismType
    out.push(0xfc); // chromaFormat
    out.push(0xf8); // bitDepthLumaMinus8
    out.push(0xf8); // bitDepthChromaMinus8
    out.extend_from_slice(&0_u16.to_be_bytes()); // avgFrameRate
    out.push(0x0f); // temporal fields + lengthSizeMinusOne = 3 (4-byte NALU lengths)
    out.push(3); // numOfArrays: VPS, SPS, PPS

    append_hvcc_array(&mut out, 32, vps);
    append_hvcc_array(&mut out, 33, sps);
    append_hvcc_array(&mut out, 34, pps);

    Bytes::from(out)
}

/// Append one hvcC array (NAL type, count, and length-prefixed units).
///
/// 向 hvcC 追加一个数组（NAL 类型、数量与长度前缀单元）。
fn append_hvcc_array(out: &mut Vec<u8>, nal_unit_type: u8, units: &[Bytes]) {
    out.push(0x80 | (nal_unit_type & 0x3f)); // array_completeness + NAL unit type
    out.extend_from_slice(&(units.len().min(u16::MAX as usize) as u16).to_be_bytes());
    for unit in units.iter().take(u16::MAX as usize) {
        let unit = &unit[..unit.len().min(u16::MAX as usize)];
        out.extend_from_slice(&(unit.len() as u16).to_be_bytes());
        out.extend_from_slice(unit);
    }
}

/// Convert a frame payload into the fMP4 sample format.
///
/// Canonical H.26x is converted from Annex-B start codes to 4-byte length prefix.
///
/// 将帧负载转换为 fMP4 采样格式。
///
/// 将标准 H.26x 从 Annex-B 起始码转换为 4 字节长度前缀。
pub(crate) fn fmp4_sample_payload(frame: &AVFrame) -> Bytes {
    if frame.format != cheetah_codec::FrameFormat::CanonicalH26x {
        return frame.payload.clone();
    }
    if !matches!(frame.codec, CodecId::H264 | CodecId::H265 | CodecId::H266) {
        return frame.payload.clone();
    }
    h26x_length_prefixed_from_payload(frame.payload.clone())
}

/// Convert microsecond DTS to 90 kHz clock ticks for MPEG-TS.
///
/// 将微秒 DTS 转换为 MPEG-TS 使用的 90 kHz 时钟刻度。
fn us_to_90k(us: u64) -> u64 {
    us * 9 / 100
}

/// Convert length-prefixed H.26x NALUs to Annex-B start-code format.
///
/// If the payload is already Annex-B, it is returned unchanged.
///
/// 将长度前缀 H.26x NALU 转换为 Annex-B 起始码格式。
///
/// 如果负载已经是 Annex-B，则原样返回。
fn to_annexb(payload: &[u8]) -> Vec<u8> {
    // Quick check: if starts with start code, already Annex-B
    if payload.len() >= 4 && payload[0] == 0 && payload[1] == 0 {
        if payload[2] == 0 && payload[3] == 1 {
            return payload.to_vec();
        }
        if payload[2] == 1 {
            return payload.to_vec();
        }
    }

    // Convert 4-byte length-prefixed NALUs to Annex-B
    let mut out = Vec::with_capacity(payload.len() + 32);
    let mut cursor = payload;
    while cursor.len() >= 4 {
        let size = u32::from_be_bytes([cursor[0], cursor[1], cursor[2], cursor[3]]) as usize;
        cursor = &cursor[4..];
        if size == 0 || cursor.len() < size {
            break;
        }
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        out.extend_from_slice(&cursor[..size]);
        cursor = &cursor[size..];
    }
    if out.is_empty() {
        payload.to_vec()
    } else {
        out
    }
}

/// Health tracking for muxer crash recovery.
///
/// Tracks crash frequency and exponential backoff delay for rebuild attempts.
///
/// 复用器崩溃恢复的健康跟踪。
///
/// 记录崩溃频率与重建尝试的指数退避延迟。
#[allow(dead_code)]
pub struct MuxerHealth {
    crash_count: u32,
    last_crash_us: u64,
    rebuild_delay_ms: u64,
}

#[allow(dead_code)]
/// Create a healthy muxer state with no crashes.
///
/// 创建无崩溃的健康复用器状态。
impl MuxerHealth {
    pub fn new() -> Self {
        Self {
            crash_count: 0,
            last_crash_us: 0,
            rebuild_delay_ms: 0,
        }
    }

    /// Record a crash and return the next rebuild delay in milliseconds.
    ///
    /// Uses exponential backoff capping at 30 s.
    ///
    /// 记录崩溃并返回下次重建延迟（毫秒）。
    ///
    /// 使用指数退避，上限 30 秒。
    pub fn on_crash(&mut self, now_us: u64) -> u64 {
        self.crash_count += 1;
        self.last_crash_us = now_us;
        // Exponential backoff: 1s, 2s, 4s, 8s, 16s, max 30s
        self.rebuild_delay_ms = (1000 * (1u64 << self.crash_count.min(4))).min(30_000);
        self.rebuild_delay_ms
    }

    /// Reset backoff after a successful rebuild.
    ///
    /// 成功重建后重置退避。
    pub fn on_rebuild_success(&mut self) {
        self.crash_count = 0;
        self.rebuild_delay_ms = 0;
    }

    /// Whether too many crashes have occurred to keep rebuilding.
    ///
    /// 是否因崩溃次数过多而放弃重建。
    pub fn should_give_up(&self) -> bool {
        self.crash_count >= 10
    }

    /// Return the number of recorded crashes.
    ///
    /// 返回已记录的崩溃次数。
    pub fn crash_count(&self) -> u32 {
        self.crash_count
    }
}

/// Default `MuxerHealth` with no crash history.
///
/// 没有崩溃历史的默认 `MuxerHealth`。
impl Default for MuxerHealth {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_codec::{
        subtitle::WebVttCue, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase,
        TrackId, TrackInfo,
    };
    use cheetah_hls_core::{Fmp4DemuxEvent, Fmp4Demuxer, VttMuxConfig};

    fn make_config_llhls() -> StreamMuxerConfig {
        StreamMuxerConfig {
            segment_duration_ms: 2000,
            segment_count: 3,
            ready_threshold: 1,
            force_segment_after_ms: 10000,
            fast_register: true,
            container: HlsContainer::Fmp4,
            ll_hls_enabled: true,
            part_target_ms: 200,
            max_completed_segments: 5,
            ll_hls_packaging_mode: LlHlsPackagingMode::VideoOnly,
            origin_mode: false,
            stream_name: String::new(),
            vtt_config: None,
        }
    }

    fn make_video_frame(dts_us: i64, keyframe: bool) -> cheetah_codec::AVFrame {
        let tb = Timebase::new(1, 1_000_000);
        let mut frame = cheetah_codec::AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            dts_us,
            dts_us,
            tb,
            Bytes::from(vec![0u8; 100]),
        );
        if keyframe {
            frame.flags |= FrameFlags::KEY;
        }
        frame
    }

    fn make_audio_frame(dts_us: i64) -> cheetah_codec::AVFrame {
        cheetah_codec::AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            dts_us,
            dts_us,
            Timebase::new(1, 1_000_000),
            Bytes::from_static(&[0x21, 0x16, 0xc4, 0x79]),
        )
    }

    fn setup_muxer() -> StreamMuxer {
        let mut muxer = StreamMuxer::new(make_config_llhls());
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90000);
        track.width = Some(1920);
        track.height = Some(1080);
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1e])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x38])],
            avcc: Some(Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E, 0x01, 0x00,
                0x03, 0x68, 0xCE, 0x38,
            ])),
        };
        muxer.set_tracks(&[track]);
        muxer
    }

    fn mdat_payload(data: &[u8]) -> &[u8] {
        let mdat_pos = data.windows(4).position(|w| w == b"mdat").expect("mdat");
        &data[mdat_pos + 4..]
    }

    #[test]
    fn h264_extradata_without_avcc_builds_fmp4_decoder_config() {
        let extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1e])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x38])],
            avcc: None,
        };

        let avcc = extract_raw_extradata(&extradata);

        assert_eq!(
            avcc.as_ref(),
            &[
                0x01, 0x42, 0x00, 0x1e, 0xff, 0xe1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1e, 0x01, 0x00,
                0x03, 0x68, 0xce, 0x38,
            ]
        );
    }

    #[test]
    fn h265_extradata_without_hvcc_builds_fmp4_decoder_config() {
        // Realistic H265 SPS with profile_tier_level:
        // NAL header: 0x42, 0x01 (SPS, temporal_id=1)
        // Byte 2: vps_id=0(4b) + max_sub_layers_minus1=0(3b) + temporal_id_nesting=1(1b) = 0x01
        // profile_tier_level (12 bytes):
        //   byte 0: profile_space=0(2b) + tier_flag=0(1b) + profile_idc=1(5b) = 0x01
        //   bytes 1-4: profile_compatibility_flags = 0x60000000 (Main compatible)
        //   bytes 5-10: constraint_indicator_flags = [0x90, 0x00, 0x00, 0x00, 0x00, 0x00]
        //   byte 11: level_idc = 153 (Level 5.1)
        let sps = Bytes::from_static(&[
            0x42, 0x01, // NAL header (SPS)
            0x01, // vps_id + max_sub_layers + nesting
            0x01, // profile_space=0, tier=0, profile_idc=1 (Main)
            0x60, 0x00, 0x00, 0x00, // profile_compatibility_flags
            0x90, 0x00, 0x00, 0x00, 0x00, 0x00, // constraint_indicator_flags
            0x99, // level_idc = 153 (Level 5.1)
            0x00, 0x00, // remaining SPS data (truncated, not needed for hvcC)
        ]);
        let extradata = CodecExtradata::H265 {
            vps: vec![Bytes::from_static(&[0x40, 0x01, 0x0c, 0x01])],
            sps: vec![sps],
            pps: vec![Bytes::from_static(&[0x44, 0x01, 0xc0])],
            hvcc: None,
        };

        let hvcc = extract_raw_extradata(&extradata);

        assert!(!hvcc.is_empty());
        // configurationVersion
        assert_eq!(hvcc[0], 1);
        // profile_space=0, tier=0, profile_idc=1
        assert_eq!(hvcc[1], 0x01);
        // profile_compatibility_flags = 0x60000000
        assert_eq!(&hvcc[2..6], &[0x60, 0x00, 0x00, 0x00]);
        // constraint_indicator_flags
        assert_eq!(&hvcc[6..12], &[0x90, 0x00, 0x00, 0x00, 0x00, 0x00]);
        // level_idc = 153
        assert_eq!(hvcc[12], 0x99);
        // lengthSizeMinusOne = 3
        assert_eq!(hvcc[21] & 0x03, 3);
        // numOfArrays = 3
        assert_eq!(hvcc[22], 3);
        // VPS, SPS, PPS data present
        assert!(hvcc.windows(4).any(|w| w == [0x40, 0x01, 0x0c, 0x01]));
        assert!(hvcc.windows(3).any(|w| w == [0x44, 0x01, 0xc0]));
    }

    #[test]
    fn fmp4_h264_samples_are_length_prefixed_not_annexb() {
        let mut muxer = setup_muxer();
        let mut keyframe = make_video_frame(0, true);
        keyframe.payload = Bytes::from_static(&[0, 0, 0, 1, 0x65, 0xaa, 0xbb]);
        muxer.push_frame(&keyframe);

        let mut next_keyframe = make_video_frame(33_000, true);
        next_keyframe.payload = Bytes::from_static(&[0, 0, 0, 1, 0x65, 0xcc, 0xdd]);
        muxer.push_frame(&next_keyframe);

        let (_name, segment) = muxer.latest_segment().expect("finalized segment");
        let payload = mdat_payload(&segment);
        assert_eq!(&payload[..4], &[0, 0, 0, 3]);
        assert_eq!(&payload[4..7], &[0x65, 0xaa, 0xbb]);
        assert!(!payload.starts_with(&[0, 0, 0, 1]));
    }

    #[test]
    fn fmp4_h264_raw_nal_samples_are_length_prefixed() {
        let mut muxer = setup_muxer();
        let mut keyframe = make_video_frame(0, true);
        keyframe.payload = Bytes::from_static(&[0x65, 0xaa, 0xbb]);
        muxer.push_frame(&keyframe);

        let mut next_keyframe = make_video_frame(33_000, true);
        next_keyframe.payload = Bytes::from_static(&[0x65, 0xcc, 0xdd]);
        muxer.push_frame(&next_keyframe);

        let (_name, segment) = muxer.latest_segment().expect("finalized segment");
        let payload = mdat_payload(&segment);
        assert_eq!(&payload[..4], &[0, 0, 0, 3]);
        assert_eq!(&payload[4..7], &[0x65, 0xaa, 0xbb]);
    }

    #[test]
    fn fmp4_samples_follow_init_track_kind_when_source_track_ids_are_inverted() {
        let mut muxer = StreamMuxer::new(make_config_llhls());
        let mut audio_track = TrackInfo::new(TrackId(1), MediaKind::Audio, CodecId::AAC, 48000);
        audio_track.sample_rate = Some(48000);
        audio_track.channels = Some(6);
        audio_track.extradata = CodecExtradata::AAC {
            asc: Bytes::from_static(&[0x11, 0xb0]),
        };
        let mut video_track = TrackInfo::new(TrackId(2), MediaKind::Video, CodecId::H264, 90000);
        video_track.width = Some(1920);
        video_track.height = Some(1080);
        video_track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1e])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x38])],
            avcc: Some(Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1e, 0xff, 0xe1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1e, 0x01, 0x00,
                0x03, 0x68, 0xce, 0x38,
            ])),
        };
        muxer.set_tracks(&[audio_track, video_track]);

        let mut video = make_video_frame(0, true);
        video.payload = Bytes::from_static(&[0x65, 0xaa, 0xbb]);
        muxer.push_frame(&video);
        muxer.push_frame(&make_audio_frame(0));
        let mut next_video = make_video_frame(33_000, true);
        next_video.payload = Bytes::from_static(&[0x65, 0xcc, 0xdd]);
        muxer.push_frame(&next_video);

        let init = muxer.init_segment().expect("init segment");
        let (_name, segment) = muxer.latest_segment().expect("finalized segment");
        let mut demuxer = Fmp4Demuxer::new();
        demuxer.parse_init(&init).expect("parse init");
        let events = demuxer.parse_segment(&segment).expect("parse segment");
        let video_sample = events
            .iter()
            .find_map(|event| match event {
                Fmp4DemuxEvent::Frame {
                    media_kind: MediaKind::Video,
                    data,
                    ..
                } => Some(data),
                _ => None,
            })
            .expect("video sample");

        assert_eq!(&video_sample[..4], &[0, 0, 0, 3]);
        assert_eq!(&video_sample[4..7], &[0x65, 0xaa, 0xbb]);
    }

    #[test]
    fn fmp4_waits_for_initial_video_keyframe_before_segmenting() {
        let mut muxer = setup_muxer();

        let first = make_video_frame(0, false);
        assert!(muxer.push_frame(&first).is_empty());
        let force_cut_candidate = make_video_frame(12_000_000, false);
        assert!(muxer.push_frame(&force_cut_candidate).is_empty());
        assert!(muxer.latest_segment().is_none());

        let keyframe = make_video_frame(12_033_000, true);
        assert!(muxer.push_frame(&keyframe).is_empty());
        assert!(muxer.latest_segment().is_none());
    }

    #[test]
    fn playlist_with_stream_key_validation_token_covers_all_media_uris() {
        let mut muxer = setup_muxer();
        let mut keyframe = make_video_frame(0, true);
        keyframe.payload = Bytes::from_static(&[0, 0, 0, 1, 0x65, 0xaa, 0xbb]);
        muxer.push_frame(&keyframe);

        let mut next_keyframe = make_video_frame(33_000, true);
        next_keyframe.payload = Bytes::from_static(&[0, 0, 0, 1, 0x65, 0xcc, 0xdd]);
        muxer.push_frame(&next_keyframe);

        let playlist = muxer.playlist_with_options_and_token(Some(7), false, true);
        let token = muxer.stream_key();
        assert!(playlist.contains(&format!("init.mp4?uid=7&k={token}")));
        assert!(playlist.contains(&format!("seg_0.m4s?uid=7&k={token}")));
        assert!(playlist.contains(&format!("part_0.m4s?k={token}")));
        assert!(playlist.contains(&format!(
            "PRELOAD-HINT:TYPE=PART,URI=\"part_1.m4s?k={token}\""
        )));
    }

    #[test]
    fn rewind_playlist_only_lists_segments_still_in_memory_ring() {
        let mut muxer = setup_muxer();
        for dts_us in [0, 33_000, 66_000, 2_100_000, 4_200_000, 6_300_000] {
            muxer.push_frame(&make_video_frame(dts_us, true));
        }

        assert!(muxer.rewind_history.len() > muxer.ring.len());
        let playlist = muxer.playlist_rewind(None);

        for meta in &muxer.rewind_history {
            let listed = playlist.contains(&format!("{}.m4s", meta.name));
            let retrievable = muxer.get_segment(&meta.name).is_some();
            assert_eq!(listed, retrievable, "rewind advertised {}", meta.name);
        }
    }

    #[test]
    fn llhls_muxer_produces_parts() {
        let mut muxer = setup_muxer();

        // Feed frames at 33ms intervals (30fps), 200ms part target = ~6 frames per part
        let mut outputs_all = Vec::new();
        for i in 0..20 {
            let dts_us = i * 33_000;
            let keyframe = i == 0;
            let frame = make_video_frame(dts_us, keyframe);
            let outputs = muxer.push_frame(&frame);
            outputs_all.extend(outputs);
        }

        let part_count = outputs_all
            .iter()
            .filter(|o| matches!(o, MuxerOutput::PartReady(_)))
            .count();
        assert!(part_count >= 2, "expected >=2 parts, got {part_count}");
    }

    #[test]
    fn llhls_playlist_contains_part_tags() {
        let mut muxer = setup_muxer();

        // Feed enough frames to produce at least one segment with parts
        for i in 0..120 {
            let dts_us = i * 33_000;
            let keyframe = i % 60 == 0;
            let frame = make_video_frame(dts_us, keyframe);
            muxer.push_frame(&frame);
        }

        let playlist = muxer.playlist(None);
        assert!(playlist.contains("#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES"));
        assert!(playlist.contains("#EXT-X-PART-INF:PART-TARGET=0.200"));
        assert!(playlist.contains("#EXT-X-PART:DURATION="));
        assert!(playlist.contains("#EXT-X-PRELOAD-HINT:TYPE=PART"));
        assert!(playlist.contains("#EXT-X-PROGRAM-DATE-TIME:"));
    }

    #[test]
    fn llhls_get_part_returns_data() {
        let mut muxer = setup_muxer();

        for i in 0..20 {
            let dts_us = i * 33_000;
            let keyframe = i == 0;
            let frame = make_video_frame(dts_us, keyframe);
            muxer.push_frame(&frame);
        }

        let part = muxer.get_part(0);
        assert!(part.is_some(), "part_0 should be available");
        let data = part.unwrap();
        assert!(!data.is_empty());
        assert!(data.windows(4).any(|w| w == b"moof"));
    }

    #[test]
    fn vtt_segments_produced_on_video_segment_boundary() {
        let mut config = make_config_llhls();
        config.vtt_config = Some(VttMuxConfig::default());
        let mut muxer = StreamMuxer::new(config);
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90000);
        track.width = Some(1920);
        track.height = Some(1080);
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1e])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x38])],
            avcc: None,
        };
        muxer.set_tracks(&[track]);

        muxer
            .push_cue(WebVttCue {
                id: None,
                start_ms: 500,
                end_ms: 3_500,
                payload: "Hello".to_string(),
                settings: None,
            })
            .unwrap();

        for i in 0..2 {
            let dts_us = i * 2_000_000;
            let keyframe = i == 0 || i == 1;
            let outputs = muxer.push_frame(&make_video_frame(dts_us, keyframe));
            if i == 1 {
                assert!(
                    outputs
                        .iter()
                        .any(|o| matches!(o, MuxerOutput::VttSegmentReady(_))),
                    "expected a VTT segment when the first video segment closes"
                );
            }
        }

        assert_eq!(muxer.vtt_segments().len(), 1);
        let seg = muxer.vtt_segments().back().unwrap();
        assert!(seg.payload.contains("Hello"));
        assert!(seg.payload.contains("00:00:00.500 --> 00:00:02.000"));
        assert!(muxer.vtt_ready());
        assert!(muxer.vtt_mux().is_some());
    }

    #[test]
    fn vtt_media_playlist_can_be_built_from_muxer() {
        let mut config = make_config_llhls();
        config.vtt_config = Some(VttMuxConfig::default());
        let mut muxer = StreamMuxer::new(config);
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90000);
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1e])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x38])],
            avcc: None,
        };
        muxer.set_tracks(&[track]);

        muxer
            .push_cue(WebVttCue {
                id: None,
                start_ms: 500,
                end_ms: 3_500,
                payload: "Hello".to_string(),
                settings: None,
            })
            .unwrap();
        muxer.push_frame(&make_video_frame(0, true));
        muxer.push_frame(&make_video_frame(2_000_000, true));

        let m3u8 =
            cheetah_hls_core::PlaylistBuilder::build_vtt_media(muxer.vtt_mux().unwrap(), Some(42));
        assert!(m3u8.contains("#EXTM3U"));
        assert!(m3u8.contains("sub0.vtt?uid=42"));
    }

    #[test]
    fn vtt_segments_captured_after_internal_ring_is_full() {
        let mut config = make_config_llhls();
        config.segment_duration_ms = 1000;
        config.vtt_config = Some(VttMuxConfig {
            segment_duration_ms: 1000,
            max_segments: 2,
        });
        let mut muxer = StreamMuxer::new(config);
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90000);
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1e])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x38])],
            avcc: None,
        };
        muxer.set_tracks(&[track]);

        let mut vtt_count = 0;
        for i in 0..5 {
            let dts_us = i * 1_000_000;
            let outputs = muxer.push_frame(&make_video_frame(dts_us, true));
            vtt_count += outputs
                .iter()
                .filter(|o| matches!(o, MuxerOutput::VttSegmentReady(_)))
                .count();
        }
        assert_eq!(
            vtt_count, 4,
            "expected one VTT segment per closed video segment"
        );
        assert!(muxer.vtt_segments().len() <= 3);
        assert_eq!(
            muxer.vtt_segments().back().unwrap().sequence,
            3,
            "latest segment should be sub3 even after internal ring eviction"
        );
    }

    #[test]
    fn concluded_llhls_stream_satisfies_blocking_playlist_request() {
        let mut muxer = setup_muxer();

        for i in 0..20 {
            let dts_us = i * 33_000;
            let keyframe = i == 0;
            muxer.push_frame(&make_video_frame(dts_us, keyframe));
        }

        let waiting_msn = muxer.current_msn() + 1;
        let waiting_part = None;
        assert!(!muxer.is_blocking_satisfied(waiting_msn, waiting_part));

        muxer.conclude();

        assert!(muxer.is_blocking_satisfied(waiting_msn, waiting_part));
    }

    #[test]
    fn traditional_hls_mode_no_parts() {
        let config = StreamMuxerConfig {
            segment_duration_ms: 2000,
            segment_count: 3,
            ready_threshold: 1,
            force_segment_after_ms: 10000,
            fast_register: true,
            container: HlsContainer::Fmp4,
            ll_hls_enabled: false,
            part_target_ms: 200,
            max_completed_segments: 5,
            ll_hls_packaging_mode: LlHlsPackagingMode::VideoOnly,
            origin_mode: false,
            stream_name: String::new(),
            vtt_config: None,
        };
        let mut muxer = StreamMuxer::new(config);
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90000);
        track.width = Some(1920);
        track.height = Some(1080);
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1e])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x38])],
            avcc: Some(Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E, 0x01, 0x00,
                0x03, 0x68, 0xCE, 0x38,
            ])),
        };
        muxer.set_tracks(&[track]);

        for i in 0..20 {
            let dts_us = i * 33_000;
            let keyframe = i == 0;
            let frame = make_video_frame(dts_us, keyframe);
            let outputs = muxer.push_frame(&frame);
            assert!(
                !outputs
                    .iter()
                    .any(|o| matches!(o, MuxerOutput::PartReady(_))),
                "no parts in traditional mode"
            );
        }

        let playlist = muxer.playlist(None);
        assert!(!playlist.contains("#EXT-X-PART"));
        assert!(!playlist.contains("#EXT-X-SERVER-CONTROL"));
    }

    #[test]
    fn extinf_includes_last_frame_duration_at_keyframe_cut() {
        let mut config = make_config_llhls();
        config.fast_register = false;
        config.ll_hls_enabled = false;
        config.segment_duration_ms = 2000;
        let mut muxer = StreamMuxer::new(config);
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90000);
        track.width = Some(1920);
        track.height = Some(1080);
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1e])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x38])],
            avcc: Some(Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E, 0x01, 0x00,
                0x03, 0x68, 0xCE, 0x38,
            ])),
        };
        muxer.set_tracks(&[track]);

        // 20 fps (50 ms inter-frame), keyframe every 60 frames (3 s GOP).
        // segment_duration_ms = 2000 => cuts at every keyframe (3 s elapsed >= 2 s).
        let frame_interval_us: i64 = 50_000;
        let kf_interval = 60;
        for i in 0..(kf_interval * 3) {
            let dts_us = i as i64 * frame_interval_us;
            let keyframe = i % kf_interval == 0;
            muxer.push_frame(&make_video_frame(dts_us, keyframe));
        }

        let playlist = muxer.playlist(None);
        let extinf_durations: Vec<f64> = playlist
            .lines()
            .filter_map(|l| l.strip_prefix("#EXTINF:"))
            .filter_map(|l| l.split(',').next())
            .filter_map(|s| s.parse::<f64>().ok())
            .collect();

        assert!(
            !extinf_durations.is_empty(),
            "expected at least one EXTINF, playlist:\n{playlist}"
        );
        // 60 frames at 50 ms = 3.000 s actual coverage; tolerate 1 ms float noise.
        for d in &extinf_durations {
            assert!(
                (*d - 3.000).abs() < 0.005,
                "EXTINF must include last frame duration (expected 3.000, got {d}); \
                 dropping it causes accumulated drift and 1 s stalls on the player. \
                 playlist:\n{playlist}"
            );
        }
    }
}
