//! M3U8 playlist builder for live, VOD, and Low-Latency HLS.
//!
//! M3U8 播放列表构建器，支持直播、点播与低延迟 HLS。
//! 将 `SegmentRing` 与 `LowLatencyState` 转换为符合 HLS 规范的文本。

use crate::ll_hls::LowLatencyState;
use crate::segment::SegmentRing;
use crate::vtt_mux::{VttMux, VttSegment};

/// Container mode for playlist generation.
///
/// 播放列表生成的容器模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HlsContainer {
    /// MPEG-TS segments (.ts)
    ///
    /// MPEG-TS 分片（.ts）。
    Ts,
    /// Fragmented MP4 segments (.m4s) with init segment
    ///
    /// 带 init 分段的 fMP4 分片（.m4s）。
    Fmp4,
}

/// Lightweight M3U8 playlist builder for live HLS.
///
/// 轻量级直播 HLS 用 M3U8 播放列表构建器。
pub struct PlaylistBuilder;

impl PlaylistBuilder {
    /// Generate a master playlist that redirects to the media playlist with a session UID.
    ///
    /// 生成主播放列表，重定向到带会话 UID 的媒体播放列表。
    pub fn build_master(stream_name: &str, session_id: u64) -> String {
        Self::build_master_with_subtitles(stream_name, session_id, None, None)
    }

    /// Generate a master playlist with an optional WebVTT subtitle rendition.
    ///
    /// 生成主播放列表，可包含 WebVTT 字幕 rendition。
    pub fn build_master_with_subtitles(
        stream_name: &str,
        session_id: u64,
        subtitle: Option<&SubtitleRenditionInfo>,
        subtitle_token: Option<&str>,
    ) -> String {
        let mut out = String::with_capacity(256);
        out.push_str("#EXTM3U\n");
        if let Some(sub) = subtitle {
            let default = if sub.is_default { "YES" } else { "NO" };
            let autoselect = if sub.autoselect { "YES" } else { "NO" };
            let uid_suffix = format!("?uid={session_id}");
            let key_suffix = subtitle_token
                .filter(|t| !t.is_empty())
                .map(|t| format!("&k={t}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"{}\",\
                 LANGUAGE=\"{}\",DEFAULT={default},AUTOSELECT={autoselect},\
                 URI=\"{stream_name}/subtitle.m3u8{uid_suffix}{key_suffix}\"\n",
                sub.name, sub.language
            ));
        }
        out.push_str("#EXT-X-STREAM-INF:BANDWIDTH=2000000");
        if subtitle.is_some() {
            out.push_str(",SUBTITLES=\"subs\"");
        }
        out.push_str(&format!("\n{stream_name}/index.m3u8?uid={session_id}\n"));
        out
    }

    /// Generate a master playlist with ABR variants and an optional subtitle rendition.
    ///
    /// 生成包含 ABR 档位和可选字幕 rendition 的主播放列表。
    pub fn build_master_with_variants(
        variants: &[VariantRenditionInfo],
        subtitle: Option<&SubtitleRenditionInfo>,
        subtitle_uri: Option<&str>,
    ) -> String {
        let mut out = String::with_capacity(512);
        out.push_str("#EXTM3U\n");
        if let Some(sub) = subtitle {
            let default = if sub.is_default { "YES" } else { "NO" };
            let autoselect = if sub.autoselect { "YES" } else { "NO" };
            let uri = subtitle_uri.unwrap_or("");
            out.push_str(&format!(
                "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"{}\",\
                 LANGUAGE=\"{}\",DEFAULT={default},AUTOSELECT={autoselect},\
                 URI=\"{uri}\"\n",
                sub.name, sub.language
            ));
        }
        for variant in variants {
            let info = &variant.info;
            let mut attrs = format!("BANDWIDTH={}", info.bandwidth);
            if let (Some(w), Some(h)) = (info.width, info.height) {
                attrs.push_str(&format!(",RESOLUTION={w}x{h}"));
            }
            if let Some(fps) = info.frame_rate {
                attrs.push_str(&format!(",FRAME-RATE={fps:.3}"));
            }
            if !info.codecs.is_empty() {
                attrs.push_str(&format!(",CODECS=\"{}\"", info.codecs));
            }
            if subtitle.is_some() {
                attrs.push_str(",SUBTITLES=\"subs\"");
            }
            out.push_str(&format!("#EXT-X-STREAM-INF:{attrs}\n"));
            out.push_str(&format!("{}\n", variant.uri));
        }
        // If no variants were provided, fall back to a single default entry so the
        // master playlist is still valid. Callers with known variants should not
        // rely on this fallback.
        if variants.is_empty() {
            out.push_str("#EXT-X-STREAM-INF:BANDWIDTH=2000000\n");
            out.push_str("index.m3u8\n");
        }
        out
    }

