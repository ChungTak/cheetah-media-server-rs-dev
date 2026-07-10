//! Per-stream HLS muxer: receives AVFrames, produces TS/fMP4 segments, manages playlist.

use bytes::Bytes;
use cheetah_codec::{
    aac_channel_count_from_asc, adts_wrap, h26x_length_prefixed_from_payload, AVFrame,
    AacAudioSpecificConfig, CodecExtradata, CodecId, FrameFlags, MediaKind, TrackInfo,
};
use cheetah_hls_core::{
    Fmp4Muxer, Fmp4Sample, Fmp4TrackDesc, HlsContainer, HlsPart, LlHlsPackagingMode,
    LowLatencyState, PlaylistBuilder, SegmentRing, TrackLane, TsMuxer,
};

use crate::demuxed_muxer::{DemuxedMuxerConfig, DemuxedStreamMuxer};

/// Configuration for the stream muxer.
#[derive(Debug, Clone)]
pub struct StreamMuxerConfig {
    /// `segment_duration_ms` field of type `u64`.
    /// `segment_duration_ms` 字段，类型为 `u64`.
    pub segment_duration_ms: u64,
    /// `segment_count` field of type `usize`.
    /// `segment_count` 字段，类型为 `usize`.
    pub segment_count: usize,
    /// `ready_threshold` field of type `usize`.
    /// `ready_threshold` 字段，类型为 `usize`.
    pub ready_threshold: usize,
    /// `force_segment_after_ms` field of type `u64`.
    /// `force_segment_after_ms` 字段，类型为 `u64`.
    pub force_segment_after_ms: u64,
    /// Force first 2 segments to cut on any keyframe for fast stream discovery.
    pub fast_register: bool,
    /// Container format: Ts or Fmp4.
    pub container: HlsContainer,
    /// Enable LL-HLS part generation (requires fMP4 container).
    pub ll_hls_enabled: bool,
    /// Part target duration in milliseconds (default 200).
    pub part_target_ms: u64,
    /// Maximum number of completed segment part-lists to retain.
    pub max_completed_segments: usize,
    /// LL-HLS packaging mode.
    pub ll_hls_packaging_mode: LlHlsPackagingMode,
    /// Origin mode hint retained for config compatibility; validation keys are always random.
    #[allow(dead_code)]
    pub origin_mode: bool,
    /// Stream name retained for config compatibility; not used for validation key generation.
    #[allow(dead_code)]
    pub stream_name: String,
}

/// Lightweight segment metadata kept for DVR/rewind playlist generation.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SegmentMeta {
    /// `name` field of type `String`.
    /// `name` 字段，类型为 `String`.
    pub name: String,
    /// `duration_secs` field of type `f64`.
    /// `duration_secs` 字段，类型为 `f64`.
    pub duration_secs: f64,
    /// `sequence` field of type `u64`.
    /// `sequence` 字段，类型为 `u64`.
    pub sequence: u64,
    /// `program_date_time_ms` field.
    /// `program_date_time_ms` 字段.
    pub program_date_time_ms: Option<i64>,
    /// `markers` field.
    /// `markers` 字段.
    pub markers: Vec<cheetah_hls_core::CueMarker>,
}

/// Output event from the muxer when a segment or part is produced.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum MuxerOutput {
    /// `SegmentReady` variant.
    /// `SegmentReady` 变体.
    SegmentReady {
        name: String,
        duration_secs: f64,
        data: Bytes,
    },
    /// `PartReady` variant.
    /// `PartReady` 变体.
    PartReady(HlsPart),
}

