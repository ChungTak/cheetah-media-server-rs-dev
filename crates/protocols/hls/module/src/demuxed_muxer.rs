//! Demuxed LLHLS stream muxer: routes frames to per-track muxers.
//!
//! `DemuxedStreamMuxer` manages independent video and audio `TrackMuxer` instances,
//! producing separate init segments, parts, segments, and playlists per lane.
//!
//! 分离式 LLHLS 流复用器：将帧路由到各轨道复用器。
//!
//! `DemuxedStreamMuxer` 管理独立的视频与音频 `TrackMuxer` 实例，为每条 lane 生成
//! 独立的 init segment、分片、分段和播放列表。
//!

use bytes::Bytes;
use cheetah_codec::{
    aac_channel_count_from_asc, AVFrame, CodecId, FrameFlags, MediaKind, TrackInfo,
};
use cheetah_hls_core::{Fmp4TrackDesc, PlaylistBuilder, TrackLane};

use crate::muxer::{extract_raw_extradata, fmp4_sample_payload, MuxerOutput};
use crate::track_muxer::{TrackMuxer, TrackMuxerOutput};

/// Configuration for the demuxed stream muxer.
///
/// 分离式流复用器的配置。
#[derive(Debug, Clone)]
pub struct DemuxedMuxerConfig {
    pub segment_duration_ms: u64,
    pub segment_count: usize,
    pub force_segment_after_ms: u64,
    pub part_target_ms: u64,
    pub max_completed_segments: usize,
}

/// Demuxed audio/video LLHLS muxer.
///
/// Owns separate video and audio `TrackMuxer` lanes, aligns their start on the
/// first video keyframe, and shares wallclock/DTS origins across lanes.
///
/// 分离式音视频 LLHLS 复用器。
///
/// 拥有独立的视频和音频 `TrackMuxer` 轨道，在首个视频关键帧处对齐起点，
/// 并在各轨道间共享墙上时间/DTS 原点。
pub struct DemuxedStreamMuxer {
    config: DemuxedMuxerConfig,
    video: Option<TrackMuxer>,
    audio: Option<TrackMuxer>,
    wallclock_offset_ms: Option<i64>,
    dts_origin_ms: Option<u64>,
    waiting_for_keyframe: bool,
    concluded: bool,
}

impl DemuxedStreamMuxer {
    pub fn new(config: DemuxedMuxerConfig) -> Self {
        Self {
            config,
            video: None,
            audio: None,
            wallclock_offset_ms: None,
            dts_origin_ms: None,
            waiting_for_keyframe: true,
            concluded: false,
        }
    }

