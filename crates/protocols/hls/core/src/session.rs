//! HLS core Sans-I/O state machine.
//!
//! Handles HTTP request routing and response generation for HLS.
//! Does NOT manage segment creation (that's driven by the module layer feeding frames).

use bytes::Bytes;

use crate::error::HlsCoreError;
use crate::ll_hls::TrackLane;
use crate::request::{parse_hls_request, BlockingParams, HlsRequestKind, SkipMode, StreamKeyParts};

/// `HlsSessionId` type alias.
/// `HlsSessionId` 类型别名.
pub type HlsSessionId = u64;

/// Extracted HTTP request headers relevant to HLS.
#[derive(Debug, Clone, Default)]
pub struct HlsRequestHeaders {
    /// Authorization header value (e.g., "Bearer <token>").
    pub authorization: Option<String>,
    /// User-Agent header value.
    pub user_agent: Option<String>,
    /// If-None-Match header value (for conditional requests).
    pub if_none_match: Option<String>,
    /// Whether client accepts gzip encoding.
    pub accept_gzip: bool,
}

/// Input to the HLS core state machine.
#[derive(Debug)]
pub enum HlsCoreInput {
    /// An HTTP request arrived.
    HttpRequest {
        method: HttpMethod,
        target: String,
        connection_id: u64,
        headers: HlsRequestHeaders,
    },
    /// Command from module layer.
    Command(HlsCoreCommand),
}

/// HTTP method (simplified for HLS — only GET and OPTIONS matter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// `Get` variant.
    /// `Get` 变体.
    Get,
    /// `Options` variant.
    /// `Options` 变体.
    Options,
    /// `Head` variant.
    /// `Head` 变体.
    Head,
    /// `Other` variant.
    /// `Other` 变体.
    Other,
}

/// Commands from module to core.
#[derive(Debug)]
pub enum HlsCoreCommand {
    /// Provide playlist content for a pending request.
    SendPlaylist { connection_id: u64, content: String },
    /// Provide segment data for a pending request.
    SendSegment { connection_id: u64, data: Bytes },
    /// Report an error for a pending request.
    SendError {
        connection_id: u64,
        error: HlsCoreError,
    },
}

/// Output from the HLS core state machine.
#[derive(Debug)]
pub enum HlsCoreOutput {
    /// Send an HTTP response.
    SendResponse {
        connection_id: u64,
        status: u16,
        content_type: &'static str,
        body: Bytes,
        headers: Vec<(&'static str, String)>,
    },
    /// Event for the module layer.
    Event(HlsCoreEvent),
}

/// Events bubbled up to the module layer.
#[derive(Debug, Clone)]
pub enum HlsCoreEvent {
    /// A master playlist was requested — module should assign a session UID.
    MasterPlaylistRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        headers: HlsRequestHeaders,
    },
    /// A media playlist was requested (non-blocking).
    MediaPlaylistRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        session_id: Option<u64>,
        legacy: bool,
        rewind: bool,
        headers: HlsRequestHeaders,
    },
    /// A blocking media playlist was requested (LL-HLS _HLS_msn/_HLS_part).
    BlockingPlaylistRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        session_id: Option<u64>,
        blocking: BlockingParams,
        skip: Option<SkipMode>,
        legacy: bool,
        rewind: bool,
        headers: HlsRequestHeaders,
    },
    /// A TS/fMP4 segment was requested.
    SegmentRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        segment_name: String,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// An fMP4 init segment was requested.
    InitSegmentRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// An LL-HLS part was requested.
    PartRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        part_name: String,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// Per-track media playlist requested (demuxed LLHLS).
    TrackMediaPlaylistRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        lane: TrackLane,
        session_id: Option<u64>,
        blocking: Option<BlockingParams>,
        skip: Option<SkipMode>,
        key_token: Option<String>,
        headers: HlsRequestHeaders,
    },
    /// Per-track init segment requested (demuxed LLHLS).
    TrackInitSegmentRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        lane: TrackLane,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// Per-track part requested (demuxed LLHLS).
    TrackPartRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        lane: TrackLane,
        part_name: String,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// Per-track segment requested (demuxed LLHLS).
    TrackSegmentRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        lane: TrackLane,
        segment_name: String,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
}