/// Per-stream HLS muxer state.
pub struct StreamMuxer {
    /// `config` field of type `StreamMuxerConfig`.
    /// `config` 字段，类型为 `StreamMuxerConfig`.
    config: StreamMuxerConfig,
    /// `ts_muxer` field.
    /// `ts_muxer` 字段.
    ts_muxer: Option<TsMuxer>,
    /// `fmp4_muxer` field.
    /// `fmp4_muxer` 字段.
    fmp4_muxer: Option<Fmp4Muxer>,
    /// Cached fMP4 init segment.
    fmp4_init: Option<Bytes>,
    /// Pending fMP4 samples for current segment.
    pending_fmp4_samples: Vec<Fmp4Sample>,
    /// Pending fMP4 samples for current part (LL-HLS).
    pending_part_samples: Vec<Fmp4Sample>,
    /// Accumulated part binary data for current segment (LL-HLS).
    /// Segment = concatenation of all its parts.
    pending_segment_part_data: Vec<Bytes>,
    /// `ring` field of type `SegmentRing`.
    /// `ring` 字段，类型为 `SegmentRing`.
    ring: SegmentRing,
    /// LL-HLS state (None when ll_hls disabled or container is TS).
    ll_state: Option<LowLatencyState>,
    /// `video_codec` field of type `CodecId`.
    /// `video_codec` 字段，类型为 `CodecId`.
    video_codec: CodecId,
    /// `audio_codec` field of type `CodecId`.
    /// `audio_codec` 字段，类型为 `CodecId`.
    audio_codec: CodecId,
    /// `has_video` field of type `bool`.
    /// `has_video` 字段，类型为 `bool`.
    has_video: bool,
    /// `has_audio` field of type `bool`.
    /// `has_audio` 字段，类型为 `bool`.
    has_audio: bool,
    /// AAC AudioSpecificConfig for ADTS wrapping.
    aac_config: Option<AacAudioSpecificConfig>,
    /// Cached parameter sets (SPS/PPS/VPS as Annex-B) for segment-start prepending.
    parameter_sets: Option<Bytes>,
    /// Raw extradata bytes for fMP4 codec config boxes.
    video_extradata: Bytes,
    /// `audio_extradata` field of type `Bytes`.
    /// `audio_extradata` 字段，类型为 `Bytes`.
    audio_extradata: Bytes,
    /// `video_width` field of type `u16`.
    /// `video_width` 字段，类型为 `u16`.
    video_width: u16,
    /// `video_height` field of type `u16`.
    /// `video_height` 字段，类型为 `u16`.
    video_height: u16,
    /// `audio_sample_rate` field of type `u32`.
    /// `audio_sample_rate` 字段，类型为 `u32`.
    audio_sample_rate: u32,
    /// `audio_channels` field of type `u8`.
    /// `audio_channels` 字段，类型为 `u8`.
    audio_channels: u8,
    /// Segment timing state.
    segment_start_dts: Option<u64>,
    /// `segment_last_dts` field of type `u64`.
    /// `segment_last_dts` 字段，类型为 `u64`.
    segment_last_dts: u64,
    /// Last observed inter-frame DTS interval for video (microseconds).
    /// Used as a fallback to estimate the last frame's display duration when
    /// the next segment's start DTS is not yet known (e.g. flush at end-of-stream).
    last_video_frame_interval_us: Option<u64>,
    /// DTS of the previous video frame (microseconds), tracked to compute
    /// `last_video_frame_interval_us`.
    prev_video_dts_us: Option<u64>,
    /// `segment_has_keyframe` field of type `bool`.
    /// `segment_has_keyframe` 字段，类型为 `bool`.
    segment_has_keyframe: bool,
    /// `segment_seq` field of type `u64`.
    /// `segment_seq` 字段，类型为 `u64`.
    segment_seq: u64,
    /// `ready` field of type `bool`.
    /// `ready` 字段，类型为 `bool`.
    ready: bool,
    /// Whether muxing is enabled (for hls_demand mode).
    pub enabled: bool,
    /// Cached default playlist (regenerated on each part/segment).
    cached_playlist: Option<String>,
    /// Cached gzip-compressed playlist (pre-generated to avoid per-request compression).
    cached_playlist_gzip: Option<Bytes>,
    /// Stream has been concluded (EXT-X-ENDLIST).
    concluded: bool,
    /// Wallclock offset: publish_time_ms - first_sample_dts_ms.
    wallclock_offset_ms: Option<i64>,
    /// Stream key for URL validation (prevents URL guessing).
    stream_key: String,
    /// History of evicted segment metadata for DVR/rewind playlist.
    rewind_history: Vec<SegmentMeta>,
    /// Pending CUE markers to be associated with the next segment.
    pending_markers: Vec<cheetah_hls_core::CueMarker>,
    /// Demuxed LLHLS muxer (active when packaging_mode = DemuxedAv).
    demuxed: Option<DemuxedStreamMuxer>,
}