    /// Initialize video and audio track muxers from track info.
    ///
    /// Builds per-lane `Fmp4TrackDesc` from codec extradata, normalizes sample rate
    /// and channel layout for AAC, and creates the `TrackMuxer` instances.
    ///
    /// 根据轨道信息初始化视频和音频轨道复用器。
    ///
    /// 从编解码器 extradata 构建每轨 `Fmp4TrackDesc`，归一化 AAC 采样率与声道布局，
    /// 并创建 `TrackMuxer` 实例。
    pub fn set_tracks(&mut self, tracks: &[TrackInfo]) {
        if let Some(video) = tracks.iter().find(|t| t.media_kind == MediaKind::Video) {
            let video_part_target = compute_video_part_target(self.config.part_target_ms, video);
            let vw = video.width.unwrap_or(0).min(u16::MAX as u32) as u16;
            let vh = video.height.unwrap_or(0).min(u16::MAX as u32) as u16;
            let desc = Fmp4TrackDesc {
                track_id: 1,
                codec: video.codec,
                media_kind: MediaKind::Video,
                timescale: 90000,
                extradata: extract_raw_extradata(&video.extradata),
                // Chrome MSE requires non-zero dimensions in avc1 sample entry.
                // Use 1920x1080 fallback when dimensions are unknown (common with RTMP).
                width: if vw > 0 { vw } else { 1920 },
                height: if vh > 0 { vh } else { 1080 },
                sample_rate: 0,
                channels: 0,
            };
            self.video = Some(TrackMuxer::new(
                TrackLane::Video,
                video.track_id,
                desc,
                video_part_target,
                self.config.segment_count,
                self.config.max_completed_segments,
            ));
        }

        if let Some(audio) = tracks.iter().find(|t| t.media_kind == MediaKind::Audio) {
            let audio_part_target = compute_audio_part_target(self.config.part_target_ms, audio);
            let raw_extradata = extract_raw_extradata(&audio.extradata);
            // Parse actual sample rate/channels from AudioSpecificConfig (RTMP FLV header
            // often reports wrong sample_rate like 11025 when actual ASC says 48000)
            let (actual_sr, actual_ch) = parse_aac_params(&raw_extradata, audio);
            // Trim extradata to actual ASC length (RTMP may include encoder metadata after ASC)
            let asc_extradata = trim_aac_asc(&raw_extradata, actual_ch);
            let desc = Fmp4TrackDesc {
                track_id: 1,
                codec: audio.codec,
                media_kind: MediaKind::Audio,
                timescale: actual_sr,
                extradata: asc_extradata,
                width: 0,
                height: 0,
                sample_rate: actual_sr,
                channels: actual_ch,
            };
            self.audio = Some(TrackMuxer::new(
                TrackLane::Audio,
                audio.track_id,
                desc,
                audio_part_target,
                self.config.segment_count,
                self.config.max_completed_segments,
            ));
        }

        self.waiting_for_keyframe = self.video.is_some();
    }

    /// Push a frame into the appropriate lane and produce segment/part events.
    ///
    /// Waits for the first video keyframe, normalizes DTS/PTS, strips ADTS headers
    /// from AAC frames, and forwards to the video or audio track muxer.
    ///
    /// 将帧推入对应轨道并生成分段/分片事件。
    ///
    /// 等待首个视频关键帧，归一化 DTS/PTS，从 AAC 帧去除 ADTS 头，并转发到视频或音频轨道复用器。
    pub fn push_frame(&mut self, frame: &AVFrame) -> Vec<MuxerOutput> {
        if self.concluded {
            return Vec::new();
        }

        // Skip CONFIG and NON_PICTURE frames
        if frame.flags.contains(FrameFlags::CONFIG) || frame.flags.contains(FrameFlags::NON_PICTURE)
        {
            return Vec::new();
        }

        let is_video = frame.media_kind == MediaKind::Video;
        let is_keyframe = frame.flags.contains(FrameFlags::KEY);

        // Both lanes wait for first video keyframe to ensure aligned timelines.
        // Chrome MSE rejects audio appends when video SourceBuffer is empty.
        if self.waiting_for_keyframe {
            if is_video && is_keyframe {
                self.waiting_for_keyframe = false;
            } else {
                return Vec::new();
            }
        }

        let raw_dts_ms = (frame.dts_us.max(0) as u64) / 1000;
        let raw_pts_ms = (frame.pts_us.max(frame.dts_us) as u64) / 1000;

        let dts_ms = raw_dts_ms;
        let pts_ms = raw_pts_ms;
        let data = fmp4_sample_payload(frame);

        // Initialize wallclock offset on first sample
        if self.wallclock_offset_ms.is_none() {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let offset = now_ms - dts_ms as i64;
            self.wallclock_offset_ms = Some(offset);
            if let Some(ref mut v) = self.video {
                v.set_wallclock_offset(offset);
            }
            if let Some(ref mut a) = self.audio {
                a.set_wallclock_offset(offset);
            }
        }

        let outputs = if is_video {
            if let Some(ref mut video) = self.video {
                video.push_sample(
                    dts_ms,
                    pts_ms,
                    is_keyframe,
                    data,
                    self.config.segment_duration_ms,
                    self.config.force_segment_after_ms,
                )
            } else {
                Vec::new()
            }
        } else {
            if let Some(ref mut audio) = self.audio {
                // Strip ADTS header if present — fMP4 requires raw AAC AU
                let audio_data = strip_adts_header(&data);
                // Audio frames in AAC/Opus are always independent
                audio.push_sample(
                    dts_ms,
                    pts_ms,
                    true,
                    audio_data,
                    self.config.segment_duration_ms,
                    self.config.force_segment_after_ms,
                )
            } else {
                Vec::new()
            }
        };

        // Convert TrackMuxerOutput to MuxerOutput
        outputs
            .into_iter()
            .filter_map(|o| match o {
                TrackMuxerOutput::PartReady(part) => Some(MuxerOutput::PartReady(part)),
                TrackMuxerOutput::SegmentReady {
                    name,
                    duration_secs,
                    ..
                } => {
                    let data = if is_video {
                        self.video.as_ref().and_then(|v| v.get_segment(&name))
                    } else {
                        self.audio.as_ref().and_then(|a| a.get_segment(&name))
                    };
                    data.map(|d| MuxerOutput::SegmentReady {
                        name,
                        duration_secs,
                        data: d,
                    })
                }
            })
            .collect()
    }