/// The HLS core state machine.
pub struct HlsCore {
    /// `_private` field.
    /// `_private` 字段.
    _private: (),
}

impl HlsCore {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Process an input and produce outputs.
    pub fn handle_input(&mut self, input: HlsCoreInput) -> Vec<HlsCoreOutput> {
        match input {
            HlsCoreInput::HttpRequest {
                method,
                target,
                connection_id,
                headers,
            } => self.handle_http_request(method, &target, connection_id, headers),
            HlsCoreInput::Command(cmd) => self.handle_command(cmd),
        }
    }

    fn handle_http_request(
        &mut self,
        method: HttpMethod,
        target: &str,
        connection_id: u64,
        headers: HlsRequestHeaders,
    ) -> Vec<HlsCoreOutput> {
        // OPTIONS → CORS preflight
        if method == HttpMethod::Options {
            return vec![HlsCoreOutput::SendResponse {
                connection_id,
                status: 204,
                content_type: "",
                body: Bytes::new(),
                headers: cors_headers(),
            }];
        }

        if method != HttpMethod::Get && method != HttpMethod::Head {
            return vec![HlsCoreOutput::SendResponse {
                connection_id,
                status: 405,
                content_type: "text/plain",
                body: Bytes::from_static(b"Method Not Allowed"),
                headers: cors_headers(),
            }];
        }

        match parse_hls_request(target) {
            Ok(HlsRequestKind::MasterPlaylist { stream_key }) => {
                vec![HlsCoreOutput::Event(
                    HlsCoreEvent::MasterPlaylistRequested {
                        connection_id,
                        stream_key,
                        headers,
                    },
                )]
            }
            Ok(HlsRequestKind::MediaPlaylist {
                stream_key,
                session_id,
                blocking,
                skip,
                legacy,
                rewind,
            }) => {
                if let Some(blocking) = blocking {
                    vec![HlsCoreOutput::Event(
                        HlsCoreEvent::BlockingPlaylistRequested {
                            connection_id,
                            stream_key,
                            session_id,
                            blocking,
                            skip,
                            legacy,
                            rewind,
                            headers,
                        },
                    )]
                } else {
                    vec![HlsCoreOutput::Event(HlsCoreEvent::MediaPlaylistRequested {
                        connection_id,
                        stream_key,
                        session_id,
                        legacy,
                        rewind,
                        headers,
                    })]
                }
            }
            Ok(HlsRequestKind::Segment {
                stream_key,
                segment_name,
                session_id,
                key_token,
                ..
            }) => {
                vec![HlsCoreOutput::Event(HlsCoreEvent::SegmentRequested {
                    connection_id,
                    stream_key,
                    segment_name,
                    session_id,
                    key_token,
                })]
            }
            Ok(HlsRequestKind::InitSegment {
                stream_key,
                session_id,
                key_token,
            }) => {
                vec![HlsCoreOutput::Event(HlsCoreEvent::InitSegmentRequested {
                    connection_id,
                    stream_key,
                    session_id,
                    key_token,
                })]
            }
            Ok(HlsRequestKind::Part {
                stream_key,
                part_name,
                session_id,
                key_token,
                ..
            }) => {
                vec![HlsCoreOutput::Event(HlsCoreEvent::PartRequested {
                    connection_id,
                    stream_key,
                    part_name,
                    session_id,
                    key_token,
                })]
            }
            Ok(HlsRequestKind::PlayerPage { stream_key }) => {
                // Serve embedded hls.js player page directly
                // URL is relative from /{ns}/{stream}/ → need ../{stream}.m3u8
                let playlist_url = format!("../{}.m3u8", stream_key.stream_path);
                let html = generate_player_html(&stream_key.stream_path, &playlist_url);
                vec![HlsCoreOutput::SendResponse {
                    connection_id,
                    status: 200,
                    content_type: "text/html; charset=utf-8",
                    body: Bytes::from(html),
                    headers: cors_headers(),
                }]
            }
            Ok(HlsRequestKind::TrackMediaPlaylist {
                stream_key,
                lane,
                session_id,
                blocking,
                skip,
                key_token,
            }) => {
                vec![HlsCoreOutput::Event(
                    HlsCoreEvent::TrackMediaPlaylistRequested {
                        connection_id,
                        stream_key,
                        lane,
                        session_id,
                        blocking,
                        skip,
                        key_token,
                        headers,
                    },
                )]
            }
            Ok(HlsRequestKind::TrackInitSegment {
                stream_key,
                lane,
                session_id,
                key_token,
            }) => {
                vec![HlsCoreOutput::Event(
                    HlsCoreEvent::TrackInitSegmentRequested {
                        connection_id,
                        stream_key,
                        lane,
                        session_id,
                        key_token,
                    },
                )]
            }
            Ok(HlsRequestKind::TrackPart {
                stream_key,
                lane,
                part_name,
                session_id,
                key_token,
            }) => {
                vec![HlsCoreOutput::Event(HlsCoreEvent::TrackPartRequested {
                    connection_id,
                    stream_key,
                    lane,
                    part_name,
                    session_id,
                    key_token,
                })]
            }
            Ok(HlsRequestKind::TrackSegment {
                stream_key,
                lane,
                segment_name,
                session_id,
                key_token,
            }) => {
                vec![HlsCoreOutput::Event(HlsCoreEvent::TrackSegmentRequested {
                    connection_id,
                    stream_key,
                    lane,
                    segment_name,
                    session_id,
                    key_token,
                })]
            }
            Err(_) => {
                vec![HlsCoreOutput::SendResponse {
                    connection_id,
                    status: 404,
                    content_type: "text/plain",
                    body: Bytes::from_static(b"Not Found"),
                    headers: cors_headers(),
                }]
            }
        }
    }

