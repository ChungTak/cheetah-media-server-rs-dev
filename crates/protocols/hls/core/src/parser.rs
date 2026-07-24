//! Lightweight M3U8 media playlist parser for HLS pull scenarios.
//!
//! 轻量级 M3U8 媒体播放列表解析器，用于 HLS 拉流场景。
//! 仅解析 `media playlist` 与 `master playlist` 的关键字段，不做完整属性校验。

use crate::error::HlsCoreError;

/// Parsed media playlist.
///
/// 解析后的媒体播放列表。
#[derive(Debug, Clone)]
pub struct ParsedMediaPlaylist {
    pub target_duration: u32,
    pub media_sequence: u64,
    pub segments: Vec<ParsedSegment>,
    pub end_list: bool,
}

/// A single segment entry from a parsed playlist.
///
/// 解析后播放列表中的单个分片条目。
#[derive(Debug, Clone)]
pub struct ParsedSegment {
    pub duration: f64,
    pub uri: String,
}

/// Parsed master (multivariant) playlist.
///
/// 解析后的主（多码率）播放列表。
#[derive(Debug, Clone)]
pub struct ParsedMasterPlaylist {
    pub variants: Vec<ParsedVariant>,
}

/// A variant stream entry.
///
/// 变体流条目。
#[derive(Debug, Clone)]
pub struct ParsedVariant {
    pub bandwidth: u64,
    pub uri: String,
}

/// Parse a media playlist from text.
///
/// The parser validates the `#EXTM3U` magic, then scans line by line. It extracts
/// `TARGETDURATION`, `MEDIA-SEQUENCE`, and `#EXT-X-ENDLIST`. Each `#EXTINF:` line
/// stores the duration in a pending slot; the next non-comment line is consumed as
/// the segment URI. This is a one-pass finite-state scan over the playlist.
///
/// 从文本解析媒体播放列表。
/// 先校验 `#EXTM3U` 魔数，再逐行扫描。提取 `TARGETDURATION`、`MEDIA-SEQUENCE` 和 `#EXT-X-ENDLIST`。
/// 每个 `#EXTINF:` 行将时长存入 pending 槽；下一行非注释行作为分片 URI 消费。
/// 这是针对播放列表的单遍有限状态扫描。
pub fn parse_media_playlist(input: &str) -> Result<ParsedMediaPlaylist, HlsCoreError> {
    if !input.trim_start().starts_with("#EXTM3U") {
        return Err(HlsCoreError::InvalidPath {
            path: "not a valid M3U8 playlist".to_string(),
        });
    }

    let mut target_duration: Option<u32> = None;
    let mut media_sequence: Option<u64> = None;
    let mut segments = Vec::new();
    let mut end_list = false;
    let mut pending_duration: Option<f64> = None;

    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(val) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            let parsed: u32 = val
                .trim()
                .parse()
                .map_err(|_| HlsCoreError::InvalidPlaylist {
                    reason: format!("invalid #EXT-X-TARGETDURATION: {}", val.trim()),
                })?;
            if parsed == 0 {
                return Err(HlsCoreError::InvalidPlaylist {
                    reason: "#EXT-X-TARGETDURATION must be > 0".to_string(),
                });
            }
            target_duration = Some(parsed);
        } else if let Some(val) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            let parsed: u64 = val
                .trim()
                .parse()
                .map_err(|_| HlsCoreError::InvalidPlaylist {
                    reason: format!("invalid #EXT-X-MEDIA-SEQUENCE: {}", val.trim()),
                })?;
            media_sequence = Some(parsed);
        } else if line == "#EXT-X-ENDLIST" {
            end_list = true;
        } else if let Some(val) = line.strip_prefix("#EXTINF:") {
            // Format: duration[,title]
            let dur_str = val.split(',').next().unwrap_or("0");
            let parsed: f64 =
                dur_str
                    .trim()
                    .parse()
                    .map_err(|_| HlsCoreError::InvalidPlaylist {
                        reason: format!("invalid #EXTINF duration: {}", dur_str.trim()),
                    })?;
            if parsed < 0.0 {
                return Err(HlsCoreError::InvalidPlaylist {
                    reason: "#EXTINF duration must be >= 0".to_string(),
                });
            }
            pending_duration = Some(parsed);
        } else if !line.starts_with('#') {
            // URI line
            if let Some(dur) = pending_duration.take() {
                segments.push(ParsedSegment {
                    duration: dur,
                    uri: line.to_string(),
                });
            }
        }
    }

    let target_duration = target_duration.ok_or_else(|| HlsCoreError::InvalidPlaylist {
        reason: "missing #EXT-X-TARGETDURATION".to_string(),
    })?;

    Ok(ParsedMediaPlaylist {
        target_duration,
        media_sequence: media_sequence.unwrap_or(1),
        segments,
        end_list,
    })
}