    /// Flush all lanes and mark the stream as concluded.
    ///
    /// 冲洗所有轨道并将流标记为已结束。
    pub fn conclude(&mut self) {
        if let Some(ref mut v) = self.video {
            v.flush();
        }
        if let Some(ref mut a) = self.audio {
            a.flush();
        }
        self.concluded = true;
    }

    /// Whether the stream has been concluded.
    ///
    /// 流是否已结束。
    pub fn is_concluded(&self) -> bool {
        self.concluded
    }

    /// Whether at least one lane has been initialized.
    ///
    /// 是否至少初始化了一条轨道。
    pub fn is_active(&self) -> bool {
        self.video.is_some() || self.audio.is_some()
    }

    /// Whether both video and audio lanes are present.
    ///
    /// 是否同时存在视频和音频轨道。
    pub fn has_both_lanes(&self) -> bool {
        self.video.is_some() && self.audio.is_some()
    }

    /// Access the video lane.
    ///
    /// 访问视频轨道。
    pub fn video(&self) -> Option<&TrackMuxer> {
        self.video.as_ref()
    }

    /// Access the audio lane.
    ///
    /// 访问音频轨道。
    pub fn audio(&self) -> Option<&TrackMuxer> {
        self.audio.as_ref()
    }

    /// Mutable access to the video lane.
    ///
    /// 可变访问视频轨道。
    pub fn video_mut(&mut self) -> Option<&mut TrackMuxer> {
        self.video.as_mut()
    }

    /// Mutable access to the audio lane.
    ///
    /// 可变访问音频轨道。
    pub fn audio_mut(&mut self) -> Option<&mut TrackMuxer> {
        self.audio.as_mut()
    }

    /// Get the track muxer for a specific lane.
    ///
    /// 获取指定轨道的复用器。
    pub fn lane(&self, lane: TrackLane) -> Option<&TrackMuxer> {
        match lane {
            TrackLane::Video => self.video.as_ref(),
            TrackLane::Audio => self.audio.as_ref(),
        }
    }

    /// Get the fMP4 init segment for a lane.
    ///
    /// 获取某轨道的 fMP4 init segment。
    pub fn track_init_segment(&self, lane: TrackLane) -> Option<Bytes> {
        self.lane(lane).map(|t| t.init_segment.clone())
    }

    /// Get a part by lane and global sequence.
    ///
    /// 根据轨道和全局序列号获取分片。
    pub fn track_part(&self, lane: TrackLane, seq: u64) -> Option<Bytes> {
        self.lane(lane).and_then(|t| t.get_part(seq))
    }

    /// Get a segment by lane and name.
    ///
    /// 根据轨道和名称获取分段。
    pub fn track_segment(&self, lane: TrackLane, name: &str) -> Option<Bytes> {
        self.lane(lane).and_then(|t| t.get_segment(name))
    }