impl StreamMuxer {
    /// Creates a new instance.
    /// 创建 新的 实例.
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
        }
    }

    /// Initialize or update tracks.
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

    /// Feed a frame into the muxer. Returns outputs (segment/part ready events).
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

        match self.config.container {
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
        }
    }

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

    fn waiting_for_initial_video_keyframe(&self, frame: &AVFrame) -> bool {
        self.has_video
            && self.segment_start_dts.is_none()
            && !(frame.media_kind == MediaKind::Video && frame.flags.contains(FrameFlags::KEY))
    }

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

    /// Force finalize the current segment.
    pub fn flush(&mut self) {
        if self.segment_start_dts.is_some() {
            self.try_finalize_current_part();
            // No next-segment hint at flush time: finalize_segment will fall back
            // to last_dts + estimated frame interval so EXTINF still includes the
            // final frame's display duration.
            self.finalize_segment(None);
        }
    }

    /// Returns `true` if `ready` is true.
    /// 返回 `真` 如果 `ready` is 真.
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// `playlist` function.
    /// `playlist` 函数.
    pub fn playlist(&self, session_id: Option<u64>) -> String {
        // Use cached playlist when no special parameters
        if session_id.is_none() {
            if let Some(ref cached) = self.cached_playlist {
                return cached.clone();
            }
        }
        self.playlist_with_options(session_id, false)
    }

    /// `playlist_with_options_and_token` function.
    /// `playlist_with_options_and_token` 函数.
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

    /// Get pre-compressed gzip cached playlist (avoids per-request compression).
    pub fn cached_playlist_gzip(&self) -> Option<Bytes> {
        self.cached_playlist_gzip.clone()
    }

    /// Generate playlist with legacy mode option.
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

    /// Rebuild the cached default playlist. Call after each part/segment change.
    fn rebuild_playlist_cache(&mut self) {
        if self.ready {
            let content = self.playlist_with_options(None, false);
            // Pre-generate gzip version
            self.cached_playlist_gzip = Some(gzip_compress(content.as_bytes()));
            self.cached_playlist = Some(content);
        }
    }

    /// Compute frame-aligned part duration based on video fps or audio sample rate.
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

    /// Conclude the live stream (append EXT-X-ENDLIST). No more frames accepted.
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
    #[allow(dead_code)]
    pub fn is_concluded(&self) -> bool {
        self.concluded
    }

    /// Insert a CUE marker to be associated with the next finalized segment.
    #[allow(dead_code)]
    pub fn insert_marker(&mut self, marker: cheetah_hls_core::CueMarker) {
        self.pending_markers.push(marker);
    }

    /// Get the stream key for URL validation.
    pub fn stream_key(&self) -> &str {
        &self.stream_key
    }

    /// Generate a rewind playlist containing all segments (for DVR/timeshift).
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

    /// `playlist_rewind_with_token` function.
    /// `playlist_rewind_with_token` 函数.
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

    /// Returns the `segment` value.
    /// 返回 `segment` 值.
    pub fn get_segment(&self, name: &str) -> Option<Bytes> {
        if let Some(ref demuxed) = self.demuxed {
            // Try exact name first, then with video_ prefix for legacy URLs
            return demuxed
                .track_segment(TrackLane::Video, name)
                .or_else(|| demuxed.track_segment(TrackLane::Video, &format!("video_{name}")));
        }
        self.ring.get(name).map(|s| s.data.clone())
    }

    /// Get a part by its global sequence number (LL-HLS).
    pub fn get_part(&self, part_seq: u64) -> Option<Bytes> {
        if let Some(ref demuxed) = self.demuxed {
            return demuxed.track_part(TrackLane::Video, part_seq);
        }
        self.ll_state
            .as_ref()
            .and_then(|ll| ll.get_part(part_seq))
            .map(|p| p.data.clone())
    }

    /// Get the fMP4 init segment (only available in fMP4 mode after set_tracks).
    pub fn init_segment(&self) -> Option<Bytes> {
        if let Some(ref demuxed) = self.demuxed {
            return demuxed.track_init_segment(TrackLane::Video);
        }
        self.fmp4_init.clone()
    }

    /// Whether this muxer is in demuxed mode.
    pub fn is_demuxed(&self) -> bool {
        self.demuxed.is_some()
    }

    /// Video codec ID.
    pub fn video_codec(&self) -> CodecId {
        self.video_codec
    }

    /// Video dimensions (width, height). Returns (0,0) if unknown.
    pub fn video_dimensions(&self) -> (u16, u16) {
        (self.video_width, self.video_height)
    }

    /// Audio codec ID.
    pub fn audio_codec(&self) -> CodecId {
        self.audio_codec
    }

    /// Audio channels count.
    pub fn audio_channels(&self) -> u8 {
        self.audio_channels
    }

    /// Whether the muxer is still missing the AAC AudioSpecificConfig needed to wrap raw
    /// AAC frames as ADTS. The HLS module uses this to decide if it should re-fetch the
    /// stream snapshot in case the publisher delivered the AAC config after the initial
    /// `set_tracks` call.
    pub fn needs_aac_config_refresh(&self) -> bool {
        self.has_audio && self.audio_codec == CodecId::AAC && self.aac_config.is_none()
    }

    /// Video extradata (avcC/hvcC) for codec string generation.
    pub fn video_extradata(&self) -> &[u8] {
        &self.video_extradata
    }

    /// Audio extradata (AudioSpecificConfig) for codec string generation.
    pub fn audio_extradata(&self) -> &[u8] {
        &self.audio_extradata
    }

    /// Get init segment for a specific lane (demuxed mode).
    pub fn track_init_segment(&self, lane: TrackLane) -> Option<Bytes> {
        self.demuxed.as_ref()?.track_init_segment(lane)
    }

    /// Get a part for a specific lane (demuxed mode).
    pub fn track_part(&self, lane: TrackLane, seq: u64) -> Option<Bytes> {
        self.demuxed.as_ref()?.track_part(lane, seq)
    }

    /// Get a segment for a specific lane (demuxed mode).
    pub fn track_segment(&self, lane: TrackLane, name: &str) -> Option<Bytes> {
        self.demuxed.as_ref()?.track_segment(lane, name)
    }

    /// Generate per-track chunklist (demuxed mode).
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

    /// Get rendition state (last_msn, last_part_index) for a lane.
    pub fn rendition_state(&self, lane: TrackLane) -> Option<(u64, u64)> {
        self.demuxed.as_ref()?.rendition_state(lane)
    }

    /// Check if a blocking request for a specific lane is satisfied.
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

    /// Get the most recently added segment (name, data).
    pub fn latest_segment(&self) -> Option<(String, Bytes)> {
        self.ring.latest().map(|s| (s.name.clone(), s.data.clone()))
    }

    /// Get the container format.
    pub fn container(&self) -> HlsContainer {
        self.config.container
    }

    /// Whether LL-HLS mode is active.
    #[allow(dead_code)]
    pub fn is_ll_hls(&self) -> bool {
        self.ll_state.is_some()
    }

    /// Current media sequence number (segment sequence).
    #[allow(dead_code)]
    pub fn current_msn(&self) -> u64 {
        self.segment_seq
    }

    /// Next part sequence number (the part that will be produced next).
    pub fn next_part_seq(&self) -> u64 {
        if let Some(ref demuxed) = self.demuxed {
            return demuxed.video().map(|v| v.next_part_seq()).unwrap_or(0);
        }
        self.ll_state
            .as_ref()
            .map(|ll| ll.next_part_seq())
            .unwrap_or(0)
    }

    /// Next part sequence for a specific lane.
    pub fn track_next_part_seq(&self, lane: TrackLane) -> u64 {
        self.demuxed
            .as_ref()
            .and_then(|d| d.lane(lane))
            .map(|t| t.next_part_seq())
            .unwrap_or(0)
    }

    /// Check if a blocking playlist request is satisfied.
    /// A request for (msn, part) is satisfied when our current state has progressed past it.
    /// msn = segment media sequence number, part = part index within that segment (0-based).
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

    /// Finalize the current part if there are pending part samples.
    fn try_finalize_current_part(&mut self) -> Option<HlsPart> {
        self.finalize_part_inner_with_end(None)
    }

    /// Internal: finalize current part from pending_part_samples.
    /// `end_dts_ms`: DTS of the next sample (precise end boundary), or None for flush/estimate.
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

    /// Finalize the current segment.
    ///
    /// `next_video_start_dts_us` is the DTS of the keyframe that triggered the cut,
    /// i.e. the *first* sample of the next segment. When provided, EXTINF is computed
    /// from `(next_start - this_start)` so that the reported duration includes the
    /// display time of the segment's last frame and matches the segment's actual
    /// wall-clock coverage (and the next segment's tfdt). When `None` (e.g. flush
    /// at end-of-stream), we fall back to `last_dts - start_dts` plus a one-frame
    /// estimate so EXTINF still does not under-report.
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
}

