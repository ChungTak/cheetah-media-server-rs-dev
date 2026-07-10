use cheetah_sdk::BackpressurePolicy;
use serde::{Deserialize, Serialize};

/// Configuration for `SRT Module`.
/// `SRT Module` 的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtModuleConfig {
    pub enabled: bool,
    pub listen: String,
    pub max_connections: usize,
    pub idle_timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub latency_ms: u64,
    pub stats_interval_ms: u64,
    pub payload: SrtPayloadModuleConfig,
    pub encryption: SrtEncryptionModuleConfig,
    pub auth: SrtAuthConfig,
    pub ingress: SrtIngressConfig,
    pub egress: SrtEgressConfig,
    pub ingress_jobs: Vec<SrtIngressJobConfig>,
    pub egress_jobs: Vec<SrtEgressJobConfig>,
    pub relay_jobs: Vec<SrtRelayJobConfig>,
}

/// Configuration for `SRT Payload Module`.
/// `SRT Payload Module` 的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtPayloadModuleConfig {
    pub kind: String,
}

/// Configuration for `SRT Encryption Module`.
/// `SRT Encryption Module` 的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtEncryptionModuleConfig {
    pub enabled: bool,
    pub passphrase: String,
    pub key_length: u16,
}

/// Configuration for `SRT Auth`.
/// `SRT Auth` 的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SrtAuthConfig {
    pub enabled: bool,
    pub publish_token: String,
    pub request_token: String,
    pub users: Vec<SrtAuthUserConfig>,
}

/// Configuration for `SRT Auth User`.
/// `SRT Auth User` 的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrtAuthUserConfig {
    pub username: String,
    pub token: String,
}

/// Configuration for `SRT Ingress`.
/// `SRT Ingress` 的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtIngressConfig {
    pub default_mode: String,
    pub default_publish_stream_key: String,
    pub publish_keepalive_ms: u64,
}

/// Configuration for `SRT Egress`.
/// `SRT Egress` 的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtEgressConfig {
    pub subscriber_queue_capacity: usize,
    pub subscriber_backpressure: BackpressurePolicy,
    pub bootstrap_max_frames: usize,
    pub start_from_keyframe: bool,
    pub play_wait_source_timeout_ms: u64,
    pub track_ready_timeout_ms: u64,
    pub send_queue_capacity: usize,
    pub disconnect_on_send_queue_overflow: bool,
}

/// Configuration for `SRT Ingress Job`.
/// `SRT Ingress Job` 的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtIngressJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
    pub target_stream_key: String,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}

/// Configuration for `SRT Egress Job`.
/// `SRT Egress Job` 的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtEgressJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_stream_key: String,
    pub target_url: String,
    pub disable_video: bool,
    pub disable_audio: bool,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}

/// Configuration for `SRT Relay Job`.
/// `SRT Relay Job` 的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtRelayJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
    pub target_url: String,
    pub stream_key: String,
    pub retry_backoff_ms: u64,
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
    /// 从输入创建 `value`。
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// `default_json` function of `SrtModuleConfig`.
    /// `SrtModuleConfig` 的 `default_json` 函数。
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).unwrap()
    }
}
