//! Lightweight M3U8 media playlist parser for HLS pull scenarios.

use crate::error::HlsCoreError;

/// Parsed media playlist.
#[derive(Debug, Clone)]
pub struct ParsedMediaPlaylist {
    /// `target_duration` field of type `u32`.
    /// `target_duration` 字段，类型为 `u32`.
    pub target_duration: u32,
    /// `media_sequence` field of type `u64`.
    /// `media_sequence` 字段，类型为 `u64`.
    pub media_sequence: u64,
    /// `segments` field.
    /// `segments` 字段.
    pub segments: Vec<ParsedSegment>,
    /// `end_list` field of type `bool`.
    /// `end_list` 字段，类型为 `bool`.
    pub end_list: bool,
}

/// A single segment entry from a parsed playlist.
#[derive(Debug, Clone)]
pub struct ParsedSegment {
    /// `duration` field of type `f64`.
    /// `duration` 字段，类型为 `f64`.
    pub duration: f64,
    /// `uri` field of type `String`.
    /// `uri` 字段，类型为 `String`.
    pub uri: String,
}

/// Parsed master (multivariant) playlist.
#[derive(Debug, Clone)]
pub struct ParsedMasterPlaylist {
    /// `variants` field.
    /// `variants` 字段.
    pub variants: Vec<ParsedVariant>,
}

/// A variant stream entry.
#[derive(Debug, Clone)]
pub struct ParsedVariant {
    /// `bandwidth` field of type `u64`.
    /// `bandwidth` 字段，类型为 `u64`.
    pub bandwidth: u64,
    /// `uri` field of type `String`.
    /// `uri` 字段，类型为 `String`.
    pub uri: String,
}

/// Parse a media playlist from text.
pub fn parse_media_playlist(input: &str) -> Result<ParsedMediaPlaylist, HlsCoreError> {
    if !input.trim_start().starts_with("#EXTM3U") {
        return Err(HlsCoreError::InvalidPath {
            path: "not a valid M3U8 playlist".to_string(),
        });
    }

    let mut target_duration: u32 = 0;
    let mut media_sequence: u64 = 0;
    let mut segments = Vec::new();
    let mut end_list = false;
    let mut pending_duration: Option<f64> = None;

    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(val) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            target_duration = val.trim().parse().unwrap_or(0);
        } else if let Some(val) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            media_sequence = val.trim().parse().unwrap_or(0);
        } else if line == "#EXT-X-ENDLIST" {
            end_list = true;
        } else if let Some(val) = line.strip_prefix("#EXTINF:") {
            // Format: duration[,title]
            let dur_str = val.split(',').next().unwrap_or("0");
            pending_duration = Some(dur_str.trim().parse().unwrap_or(0.0));
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

    Ok(ParsedMediaPlaylist {
        target_duration,
        media_sequence,
        segments,
        end_list,
    })
}

/// Parse a master playlist from text.
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
    fn rejects_non_m3u8() {
        assert!(parse_media_playlist("not a playlist").is_err());
        assert!(parse_master_playlist("garbage").is_err());
    }
}