    /// Get the (last_msn, last_part_index) for a lane.
    ///
    /// 获取某轨道的 (last_msn, last_part_index)。
    pub fn rendition_state(&self, lane: TrackLane) -> Option<(u64, u64)> {
        self.lane(lane).map(|t| {
            let msn = t.ll_state.parent_segment_seq();
            let parts = t.ll_state.current_parts().len() as u64;
            let last_part = if parts > 0 { parts - 1 } else { 0 };
            (msn, last_part)
        })
    }

    /// Generate the per-track chunklist playlist for a lane.
    ///
    /// Rewrites generic URI prefixes to lane-specific names and appends an
    /// `EXT-X-RENDITION-REPORT` for the opposite lane in LL-HLS.
    ///
    /// 生成某轨道的每轨分片列表。
    ///
    /// 将通用 URI 前缀重写为轨道专属名称，并在 LL-HLS 中为对端轨道追加 `EXT-X-RENDITION-REPORT`。
    pub fn track_playlist(
        &self,
        lane: TrackLane,
        session_id: Option<u64>,
        include_stream_key: bool,
        stream_key: &str,
    ) -> Option<String> {
        let track = self.lane(lane)?;
        if track.ring.is_empty() {
            return None;
        }
        let other_lane = match lane {
            TrackLane::Video => TrackLane::Audio,
            TrackLane::Audio => TrackLane::Video,
        };

        // Build playlist with empty prefix, then rewrite URIs to lane-specific names
        let playlist = PlaylistBuilder::build_media_ll(
            &track.ring,
            &track.ll_state,
            session_id,
            "",
            false,
            self.concluded,
        );

        // Rewrite init.mp4 -> init_{lane}.mp4, part_N -> {lane}_part_N, seg_N -> {lane}_seg_N
        let playlist = prefix_part_uris(&playlist, lane);

        // Add rendition report for the other lane
        let playlist = if !self.concluded {
            if let Some((msn, last_part)) = self.rendition_state(other_lane) {
                let other_chunklist = format!("chunklist_{}.m3u8", other_lane.prefix());
                let report = format!(
                    "#EXT-X-RENDITION-REPORT:URI=\"{other_chunklist}\",LAST-MSN={msn},LAST-PART={last_part}\n"
                );
                // Insert before the last line or at end
                if playlist.ends_with('\n') {
                    format!("{playlist}{report}")
                } else {
                    format!("{playlist}\n{report}")
                }
            } else {
                playlist
            }
        } else {
            playlist
        };

        if include_stream_key {
            Some(append_stream_key_to_playlist_uris(&playlist, stream_key))
        } else {
            Some(playlist)
        }
    }

    /// Whether every present lane has produced at least one segment.
    ///
    /// 每条存在的轨道是否都已生成至少一个分段。
    pub fn is_ready(&self) -> bool {
        let video_ready = self
            .video
            .as_ref()
            .map(|v| !v.ring.is_empty())
            .unwrap_or(true); // no video track = not blocking
        let audio_ready = self
            .audio
            .as_ref()
            .map(|a| !a.ring.is_empty())
            .unwrap_or(true); // no audio track = not blocking
                              // At least one track must exist, and all present tracks must have segments
        (self.video.is_some() || self.audio.is_some()) && video_ready && audio_ready
    }

    /// Return the shared wallclock offset for `PROGRAM-DATE-TIME`.
    ///
    /// 返回用于 `PROGRAM-DATE-TIME` 的共享墙上时间偏移。
    pub fn wallclock_offset_ms(&self) -> Option<i64> {
        self.wallclock_offset_ms
    }
}

