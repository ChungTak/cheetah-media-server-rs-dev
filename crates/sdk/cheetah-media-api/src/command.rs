use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::MediaError;
use crate::ids::*;
use crate::model::*;
use crate::outbound_policy::OutboundUrlPolicy;

/// Query for media list.
///
/// 媒体列表查询。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaQuery {
    #[serde(default)]
    pub vhost: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub online: Option<bool>,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
    #[serde(default)]
    pub order: Option<String>,
}

fn default_page_size() -> u64 {
    20
}

impl MediaQuery {
    /// Maximum allowed page size.
    ///
    /// 允许的最大分页大小。
    pub const MAX_PAGE_SIZE: u64 = 1_000;

    /// Clamp the page size to the allowed maximum.
    ///
    /// 将 page size 限制到允许的最大值。
    pub fn clamp_page_size(&mut self) {
        if self.page_size == 0 {
            self.page_size = default_page_size();
        }
        self.page_size = self.page_size.min(Self::MAX_PAGE_SIZE);
    }
}

/// Query for sessions.
///
/// 会话查询。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionQuery {
    #[serde(default)]
    pub vhost: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub kind: Option<SessionKind>,
    #[serde(default)]
    pub state: Option<SessionState>,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
}

impl SessionQuery {
    /// Maximum allowed page size.
    pub const MAX_PAGE_SIZE: u64 = 1_000;

    pub fn clamp_page_size(&mut self) {
        if self.page_size == 0 {
            self.page_size = default_page_size();
        }
        self.page_size = self.page_size.min(Self::MAX_PAGE_SIZE);
    }
}

/// Publish request.
///
/// 发布请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishRequest {
    pub media_key: MediaKey,
    pub protocol: String,
    pub origin: Option<String>,
    #[serde(default)]
    pub remote_endpoint: Option<String>,
    #[serde(default)]
    pub lease_token: Option<String>,
    #[serde(default)]
    pub auth_context: HashMap<String, String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Subscribe request.
///
/// 订阅请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubscribeRequest {
    pub media_key: MediaKey,
    pub output_schema: MediaSchema,
    #[serde(default)]
    pub subscriber_kind: String,
    #[serde(default)]
    pub start_policy: String,
    /// Transport protocol the subscriber intends to use for admission decisions.
    /// Falls back to `output_schema` when empty.
    #[serde(default)]
    pub protocol: String,
    /// Optional client endpoint of the viewer, forwarded to admission hooks.
    #[serde(default)]
    pub remote_endpoint: Option<String>,
    #[serde(default)]
    pub auth_context: HashMap<String, String>,
}

impl Default for SubscribeRequest {
    fn default() -> Self {
        Self {
            media_key: MediaKey::new("__defaultVhost__", "live", "test", None)
                .expect("default key valid"),
            output_schema: MediaSchema::Hls,
            subscriber_kind: String::new(),
            start_policy: String::new(),
            protocol: String::new(),
            remote_endpoint: None,
            auth_context: HashMap::new(),
        }
    }
}

/// Start record request.
///
/// 开始录制请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartRecordRequest {
    pub media_key: MediaKey,
    pub format: String,
    #[serde(default)]
    pub template: RecordTemplate,
    #[serde(default)]
    pub segment_duration_ms: Option<u64>,
    #[serde(default)]
    pub max_segments: Option<u32>,
    #[serde(default)]
    pub storage_policy: StoragePolicy,
    #[serde(default)]
    pub idempotency_key: Option<IdempotencyKey>,
}

/// Stop record request.
///
/// 停止录制请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StopRecordRequest {
    pub task_id: RecordTaskId,
}

/// Record task query.
///
/// 录制任务查询。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecordTaskQuery {
    #[serde(default)]
    pub vhost: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub state: Option<RecordTaskState>,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
}

impl RecordTaskQuery {
    pub const MAX_PAGE_SIZE: u64 = 1_000;

