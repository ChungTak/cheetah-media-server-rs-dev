use cheetah_sdk::BackpressurePolicy;
use serde::{Deserialize, Serialize};

/// `SrtModuleConfig` data structure.
/// `SrtModuleConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtModuleConfig {
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `listen` field of type `String`.
    /// `listen` 字段，类型为 `String`.
    pub listen: String,
    /// `max_connections` field of type `usize`.
    /// `max_connections` 字段，类型为 `usize`.
    pub max_connections: usize,
    /// `idle_timeout_ms` field of type `u64`.
    /// `idle_timeout_ms` 字段，类型为 `u64`.
    pub idle_timeout_ms: u64,
    /// `connect_timeout_ms` field of type `u64`.
    /// `connect_timeout_ms` 字段，类型为 `u64`.
    pub connect_timeout_ms: u64,
    /// `latency_ms` field of type `u64`.
    /// `latency_ms` 字段，类型为 `u64`.
    pub latency_ms: u64,
    /// `stats_interval_ms` field of type `u64`.
    /// `stats_interval_ms` 字段，类型为 `u64`.
    pub stats_interval_ms: u64,
    /// `payload` field of type `SrtPayloadModuleConfig`.
    /// `payload` 字段，类型为 `SrtPayloadModuleConfig`.
    pub payload: SrtPayloadModuleConfig,
    /// `encryption` field of type `SrtEncryptionModuleConfig`.
    /// `encryption` 字段，类型为 `SrtEncryptionModuleConfig`.
    pub encryption: SrtEncryptionModuleConfig,
    /// `auth` field of type `SrtAuthConfig`.
    /// `auth` 字段，类型为 `SrtAuthConfig`.
    pub auth: SrtAuthConfig,
    /// `ingress` field of type `SrtIngressConfig`.
    /// `ingress` 字段，类型为 `SrtIngressConfig`.
    pub ingress: SrtIngressConfig,
    /// `egress` field of type `SrtEgressConfig`.
    /// `egress` 字段，类型为 `SrtEgressConfig`.
    pub egress: SrtEgressConfig,
    /// `ingress_jobs` field.
    /// `ingress_jobs` 字段.
    pub ingress_jobs: Vec<SrtIngressJobConfig>,
    /// `egress_jobs` field.
    /// `egress_jobs` 字段.
    pub egress_jobs: Vec<SrtEgressJobConfig>,
    /// `relay_jobs` field.
    /// `relay_jobs` 字段.
    pub relay_jobs: Vec<SrtRelayJobConfig>,
}

/// `SrtPayloadModuleConfig` data structure.
/// `SrtPayloadModuleConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtPayloadModuleConfig {
    /// `kind` field of type `String`.
    /// `kind` 字段，类型为 `String`.
    pub kind: String,
}

/// `SrtEncryptionModuleConfig` data structure.
/// `SrtEncryptionModuleConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtEncryptionModuleConfig {
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `passphrase` field of type `String`.
    /// `passphrase` 字段，类型为 `String`.
    pub passphrase: String,
    /// `key_length` field of type `u16`.
    /// `key_length` 字段，类型为 `u16`.
    pub key_length: u16,
}

/// `SrtAuthConfig` data structure.
/// `SrtAuthConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SrtAuthConfig {
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `publish_token` field of type `String`.
    /// `publish_token` 字段，类型为 `String`.
    pub publish_token: String,
    /// `request_token` field of type `String`.
    /// `request_token` 字段，类型为 `String`.
    pub request_token: String,
    /// `users` field.
    /// `users` 字段.
    pub users: Vec<SrtAuthUserConfig>,
}

/// `SrtAuthUserConfig` data structure.
/// `SrtAuthUserConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrtAuthUserConfig {
    /// `username` field of type `String`.
    /// `username` 字段，类型为 `String`.
    pub username: String,
    /// `token` field of type `String`.
    /// `token` 字段，类型为 `String`.
    pub token: String,
}

/// `SrtIngressConfig` data structure.
/// `SrtIngressConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtIngressConfig {
    /// `default_mode` field of type `String`.
    /// `default_mode` 字段，类型为 `String`.
    pub default_mode: String,
    /// `default_publish_stream_key` field of type `String`.
    /// `default_publish_stream_key` 字段，类型为 `String`.
    pub default_publish_stream_key: String,
    /// `publish_keepalive_ms` field of type `u64`.
    /// `publish_keepalive_ms` 字段，类型为 `u64`.
    pub publish_keepalive_ms: u64,
}

/// `SrtEgressConfig` data structure.
/// `SrtEgressConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtEgressConfig {
    /// `subscriber_queue_capacity` field of type `usize`.
    /// `subscriber_queue_capacity` 字段，类型为 `usize`.
    pub subscriber_queue_capacity: usize,
    /// `subscriber_backpressure` field of type `BackpressurePolicy`.
    /// `subscriber_backpressure` 字段，类型为 `BackpressurePolicy`.
    pub subscriber_backpressure: BackpressurePolicy,
    /// `bootstrap_max_frames` field of type `usize`.
    /// `bootstrap_max_frames` 字段，类型为 `usize`.
    pub bootstrap_max_frames: usize,
    /// `start_from_keyframe` field of type `bool`.
    /// `start_from_keyframe` 字段，类型为 `bool`.
    pub start_from_keyframe: bool,
    /// `play_wait_source_timeout_ms` field of type `u64`.
    /// `play_wait_source_timeout_ms` 字段，类型为 `u64`.
    pub play_wait_source_timeout_ms: u64,
    /// `track_ready_timeout_ms` field of type `u64`.
    /// `track_ready_timeout_ms` 字段，类型为 `u64`.
    pub track_ready_timeout_ms: u64,
    /// `send_queue_capacity` field of type `usize`.
    /// `send_queue_capacity` 字段，类型为 `usize`.
    pub send_queue_capacity: usize,
    /// `disconnect_on_send_queue_overflow` field of type `bool`.
    /// `disconnect_on_send_queue_overflow` 字段，类型为 `bool`.
    pub disconnect_on_send_queue_overflow: bool,
}