/// Strip ADTS header from an AAC frame if present.
///
/// fMP4 and TS muxing require raw AAC access units, so sync-word frames have
/// their ADTS header removed.
///
/// 如果存在，从 AAC 帧中去除 ADTS 头。
///
/// fMP4 和 TS 复用需要原始 AAC 访问单元，因此同步字帧的 ADTS 头被移除。
fn strip_adts_header(data: &Bytes) -> Bytes {
    if data.len() >= 7 && data[0] == 0xFF && (data[1] & 0xF0) == 0xF0 {
        let protection_absent = (data[1] & 0x01) != 0;
        let header_len: usize = if protection_absent { 7 } else { 9 };
        if data.len() > header_len {
            return data.slice(header_len..);
        }
    }
    data.clone()
}

/// Trim AAC extradata to the bytes needed by fMP4 decoder config.
///
/// Preserves a PCE when present for multichannel layouts, otherwise rewrites
/// `channelConfiguration=0` to a valid value.
///
/// 将 AAC extradata 裁剪为 fMP4 解码器配置所需字节。
///
/// 在存在多声道布局时保留 PCE，否则将 `channelConfiguration=0` 重写为有效值。
fn trim_aac_asc(extradata: &Bytes, _channels: u8) -> Bytes {
    if extradata.len() < 2 {
        return extradata.clone();
    }
    let b0 = extradata[0];
    let b1 = extradata[1];
    let ch_cfg = (b1 >> 3) & 0x0f;
    let aot = (b0 >> 3) & 0x1f;
    if ch_cfg == 0 {
        if aac_channel_count_from_asc(extradata).is_some() {
            return extradata.clone();
        }
        // ch_cfg=0 without a parseable PCE: rewrite to a channelConfiguration
        // matching the best track-level hint so fMP4 decoders do not assume mono.
        let fallback_ch_cfg = channels_to_aac_channel_configuration(_channels).unwrap_or(2);
        let new_b1 = (b1 & 0x87) | (fallback_ch_cfg << 3);
        Bytes::from(vec![b0, new_b1])
    } else {
        let asc_len = if aot == 5 || aot == 29 {
            extradata.len().min(5)
        } else {
            2
        };
        extradata.slice(..asc_len.min(extradata.len()))
    }
}

/// Parse the actual AAC sample rate and channel count from AudioSpecificConfig.
///
/// Falls back to the `TrackInfo` values when the ASC is too short or malformed.
///
/// 从 AudioSpecificConfig 解析实际 AAC 采样率和声道数。
///
/// 当 ASC 过短或损坏时回退到 `TrackInfo` 的值。
fn parse_aac_params(extradata: &Bytes, track: &TrackInfo) -> (u32, u8) {
    const AAC_SAMPLE_RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    if extradata.len() >= 2 {
        let b0 = extradata[0];
        let b1 = extradata[1];
        let freq_idx = (((b0 & 0x07) << 1) | (b1 >> 7)) as usize;
        let sr = AAC_SAMPLE_RATES.get(freq_idx).copied().unwrap_or(44100);
        let ch = aac_channel_count_from_asc(extradata)
            .or(track.channels)
            .unwrap_or(2);
        (sr, ch)
    } else {
        (
            track.sample_rate.unwrap_or(44100),
            track.channels.unwrap_or(2),
        )
    }
}

/// Map a channel count to the ADTS `channel_configuration` enum.
///
/// ADTS reserves only 3 bits, so some layouts cannot be represented.
///
/// 将声道数映射到 ADTS 的 `channel_configuration` 枚举。
///
/// ADTS 仅保留 3 位，因此某些声道布局无法表示。
fn channels_to_aac_channel_configuration(channels: u8) -> Option<u8> {
    match channels {
        1 => Some(1),
        2 => Some(2),
        3 => Some(3),
        4 => Some(4),
        5 => Some(5),
        6 => Some(6),
        8 => Some(7),
        _ => None,
    }
}

