//! ZLMediaKit-compatible response DTOs.
//!
//! Each ZLM endpoint returns a typed payload with fields located at the
//! positions expected by the target API. The adapter no longer uses a
//! generic `{code,msg,data}` envelope for every response.
//!
//! ZLMediaKit 兼容响应 DTO。每个端点返回与目标 API 形状一致的
//! 强类型载荷，不再无条件使用统一的 `{code,msg,data}` 信封。

use bytes::Bytes;
use cheetah_media_api::ids::MediaKey;
use cheetah_media_api::model::{
    CodecKind, MediaType, OnlineState, ProxyInfo, ProxyState, RtpSession, SessionInfo, StreamInfo,
    TrackSummary,
};
use cheetah_sdk::{HttpHeader, HttpResponse};
use serde::Serialize;

/// Generic ZLM envelope. The `msg` field is omitted for successful
/// responses; it is only present when explicitly supplied (e.g.
/// `close_stream` keeps the original `msg` field).
#[derive(Serialize)]
pub(crate) struct ZlmResponse<T> {
    pub code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,
    #[serde(flatten)]
    pub body: T,
}

impl<T: serde::Serialize> ZlmResponse<T> {
    pub(crate) fn ok(body: T) -> Self {
        Self {
            code: 0,
            msg: None,
            body,
        }
    }

    pub(crate) fn with_msg(code: i32, msg: &str, body: T) -> Self {
        Self {
            code,
            msg: Some(msg.to_string()),
            body,
        }
    }
}

pub(crate) fn zlm_response<T: serde::Serialize>(response: ZlmResponse<T>) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![HttpHeader {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        }],
        body: Bytes::from(serde_json::to_vec(&response).unwrap_or_default()),
    }
}

/// Empty body used for endpoints that return only `code`.
#[derive(Serialize)]
pub(crate) struct Empty;

/// Top-level `result` field used by action endpoints.
#[derive(Serialize)]
pub(crate) struct ZlmResult {
    pub result: bool,
}

/// `{result, taskId}` returned by `startRecord`.
#[derive(Serialize)]
pub(crate) struct StartRecordResult {
    pub result: bool,
    #[serde(rename = "taskId")]
    pub task_id: String,
}

/// Top-level `result` as an integer (0 success, -1 fail, -2 not found).
#[derive(Serialize)]
pub(crate) struct ZlmCloseStreamResult {
    pub result: i32,
}

/// `{count_hit, count_closed}` returned by `close_streams`.
#[derive(Serialize)]
pub(crate) struct CloseStreamsResult {
    pub count_hit: u64,
    pub count_closed: u64,
}

/// `{count_hit, msg}` returned by `kick_sessions`.
#[derive(Serialize)]
pub(crate) struct KickSessionsResult {
    pub count_hit: u64,
}

/// `{status: bool}` returned by `isRecording`.
#[derive(Serialize)]
pub(crate) struct StatusResult {
    pub status: bool,
}

/// `{online: bool}` returned by `isMediaOnline`.
#[derive(Serialize)]
pub(crate) struct OnlineResult {
    pub online: bool,
}

/// `{port, ssrc, session_id}` returned by `openRtpServer`.
#[derive(Serialize)]
pub(crate) struct OpenRtpServerResult {
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssrc: Option<u32>,
    pub session_id: String,
}

/// `{local_port, ssrc, session_id}` returned by `startSendRtp`.
#[derive(Serialize)]
pub(crate) struct StartSendRtpResult {
    pub local_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssrc: Option<u32>,
    pub session_id: String,
}

/// `{hit: u32}` returned by `closeRtpServer`.
#[derive(Serialize)]
pub(crate) struct HitResult {
    pub hit: u32,
}

/// `{key: String}` wrapped in `data`.
#[derive(Serialize)]
pub(crate) struct KeyData {
    pub key: String,
}