    pub fn clamp_page_size(&mut self) {
        if self.page_size == 0 {
            self.page_size = default_page_size();
        }
        self.page_size = self.page_size.min(Self::MAX_PAGE_SIZE);
    }
}

/// Record file query.
///
/// 录制文件查询。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecordFileQuery {
    #[serde(default)]
    pub vhost: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub start_time_ms: Option<i64>,
    #[serde(default)]
    pub end_time_ms: Option<i64>,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub file_id: Option<String>,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
}

impl RecordFileQuery {
    pub const MAX_PAGE_SIZE: u64 = 1_000;

    pub fn clamp_page_size(&mut self) {
        if self.page_size == 0 {
            self.page_size = default_page_size();
        }
        self.page_size = self.page_size.min(Self::MAX_PAGE_SIZE);
    }
}

/// Delete record request.
///
/// 删除录制请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteRecordRequest {
    pub file_id: RecordFileId,
}

/// Record playback command.
///
/// 录制回放控制命令。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "command")]
pub enum RecordPlaybackCommand {
    Pause,
    Resume,
    Scale { value: f64 },
    Seek { value: i64 },
}

/// Snapshot request.
///
/// 快照请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotRequest {
    pub media_key: MediaKey,
    #[serde(default = "default_snapshot_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_snapshot_format")]
    pub format: String,
    #[serde(default)]
    pub quality: Option<u8>,
    #[serde(default)]
    pub max_width: Option<u32>,
    #[serde(default)]
    pub max_height: Option<u32>,
    #[serde(default)]
    pub storage_policy: StoragePolicy,
    #[serde(default)]
    pub capture_policy: HashMap<String, String>,
}

fn default_snapshot_timeout_ms() -> u64 {
    10_000
}

fn default_snapshot_format() -> String {
    "jpg".to_string()
}

/// Snapshot query.
///
/// 快照查询。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SnapshotQuery {
    #[serde(default)]
    pub vhost: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub snapshot_id: Option<String>,
    #[serde(default)]
    pub start_time_ms: Option<i64>,
    #[serde(default)]
    pub end_time_ms: Option<i64>,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
}

impl SnapshotQuery {
    pub const MAX_PAGE_SIZE: u64 = 1_000;

    pub fn clamp_page_size(&mut self) {
        if self.page_size == 0 {
            self.page_size = default_page_size();
        }
        self.page_size = self.page_size.min(Self::MAX_PAGE_SIZE);
    }
}

/// Delete snapshot request.
///
/// 删除快照请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteSnapshotRequest {
    pub media_key: MediaKey,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub retain_count: Option<u32>,
}

/// Destination for a fetched snapshot.
///
/// 抓取快照的目的地。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SnapshotDestination {
    /// FileStore namespace configured by the deployment.
    Namespace(String),
    /// Named storage policy from the deployment configuration.
    Policy(String),
}

/// Request to fetch a snapshot from an external URL.
///
/// 从外部 URL 抓取快照的请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchSnapshotRequest {
    /// Sanitized source URL. Must not contain userinfo.
    pub source_url: String,
    /// Optional credential handle for authenticated URLs.
    pub credential_handle: Option<CredentialHandle>,
    /// Destination policy/namespace for the fetched image.
    pub destination: SnapshotDestination,
    /// Expected media type (e.g. `image/jpeg`).
    pub expected_media_type: String,
    /// Expected format (e.g. `jpg`, `png`).
    pub expected_format: String,
    pub timeout_ms: u64,
    pub max_bytes: u64,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
}