/// Compute a frame-aligned video part target from track FPS.
///
/// Rounds the target to a whole number of frames so part boundaries do not fall
/// inside a frame.
///
/// 根据轨道 FPS 计算帧对齐的视频分片目标。
///
/// 将目标时长四舍五入为整数帧数，使分片边界不落在帧内。
fn compute_video_part_target(target_ms: u64, track: &TrackInfo) -> u64 {
    if let Some(fps) = track.fps {
        let fps_f = fps.num as f64 / fps.den as f64;
        if fps_f > 0.0 {
            let frame_ms = 1000.0 / fps_f;
            let frames = (target_ms as f64 / frame_ms).round().max(1.0);
            return (frames * frame_ms).round() as u64;
        }
    }
    target_ms
}

/// Compute a frame-aligned audio part target from sample rate.
///
/// Uses 1024-sample AAC frames or 960-sample Opus frames as the base unit.
///
/// 根据采样率计算帧对齐的音频分片目标。
///
/// 以 1024 采样 AAC 帧或 960 采样 Opus 帧为基本单元。
fn compute_audio_part_target(target_ms: u64, track: &TrackInfo) -> u64 {
    let sr = track.sample_rate.unwrap_or(44100) as f64;
    if sr > 0.0 {
        let samples_per_frame = if track.codec == CodecId::AAC {
            1024.0
        } else {
            960.0
        };
        let frame_ms = samples_per_frame / sr * 1000.0;
        let frames = (target_ms as f64 / frame_ms).round().max(1.0);
        return (frames * frame_ms).round() as u64;
    }
    target_ms
}