    /// Generate a live media playlist from the current segment ring (TS mode).
    ///
    /// 从当前分片环生成直播媒体播放列表（TS 模式）。
    pub fn build_media(ring: &SegmentRing, session_id: Option<u64>) -> String {
        Self::build_media_with_container(ring, session_id, HlsContainer::Ts)
    }

    /// Generate a live media playlist with specified container format.
    ///
    /// Picks the larger version number and the correct extension for TS (3/.ts) or fMP4
    /// (7/.m4s). For fMP4, an `#EXT-X-MAP` reference to the init segment is emitted.
    ///
    /// 使用指定容器格式生成直播媒体播放列表。
    /// 根据 TS（3/.ts）或 fMP4（7/.m4s）选择版本号与扩展名。
    /// fMP4 会输出指向 init 分段的 `#EXT-X-MAP`。
    pub fn build_media_with_container(
        ring: &SegmentRing,
        session_id: Option<u64>,
        container: HlsContainer,
    ) -> String {
        if ring.is_empty() {
            return Self::build_empty_media(container);
        }

        let target_duration = ring.max_duration().ceil() as u64;
        let media_sequence = ring.first_sequence();
        let (version, ext) = match container {
            HlsContainer::Ts => (3, ".ts"),
            HlsContainer::Fmp4 => (7, ".m4s"),
        };

        let mut out = String::with_capacity(256);
        out.push_str("#EXTM3U\n");
        out.push_str(&format!("#EXT-X-VERSION:{version}\n"));
        out.push_str("#EXT-X-ALLOW-CACHE:NO\n");
        out.push_str(&format!("#EXT-X-TARGETDURATION:{target_duration}\n"));
        out.push_str(&format!("#EXT-X-MEDIA-SEQUENCE:{media_sequence}\n"));

        if container == HlsContainer::Fmp4 {
            match session_id {
                Some(uid) => out.push_str(&format!("#EXT-X-MAP:URI=\"init.mp4?uid={uid}\"\n")),
                None => out.push_str("#EXT-X-MAP:URI=\"init.mp4\"\n"),
            }
        }

        for seg in ring.iter() {
            if let Some(pdt_ms) = seg.program_date_time_ms {
                out.push_str(&format!(
                    "#EXT-X-PROGRAM-DATE-TIME:{}\n",
                    format_iso8601(pdt_ms)
                ));
            }
            out.push_str(&format!("#EXTINF:{:.3},\n", seg.duration_secs));
            match session_id {
                Some(uid) => out.push_str(&format!("{}{ext}?uid={uid}\n", seg.name)),
                None => out.push_str(&format!("{}{ext}\n", seg.name)),
            }
        }

        out
    }

    fn build_empty_media(container: HlsContainer) -> String {
        let version = match container {
            HlsContainer::Ts => 3,
            HlsContainer::Fmp4 => 7,
        };
        format!(
            "#EXTM3U\n#EXT-X-VERSION:{version}\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n"
        )
    }

    /// Generate a live playlist from a list of segment file entries (for disk-based mode).
    ///
    /// 从分片文件条目列表生成直播播放列表（用于磁盘模式）。
    pub fn build_live_file(
        segments: &[SegmentFileEntry],
        media_sequence: u64,
        container: HlsContainer,
    ) -> String {
        if segments.is_empty() {
            return Self::build_empty_media(container);
        }

        let target_duration = segments
            .iter()
            .map(|s| s.duration_secs.ceil() as u64)
            .max()
            .unwrap_or(4);
        let (version, ext) = match container {
            HlsContainer::Ts => (3, ".ts"),
            HlsContainer::Fmp4 => (7, ".m4s"),
        };

        let mut out = String::with_capacity(256);
        out.push_str("#EXTM3U\n");
        out.push_str(&format!("#EXT-X-VERSION:{version}\n"));
        out.push_str("#EXT-X-ALLOW-CACHE:NO\n");
        out.push_str(&format!("#EXT-X-TARGETDURATION:{target_duration}\n"));
        out.push_str(&format!("#EXT-X-MEDIA-SEQUENCE:{media_sequence}\n"));

        if container == HlsContainer::Fmp4 {
            out.push_str("#EXT-X-MAP:URI=\"init.mp4\"\n");
        }

        for seg in segments {
            out.push_str(&format!("#EXTINF:{:.3},\n", seg.duration_secs));
            out.push_str(&format!("{}{ext}\n", seg.filename));
        }

        out
    }