/// `{paths, rootPath}` wrapped in `data` for `getMP4RecordFile`.
#[derive(Serialize)]
pub(crate) struct Mp4FilesData {
    pub paths: Vec<String>,
    #[serde(rename = "rootPath")]
    pub root_path: String,
}

/// `{result, deleted, failed}` returned by `deleteRecordDirectory`.
#[derive(Serialize)]
pub(crate) struct DeleteRecordDirectoryResult {
    pub result: bool,
    pub deleted: usize,
    pub failed: usize,
}

/// `{session_id, ssrc}` returned by `updateRtpServerSSRC`.
#[derive(Serialize)]
pub(crate) struct RtpUpdateResult {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssrc: Option<u32>,
}

/// `{session_id, check_paused}` returned by pause/resume RTP check.
#[derive(Serialize)]
pub(crate) struct RtpPauseResult {
    pub session_id: String,
    pub check_paused: bool,
}

/// `{apis, capabilities}` returned by `getApiList`.
#[derive(Serialize)]
pub(crate) struct ApiListData {
    pub apis: Vec<String>,
    pub capabilities: cheetah_media_api::capability::MediaCapabilitySet,
}

/// `data` wrapper.
#[derive(Serialize)]
pub(crate) struct Data<T> {
    pub data: T,
}

impl<T> Data<T> {
    pub(crate) fn new(data: T) -> Self {
        Self { data }
    }
}

/// Version information returned by `version`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VersionInfo {
    pub branch_name: &'static str,
    pub build_time: &'static str,
    pub commit_hash: &'static str,
}

impl Default for VersionInfo {
    fn default() -> Self {
        Self {
            branch_name: env!("ZLM_BRANCH"),
            build_time: env!("ZLM_BUILD_TIME"),
            commit_hash: env!("ZLM_COMMIT"),
        }
    }
}

/// Media list item matching ZLM `getMediaList`/`getMediaInfo` fields.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaItem {
    pub app: String,
    pub stream: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub reader_count: u64,
    pub total_reader_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_sock: Option<OriginSock>,
    pub origin_type: i32,
    pub origin_type_str: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_url: Option<String>,
    pub create_stamp: i64,
    pub alive_second: u64,
    pub bytes_speed: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<MediaTrackItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OriginSock {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    pub local_ip: String,
    pub local_port: u16,
    pub peer_ip: String,
    pub peer_port: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaTrackItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u8>,
    pub codec_id: i32,
    pub codec_id_name: String,
    pub codec_type: i32,
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_bit: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames: Option<u64>,
}

impl From<StreamInfo> for MediaItem {
    fn from(info: StreamInfo) -> Self {
        let origin_url = info.origin.clone();
        let _online = info.online == OnlineState::Online;
        Self {
            app: info.key.app.0.clone(),
            stream: info.key.stream.0.clone(),
            schema: info.key.schema.map(|s| zlm_schema_name(&s)),
            reader_count: info.readers,
            total_reader_count: info.readers,
            origin_sock: None,
            origin_type: 0,
            origin_type_str: "MediaOriginType::unknown".to_string(),
            origin_url,
            create_stamp: info.created_at / 1000,
            alive_second: info.duration_ms / 1000,
            bytes_speed: 0,
            tracks: info.tracks.into_iter().map(MediaTrackItem::from).collect(),
        }
    }
}

impl From<TrackSummary> for MediaTrackItem {
    fn from(track: TrackSummary) -> Self {
        let base = Self {
            channels: None,
            codec_id: zlm_codec_id(&track.codec),
            codec_id_name: zlm_codec_id_name(&track.codec),
            codec_type: zlm_codec_type(&track.media_type),
            ready: track.readiness == cheetah_media_api::model::TrackReadiness::Ready,
            sample_bit: None,
            sample_rate: track.sample_rate,
            width: None,
            height: None,
            fps: None,
            frames: None,
        };
        match track.media_type {
            MediaType::Audio => Self {
                channels: track.channels,
                sample_bit: track.channels.map(|_| 16), // preserve old default
                sample_rate: track.sample_rate,
                ..base
            },
            MediaType::Video => Self {
                width: track.width,
                height: track.height,
                fps: None,
                ..base
            },
            _ => base,
        }
    }
}