/// `generate_stream_validation_key` function.
/// `generate_stream_validation_key` 函数.
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

/// Gzip compress bytes. Returns compressed data.
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

/// Extract parameter sets from CodecExtradata as Annex-B byte sequence.
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

/// Extract AacAudioSpecificConfig from CodecExtradata.
fn extract_aac_config(extradata: &CodecExtradata) -> Option<AacAudioSpecificConfig> {
    match extradata {
        CodecExtradata::AAC { asc } => AacAudioSpecificConfig::from_bytes(asc),
        _ => None,
    }
}

/// Map a track-level channel count to the AAC channelConfiguration enum that ADTS frames
/// can carry. ADTS reserves only 3 bits for channel_configuration (values 0–7), so values
/// like 11 (7 channels in MPEG-4 ASC) cannot round-trip through ADTS. The caller should
/// fall back to a stereo configuration (2) when this returns `None`.
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

/// Patch a parsed AAC AudioSpecificConfig so that channel_configuration is non-zero.
///
/// MPEG-4 ASC permits channelConfiguration=0 to signal that a Program Config Element
/// describes the layout (common for FLV-sourced 5.1 streams). ADTS frames have no PCE,
/// so writing ch_cfg=0 produces frames where decoders interpret the layout as
/// "channels=0, sample_rate=0" and refuse to play. When that happens we copy the
/// channel layout from the track's reported channel count so that ADTS consumers
/// (ffmpeg, hls.js, native HLS players) see a consistent channel configuration.
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