    /// Generate a VOD playlist with `#EXT-X-ENDLIST`.
    ///
    /// 生成带 `#EXT-X-ENDLIST` 的点播播放列表。
    pub fn build_vod(segments: &[SegmentFileEntry], container: HlsContainer) -> String {
        let mut out = Self::build_live_file(segments, 0, container);
        out.push_str("#EXT-X-ENDLIST\n");
        out
    }

    /// Generate a live WebVTT media playlist from a [`VttMux`].
    ///
    /// 从 `VttMux` 生成直播 WebVTT 媒体播放列表。
    pub fn build_vtt_media(mux: &VttMux, session_id: Option<u64>) -> String {
        let segments: Vec<&VttSegment> = mux.segments().iter().collect();
        if segments.is_empty() {
            return "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n"
                .to_string();
        }

        let target_duration = segments
            .iter()
            .map(|s| s.duration_secs.ceil() as u64)
            .max()
            .unwrap_or(4)
            .max(1);
        let media_sequence = segments.first().map(|s| s.sequence).unwrap_or(0);

        let mut out = String::with_capacity(256);
        out.push_str("#EXTM3U\n");
        out.push_str("#EXT-X-VERSION:3\n");
        out.push_str("#EXT-X-ALLOW-CACHE:NO\n");
        out.push_str(&format!("#EXT-X-TARGETDURATION:{target_duration}\n"));
        out.push_str(&format!("#EXT-X-MEDIA-SEQUENCE:{media_sequence}\n"));

        for seg in segments {
            out.push_str(&format!("#EXTINF:{:.3},\n", seg.duration_secs));
            match session_id {
                Some(uid) => out.push_str(&format!("{}.vtt?uid={uid}\n", seg.name)),
                None => out.push_str(&format!("{}.vtt\n", seg.name)),
            }
        }

        out
    }

    /// Generate a delayed playlist that includes extra segments beyond the normal window.
    /// `ring` contains all available segments; the delayed playlist shows all segments.
    ///
    /// 生成延迟播放列表，包含超出常规窗口的额外分片。
    /// `ring` 包含所有可用分片；延迟播放列表显示全部分片。
    pub fn build_media_delayed(ring: &SegmentRing, session_id: Option<u64>) -> String {
        // Delayed playlist shows all segments in the ring (same as build_media but explicitly named)
        Self::build_media_with_container(ring, session_id, HlsContainer::Ts)
    }