fn zlm_codec_id(codec: &CodecKind) -> i32 {
    match codec {
        CodecKind::H264 => 0,
        CodecKind::H265 => 1,
        CodecKind::Aac => 2,
        CodecKind::G711A => 3,
        CodecKind::G711U => 4,
        CodecKind::Opus => 5,
        CodecKind::Vp8 => 6,
        CodecKind::Vp9 => 7,
        CodecKind::Av1 => 8,
        CodecKind::H266 => 9,
        _ => -1,
    }
}

fn zlm_codec_id_name(codec: &CodecKind) -> String {
    match codec {
        CodecKind::H264 => "CodecH264".to_string(),
        CodecKind::H265 => "CodecH265".to_string(),
        CodecKind::Aac => "CodecAAC".to_string(),
        CodecKind::G711A => "CodecG711A".to_string(),
        CodecKind::G711U => "CodecG711U".to_string(),
        CodecKind::Opus => "CodecOpus".to_string(),
        CodecKind::Vp8 => "CodecVP8".to_string(),
        CodecKind::Vp9 => "CodecVP9".to_string(),
        CodecKind::Av1 => "CodecAV1".to_string(),
        CodecKind::H266 => "CodecH266".to_string(),
        _ => format!("Codec{codec:?}"),
    }
}

fn zlm_codec_type(media_type: &MediaType) -> i32 {
    match media_type {
        MediaType::Video => 0,
        MediaType::Audio => 1,
        MediaType::Data => 2,
    }
}

fn zlm_schema_name(schema: &cheetah_media_api::ids::MediaSchema) -> String {
    use cheetah_media_api::ids::MediaSchema;
    match schema {
        MediaSchema::Rtsp => "rtsp".to_string(),
        MediaSchema::Rtmp => "rtmp".to_string(),
        MediaSchema::HttpFlv => "http-flv".to_string(),
        MediaSchema::Hls => "hls".to_string(),
        MediaSchema::Webrtc => "webrtc".to_string(),
        MediaSchema::Ts => "ts".to_string(),
        MediaSchema::Fmp4 => "fmp4".to_string(),
        MediaSchema::Srt => "srt".to_string(),
        MediaSchema::Rtp => "rtp".to_string(),
        _ => format!("{schema:?}").to_lowercase(),
    }
}

/// Session list item matching ZLM `getAllSession`/`getMediaPlayerList`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionItem {
    pub id: String,
    pub local_ip: String,
    pub local_port: u16,
    pub peer_ip: String,
    pub peer_port: u16,
    pub typeid: String,
}

impl From<SessionInfo> for SessionItem {
    fn from(info: SessionInfo) -> Self {
        let (local_ip, local_port) = parse_endpoint(&info.local_endpoint);
        let (peer_ip, peer_port) = parse_endpoint(&info.remote_endpoint);
        Self {
            id: info.session_id.0,
            local_ip,
            local_port,
            peer_ip,
            peer_port,
            typeid: session_typeid(&info.kind, &info.protocol),
        }
    }
}

fn parse_endpoint(endpoint: &Option<String>) -> (String, u16) {
    let default = ("127.0.0.1".to_string(), 0);
    let Some(ep) = endpoint else {
        return default;
    };
    if let Some((host, port)) = ep.rsplit_once(':') {
        if let Ok(p) = port.parse::<u16>() {
            return (host.to_string(), p);
        }
    }
    (ep.clone(), 0)
}