impl FetchSnapshotRequest {
    /// Validate that the request does not contain forbidden inputs.
    pub fn validate(&self) -> Result<(), crate::error::MediaError> {
        let parsed = url::Url::parse(&self.source_url).map_err(|e| {
            crate::error::MediaError::invalid_argument(format!("source URL is not valid: {e}"))
        })?;
        let scheme = parsed.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(crate::error::MediaError::invalid_argument(format!(
                "unsupported URL scheme {scheme} for snapshot fetch"
            )));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(crate::error::MediaError::invalid_argument(
                "source URL must not contain userinfo",
            ));
        }
        if self.timeout_ms == 0 {
            return Err(crate::error::MediaError::invalid_argument(
                "timeout must be non-zero",
            ));
        }
        if self.max_bytes == 0 {
            return Err(crate::error::MediaError::invalid_argument(
                "max_bytes must be non-zero",
            ));
        }
        Ok(())
    }
}

/// Open playback request.
///
/// 打开回放请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenPlaybackRequest {
    pub file_handle: FileHandle,
    pub media_key: MediaKey,
    #[serde(default)]
    pub start_position_ms: i64,
    #[serde(default = "default_playback_scale")]
    pub scale: f64,
}

fn default_playback_scale() -> f64 {
    1.0
}

/// Playback control command.
///
/// 回放控制命令。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "command")]
pub enum PlaybackControl {
    Pause,
    Resume,
    Seek { position_ms: i64 },
    SetScale { scale: f64 },
}

/// Playback query.
///
/// 回放查询。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlaybackQuery {
    #[serde(default)]
    pub vhost: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub state: Option<crate::model::PlaybackSessionState>,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
}

impl PlaybackQuery {
    pub const MAX_PAGE_SIZE: u64 = 1_000;

    pub fn clamp_page_size(&mut self) {
        if self.page_size == 0 {
            self.page_size = default_page_size();
        }
        self.page_size = self.page_size.min(Self::MAX_PAGE_SIZE);
    }
}

/// Pull proxy request.
///
/// 拉流代理请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullProxyRequest {
    pub source_url: String,
    pub destination: MediaKey,
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    #[serde(default)]
    pub heartbeat_ms: Option<u64>,
    #[serde(default)]
    pub timeout_ms: u64,
    /// How the pulled stream should be processed before it is published to
    /// `destination`.
    #[serde(default, alias = "transcode_policy")]
    pub processing_policy: crate::processing::ProcessingPolicy,
    #[serde(default)]
    pub output_policy: OutputPolicy,
    #[serde(default)]
    pub record_policy: Option<StartRecordRequest>,
}

impl PullProxyRequest {
    /// Return the sanitized source URL suitable for storage, audit and events.
    ///
    /// Rejects URLs that contain userinfo (`user:pass@`) and applies the
    /// configured query-key denylist.
    pub fn sanitized_source_url(&self, policy: &OutboundUrlPolicy) -> Result<String, MediaError> {
        policy.sanitize_url(&self.source_url)
    }
}

/// Push proxy request.
///
/// 推流代理请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushProxyRequest {
    pub source_media_key: MediaKey,
    pub destination_url: String,
    pub protocol: String,
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    #[serde(default)]
    pub protocol_options: HashMap<String, String>,
}

impl PushProxyRequest {
    /// Return the sanitized destination URL suitable for storage, audit and events.
    ///
    /// Rejects URLs that contain userinfo (`user:pass@`) and applies the
    /// configured query-key denylist.
    pub fn sanitized_destination_url(
        &self,
        policy: &OutboundUrlPolicy,
    ) -> Result<String, MediaError> {
        policy.sanitize_url(&self.destination_url)
    }
}

/// Retry policy.
///
/// 重试策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub retry_delay_ms: u64,
    pub max_retry_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_delay_ms: 1_000,
            max_retry_delay_ms: 30_000,
        }
    }
}

/// Proxy query.
///
/// 代理查询。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProxyQuery {
    #[serde(default)]
    pub kind: Option<ProxyKind>,
    #[serde(default)]
    pub state: Option<ProxyState>,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
}

impl ProxyQuery {
    pub const MAX_PAGE_SIZE: u64 = 1_000;

    pub fn clamp_page_size(&mut self) {
        if self.page_size == 0 {
            self.page_size = default_page_size();
        }
        self.page_size = self.page_size.min(Self::MAX_PAGE_SIZE);
    }
}