    /// Generate a Low-Latency HLS media playlist with `#EXT-X-PART` tags.
    ///
    /// Includes: `EXT-X-SERVER-CONTROL`, `EXT-X-PART-INF`, `EXT-X-PART` per segment,
    /// and `EXT-X-PRELOAD-HINT` for the next expected part.
    ///
    /// When `legacy` is true, all LL-HLS tags are stripped and only completed segments
    /// are output (traditional HLS compatibility mode).
    ///
    /// When `concluded` is true, `#EXT-X-ENDLIST` is appended (stream ended).
    ///
    /// 生成带 `#EXT-X-PART` 标签的低延迟 HLS 媒体播放列表。
    /// 包含：`EXT-X-SERVER-CONTROL`、`EXT-X-PART-INF`、每个分段的 `EXT-X-PART`，
    /// 以及下一个预期 part 的 `EXT-X-PRELOAD-HINT`。
    /// 当 `legacy` 为 true 时，去掉所有 LL-HLS 标签，仅输出已完成分段（传统 HLS 兼容模式）。
    /// 当 `concluded` 为 true 时，追加 `#EXT-X-ENDLIST`（流已结束）。
    pub fn build_media_ll(
        ring: &SegmentRing,
        ll_state: &LowLatencyState,
        session_id: Option<u64>,
        prefix: &str,
        legacy: bool,
        concluded: bool,
    ) -> String {
        if ring.is_empty() && ll_state.current_parts().is_empty() {
            return Self::build_empty_media(HlsContainer::Fmp4);
        }

        let target_duration = ring.max_duration().ceil().max(1.0) as u64;
        let media_sequence = ring.first_sequence();

        let mut out = String::with_capacity(1024);
        out.push_str("#EXTM3U\n");
        if legacy {
            out.push_str("#EXT-X-VERSION:6\n");
        } else {
            out.push_str("#EXT-X-VERSION:9\n");
        }
        out.push_str(&format!("#EXT-X-TARGETDURATION:{target_duration}\n"));
        out.push_str(&format!("#EXT-X-MEDIA-SEQUENCE:{media_sequence}\n"));

        // LL-HLS header tags (only in non-legacy mode)
        if !legacy {
            out.push_str(&ll_state.playlist_header_tags());
        }

        // EXT-X-MAP for fMP4 init segment
        match session_id {
            Some(uid) => out.push_str(&format!("#EXT-X-MAP:URI=\"{prefix}init.mp4?uid={uid}\"\n")),
            None => out.push_str(&format!("#EXT-X-MAP:URI=\"{prefix}init.mp4\"\n")),
        }

        // Completed segments with their archived parts
        for seg in ring.iter() {
            // CUE markers before this segment
            if let Some(ref cue) = seg.cue_tags {
                out.push_str(cue);
            }
            if let Some(pdt_ms) = seg.program_date_time_ms {
                out.push_str(&format!(
                    "#EXT-X-PROGRAM-DATE-TIME:{}\n",
                    format_iso8601(pdt_ms)
                ));
            }
            // EXT-X-PART tags only in non-legacy mode
            if !legacy {
                if let Some(sp) = ll_state
                    .completed_segments_parts()
                    .iter()
                    .find(|sp| sp.segment_sequence == seg.sequence)
                {
                    out.push_str(&LowLatencyState::format_part_tags(&sp.parts, prefix));
                }
            }
            out.push_str(&format!("#EXTINF:{:.3},\n", seg.duration_secs));
            match session_id {
                Some(uid) => out.push_str(&format!("{prefix}{}.m4s?uid={uid}\n", seg.name)),
                None => out.push_str(&format!("{prefix}{}.m4s\n", seg.name)),
            }
        }

        // Current (in-progress) segment parts — only in non-legacy mode
        if !legacy && !ll_state.current_parts().is_empty() {
            out.push_str(&ll_state.part_tags(prefix));
        }

        // Preload hint for next part — only in non-legacy mode and not concluded
        if !legacy && !concluded {
            out.push_str(&ll_state.preload_hint_tag(prefix));
        }

        // Rendition reports — only in non-legacy live mode
        if !legacy && !concluded {
            out.push_str(&ll_state.rendition_report_tags());
        }

        if concluded {
            out.push_str("#EXT-X-ENDLIST\n");
        }

        out
    }
}

/// Format Unix milliseconds as ISO 8601 date-time string.
///
/// Output: "2006-01-02T15:04:05.000Z"
///
/// 将 Unix 毫秒格式化为 ISO 8601 日期时间字符串。
/// 输出："2006-01-02T15:04:05.000Z"。
///
/// This implementation uses a Euclidean algorithm to convert days since the epoch into
/// a civil calendar date without pulling in a chrono dependency.
///
/// 本实现使用欧几里得算法将自纪元以来的天数转换为公历日期，无需引入 chrono 依赖。
pub fn format_iso8601(unix_ms: i64) -> String {
    let secs = unix_ms / 1000;
    let millis = (unix_ms % 1000).unsigned_abs() as u32;
    // Simple UTC formatting without chrono dependency
    let days_since_epoch = secs / 86400;
    let time_of_day = (secs % 86400 + 86400) % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Civil date from days since 1970-01-01 (Euclidean algorithm)
    let z = days_since_epoch + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y, m, d, hours, minutes, seconds, millis
    )
}

/// A segment entry for file-based playlist generation.
///
/// 基于文件的播放列表生成所用的分片条目。
#[derive(Debug, Clone)]
pub struct SegmentFileEntry {
    pub filename: String,
    pub duration_secs: f64,
}