    fn handle_command(&mut self, cmd: HlsCoreCommand) -> Vec<HlsCoreOutput> {
        match cmd {
            HlsCoreCommand::SendPlaylist {
                connection_id,
                content,
            } => {
                vec![HlsCoreOutput::SendResponse {
                    connection_id,
                    status: 200,
                    content_type: "application/vnd.apple.mpegurl",
                    body: Bytes::from(content),
                    headers: cors_headers_with_cache_control("no-cache"),
                }]
            }
            HlsCoreCommand::SendSegment {
                connection_id,
                data,
            } => {
                vec![HlsCoreOutput::SendResponse {
                    connection_id,
                    status: 200,
                    content_type: "video/mp2t",
                    body: data,
                    headers: cors_headers(),
                }]
            }
            HlsCoreCommand::SendError {
                connection_id,
                error,
            } => {
                let (status, msg) = match &error {
                    HlsCoreError::StreamNotFound { .. } => (404, "Stream Not Found"),
                    HlsCoreError::SegmentNotFound { .. } => (404, "Segment Not Found"),
                    HlsCoreError::NotReady => (503, "Not Ready"),
                    HlsCoreError::InvalidPath { .. } => (400, "Bad Request"),
                };
                vec![HlsCoreOutput::SendResponse {
                    connection_id,
                    status,
                    content_type: "text/plain",
                    body: Bytes::from(msg),
                    headers: cors_headers(),
                }]
            }
        }
    }
}

impl Default for HlsCore {
    fn default() -> Self {
        Self::new()
    }
}

/// Standard CORS headers for cross-origin HLS playback.
fn cors_headers() -> Vec<(&'static str, String)> {
    vec![
        ("Access-Control-Allow-Origin", "*".to_string()),
        (
            "Access-Control-Allow-Methods",
            "GET, HEAD, OPTIONS".to_string(),
        ),
        (
            "Access-Control-Allow-Headers",
            "Origin, Range, Accept-Encoding, Referer, If-None-Match, Cookie".to_string(),
        ),
        (
            "Access-Control-Expose-Headers",
            "Content-Length, Content-Range, ETag, Set-Cookie".to_string(),
        ),
        ("Access-Control-Max-Age", "86400".to_string()),
    ]
}

fn cors_headers_with_cache_control(directive: &str) -> Vec<(&'static str, String)> {
    let mut h = cors_headers();
    h.push(("Cache-Control", directive.to_string()));
    h
}

/// Generate an embedded hls.js player HTML page.
fn generate_player_html(stream_name: &str, playlist_url: &str) -> String {
    // Minimal HTML escape for stream_name (defense in depth; URL validator already rejects special chars)
    let safe_name = stream_name
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    let safe_url = playlist_url
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "\\x27");
    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>HLS Player - {safe_name}</title>