/// RTP receiver request.
///
/// RTP 接收端请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RtpReceiverRequest {
    pub media_key: MediaKey,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub ssrc: Option<u32>,
    #[serde(default)]
    pub enable_rtcp: bool,
    #[serde(default)]
    pub tcp_mode: Option<RtpTcpMode>,
    #[serde(default)]
    pub payload_type: Option<u8>,
    #[serde(default)]
    pub codec_hint: Option<String>,
    #[serde(default)]
    pub reuse_port: bool,
    #[serde(default)]
    pub timeout_ms: u64,
}

/// RTP connect request.
///
/// RTP 连接请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RtpConnectRequest {
    #[serde(default)]
    pub session_id: RtpSessionId,
    pub remote_endpoint: String,
    #[serde(default)]
    pub ssrc: Option<u32>,
}

/// RTP sender request.
///
/// RTP 发送端请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RtpSenderRequest {
    pub media_key: MediaKey,
    pub destination_endpoint: String,
    #[serde(default)]
    pub ssrc: Option<u32>,
    #[serde(default)]
    pub payload_type: Option<u8>,
    #[serde(default)]
    pub codec_hint: Option<String>,
    #[serde(default)]
    pub mode: RtpSenderMode,
    #[serde(default)]
    pub transport_options: HashMap<String, String>,
}

/// RTP sender mode.
///
/// RTP 发送模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RtpSenderMode {
    #[default]
    Active,
    Passive,
    Talk,
}

/// RTP query.
///
/// RTP 查询。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RtpQuery {
    #[serde(default)]
    pub kind: Option<RtpSessionKind>,
    #[serde(default)]
    pub state: Option<RtpSessionState>,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
}

impl RtpQuery {
    pub const MAX_PAGE_SIZE: u64 = 1_000;

    pub fn clamp_page_size(&mut self) {
        if self.page_size == 0 {
            self.page_size = default_page_size();
        }
        self.page_size = self.page_size.min(Self::MAX_PAGE_SIZE);
    }
}