/// Info about a media rendition for demuxed master playlist generation.
///
/// 用于解复用主播放列表生成的媒体 rendition 信息。
#[derive(Debug, Clone)]
pub struct MediaRenditionInfo {
    pub codecs: String,
    pub bandwidth: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<f64>,
    pub channels: Option<u8>,
}

/// Info about a subtitle rendition for the master playlist.
///
/// 用于主播放列表的字幕 rendition 信息。
#[derive(Debug, Clone)]
pub struct SubtitleRenditionInfo {
    /// Display name for the subtitle track.
    pub name: String,
    /// BCP-47 language tag, e.g. `en` or `zh-CN`.
    pub language: String,
    /// Whether this track is selected by default.
    pub is_default: bool,
    /// Whether the player may auto-select this track.
    pub autoselect: bool,
}

/// Info about a video variant rendition for a master playlist.
///
/// 用于主播放列表的视频档位 rendition 信息。
#[derive(Debug, Clone)]
pub struct VariantRenditionInfo {
    /// Media attributes (codecs, bandwidth, resolution, frame-rate, channels).
    pub info: MediaRenditionInfo,
    /// URI to the variant's media playlist.
    pub uri: String,
}

/// Builder for demuxed LLHLS master playlist with independent audio rendition.
///
/// 生成独立音频 rendition 的解复用 LLHLS 主播放列表构建器。
pub struct DemuxedMasterPlaylist;

impl DemuxedMasterPlaylist {
    /// Generate a demuxed master playlist.
    ///
    /// Output includes `EXT-X-MEDIA:TYPE=AUDIO` and `EXT-X-STREAM-INF` with `AUDIO="audio"`.
    ///
    /// 生成解复用主播放列表。
    /// 输出包含 `EXT-X-MEDIA:TYPE=AUDIO` 与带 `AUDIO="audio"` 的 `EXT-X-STREAM-INF`。
    pub fn build(
        video: Option<&MediaRenditionInfo>,
        audio: Option<&MediaRenditionInfo>,
        stream_name: &str,
        session_id: Option<u64>,
        include_stream_key: bool,
        stream_key: &str,
    ) -> String {
        Self::build_with_subtitles(
            video,
            audio,
            None,
            stream_name,
            session_id,
            include_stream_key,
            stream_key,
        )
    }