/// Extract raw extradata bytes suitable for fMP4 codec config boxes.
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

fn append_hvcc_array(out: &mut Vec<u8>, nal_unit_type: u8, units: &[Bytes]) {
    out.push(0x80 | (nal_unit_type & 0x3f)); // array_completeness + NAL unit type
    out.extend_from_slice(&(units.len().min(u16::MAX as usize) as u16).to_be_bytes());
    for unit in units.iter().take(u16::MAX as usize) {
        let unit = &unit[..unit.len().min(u16::MAX as usize)];
        out.extend_from_slice(&(unit.len() as u16).to_be_bytes());
        out.extend_from_slice(unit);
    }
}

/// `fmp4_sample_payload` function.
/// `fmp4_sample_payload` 函数.
pub(crate) fn fmp4_sample_payload(frame: &AVFrame) -> Bytes {
    if frame.format != cheetah_codec::FrameFormat::CanonicalH26x {
        return frame.payload.clone();
    }
    if !matches!(frame.codec, CodecId::H264 | CodecId::H265 | CodecId::H266) {
        return frame.payload.clone();
    }
    h26x_length_prefixed_from_payload(frame.payload.clone())
}

fn us_to_90k(us: u64) -> u64 {
    us * 9 / 100
}

/// Convert length-prefixed H.26x NALUs to Annex-B format (start code prefixed).
/// If already in Annex-B format, returns as-is.
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
#[allow(dead_code)]
pub struct MuxerHealth {
    /// `crash_count` field of type `u32`.
    /// `crash_count` 字段，类型为 `u32`.
    crash_count: u32,
    /// `last_crash_us` field of type `u64`.
    /// `last_crash_us` 字段，类型为 `u64`.
    last_crash_us: u64,
    /// `rebuild_delay_ms` field of type `u64`.
    /// `rebuild_delay_ms` 字段，类型为 `u64`.
    rebuild_delay_ms: u64,
}

#[allow(dead_code)]
impl MuxerHealth {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new() -> Self {
        Self {
            crash_count: 0,
            last_crash_us: 0,
            rebuild_delay_ms: 0,
        }
    }

    /// Record a crash, returns delay before next rebuild attempt (ms).
    pub fn on_crash(&mut self, now_us: u64) -> u64 {
        self.crash_count += 1;
        self.last_crash_us = now_us;
        // Exponential backoff: 1s, 2s, 4s, 8s, 16s, max 30s
        self.rebuild_delay_ms = (1000 * (1u64 << self.crash_count.min(4))).min(30_000);
        self.rebuild_delay_ms
    }

    /// Called on successful rebuild — resets backoff.
    pub fn on_rebuild_success(&mut self) {
        self.crash_count = 0;
        self.rebuild_delay_ms = 0;
    }

    /// Whether we should give up rebuilding (too many crashes).
    pub fn should_give_up(&self) -> bool {
        self.crash_count >= 10
    }

    /// Number of crashes so far.
    pub fn crash_count(&self) -> u32 {
        self.crash_count
    }
}

impl Default for MuxerHealth {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_codec::{
        CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId, TrackInfo,
    };
    use cheetah_hls_core::{Fmp4DemuxEvent, Fmp4Demuxer};

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

    /// Regression test for the EXTINF-vs-trun-duration bug that caused periodic
    /// 1 s stalls on long-running HLS playback.
    ///
    /// `finalize_segment` used to compute EXTINF as `last_dts - first_dts`,
    /// which drops the display time of the segment's last frame. With a 20 fps
    /// source that under-reports each segment by 50 ms; over ~20 segments the
    /// drift reached ~1 s and ffplay's playlist-reload window started missing
    /// the next segment, producing the stutter.
    ///
    /// EXTINF MUST equal `next_seg_first_dts - this_seg_first_dts`, which is
    /// also what tfdt and trun durations describe.
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
