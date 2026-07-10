use crate::error::HlsCoreError;
use crate::ll_hls::TrackLane;

/// Parsed stream key parts from URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamKeyParts {
    /// `namespace` field of type `String`.
    /// `namespace` 字段，类型为 `String`.
    pub namespace: String,
    /// `stream_path` field of type `String`.
    /// `stream_path` 字段，类型为 `String`.
    pub stream_path: String,
}

/// Blocking Playlist Reload parameters (LL-HLS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockingParams {
    /// `_HLS_msn` — target Media Sequence Number.
    pub msn: u64,
    /// `_HLS_part` — target Part Index (optional).
    pub part: Option<u64>,
}

/// Delta Update mode (LL-HLS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipMode {
    /// `_HLS_skip=YES`
    Yes,
    /// `_HLS_skip=v2` (includes Rendition Report)
    V2,
}

/// What kind of HLS resource is being requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HlsRequestKind {
    /// Master playlist: /{namespace}/{stream}.m3u8
    MasterPlaylist { stream_key: StreamKeyParts },
    /// Media playlist: /{namespace}/{stream}/{stream}.m3u8?uid={uid}
    MediaPlaylist {
        stream_key: StreamKeyParts,
        session_id: Option<u64>,
        /// LL-HLS blocking parameters (if _HLS_msn present).
        blocking: Option<BlockingParams>,
        /// LL-HLS delta update mode (if _HLS_skip present).
        skip: Option<SkipMode>,
        /// Legacy mode: strip all LL-HLS tags, output traditional HLS only.
        legacy: bool,
        /// Rewind mode: output all available segments (DVR/timeshift).
        rewind: bool,
    },
    /// TS/fMP4 segment: /{namespace}/{stream}/{segment_name}.ts or .m4s
    Segment {
        stream_key: StreamKeyParts,
        segment_name: String,
        session_id: Option<u64>,
        /// Stream key validation token (from ?k= param).
        key_token: Option<String>,
    },
    /// fMP4 init segment: /{namespace}/{stream}/init.mp4
    InitSegment {
        stream_key: StreamKeyParts,
        session_id: Option<u64>,
        /// Stream key validation token (from ?k= param).
        key_token: Option<String>,
    },
    /// LL-HLS part: /{namespace}/{stream}/part_{seq}.m4s
    Part {
        stream_key: StreamKeyParts,
        part_name: String,
        session_id: Option<u64>,
        /// Stream key validation token (from ?k= param).
        key_token: Option<String>,
    },
    /// Per-track media playlist: /{ns}/{stream}/chunklist_video.m3u8 or chunklist_audio.m3u8
    TrackMediaPlaylist {
        stream_key: StreamKeyParts,
        lane: TrackLane,
        session_id: Option<u64>,
        blocking: Option<BlockingParams>,
        skip: Option<SkipMode>,
        key_token: Option<String>,
    },
    /// Per-track init segment: /{ns}/{stream}/init_video.mp4 or init_audio.mp4
    TrackInitSegment {
        stream_key: StreamKeyParts,
        lane: TrackLane,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// Per-track part: /{ns}/{stream}/video_part_N.m4s or audio_part_N.m4s
    TrackPart {
        stream_key: StreamKeyParts,
        lane: TrackLane,
        part_name: String,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// Per-track segment: /{ns}/{stream}/video_seg_N.m4s or audio_seg_N.m4s
    TrackSegment {
        stream_key: StreamKeyParts,
        lane: TrackLane,
        segment_name: String,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// Embedded hls.js player page: /{namespace}/{stream}/
    PlayerPage { stream_key: StreamKeyParts },
}

/// Parse an HLS request target into a typed request.
pub fn parse_hls_request(target: &str) -> Result<HlsRequestKind, HlsCoreError> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let params = QueryParams::parse(query);

    let path = path.strip_prefix('/').unwrap_or(path);

    // Handle player page: /{ns}/{stream}/ (trailing slash, path ends without file)
    if path.ends_with('/') {
        let trimmed = path.trim_end_matches('/');
        let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
        if segments.len() == 2 {
            return Ok(HlsRequestKind::PlayerPage {
                stream_key: StreamKeyParts {
                    namespace: segments[0].to_string(),
                    stream_path: segments[1].to_string(),
                },
            });
        }
    }

    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    match segments.len() {
        // /{namespace}/{stream}.m3u8
        2 => {
            let namespace = segments[0];
            let file = segments[1];
            if let Some(stream) = file.strip_suffix(".m3u8") {
                Ok(HlsRequestKind::MasterPlaylist {
                    stream_key: StreamKeyParts {
                        namespace: namespace.to_string(),
                        stream_path: stream.to_string(),
                    },
                })
            } else {
                Err(HlsCoreError::InvalidPath {
                    path: target.to_string(),
                })
            }
        }
        // /{namespace}/{stream}/{file}
        3 => {
            let namespace = segments[0];
            let stream = segments[1];
            let file = segments[2];
            let stream_key = StreamKeyParts {
                namespace: namespace.to_string(),
                stream_path: stream.to_string(),
            };

            if file.ends_with(".m3u8") {
                // Per-track chunklist: chunklist_video.m3u8 / chunklist_audio.m3u8
                let base = file.strip_suffix(".m3u8").unwrap_or("");
                if let Some(lane) = parse_chunklist_lane(base) {
                    Ok(HlsRequestKind::TrackMediaPlaylist {
                        stream_key,
                        lane,
                        session_id: params.uid,
                        blocking: params.blocking,
                        skip: params.skip,
                        key_token: params.key_token,
                    })
                } else {
                    Ok(HlsRequestKind::MediaPlaylist {
                        stream_key,
                        session_id: params.uid,
                        blocking: params.blocking,
                        skip: params.skip,
                        legacy: params.legacy,
                        rewind: params.rewind,
                    })
                }
            } else if file == "init.mp4" || file == "init.mp" {
                Ok(HlsRequestKind::InitSegment {
                    stream_key,
                    session_id: params.uid,
                    key_token: params.key_token,
                })
            } else if file == "init_video.mp4" {
                Ok(HlsRequestKind::TrackInitSegment {
                    stream_key,
                    lane: TrackLane::Video,
                    session_id: params.uid,
                    key_token: params.key_token,
                })
            } else if file == "init_audio.mp4" {
                Ok(HlsRequestKind::TrackInitSegment {
                    stream_key,
                    lane: TrackLane::Audio,
                    session_id: params.uid,
                    key_token: params.key_token,
                })
            } else if let Some(seg_name) = file.strip_suffix(".m4s") {
                // Per-track parts: video_part_N, audio_part_N
                if let Some(rest) = seg_name.strip_prefix("video_part_") {
                    Ok(HlsRequestKind::TrackPart {
                        stream_key,
                        lane: TrackLane::Video,
                        part_name: format!("video_part_{rest}"),
                        session_id: params.uid,
                        key_token: params.key_token,
                    })
                } else if let Some(rest) = seg_name.strip_prefix("audio_part_") {
                    Ok(HlsRequestKind::TrackPart {
                        stream_key,
                        lane: TrackLane::Audio,
                        part_name: format!("audio_part_{rest}"),
                        session_id: params.uid,
                        key_token: params.key_token,
                    })
                } else if let Some(rest) = seg_name.strip_prefix("video_seg_") {
                    Ok(HlsRequestKind::TrackSegment {
                        stream_key,
                        lane: TrackLane::Video,
                        segment_name: format!("video_seg_{rest}"),
                        session_id: params.uid,
                        key_token: params.key_token,
                    })
                } else if let Some(rest) = seg_name.strip_prefix("audio_seg_") {
                    Ok(HlsRequestKind::TrackSegment {
                        stream_key,
                        lane: TrackLane::Audio,
                        segment_name: format!("audio_seg_{rest}"),
                        session_id: params.uid,
                        key_token: params.key_token,
                    })
                } else if seg_name.starts_with("part_") {
                    // Legacy part URL
                    Ok(HlsRequestKind::Part {
                        stream_key,
                        part_name: seg_name.to_string(),
                        session_id: params.uid,
                        key_token: params.key_token,
                    })
                } else {
                    // Legacy segment URL
                    Ok(HlsRequestKind::Segment {
                        stream_key,
                        segment_name: seg_name.to_string(),
                        session_id: params.uid,
                        key_token: params.key_token,
                    })
                }
            } else if let Some(seg_name) = file.strip_suffix(".ts") {
                Ok(HlsRequestKind::Segment {
                    stream_key,
                    segment_name: seg_name.to_string(),
                    session_id: params.uid,
                    key_token: params.key_token,
                })
            } else {
                Err(HlsCoreError::InvalidPath {
                    path: target.to_string(),
                })
            }
        }
        _ => Err(HlsCoreError::InvalidPath {
            path: target.to_string(),
        }),
    }
}

/// Parse lane from chunklist filename: "chunklist_video" -> Video, "chunklist_audio" -> Audio.
fn parse_chunklist_lane(base: &str) -> Option<TrackLane> {
    match base {
        "chunklist_video" => Some(TrackLane::Video),
        "chunklist_audio" => Some(TrackLane::Audio),
        _ => None,
    }
}

/// Parsed query parameters.
struct QueryParams {
    uid: Option<u64>,
    blocking: Option<BlockingParams>,
    skip: Option<SkipMode>,
    legacy: bool,
    rewind: bool,
    key_token: Option<String>,
}

impl QueryParams {
    fn parse(query: &str) -> Self {
        let mut uid = None;
        let mut msn = None;
        let mut part = None;
        let mut skip = None;
        let mut legacy = false;
        let mut rewind = false;
        let mut key_token = None;

        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            if let Some((key, val)) = pair.split_once('=') {
                match key {
                    "uid" | "session" => uid = val.parse().ok(),
                    "_HLS_msn" => msn = val.parse().ok(),
                    "_HLS_part" => part = val.parse().ok(),
                    "_HLS_skip" => {
                        skip = Some(match val {
                            "v2" => SkipMode::V2,
                            _ => SkipMode::Yes,
                        });
                    }
                    "_HLS_legacy" => {
                        legacy = val.eq_ignore_ascii_case("YES");
                    }
                    "_HLS_rewind" => {
                        rewind = val.eq_ignore_ascii_case("YES");
                    }
                    "k" => {
                        key_token = Some(val.to_string());
                    }
                    _ => {}
                }
            }
        }

        let blocking = msn.map(|m| BlockingParams { msn: m, part });

        Self {
            uid,
            blocking,
            skip,
            legacy,
            rewind,
            key_token,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_master_playlist() {
        let req = parse_hls_request("/live/stream.m3u8").unwrap();
        assert_eq!(
            req,
            HlsRequestKind::MasterPlaylist {
                stream_key: StreamKeyParts {
                    namespace: "live".to_string(),
                    stream_path: "stream".to_string(),
                },
            }
        );
    }

    #[test]
    fn parse_media_playlist_with_uid() {
        let req = parse_hls_request("/live/stream/index.m3u8?uid=42").unwrap();
        assert_eq!(
            req,
            HlsRequestKind::MediaPlaylist {
                stream_key: StreamKeyParts {
                    namespace: "live".to_string(),
                    stream_path: "stream".to_string(),
                },
                session_id: Some(42),
                blocking: None,
                skip: None,
                legacy: false,
                rewind: false,
            }
        );
    }

    #[test]
    fn parse_segment_request() {
        let req = parse_hls_request("/live/stream/seg_1715000000.ts?uid=42").unwrap();
        assert_eq!(
            req,
            HlsRequestKind::Segment {
                stream_key: StreamKeyParts {
                    namespace: "live".to_string(),
                    stream_path: "stream".to_string(),
                },
                segment_name: "seg_1715000000".to_string(),
                session_id: Some(42),
                key_token: None,
            }
        );
    }

    #[test]
    fn rejects_invalid_path() {
        assert!(parse_hls_request("/live/stream.ts").is_err());
        assert!(parse_hls_request("/stream.m3u8").is_err());
    }

    #[test]
    fn parse_m4s_segment_request() {
        let req = parse_hls_request("/live/stream/seg_0.m4s?uid=1").unwrap();
        assert_eq!(
            req,
            HlsRequestKind::Segment {
                stream_key: StreamKeyParts {
                    namespace: "live".to_string(),
                    stream_path: "stream".to_string(),
                },
                segment_name: "seg_0".to_string(),
                session_id: Some(1),
                key_token: None,
            }
        );
    }

    #[test]
    fn parse_init_segment_request() {
        let req = parse_hls_request("/live/stream/init.mp4?uid=5").unwrap();
        assert_eq!(
            req,
            HlsRequestKind::InitSegment {
                stream_key: StreamKeyParts {
                    namespace: "live".to_string(),
                    stream_path: "stream".to_string(),
                },
                session_id: Some(5),
                key_token: None,
            }
        );
    }

    #[test]
    fn parse_init_segment_request_with_key_token() {
        let req = parse_hls_request("/live/stream/init.mp4?uid=5&k=abc123").unwrap();
        assert_eq!(
            req,
            HlsRequestKind::InitSegment {
                stream_key: StreamKeyParts {
                    namespace: "live".to_string(),
                    stream_path: "stream".to_string(),
                },
                session_id: Some(5),
                key_token: Some("abc123".to_string()),
            }
        );
    }

    #[test]
    fn parse_blocking_playlist_request() {
        let req =
            parse_hls_request("/live/stream/index.m3u8?uid=1&_HLS_msn=10&_HLS_part=3").unwrap();
        assert_eq!(
            req,
            HlsRequestKind::MediaPlaylist {
                stream_key: StreamKeyParts {
                    namespace: "live".to_string(),
                    stream_path: "stream".to_string(),
                },
                session_id: Some(1),
                blocking: Some(BlockingParams {
                    msn: 10,
                    part: Some(3)
                }),
                skip: None,
                legacy: false,
                rewind: false,
            }
        );
    }

    #[test]
    fn parse_delta_update_request() {
        let req = parse_hls_request("/live/stream/index.m3u8?_HLS_msn=5&_HLS_skip=YES").unwrap();
        assert_eq!(
            req,
            HlsRequestKind::MediaPlaylist {
                stream_key: StreamKeyParts {
                    namespace: "live".to_string(),
                    stream_path: "stream".to_string(),
                },
                session_id: None,
                blocking: Some(BlockingParams { msn: 5, part: None }),
                skip: Some(SkipMode::Yes),
                legacy: false,
                rewind: false,
            }
        );
    }

    #[test]
    fn parse_part_request() {
        let req = parse_hls_request("/live/stream/part_5.m4s?uid=1").unwrap();
        assert_eq!(
            req,
            HlsRequestKind::Part {
                stream_key: StreamKeyParts {
                    namespace: "live".to_string(),
                    stream_path: "stream".to_string(),
                },
                part_name: "part_5".to_string(),
                session_id: Some(1),
                key_token: None,
            }
        );
    }

    #[test]
    fn parse_skip_v2() {
        let req = parse_hls_request("/live/stream/index.m3u8?_HLS_msn=5&_HLS_skip=v2").unwrap();
        match req {
            HlsRequestKind::MediaPlaylist { skip, .. } => {
                assert_eq!(skip, Some(SkipMode::V2));
            }
            _ => panic!("expected MediaPlaylist"),
        }
    }

    #[test]
    fn parse_mp_suffix_as_init_segment() {
        let req = parse_hls_request("/live/stream/init.mp").unwrap();
        assert_eq!(
            req,
            HlsRequestKind::InitSegment {
                stream_key: StreamKeyParts {
                    namespace: "live".to_string(),
                    stream_path: "stream".to_string(),
                },
                session_id: None,
                key_token: None,
            }
        );
    }

    #[test]
    fn parse_session_query_param() {
        let req = parse_hls_request("/live/stream/index.m3u8?session=99").unwrap();
        match req {
            HlsRequestKind::MediaPlaylist { session_id, .. } => {
                assert_eq!(session_id, Some(99));
            }
            _ => panic!("expected MediaPlaylist"),
        }
    }

    #[test]
    fn parse_track_media_playlist_video() {
        let req = parse_hls_request("/live/stream/chunklist_video.m3u8?uid=1").unwrap();
        match req {
            HlsRequestKind::TrackMediaPlaylist {
                lane, session_id, ..
            } => {
                assert_eq!(lane, TrackLane::Video);
                assert_eq!(session_id, Some(1));
            }
            _ => panic!("expected TrackMediaPlaylist, got {req:?}"),
        }
    }

    #[test]
    fn parse_track_media_playlist_audio() {
        let req =
            parse_hls_request("/live/stream/chunklist_audio.m3u8?_HLS_msn=5&_HLS_part=2").unwrap();
        match req {
            HlsRequestKind::TrackMediaPlaylist { lane, blocking, .. } => {
                assert_eq!(lane, TrackLane::Audio);
                assert_eq!(
                    blocking,
                    Some(BlockingParams {
                        msn: 5,
                        part: Some(2)
                    })
                );
            }
            _ => panic!("expected TrackMediaPlaylist, got {req:?}"),
        }
    }

    #[test]
    fn parse_track_init_video() {
        let req = parse_hls_request("/live/stream/init_video.mp4?k=abc").unwrap();
        match req {
            HlsRequestKind::TrackInitSegment {
                lane, key_token, ..
            } => {
                assert_eq!(lane, TrackLane::Video);
                assert_eq!(key_token, Some("abc".to_string()));
            }
            _ => panic!("expected TrackInitSegment, got {req:?}"),
        }
    }

    #[test]
    fn parse_track_init_audio() {
        let req = parse_hls_request("/live/stream/init_audio.mp4").unwrap();
        match req {
            HlsRequestKind::TrackInitSegment { lane, .. } => {
                assert_eq!(lane, TrackLane::Audio);
            }
            _ => panic!("expected TrackInitSegment, got {req:?}"),
        }
    }

    #[test]
    fn parse_track_part_video() {
        let req = parse_hls_request("/live/stream/video_part_7.m4s?uid=1").unwrap();
        match req {
            HlsRequestKind::TrackPart {
                lane, part_name, ..
            } => {
                assert_eq!(lane, TrackLane::Video);
                assert_eq!(part_name, "video_part_7");
            }
            _ => panic!("expected TrackPart, got {req:?}"),
        }
    }

    #[test]
    fn parse_track_part_audio() {
        let req = parse_hls_request("/live/stream/audio_part_3.m4s").unwrap();
        match req {
            HlsRequestKind::TrackPart {
                lane, part_name, ..
            } => {
                assert_eq!(lane, TrackLane::Audio);
                assert_eq!(part_name, "audio_part_3");
            }
            _ => panic!("expected TrackPart, got {req:?}"),
        }
    }

    #[test]
    fn parse_track_segment_video() {
        let req = parse_hls_request("/live/stream/video_seg_0.m4s?uid=1").unwrap();
        match req {
            HlsRequestKind::TrackSegment {
                lane, segment_name, ..
            } => {
                assert_eq!(lane, TrackLane::Video);
                assert_eq!(segment_name, "video_seg_0");
            }
            _ => panic!("expected TrackSegment, got {req:?}"),
        }
    }

    #[test]
    fn parse_track_segment_audio() {
        let req = parse_hls_request("/live/stream/audio_seg_2.m4s").unwrap();
        match req {
            HlsRequestKind::TrackSegment {
                lane, segment_name, ..
            } => {
                assert_eq!(lane, TrackLane::Audio);
                assert_eq!(segment_name, "audio_seg_2");
            }
            _ => panic!("expected TrackSegment, got {req:?}"),
        }
    }

    #[test]
    fn legacy_urls_still_work() {
        // Legacy init.mp4 still parses as InitSegment
        let req = parse_hls_request("/live/stream/init.mp4").unwrap();
        assert!(matches!(req, HlsRequestKind::InitSegment { .. }));

        // Legacy part_N.m4s still parses as Part
        let req = parse_hls_request("/live/stream/part_5.m4s").unwrap();
        assert!(matches!(req, HlsRequestKind::Part { .. }));

        // Legacy seg_N.m4s still parses as Segment
        let req = parse_hls_request("/live/stream/seg_0.m4s").unwrap();
        assert!(matches!(req, HlsRequestKind::Segment { .. }));

        // index.m3u8 still parses as MediaPlaylist
        let req = parse_hls_request("/live/stream/index.m3u8").unwrap();
        assert!(matches!(req, HlsRequestKind::MediaPlaylist { .. }));
    }
}