/// Update RTP session request.
///
/// 更新 RTP 会话请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateRtpRequest {
    #[serde(default)]
    pub session_id: RtpSessionId,
    pub expected_generation: u64,
    #[serde(default)]
    pub ssrc: Option<u32>,
    #[serde(default)]
    pub payload_type: Option<u8>,
    #[serde(default)]
    pub pause_check: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_playback_command_requires_value_for_scale() {
        let cmd = RecordPlaybackCommand::Scale { value: 1.0 };
        assert!(matches!(cmd, RecordPlaybackCommand::Scale { value } if value > 0.0));
    }

    #[test]
    fn media_query_page_size_clamped() {
        let mut q = MediaQuery {
            page_size: 10_000,
            ..Default::default()
        };
        q.clamp_page_size();
        assert_eq!(q.page_size, MediaQuery::MAX_PAGE_SIZE);
    }

    #[test]
    fn fetch_snapshot_request_rejects_userinfo_and_zero_limits() {
        let req = FetchSnapshotRequest {
            source_url: "http://user:pass@example.com/s.jpg".to_string(),
            credential_handle: None,
            destination: SnapshotDestination::Namespace("snapshots".to_string()),
            expected_media_type: "image/jpeg".to_string(),
            expected_format: "jpg".to_string(),
            timeout_ms: 10_000,
            max_bytes: 1_000_000,
            max_width: None,
            max_height: None,
        };
        assert!(req.validate().is_err());

        let mut req = FetchSnapshotRequest {
            source_url: "http://example.com/s.jpg".to_string(),
            credential_handle: None,
            destination: SnapshotDestination::Namespace("snapshots".to_string()),
            expected_media_type: "image/jpeg".to_string(),
            expected_format: "jpg".to_string(),
            timeout_ms: 0,
            max_bytes: 1_000_000,
            max_width: None,
            max_height: None,
        };
        assert!(req.validate().is_err());
        req.timeout_ms = 10_000;
        req.max_bytes = 0;
        assert!(req.validate().is_err());

        let req = FetchSnapshotRequest {
            source_url: "http://example.com/images/photo@2x.png".to_string(),
            credential_handle: None,
            destination: SnapshotDestination::Namespace("snapshots".to_string()),
            expected_media_type: "image/png".to_string(),
            expected_format: "png".to_string(),
            timeout_ms: 10_000,
            max_bytes: 1_000_000,
            max_width: None,
            max_height: None,
        };
        assert!(req.validate().is_ok());

        let req = FetchSnapshotRequest {
            source_url: "file:///etc/passwd".to_string(),
            credential_handle: None,
            destination: SnapshotDestination::Namespace("snapshots".to_string()),
            expected_media_type: "image/jpeg".to_string(),
            expected_format: "jpg".to_string(),
            timeout_ms: 10_000,
            max_bytes: 1_000_000,
            max_width: None,
            max_height: None,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn fetch_snapshot_request_round_trips() {
        let req = FetchSnapshotRequest {
            source_url: "http://example.com/s.jpg".to_string(),
            credential_handle: None,
            destination: SnapshotDestination::Namespace("snapshots".to_string()),
            expected_media_type: "image/jpeg".to_string(),
            expected_format: "jpg".to_string(),
            timeout_ms: 10_000,
            max_bytes: 1_000_000,
            max_width: Some(1920),
            max_height: Some(1080),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: FetchSnapshotRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn pull_proxy_request_sanitizes_source_url() {
        let req = PullProxyRequest {
            source_url: "https://Example.COM:8443/path?token=abc&keep=1#frag".to_string(),
            destination: MediaKey::new("default", "live", "test", None).unwrap(),
            retry_policy: RetryPolicy::default(),
            heartbeat_ms: None,
            timeout_ms: 10_000,
            processing_policy: Default::default(),
            output_policy: Default::default(),
            record_policy: None,
        };
        let mut policy = OutboundUrlPolicy::default();
        policy.deny_unknown_query_keys = vec!["token".to_string()];

        let sanitized = req.sanitized_source_url(&policy).unwrap();
        assert!(sanitized.contains("example.com:8443"), "{sanitized}");
        assert!(sanitized.contains("/path"), "{sanitized}");
        assert!(sanitized.contains("keep=1"), "{sanitized}");
        assert!(!sanitized.contains("token"), "{sanitized}");
        assert!(!sanitized.contains("#"), "{sanitized}");

        let bad_req = PullProxyRequest {
            source_url: "https://user:pass@example.com/path".to_string(),
            destination: MediaKey::new("default", "live", "test", None).unwrap(),
            retry_policy: RetryPolicy::default(),
            heartbeat_ms: None,
            timeout_ms: 10_000,
            processing_policy: Default::default(),
            output_policy: Default::default(),
            record_policy: None,
        };
        assert!(bad_req.sanitized_source_url(&policy).is_err());
    }

    #[test]
    fn push_proxy_request_sanitizes_destination_url() {
        let req = PushProxyRequest {
            source_media_key: MediaKey::new("default", "live", "test", None).unwrap(),
            destination_url: "rtsps://host.example:1935/app/stream?key=secret".to_string(),
            protocol: "rtsp".to_string(),
            retry_policy: RetryPolicy::default(),
            protocol_options: HashMap::new(),
        };
        let mut policy = OutboundUrlPolicy::default();
        policy.allowed_schemes = vec!["rtsps".to_string()];
        policy.deny_unknown_query_keys = vec!["key".to_string()];

        let sanitized = req.sanitized_destination_url(&policy).unwrap();
        assert!(sanitized.contains("host.example:1935"), "{sanitized}");
        assert!(sanitized.contains("/app/stream"), "{sanitized}");
        assert!(!sanitized.contains("key"), "{sanitized}");
    }
}