fn session_typeid(kind: &cheetah_media_api::model::SessionKind, protocol: &str) -> String {
    use cheetah_media_api::model::SessionKind;
    let kind_str = match kind {
        SessionKind::Publisher => "Publisher",
        SessionKind::Player => "Player",
        SessionKind::Proxy => "Proxy",
        SessionKind::RtpSender => "RtpSender",
        SessionKind::RtpReceiver => "RtpReceiver",
    };
    format!("N8mediakit{}{kind_str}SessionE", protocol.to_uppercase())
}

/// RTP server list item.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RtpServerItem {
    pub port: u16,
    pub stream_id: String,
}

impl From<RtpSession> for RtpServerItem {
    fn from(session: RtpSession) -> Self {
        Self {
            port: session.local_port.unwrap_or(0),
            stream_id: session_key(&session.media_key),
        }
    }
}

/// `getRtpInfo` top-level payload.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RtpInfo {
    pub exist: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_port: Option<u16>,
}

impl From<RtpSession> for RtpInfo {
    fn from(session: RtpSession) -> Self {
        let (peer_ip, peer_port) = parse_endpoint(&session.remote_endpoint);
        let local_port = session.local_port.unwrap_or(0);
        Self {
            exist: true,
            peer_ip: Some(peer_ip),
            peer_port: Some(peer_port),
            local_ip: Some("0.0.0.0".to_string()),
            local_port: Some(local_port),
        }
    }
}

/// Proxy list/item DTO.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProxyItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub url: String,
    pub dst: ProxyDst,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vhost: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    pub status: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_str: Option<String>,
    pub online: bool,
    pub retry_count: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<MediaTrackItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProxyDst {
    pub vhost: String,
    pub app: String,
    pub stream: String,
}

impl ProxyItem {
    pub(crate) fn from_info(info: &ProxyInfo, key: Option<String>) -> Self {
        let online = info.state == ProxyState::Connected;
        let status = if online { 1 } else { 0 };
        Self {
            key,
            url: info.source.clone(),
            dst: ProxyDst {
                vhost: info.destination.vhost.0.clone(),
                app: info.destination.app.0.clone(),
                stream: info.destination.stream.0.clone(),
            },
            vhost: Some(info.destination.vhost.0.clone()),
            app: Some(info.destination.app.0.clone()),
            stream: Some(info.destination.stream.0.clone()),
            status,
            status_str: Some(format!("{:?}", info.state)),
            online,
            retry_count: info.retry_count,
            tracks: Vec::new(),
        }
    }
}

fn session_key(key: &MediaKey) -> String {
    format!("{}/{}/{}", key.vhost.0, key.app.0, key.stream.0)
}

/// Login success payload.
#[derive(Serialize)]
pub(crate) struct LoginSuccess<'a> {
    pub cookie_name: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_dto_serializes_with_camel_case() {
        let v = VersionInfo::default();
        let json = serde_json::to_value(&v).unwrap();
        assert!(json.get("branchName").is_some());
        assert!(json.get("buildTime").is_some());
        assert!(json.get("commitHash").is_some());
    }

    #[test]
    fn media_item_codec_mapping() {
        let track = TrackSummary {
            track_id: "0".to_string(),
            media_type: MediaType::Video,
            codec: CodecKind::H264,
            clock_rate: 90000,
            sample_rate: None,
            channels: None,
            width: Some(1920),
            height: Some(1080),
            bitrate: None,
            parameter_set_available: true,
            readiness: cheetah_media_api::model::TrackReadiness::Ready,
        };
        let dto = MediaTrackItem::from(track);
        assert_eq!(dto.codec_id, 0);
        assert_eq!(dto.codec_id_name, "CodecH264");
        assert_eq!(dto.codec_type, 0);
        assert_eq!(dto.width, Some(1920));
    }

    #[test]
    fn empty_response_serializes_to_code_only() {
        let resp = ZlmResponse::ok(Empty);
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["code"], 0);
        assert!(json.get("msg").is_none());
    }
}