<style>body{{margin:0;background:#000;display:flex;align-items:center;justify-content:center;height:100vh}}
video{{max-width:100%;max-height:100%}}#info{{position:fixed;top:10px;left:10px;color:#0f0;font:12px monospace;white-space:pre}}</style>
</head><body><video id="v" controls muted playsinline></video><div id="info"></div>
<script src="https://cdn.jsdelivr.net/npm/hls.js@latest/dist/hls.min.js"></script>
<script>
var v=document.getElementById('v'),info=document.getElementById('info'),audioTracks=0,sbTypes=[];
if(Hls.isSupported()){{
var h=new Hls({{lowLatencyMode:true,liveSyncDurationCount:3,liveMaxLatencyDurationCount:6,backBufferLength:30,debug:false}});
h.loadSource('{safe_url}');h.attachMedia(v);
h.on(Hls.Events.MANIFEST_PARSED,function(e,d){{audioTracks=d.audioTracks?d.audioTracks.length:0;v.play().catch(function(e){{console.warn('play rejected:',e.message)}});}});
h.on(Hls.Events.BUFFER_CREATED,function(e,d){{if(d.tracks)sbTypes=Object.keys(d.tracks);}});
var fatalCount=0;
h.on(Hls.Events.ERROR,function(e,d){{
  console.error('hls.js error:',d.type,d.details,d.fatal,d);
  if(d.fatal){{fatalCount++;if(fatalCount<2&&d.type===Hls.ErrorTypes.MEDIA_ERROR)h.recoverMediaError();else{{info.textContent='FATAL: '+d.details;h.destroy();}}}}
}});
setInterval(function(){{
  var buf='none';
  if(v.buffered.length>0)buf=v.buffered.start(0).toFixed(1)+'-'+v.buffered.end(0).toFixed(1);
  var lat=h.latency!==undefined?h.latency.toFixed(2)+'s':'?';
  info.textContent='state:'+v.readyState+' buf:'+buf+' lat:'+lat+' t:'+v.currentTime.toFixed(1)+'\naudio:'+audioTracks+' sb:['+sbTypes.join(',')+']';
}},300);
}}else if(v.canPlayType('application/vnd.apple.mpegurl')){{v.src='{safe_url}';v.play().catch(function(){{}});}}
</script></body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_returns_cors_204() {
        let mut core = HlsCore::new();
        let outputs = core.handle_input(HlsCoreInput::HttpRequest {
            method: HttpMethod::Options,
            target: "/live/stream.m3u8".to_string(),
            connection_id: 1,
            headers: HlsRequestHeaders::default(),
        });
        assert_eq!(outputs.len(), 1);
        match &outputs[0] {
            HlsCoreOutput::SendResponse { status, .. } => assert_eq!(*status, 204),
            _ => panic!("expected SendResponse"),
        }
    }

    #[test]
    fn get_master_playlist_emits_event() {
        let mut core = HlsCore::new();
        let outputs = core.handle_input(HlsCoreInput::HttpRequest {
            method: HttpMethod::Get,
            target: "/live/stream.m3u8".to_string(),
            connection_id: 1,
            headers: HlsRequestHeaders::default(),
        });
        assert_eq!(outputs.len(), 1);
        match &outputs[0] {
            HlsCoreOutput::Event(HlsCoreEvent::MasterPlaylistRequested { stream_key, .. }) => {
                assert_eq!(stream_key.namespace, "live");
                assert_eq!(stream_key.stream_path, "stream");
            }
            _ => panic!("expected MasterPlaylistRequested event"),
        }
    }

    #[test]
    fn invalid_method_returns_405() {
        let mut core = HlsCore::new();
        let outputs = core.handle_input(HlsCoreInput::HttpRequest {
            method: HttpMethod::Other,
            target: "/live/stream.m3u8".to_string(),
            connection_id: 1,
            headers: HlsRequestHeaders::default(),
        });
        match &outputs[0] {
            HlsCoreOutput::SendResponse { status, .. } => assert_eq!(*status, 405),
            _ => panic!("expected 405"),
        }
    }
}
