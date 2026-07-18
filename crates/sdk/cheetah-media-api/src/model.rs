use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::MediaErrorCode;
use crate::ids::*;

/// Codec kind for a track.
///
/// 轨道编解码类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CodecKind {
    H264,
    H265,
    H266,
    Av1,
    Vp8,
    Vp9,
    Aac,
    Opus,
    G711A,
    G711U,
    Mp3,
    Pcm,
    WebVtt,
    Unknown,
}

/// Media type (audio or video).
///
/// 媒体类型（音频或视频）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Audio,
    Video,
    Data,
}

/// Readiness state of a track.
///
/// 轨道就绪状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackReadiness {
    Pending,
    ParameterSetAvailable,
    Ready,
    Failed,
}

/// Summary of a single media track.
///
/// 单个媒体轨道的摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackSummary {
    pub track_id: String,
    pub media_type: MediaType,
    pub codec: CodecKind,
    pub clock_rate: u32,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bitrate: Option<u64>,
    pub parameter_set_available: bool,
    pub readiness: TrackReadiness,
}

/// A playable URL for a media resource.
///
/// 媒体资源的可播放 URL。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaUrl {
    pub schema: MediaSchema,
    pub url: String,
    pub available: bool,
    pub expires_at: Option<i64>,
}

/// Online state of a media resource.
///
/// 媒体资源的在线状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnlineState {
    Online,
    Offline,
    Unknown,
}

/// Stream information returned by media queries.
///
/// 媒体查询返回的流信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamInfo {
    pub key: MediaKey,
    pub origin: Option<String>,
    pub online: OnlineState,
    pub regist: bool,
    pub created_at: i64,
    pub last_activity_at: i64,
    pub readers: u64,
    pub publishers: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub duration_ms: u64,
    pub tracks: Vec<TrackSummary>,
    pub urls: Vec<MediaUrl>,
    pub metadata: HashMap<String, String>,
}

/// Session kind.
///
/// 会话类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Publisher,
    Player,
    Proxy,
    RtpSender,
    RtpReceiver,
}

/// Session state.
///
/// 会话状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Created,
    Connecting,
    Connected,
    Paused,
    Closing,
    Closed,
    Failed,
}

/// Close reason for a session or stream.
///
/// 会话或流的关闭原因。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    Normal,
    Timeout,
    Kicked,
    Idle,
    Error,
    Unsupported,
    Other(String),
}

/// Session information returned by session queries.
///
/// 会话查询返回的信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub kind: SessionKind,
    pub media_key: MediaKey,
    pub remote_endpoint: Option<String>,
    pub local_endpoint: Option<String>,
    pub protocol: String,
    pub started_at: i64,
    pub last_seen_at: i64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub state: SessionState,
    pub close_reason: Option<CloseReason>,
    pub owner_module: String,
}

/// Record task state.
///
/// 录制任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordTaskState {
    Pending,
    Running,
    Stopping,
    Completed,
    Failed,
    Cancelled,
}

/// Record task information.
///
/// 录制任务信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordTask {
    pub task_id: RecordTaskId,
    pub media_key: MediaKey,
    pub format: String,
    pub state: RecordTaskState,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub duration_ms: u64,
    pub file_count: u64,
    pub error: Option<String>,
}

/// Record file information.
///
/// 录制文件信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordFile {
    pub file_id: RecordFileId,
    pub task_id: RecordTaskId,
    pub media_key: MediaKey,
    pub format: String,
    pub path_handle: FileHandle,
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub download_url: Option<String>,
}

/// Snapshot handle.
///
/// 快照句柄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotHandle {
    pub snapshot_id: SnapshotId,
    pub media_key: MediaKey,
    pub state: SnapshotState,
    pub path_handle: FileHandle,
    pub download_url: Option<String>,
    pub created_at: i64,
}

/// Snapshot state.
///
/// 快照状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotState {
    Pending,
    Capturing,
    Completed,
    Failed,
    Timeout,
}

/// Snapshot information.
///
/// 快照信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub snapshot_id: SnapshotId,
    pub media_key: MediaKey,
    pub state: SnapshotState,
    pub path_handle: FileHandle,
    pub created_at: i64,
    pub size_bytes: Option<u64>,
    pub format: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

/// Proxy state.
///
/// 代理状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyState {
    Created,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
    Stopped,
}

/// Proxy information.
///
/// 代理信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxyInfo {
    pub proxy_id: ProxyId,
    pub kind: ProxyKind,
    pub source: String,
    pub destination: MediaKey,
    pub state: ProxyState,
    pub retry_count: u32,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub output_urls: Vec<MediaUrl>,
}

/// Proxy kind.
///
/// 代理类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyKind {
    Pull,
    Push,
}

