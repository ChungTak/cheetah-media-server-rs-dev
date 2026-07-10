//! HLS core Sans-I/O state machine.
//!
//! HLS core Sans-I/O 状态机。
//! 处理 HTTP 请求路由与响应生成，不管理分片创建（分片由模块层喂帧驱动）。

use bytes::Bytes;

use crate::error::HlsCoreError;
use crate::ll_hls::TrackLane;
use crate::request::{parse_hls_request, BlockingParams, HlsRequestKind, SkipMode, StreamKeyParts};

/// Session identifier for an HLS playback session.
///
/// HLS 播放会话的标识符。
pub type HlsSessionId = u64;

/// Extracted HTTP request headers relevant to HLS.
///
/// 与 HLS 相关的 HTTP 请求头提取。
#[derive(Debug, Clone, Default)]
pub struct HlsRequestHeaders {
    /// Authorization header value (e.g., "Bearer <token>").
    ///
    /// Authorization 头值（例如 "Bearer <token>"）。
    pub authorization: Option<String>,
    /// User-Agent header value.
    ///
    /// User-Agent 头值。
    pub user_agent: Option<String>,
    /// If-None-Match header value (for conditional requests).
    ///
    /// If-None-Match 头值（用于条件请求）。
    pub if_none_match: Option<String>,
    /// Whether client accepts gzip encoding.
    ///
    /// 客户端是否接受 gzip 编码。
    pub accept_gzip: bool,
}

/// Input to the HLS core state machine.
///
/// HLS core 状态机的输入。
#[derive(Debug)]
pub enum HlsCoreInput {
    /// An HTTP request arrived.
    ///
    /// HTTP 请求到达。
    HttpRequest {
        method: HttpMethod,
        target: String,
        connection_id: u64,
        headers: HlsRequestHeaders,
    },
    /// Command from module layer.
    ///
    /// 来自模块层的命令。
    Command(HlsCoreCommand),
}

/// HTTP method (simplified for HLS — only GET and OPTIONS matter).
///
/// HTTP 方法（HLS 中仅 GET 与 OPTIONS 常用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Options,
    Head,
    Other,
}

/// Commands from module to core.
///
/// 模块发送给 core 的命令。
#[derive(Debug)]
pub enum HlsCoreCommand {
    /// Provide playlist content for a pending request.
    ///
    /// 为待处理请求提供播放列表内容。
    SendPlaylist { connection_id: u64, content: String },
    /// Provide segment data for a pending request.
    ///
    /// 为待处理请求提供分片数据。
    SendSegment { connection_id: u64, data: Bytes },
    /// Report an error for a pending request.
    ///
    /// 报告待处理请求的错误。
    SendError {
        connection_id: u64,
        error: HlsCoreError,
    },
}