/// Parse a master playlist from text.
///
/// Scans for `#EXT-X-STREAM-INF:` tags. The comma-separated attributes are searched for
/// `BANDWIDTH=`. The first non-comment line after a stream-inf tag is treated as the
/// variant URI. Unknown attributes are ignored.
///
/// 从文本解析主播放列表。
/// 扫描 `#EXT-X-STREAM-INF:` 标签，在逗号分隔的属性中查找 `BANDWIDTH=`。
/// 每个 stream-inf 标签后的第一个非注释行作为变体 URI。未知属性被忽略。
pub fn parse_master_playlist(input: &str) -> Result<ParsedMasterPlaylist, HlsCoreError> {
    if !input.trim_start().starts_with("#EXTM3U") {
        return Err(HlsCoreError::InvalidPath {
            path: "not a valid M3U8 playlist".to_string(),
        });
    }

    let mut variants = Vec::new();
    let mut pending_bandwidth: Option<u64> = None;

    for line in input.lines() {
        let line = line.trim();
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            // Extract BANDWIDTH
            for attr in attrs.split(',') {
                if let Some(val) = attr.trim().strip_prefix("BANDWIDTH=") {
                    pending_bandwidth = val.parse().ok();
                }
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            if let Some(bw) = pending_bandwidth.take() {
                variants.push(ParsedVariant {
                    bandwidth: bw,
                    uri: line.to_string(),
                });
            }
        }
    }

    Ok(ParsedMasterPlaylist { variants })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_media_playlist() {
        let input = "#EXTM3U\n\
                     #EXT-X-VERSION:3\n\
                     #EXT-X-TARGETDURATION:4\n\
                     #EXT-X-MEDIA-SEQUENCE:10\n\
                     #EXTINF:3.967,\n\
                     seg_10.ts\n\
                     #EXTINF:4.000,\n\
                     seg_11.ts\n";
        let pl = parse_media_playlist(input).unwrap();
        assert_eq!(pl.target_duration, 4);
        assert_eq!(pl.media_sequence, 10);
        assert_eq!(pl.segments.len(), 2);
        assert_eq!(pl.segments[0].uri, "seg_10.ts");
        assert!((pl.segments[0].duration - 3.967).abs() < 0.001);
        assert!(!pl.end_list);
    }

    #[test]
    fn parse_vod_playlist_with_endlist() {
        let input = "#EXTM3U\n\
                     #EXT-X-TARGETDURATION:5\n\
                     #EXTINF:5.0,\n\
                     seg0.ts\n\
                     #EXT-X-ENDLIST\n";
        let pl = parse_media_playlist(input).unwrap();
        assert!(pl.end_list);
        assert_eq!(pl.segments.len(), 1);
    }

    #[test]
    fn parse_master_playlist_variants() {
        let input = "#EXTM3U\n\
                     #EXT-X-STREAM-INF:BANDWIDTH=1280000\n\
                     low/index.m3u8\n\
                     #EXT-X-STREAM-INF:BANDWIDTH=2560000\n\
                     high/index.m3u8\n";
        let pl = parse_master_playlist(input).unwrap();
        assert_eq!(pl.variants.len(), 2);
        assert_eq!(pl.variants[0].bandwidth, 1280000);
        assert_eq!(pl.variants[0].uri, "low/index.m3u8");
        assert_eq!(pl.variants[1].bandwidth, 2560000);
    }

    #[test]
    fn parse_media_playlist_defaults_missing_media_sequence() {
        let input = "#EXTM3U\n\
                     #EXT-X-TARGETDURATION:4\n\
                     #EXTINF:3.967,\n\
                     seg_10.ts\n";
        let pl = parse_media_playlist(input).unwrap();
        assert_eq!(pl.media_sequence, 1);
    }

    #[test]
    fn rejects_missing_target_duration() {
        let input = "#EXTM3U\n\
                     #EXTINF:3.967,\n\
                     seg_10.ts\n";
        assert!(parse_media_playlist(input).is_err());
    }

    #[test]
    fn rejects_zero_target_duration() {
        let input = "#EXTM3U\n\
                     #EXT-X-TARGETDURATION:0\n\
                     #EXTINF:3.967,\n\
                     seg_10.ts\n";
        assert!(parse_media_playlist(input).is_err());
    }

    #[test]
    fn rejects_invalid_media_sequence() {
        let input = "#EXTM3U\n\
                     #EXT-X-TARGETDURATION:4\n\
                     #EXT-X-MEDIA-SEQUENCE:abc\n\
                     #EXTINF:3.967,\n\
                     seg_10.ts\n";
        assert!(parse_media_playlist(input).is_err());
    }

    #[test]
    fn rejects_negative_extinf_duration() {
        let input = "#EXTM3U\n\
                     #EXT-X-TARGETDURATION:4\n\
                     #EXTINF:-1.0,\n\
                     seg_10.ts\n";
        assert!(parse_media_playlist(input).is_err());
    }

    #[test]
    fn rejects_non_m3u8() {
        assert!(parse_media_playlist("not a playlist").is_err());
        assert!(parse_master_playlist("garbage").is_err());
    }
}