/// Rewrite generic init/part/segment URIs with lane-specific prefixes.
///
/// 用轨道前缀重写通用的 init/part/segment URI。
fn prefix_part_uris(playlist: &str, lane: TrackLane) -> String {
    let prefix = lane.prefix();
    let mut out = String::with_capacity(playlist.len() + 128);
    for line in playlist.lines() {
        if line.contains("URI=\"init.mp4") {
            out.push_str(&line.replace("init.mp4", &format!("init_{prefix}.mp4")));
        } else if line.contains("URI=\"part_") {
            // LowLatencyState generates "part_N.m4s" — prefix with lane
            out.push_str(&line.replace("URI=\"part_", &format!("URI=\"{prefix}_part_")));
        } else if !line.starts_with('#') && !line.is_empty() && line.starts_with("part_") {
            // Bare part line (unlikely but defensive)
            out.push_str(&line.replacen("part_", &format!("{prefix}_part_"), 1));
        } else {
            // Segment lines already have lane prefix from TrackMuxer (e.g. "video_seg_0.m4s")
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Append the stream-key validation token to all media URIs in a playlist.
///
/// 将流密钥验证令牌追加到播放列表中的所有媒体 URI。
fn append_stream_key_to_playlist_uris(playlist: &str, stream_key: &str) -> String {
    let mut out = String::with_capacity(playlist.len() + stream_key.len() * 8);
    for line in playlist.lines() {
        if line.starts_with("#EXT-X-MAP:")
            || line.starts_with("#EXT-X-PART:")
            || line.starts_with("#EXT-X-PRELOAD-HINT:")
            || line.starts_with("#EXT-X-RENDITION-REPORT:")
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

/// Append a query parameter to a quoted URI inside an HLS tag line.
///
/// 在 HLS 标签行中的带引号 URI 追加查询参数。
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

/// Append a query parameter to a plain URI, choosing `?` or `&`.
///
/// 向普通 URI 追加查询参数，自动选择 `?` 或 `&`。
fn append_query_to_uri(uri: &str, key: &str, value: &str) -> String {
    let separator = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{separator}{key}={value}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_codec::{CodecExtradata, FrameFormat, Timebase, TrackId};

    fn make_demuxed_config() -> DemuxedMuxerConfig {
        DemuxedMuxerConfig {
            segment_duration_ms: 2000,
            segment_count: 3,
            force_segment_after_ms: 10000,
            part_target_ms: 200,
            max_completed_segments: 5,
        }
    }

    fn make_tracks() -> Vec<TrackInfo> {
        let mut video = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90000);
        video.width = Some(1920);
        video.height = Some(1080);
        video.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1e])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x38])],
            avcc: Some(Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1e, 0xff, 0xe1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1e, 0x01, 0x00,
                0x03, 0x68, 0xce, 0x38,
            ])),
        };

        let mut audio = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 44100);
        audio.sample_rate = Some(44100);
        audio.channels = Some(2);
        audio.extradata = CodecExtradata::AAC {
            asc: Bytes::from_static(&[0x12, 0x10]),
        };

        vec![video, audio]
    }

    fn make_video_frame(dts_us: i64, keyframe: bool) -> AVFrame {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            dts_us,
            dts_us,
            Timebase::new(1, 1_000_000),
            Bytes::from(vec![0u8; 100]),
        );
        if keyframe {
            frame.flags |= FrameFlags::KEY;
        }
        frame
    }

    fn make_audio_frame(dts_us: i64) -> AVFrame {
        AVFrame::new(
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

    #[test]
    fn demuxed_audio_frames_are_not_dropped() {
        let mut muxer = DemuxedStreamMuxer::new(make_demuxed_config());
        muxer.set_tracks(&make_tracks());

        // Push video keyframe to start, then enough audio frames to produce a part
        muxer.push_frame(&make_video_frame(0, true));
        // AAC at 44100: frame duration ~23.2ms, part target ~209ms = ~9 frames per part
        // Need 10+ frames to trigger a part cut
        for i in 1..=12 {
            let dts_us = (i as f64 * 23219.0) as i64;
            muxer.push_frame(&make_audio_frame(dts_us));
        }

        // Audio lane should have produced at least one part
        let audio = muxer.audio().expect("audio lane exists");
        assert!(
            audio.next_part_seq() > 0,
            "audio frames should produce parts, got next_part_seq={}",
            audio.next_part_seq()
        );
    }

    #[test]
    fn conclude_flushes_all_lanes() {
        let mut muxer = DemuxedStreamMuxer::new(make_demuxed_config());
        muxer.set_tracks(&make_tracks());

        muxer.push_frame(&make_video_frame(0, true));
        for i in 1..7 {
            muxer.push_frame(&make_video_frame(i * 33_000, false));
        }
        for i in 0..5 {
            muxer.push_frame(&make_audio_frame((i as f64 * 23219.0) as i64));
        }

        muxer.conclude();

        assert!(muxer.is_concluded());
        assert!(muxer.video().unwrap().concluded);
        assert!(muxer.audio().unwrap().concluded);
    }

    #[test]
    fn track_init_segments_are_independent() {
        let mut muxer = DemuxedStreamMuxer::new(make_demuxed_config());
        muxer.set_tracks(&make_tracks());

        let video_init = muxer.track_init_segment(TrackLane::Video).unwrap();
        let audio_init = muxer.track_init_segment(TrackLane::Audio).unwrap();

        // Each should have exactly one trak
        let v_trak = video_init.windows(4).filter(|w| *w == b"trak").count();
        let a_trak = audio_init.windows(4).filter(|w| *w == b"trak").count();
        assert_eq!(v_trak, 1);
        assert_eq!(a_trak, 1);

        // They should be different
        assert_ne!(video_init, audio_init);
    }

    #[test]
    fn aac_asc_with_pce_is_preserved_for_multichannel() {
        let asc = Bytes::from_static(&[
            0x11, 0x80, 0x04, 0xc8, 0x44, 0x00, 0x20, 0x00, 0xc4, 0x0c, 0x4c, 0x61, 0x76, 0x63,
            0x36, 0x31, 0x2e, 0x33, 0x2e, 0x31, 0x30, 0x30, 0x56, 0xe5, 0x00,
        ]);

        assert_eq!(trim_aac_asc(&asc, 6), asc);

        let mut track = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000);
        track.sample_rate = Some(48_000);
        track.channels = Some(2);
        assert_eq!(parse_aac_params(&asc, &track), (48_000, 6));
    }
}