/// `SrtIngressJobConfig` data structure.
/// `SrtIngressJobConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtIngressJobConfig {
    /// `name` field of type `String`.
    /// `name` 字段，类型为 `String`.
    pub name: String,
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `source_url` field of type `String`.
    /// `source_url` 字段，类型为 `String`.
    pub source_url: String,
    /// `target_stream_key` field of type `String`.
    /// `target_stream_key` 字段，类型为 `String`.
    pub target_stream_key: String,
    /// `retry_backoff_ms` field of type `u64`.
    /// `retry_backoff_ms` 字段，类型为 `u64`.
    pub retry_backoff_ms: u64,
    /// `max_retry_backoff_ms` field of type `u64`.
    /// `max_retry_backoff_ms` 字段，类型为 `u64`.
    pub max_retry_backoff_ms: u64,
}

/// `SrtEgressJobConfig` data structure.
/// `SrtEgressJobConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtEgressJobConfig {
    /// `name` field of type `String`.
    /// `name` 字段，类型为 `String`.
    pub name: String,
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `source_stream_key` field of type `String`.
    /// `source_stream_key` 字段，类型为 `String`.
    pub source_stream_key: String,
    /// `target_url` field of type `String`.
    /// `target_url` 字段，类型为 `String`.
    pub target_url: String,
    /// `disable_video` field of type `bool`.
    /// `disable_video` 字段，类型为 `bool`.
    pub disable_video: bool,
    /// `disable_audio` field of type `bool`.
    /// `disable_audio` 字段，类型为 `bool`.
    pub disable_audio: bool,
    /// `retry_backoff_ms` field of type `u64`.
    /// `retry_backoff_ms` 字段，类型为 `u64`.
    pub retry_backoff_ms: u64,
    /// `max_retry_backoff_ms` field of type `u64`.
    /// `max_retry_backoff_ms` 字段，类型为 `u64`.
    pub max_retry_backoff_ms: u64,
}

/// `SrtRelayJobConfig` data structure.
/// `SrtRelayJobConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtRelayJobConfig {
    /// `name` field of type `String`.
    /// `name` 字段，类型为 `String`.
    pub name: String,
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `source_url` field of type `String`.
    /// `source_url` 字段，类型为 `String`.
    pub source_url: String,
    /// `target_url` field of type `String`.
    /// `target_url` 字段，类型为 `String`.
    pub target_url: String,
    /// `stream_key` field of type `String`.
    /// `stream_key` 字段，类型为 `String`.
    pub stream_key: String,
    /// `retry_backoff_ms` field of type `u64`.
    /// `retry_backoff_ms` 字段，类型为 `u64`.
    pub retry_backoff_ms: u64,
    /// `max_retry_backoff_ms` field of type `u64`.
    /// `max_retry_backoff_ms` 字段，类型为 `u64`.
    pub max_retry_backoff_ms: u64,
}

impl Default for SrtModuleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: "0.0.0.0:9000".to_string(),
            max_connections: 1024,
            idle_timeout_ms: 30_000,
            connect_timeout_ms: 5_000,
            latency_ms: 120,
            stats_interval_ms: 5_000,
            payload: SrtPayloadModuleConfig::default(),
            encryption: SrtEncryptionModuleConfig::default(),
            auth: SrtAuthConfig::default(),
            ingress: SrtIngressConfig::default(),
            egress: SrtEgressConfig::default(),
            ingress_jobs: Vec::new(),
            egress_jobs: Vec::new(),
            relay_jobs: Vec::new(),
        }
    }
}

impl Default for SrtPayloadModuleConfig {
    fn default() -> Self {
        Self {
            kind: "mpegts".to_string(),
        }
    }
}

impl Default for SrtEncryptionModuleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            passphrase: String::new(),
            key_length: 16,
        }
    }
}

impl Default for SrtIngressConfig {
    fn default() -> Self {
        Self {
            default_mode: "publish".to_string(),
            default_publish_stream_key: String::new(),
            publish_keepalive_ms: 0,
        }
    }
}

impl Default for SrtEgressConfig {
    fn default() -> Self {
        Self {
            subscriber_queue_capacity: 256,
            subscriber_backpressure: BackpressurePolicy::DropUntilNextKeyframe,
            bootstrap_max_frames: 150,
            start_from_keyframe: true,
            play_wait_source_timeout_ms: 15_000,
            track_ready_timeout_ms: 3_000,
            send_queue_capacity: 256,
            disconnect_on_send_queue_overflow: true,
        }
    }
}

impl Default for SrtIngressJobConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: false,
            source_url: String::new(),
            target_stream_key: String::new(),
            retry_backoff_ms: 1_000,
            max_retry_backoff_ms: 30_000,
        }
    }
}

impl Default for SrtEgressJobConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: false,
            source_stream_key: String::new(),
            target_url: String::new(),
            disable_video: false,
            disable_audio: false,
            retry_backoff_ms: 1_000,
            max_retry_backoff_ms: 30_000,
        }
    }
}

impl Default for SrtRelayJobConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: false,
            source_url: String::new(),
            target_url: String::new(),
            stream_key: String::new(),
            retry_backoff_ms: 1_000,
            max_retry_backoff_ms: 30_000,
        }
    }
}

impl SrtModuleConfig {
    /// Creates `value` from input.
    /// 创建 `值` 来自 输入.
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// `default_json` function.
    /// `default_json` 函数.
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).unwrap()
    }
}