/// RTP session information.
///
/// RTP 会话信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RtpSession {
    pub session_id: RtpSessionId,
    pub kind: RtpSessionKind,
    pub media_key: MediaKey,
    pub local_port: Option<u16>,
    pub remote_endpoint: Option<String>,
    pub ssrc: Option<u32>,
    pub payload_type: Option<u8>,
    pub tcp_mode: Option<RtpTcpMode>,
    pub reuse_port: bool,
    pub state: RtpSessionState,
    pub check_paused: bool,
    pub generation: u64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<crate::error::MediaOperationError>,
}

/// RTP session kind.
///
/// RTP 会话类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RtpSessionKind {
    Receiver,
    Sender,
    Talk,
}

/// RTP session state.
///
/// RTP 会话状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RtpSessionState {
    Created,
    Listening,
    Connected,
    Bound,
    Paused,
    TimedOut,
    Stopping,
    Stopped,
    Failed,
}

/// RTP TCP mode.
///
/// RTP TCP 模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RtpTcpMode {
    Passive,
    Active,
}

/// Playback session state.
///
/// 回放会话状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackSessionState {
    Pending,
    Playing,
    Paused,
    Seeking,
    Completed,
    Failed,
}

/// Playback session information.
///
/// 回放会话信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybackSession {
    pub session_id: PlaybackSessionId,
    pub media_key: MediaKey,
    pub file_handle: FileHandle,
    pub state: PlaybackSessionState,
    pub duration_ms: u64,
    pub position_ms: i64,
    pub scale: f64,
    pub generation: u64,
    pub output_key: Option<MediaKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Page of results.
///
/// 分页结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
    pub next_cursor: Option<String>,
}

/// Close report for a kicked stream.
///
/// 被踢流返回的关闭报告。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloseReport {
    pub media_key: MediaKey,
    pub closed_sessions: Vec<SessionId>,
    pub reason: CloseReason,
}

/// Publisher handle.
///
/// 发布者句柄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublisherHandle {
    pub session_id: SessionId,
    pub media_key: MediaKey,
    pub lease_token: String,
}

/// Subscriber handle.
///
/// 订阅者句柄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriberHandle {
    pub session_id: SessionId,
    pub media_key: MediaKey,
    pub output_schema: MediaSchema,
    pub url: Option<String>,
}

/// Output policy for a proxy or record task.
///
/// 代理或录制任务的输出策略。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputPolicy {
    #[default]
    None,
    Hls,
    Mp4,
    Flv,
    Fmp4,
    Rtmp,
    Rtsp,
}

/// Storage policy.
///
/// 存储策略。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StoragePolicy {
    pub max_segments: Option<u32>,
    pub max_files: Option<u32>,
    pub max_size_bytes: Option<u64>,
    pub max_age_secs: Option<u64>,
}

/// Record template.
///
/// 录制模板。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordTemplate {
    #[default]
    Continuous,
    Segment,
    Event,
}

/// Decision returned by a synchronous admission hook.
///
/// 同步准入钩子返回的决策。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Allow,
    Deny {
        /// Stable machine-readable code describing the denial.
        code: MediaErrorCode,
        /// Human-readable reason for the denial.
        reason: String,
    },
}

/// Action that an admission request asks to authorize.
///
/// 准入请求希望授权执行的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionAction {
    Publish,
    Play,
    CreatePullProxy,
    CreatePushProxy,
    OpenRtpReceiver,
    OpenRtpSender,
}

/// Request for a synchronous admission decision before a side-effecting media
/// operation is allowed to proceed.
///
/// 副作用媒体操作执行前请求同步准入决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionRequest {
    pub action: AdmissionAction,
    pub principal: Option<crate::auth::Principal>,
    pub resource: MediaKey,
    pub protocol: String,
    pub source_address: Option<String>,
    pub params: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_serializes_items() {
        let page = Page {
            items: vec!["a", "b"],
            page: 1,
            page_size: 10,
            total: 2,
            next_cursor: None,
        };
        let json = serde_json::to_string(&page).unwrap();
        assert!(json.contains("\"items\":"));
    }

    #[test]
    fn decision_round_trips() {
        let d = Decision::Deny {
            code: MediaErrorCode::PermissionDenied,
            reason: "forbidden".to_string(),
        };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("Deny"));
        let de: Decision = serde_json::from_str(&json).unwrap();
        assert_eq!(de, d);
    }

    #[test]
    fn close_reason_string_variant_round_trips() {
        let reason = CloseReason::Other("custom".to_string());
        let json = serde_json::to_string(&reason).unwrap();
        let de: CloseReason = serde_json::from_str(&json).unwrap();
        assert_eq!(de, reason);
    }

    #[test]
    fn admission_action_round_trips() {
        for action in [
            AdmissionAction::CreatePullProxy,
            AdmissionAction::CreatePushProxy,
            AdmissionAction::OpenRtpSender,
        ] {
            let json = serde_json::to_string(&action).unwrap();
            let de: AdmissionAction = serde_json::from_str(&json).unwrap();
            assert_eq!(de, action);
        }
    }
}