    /// Generate a demuxed master playlist with optional subtitle rendition.
    ///
    /// Output includes `EXT-X-MEDIA:TYPE=SUBTITLES` and `EXT-X-STREAM-INF` with
    /// `SUBTITLES="subs"` when `subtitle` is provided.
    pub fn build_with_subtitles(
        video: Option<&MediaRenditionInfo>,
        audio: Option<&MediaRenditionInfo>,
        subtitle: Option<&SubtitleRenditionInfo>,
        stream_name: &str,
        session_id: Option<u64>,
        include_stream_key: bool,
        stream_key: &str,
    ) -> String {
        let mut out = String::with_capacity(512);
        out.push_str("#EXTM3U\n");
        out.push_str("#EXT-X-VERSION:9\n");
        out.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");

        let uid_suffix = session_id
            .map(|uid| format!("?uid={uid}"))
            .unwrap_or_default();
        let key_suffix = if include_stream_key && !stream_key.is_empty() {
            let sep = if uid_suffix.is_empty() { '?' } else { '&' };
            format!("{sep}k={stream_key}")
        } else {
            String::new()
        };

        // Audio rendition declaration
        if let Some(audio_info) = audio {
            let channels = audio_info.channels.unwrap_or(2);
            out.push_str(&format!(
                "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"default\",\
                 DEFAULT=YES,AUTOSELECT=YES,CHANNELS=\"{channels}\",\
                 URI=\"{stream_name}/chunklist_audio.m3u8{uid_suffix}{key_suffix}\"\n"
            ));
        }

        // Subtitle rendition declaration
        if let Some(sub_info) = subtitle {
            let default = if sub_info.is_default { "YES" } else { "NO" };
            let autoselect = if sub_info.autoselect { "YES" } else { "NO" };
            out.push_str(&format!(
                "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"{}\",\
                 LANGUAGE=\"{}\",DEFAULT={default},AUTOSELECT={autoselect},\
                 URI=\"{stream_name}/chunklist_subtitles.m3u8{uid_suffix}{key_suffix}\"\n",
                sub_info.name, sub_info.language
            ));
        }

        // Video variant with audio group reference
        if let Some(video_info) = video {
            let mut attrs = format!("BANDWIDTH={}", video_info.bandwidth);
            if let (Some(w), Some(h)) = (video_info.width, video_info.height) {
                attrs.push_str(&format!(",RESOLUTION={w}x{h}"));
            }
            if let Some(fps) = video_info.frame_rate {
                attrs.push_str(&format!(",FRAME-RATE={fps:.3}"));
            }
            // Build CODECS string combining video + audio
            let codecs = if let Some(audio_info) = audio {
                if video_info.codecs.is_empty() && audio_info.codecs.is_empty() {
                    String::new()
                } else {
                    let mut codecs = video_info.codecs.clone();
                    codecs.push(',');
                    codecs.push_str(&audio_info.codecs);
                    codecs
                }
            } else {
                video_info.codecs.clone()
            };
            if !codecs.is_empty() {
                attrs.push_str(&format!(",CODECS=\"{codecs}\""));
            }
            if audio.is_some() {
                attrs.push_str(",AUDIO=\"audio\"");
            }
            if subtitle.is_some() {
                attrs.push_str(",SUBTITLES=\"subs\"");
            }
            out.push_str(&format!("#EXT-X-STREAM-INF:{attrs}\n"));
            out.push_str(&format!(
                "{stream_name}/chunklist_video.m3u8{uid_suffix}{key_suffix}\n"
            ));
        } else if let Some(_audio_info) = audio {
            // Audio-only stream: variant points to audio chunklist
            let codecs = &audio.unwrap().codecs;
            let bandwidth = audio.unwrap().bandwidth;
            let mut attrs = format!("BANDWIDTH={bandwidth}");
            if !codecs.is_empty() {
                attrs.push_str(&format!(",CODECS=\"{codecs}\""));
            }
            if subtitle.is_some() {
                attrs.push_str(",SUBTITLES=\"subs\"");
            }
            out.push_str(&format!("#EXT-X-STREAM-INF:{attrs}\n"));
            out.push_str(&format!(
                "{stream_name}/chunklist_audio.m3u8{uid_suffix}{key_suffix}\n"
            ));
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;
    use crate::segment::SegmentRing;

    #[test]
    fn master_playlist_format() {
        let m3u8 = PlaylistBuilder::build_master("stream", 12345);
        assert!(m3u8.contains("#EXTM3U"));
        assert!(m3u8.contains("BANDWIDTH=2000000"));
        assert!(m3u8.contains("stream/index.m3u8?uid=12345"));
    }

    #[test]
    fn media_playlist_format() {
        let mut ring = SegmentRing::new(3);
        ring.push("seg_001".into(), 4.123, Bytes::from_static(b"x"), true);
        ring.push("seg_002".into(), 3.987, Bytes::from_static(b"y"), true);

        let m3u8 = PlaylistBuilder::build_media(&ring, Some(99));
        assert!(m3u8.contains("#EXT-X-TARGETDURATION:5"));
        assert!(m3u8.contains("#EXT-X-MEDIA-SEQUENCE:0"));
        assert!(m3u8.contains("#EXTINF:4.123,"));
        assert!(m3u8.contains("seg_001.ts?uid=99"));
        assert!(m3u8.contains("seg_002.ts?uid=99"));
        assert!(!m3u8.contains("#EXT-X-ENDLIST"));
    }

    #[test]
    fn fmp4_playlist_has_map_and_m4s() {
        let mut ring = SegmentRing::new(3);
        ring.push("seg_0".into(), 4.0, Bytes::from_static(b"x"), true);

        let m3u8 = PlaylistBuilder::build_media_with_container(&ring, Some(1), HlsContainer::Fmp4);
        assert!(m3u8.contains("#EXT-X-VERSION:7"));
        assert!(m3u8.contains("#EXT-X-MAP:URI=\"init.mp4?uid=1\""));
        assert!(m3u8.contains("seg_0.m4s?uid=1"));
        assert!(!m3u8.contains(".ts"));
    }

    #[test]
    fn live_file_playlist() {
        let segments = vec![
            SegmentFileEntry {
                filename: "seg_0".into(),
                duration_secs: 4.0,
            },
            SegmentFileEntry {
                filename: "seg_1".into(),
                duration_secs: 3.5,
            },
        ];
        let m3u8 = PlaylistBuilder::build_live_file(&segments, 5, HlsContainer::Ts);
        assert!(m3u8.contains("#EXT-X-MEDIA-SEQUENCE:5"));
        assert!(m3u8.contains("#EXT-X-TARGETDURATION:4"));
        assert!(m3u8.contains("seg_0.ts"));
        assert!(m3u8.contains("seg_1.ts"));
        assert!(!m3u8.contains("#EXT-X-ENDLIST"));
    }

    #[test]
    fn vod_playlist_has_endlist() {
        let segments = vec![SegmentFileEntry {
            filename: "seg_0".into(),
            duration_secs: 5.0,
        }];
        let m3u8 = PlaylistBuilder::build_vod(&segments, HlsContainer::Ts);
        assert!(m3u8.contains("#EXT-X-ENDLIST"));
        assert!(m3u8.contains("#EXT-X-MEDIA-SEQUENCE:0"));
    }

    #[test]
    fn ll_hls_playlist_format() {
        use crate::ll_hls::LowLatencyState;

        let mut ring = SegmentRing::new(3);
        let mut ll = LowLatencyState::new(200, 5);

        // Simulate: segment 0 with 2 parts, then finalized
        ll.note_sample(0, true);
        ll.finalize_part(Bytes::from_static(b"p0"), 0.2);
        ll.note_sample(200, false);
        ll.finalize_part(Bytes::from_static(b"p1"), 0.2);
        ll.on_segment_boundary(1);
        ring.push("seg_0".into(), 4.0, Bytes::from_static(b"seg"), true);

        // Current segment: 1 part in progress
        ll.note_sample(4000, true);
        ll.finalize_part(Bytes::from_static(b"p2"), 0.2);

        let m3u8 = PlaylistBuilder::build_media_ll(&ring, &ll, None, "", false, false);
        assert!(m3u8.contains("#EXT-X-VERSION:9"));
        assert!(m3u8.contains("#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES"));
        assert!(m3u8.contains("#EXT-X-PART-INF:PART-TARGET=0.200"));
        assert!(m3u8.contains("#EXT-X-MAP:URI=\"init.mp4\""));
        // Archived parts before segment
        assert!(m3u8.contains("#EXT-X-PART:DURATION=0.200,URI=\"part_0.m4s\",INDEPENDENT=YES"));
        assert!(m3u8.contains("#EXT-X-PART:DURATION=0.200,URI=\"part_1.m4s\""));
        // Segment
        assert!(m3u8.contains("#EXTINF:4.000,"));
        assert!(m3u8.contains("seg_0.m4s"));
        // Current part
        assert!(m3u8.contains("#EXT-X-PART:DURATION=0.200,URI=\"part_2.m4s\",INDEPENDENT=YES"));
        // Preload hint
        assert!(m3u8.contains("#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"part_3.m4s\""));
    }

    #[test]
    fn format_iso8601_epoch() {
        assert_eq!(super::format_iso8601(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn format_iso8601_known_date() {
        // 2026-05-16T01:02:03.456Z
        let result = super::format_iso8601(1778893323456);
        assert_eq!(result, "2026-05-16T01:02:03.456Z");
    }

    #[test]
    fn format_iso8601_millis_precision() {
        let result = super::format_iso8601(1000); // 1 second
        assert_eq!(result, "1970-01-01T00:00:01.000Z");
    }

    #[test]
    fn master_playlist_has_audio_rendition() {
        use super::{DemuxedMasterPlaylist, MediaRenditionInfo};
        let video = MediaRenditionInfo {
            codecs: "avc1.64001f".to_string(),
            bandwidth: 2000000,
            width: Some(1920),
            height: Some(1080),
            frame_rate: Some(30.0),
            channels: None,
        };
        let audio = MediaRenditionInfo {
            codecs: "mp4a.40.2".to_string(),
            bandwidth: 128000,
            width: None,
            height: None,
            frame_rate: None,
            channels: Some(2),
        };
        let m3u8 =
            DemuxedMasterPlaylist::build(Some(&video), Some(&audio), "stream", Some(1), false, "");
        assert!(m3u8.contains("#EXT-X-MEDIA:TYPE=AUDIO"));
        assert!(m3u8.contains("GROUP-ID=\"audio\""));
        assert!(m3u8.contains("stream/chunklist_audio.m3u8?uid=1"));
        assert!(m3u8.contains("AUDIO=\"audio\""));
        assert!(m3u8.contains("CODECS=\"avc1.64001f,mp4a.40.2\""));
        assert!(m3u8.contains("RESOLUTION=1920x1080"));
        assert!(m3u8.contains("stream/chunklist_video.m3u8?uid=1"));
    }

    #[test]
    fn master_playlist_video_only_has_no_audio_group() {
        use super::{DemuxedMasterPlaylist, MediaRenditionInfo};
        let video = MediaRenditionInfo {
            codecs: "avc1.64001f".to_string(),
            bandwidth: 2000000,
            width: Some(1280),
            height: Some(720),
            frame_rate: None,
            channels: None,
        };
        let m3u8 = DemuxedMasterPlaylist::build(Some(&video), None, "stream", None, false, "");
        assert!(!m3u8.contains("#EXT-X-MEDIA:TYPE=AUDIO"));
        assert!(!m3u8.contains("AUDIO=\"audio\""));
        assert!(m3u8.contains("stream/chunklist_video.m3u8"));
    }

    #[test]
    fn audio_only_master_points_to_audio_chunklist() {
        use super::{DemuxedMasterPlaylist, MediaRenditionInfo};
        let audio = MediaRenditionInfo {
            codecs: "mp4a.40.2".to_string(),
            bandwidth: 128000,
            width: None,
            height: None,
            frame_rate: None,
            channels: Some(2),
        };
        let m3u8 = DemuxedMasterPlaylist::build(None, Some(&audio), "stream", None, false, "");
        assert!(m3u8.contains("stream/chunklist_audio.m3u8"));
        assert!(m3u8.contains("BANDWIDTH=128000"));
    }

    #[test]
    fn vtt_media_playlist_from_mux() {
        use super::{PlaylistBuilder, VttMux};
        use crate::VttMuxConfig;
        use cheetah_codec::subtitle::WebVttCue;

        let mut mux = VttMux::new(VttMuxConfig {
            segment_duration_ms: 4_000,
            max_segments: 4,
        });
        mux.push_cue(WebVttCue {
            id: None,
            start_ms: 500,
            end_ms: 3_500,
            payload: "Hello".to_string(),
            settings: None,
        })
        .unwrap();
        mux.close_segment(4_000).unwrap();

        let m3u8 = PlaylistBuilder::build_vtt_media(&mux, Some(42));
        assert!(m3u8.contains("#EXTM3U"));
        assert!(m3u8.contains("#EXT-X-VERSION:3"));
        assert!(m3u8.contains("#EXT-X-MEDIA-SEQUENCE:0"));
        assert!(m3u8.contains("#EXTINF:4.000,"));
        assert!(m3u8.contains("sub0.vtt?uid=42"));
    }

    #[test]
    fn master_playlist_with_subtitle_rendition() {
        use super::{DemuxedMasterPlaylist, MediaRenditionInfo, SubtitleRenditionInfo};

        let video = MediaRenditionInfo {
            codecs: "avc1.64001f".to_string(),
            bandwidth: 2_000_000,
            width: Some(1280),
            height: Some(720),
            frame_rate: None,
            channels: None,
        };
        let subtitle = SubtitleRenditionInfo {
            name: "English".to_string(),
            language: "en".to_string(),
            is_default: true,
            autoselect: true,
        };
        let m3u8 = DemuxedMasterPlaylist::build_with_subtitles(
            Some(&video),
            None,
            Some(&subtitle),
            "stream",
            Some(1),
            false,
            "",
        );
        assert!(m3u8.contains("#EXT-X-MEDIA:TYPE=SUBTITLES"));
        assert!(m3u8.contains("GROUP-ID=\"subs\""));
        assert!(m3u8.contains("LANGUAGE=\"en\""));
        assert!(m3u8.contains("stream/chunklist_subtitles.m3u8?uid=1"));
        assert!(m3u8.contains("SUBTITLES=\"subs\""));
    }
}
