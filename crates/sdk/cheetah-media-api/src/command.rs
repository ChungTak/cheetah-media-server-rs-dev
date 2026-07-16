use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ids::*;
use crate::model::*;

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
    #[serde(default)]
    pub auth_context: HashMap<String, String>,
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
    #[serde(default)]
    pub transcode_policy: TranscodePolicy,
    #[serde(default)]
    pub output_policy: OutputPolicy,
    #[serde(default)]
    pub record_policy: Option<StartRecordRequest>,
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

/// FFmpeg proxy request.
///
/// FFmpeg 代理请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FfmpegProxyRequest {
    pub source_url: String,
    pub destination: MediaKey,
    #[serde(default)]
    pub input_options: Vec<String>,
    #[serde(default)]
    pub output_options: Vec<String>,
    #[serde(default)]
    pub transcode_policy: TranscodePolicy,
    #[serde(default)]
    pub output_policy: OutputPolicy,
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
}