/// Output from the HLS core state machine.
///
/// HLS core 状态机的输出。
#[derive(Debug)]
pub enum HlsCoreOutput {
    /// Send an HTTP response.
    ///
    /// 发送 HTTP 响应。
    SendResponse {
        connection_id: u64,
        status: u16,
        content_type: &'static str,
        body: Bytes,
        headers: Vec<(&'static str, String)>,
    },
    /// Event for the module layer.
    ///
    /// 发送给模块层的事件。
    Event(HlsCoreEvent),
}

/// Events bubbled up to the module layer.
///
/// 冒泡到模块层的事件。
#[derive(Debug, Clone)]
pub enum HlsCoreEvent {
    /// A master playlist was requested — module should assign a session UID.
    ///
    /// 主播放列表被请求 — 模块应分配会话 UID。
    MasterPlaylistRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        headers: HlsRequestHeaders,
    },
    /// A media playlist was requested (non-blocking).
    ///
    /// 媒体播放列表被请求（非阻塞）。
    MediaPlaylistRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        session_id: Option<u64>,
        legacy: bool,
        rewind: bool,
        headers: HlsRequestHeaders,
    },
    /// A blocking media playlist was requested (LL-HLS `_HLS_msn`/`_HLS_part`).
    ///
    /// 阻塞式媒体播放列表被请求（LL-HLS `_HLS_msn`/`_HLS_part`）。
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
    ///
    /// TS/fMP4 分片被请求。
    SegmentRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        segment_name: String,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// An fMP4 init segment was requested.
    ///
    /// fMP4 init 分段被请求。
    InitSegmentRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// An LL-HLS part was requested.
    ///
    /// LL-HLS part 被请求。
    PartRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        part_name: String,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// Per-track media playlist requested (demuxed LLHLS).
    ///
    /// 按轨道媒体播放列表被请求（解复用 LLHLS）。
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
    ///
    /// 按轨道 init 分段被请求（解复用 LLHLS）。
    TrackInitSegmentRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        lane: TrackLane,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// Per-track part requested (demuxed LLHLS).
    ///
    /// 按轨道 part 被请求（解复用 LLHLS）。
    TrackPartRequested {
        connection_id: u64,
        stream_key: StreamKeyParts,
        lane: TrackLane,
        part_name: String,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    /// Per-track segment requested (demuxed LLHLS).
    ///
    /// 按轨道分片被请求（解复用 LLHLS）。
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
///
/// HLS core 状态机。
///
/// This is a pure stateless dispatcher: each input is routed to the appropriate
/// HTTP method or command handler, producing `HlsCoreOutput` actions. It does not
/// store connection state or manage timers (those live in the driver/module layers).
///
/// 这是一个纯无状态分发器：每个输入被路由到对应的 HTTP 方法或命令处理器，
/// 产生 `HlsCoreOutput` 动作。它不保存连接状态或管理定时器（这些在驱动/模块层）。
pub struct HlsCore {
    _private: (),
}

impl HlsCore {
    /// Create a new HLS core state machine.
    ///
    /// 创建新的 HLS core 状态机。
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Process an input and produce outputs.
    ///
    /// 处理输入并产生输出。
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

    /// Handle an HTTP request: validate method, parse target, and dispatch.
    ///
    /// `OPTIONS` returns a CORS preflight response directly. Non-GET/HEAD methods return 405.
    /// Valid GET/HEAD targets are parsed by `parse_hls_request` and mapped to core events.
    /// The player page is the only path that is answered directly from the core.
    ///
    /// 处理 HTTP 请求：校验方法、解析目标并分派。
    /// `OPTIONS` 直接返回 CORS 预检响应；非 GET/HEAD 方法返回 405。
    /// 有效 GET/HEAD 目标由 `parse_hls_request` 解析并映射为 core 事件。
    /// 播放器页面是唯一由 core 直接响应的路径。
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

    /// Handle a module command and turn it into an HTTP response.
    ///
    /// `SendPlaylist` and `SendSegment` are wrapped with the appropriate MIME type and CORS
    /// headers. `SendError` is mapped to a status code based on the `HlsCoreError` variant.
    ///
    /// 处理模块命令并转换为 HTTP 响应。
    /// `SendPlaylist` 与 `SendSegment` 包装为正确的 MIME 类型与 CORS 头。
    /// `SendError` 根据 `HlsCoreError` 变体映射为状态码。
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
///
/// 跨域 HLS 播放的标准 CORS 头。
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

/// Standard CORS headers with an additional `Cache-Control` directive.
///
/// 带额外 `Cache-Control` 指令的标准 CORS 头。
fn cors_headers_with_cache_control(directive: &str) -> Vec<(&'static str, String)> {
    let mut h = cors_headers();
    h.push(("Cache-Control", directive.to_string()));
    h
}

/// Generate an embedded hls.js player HTML page.
///
/// 生成嵌入式 hls.js 播放器 HTML 页面。
///
/// The stream name and playlist URL are HTML-escaped to avoid injection. The page loads
/// hls.js from a CDN and configures low-latency playback with live sync/back-buffer settings.
///
/// 流名称与播放列表 URL 经过 HTML 转义以防止注入。
/// 页面从 CDN 加载 hls.js，并配置低延迟播放、live sync 与 back-buffer。
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
