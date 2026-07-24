use std::fmt;

use cheetah_sdk::BackpressurePolicy;
use cheetah_srt_core::parse_srt_version;
use serde::{Deserialize, Serialize};

fn is_secret_query_key(key: &str) -> bool {
    matches!(
        key.to_lowercase().as_str(),
        "authorization"
            | "token"
            | "access_token"
            | "refresh_token"
            | "api_key"
            | "apikey"
            | "key"
            | "secret"
            | "signature"
            | "sign"
            | "auth"
            | "ticket"
            | "password"
            | "passwd"
            | "x-api-key"
            | "x_zlm_secret"
            | "x-zlm-secret"
            | "cookie"
            | "proxy-authorization"
            | "passphrase"
    )
}

/// Best-effort URL redactor: strips `user:pass@` and redacts secret query keys.
fn redact_url_for_debug(url: &str) -> String {
    let mut s = url.to_string();
    if let Some(scheme_end) = s.find("://") {
        let after = &s[scheme_end + 3..];
        if let Some(at) = after.find('@') {
            s = format!("{}://{}", &s[..scheme_end], &after[at + 1..]);
        }
    }

    if let Some((path, query)) = s.split_once('?') {
        let redacted = query
            .split('&')
            .map(|part| {
                if let Some((key, _)) = part.split_once('=') {
                    if is_secret_query_key(key) {
                        return format!("{key}=<redacted>");
                    }
                }
                part.to_string()
            })
            .collect::<Vec<_>>()
            .join("&");
        format!("{path}?{redacted}")
    } else {
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// Top-level configuration for the SRT module.
///
/// SRT 模块的顶层配置。
pub struct SrtModuleConfig {
    pub enabled: bool,
    pub listen: String,
    pub max_connections: usize,
    pub idle_timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub latency_ms: u64,
    pub latency_mul: u32,
    pub pkt_buf_size: usize,
    pub stats_interval_ms: u64,
    pub default_vhost: String,
    pub min_peer_srt_version: String,
    pub local_srt_version: String,
    pub require_peer_version_extension: bool,
    pub payload: SrtPayloadModuleConfig,
    pub encryption: SrtEncryptionModuleConfig,
    pub auth: SrtAuthConfig,
    pub ingress: SrtIngressConfig,
    pub egress: SrtEgressConfig,
    pub stream_id: SrtStreamIdModuleConfig,
    pub fec: SrtFecModuleConfig,
    pub ingress_jobs: Vec<SrtIngressJobConfig>,
    pub egress_jobs: Vec<SrtEgressJobConfig>,
    pub relay_jobs: Vec<SrtRelayJobConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// Payload encapsulation configured for the SRT module.
///
/// SRT 模块配置的负载封装。
pub struct SrtPayloadModuleConfig {
    pub kind: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// Encryption passphrase/key length for the SRT module.
///
/// SRT 模块的加密口令/密钥长度。
pub struct SrtEncryptionModuleConfig {
    pub enabled: bool,
    pub passphrase: String,
    pub key_length: u16,
}

impl fmt::Debug for SrtEncryptionModuleConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SrtEncryptionModuleConfig")
            .field("enabled", &self.enabled)
            .field("passphrase", &"<redacted>")
            .field("key_length", &self.key_length)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
/// Token/user based publish/request authorization.
///
/// 基于 token/用户的发布/请求授权。
pub struct SrtAuthConfig {
    pub enabled: bool,
    pub publish_token: String,
    pub request_token: String,
    pub users: Vec<SrtAuthUserConfig>,
}

impl fmt::Debug for SrtAuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SrtAuthConfig")
            .field("enabled", &self.enabled)
            .field("publish_token", &"<redacted>")
            .field("request_token", &"<redacted>")
            .field("users", &self.users)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Per-user username/token pair for SRT authorization.
///
/// SRT 授权的每个用户名/token 对。
pub struct SrtAuthUserConfig {
    pub username: String,
    pub token: String,
}

impl fmt::Debug for SrtAuthUserConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SrtAuthUserConfig")
            .field("username", &self.username)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// Default ingress mode and stream key behavior.
///
/// 默认入口模式与流密钥行为。
pub struct SrtIngressConfig {
    /// Default when streamid omits `m`. ZLM-compatible default is `request` (play).
    /// Set to `publish` to restore pre-compat behavior.
    ///
    /// streamid 缺少 `m` 时的默认模式。ZLM 兼容默认是 `request`（拉流）。
    /// 设为 `publish` 可恢复旧行为。
    pub default_mode: String,
    pub default_publish_stream_key: String,
    pub publish_keepalive_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// Subscriber, bootstrap, and send queue configuration for egress.
///
/// 出口端的订阅者、引导与发送队列配置。
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// Stream ID parsing options for ZLM-compatible behavior.
///
/// ZLM 兼容的 stream ID 解析选项。
pub struct SrtStreamIdModuleConfig {
    pub strict_prefix: bool,
    pub strict_resource: bool,
    pub allow_bare_key: bool,
    pub stream_key_vhost_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// Forward Error Correction configuration for SRT.
///
/// SRT 前向纠错配置。
pub struct SrtFecModuleConfig {
    pub enabled: bool,
    pub required: bool,
    pub cols: u32,
    pub rows: u32,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// Pull SRT ingress job: source URL to local stream key.
///
/// 拉取 SRT 入口任务：源 URL 到本地流密钥。
pub struct SrtIngressJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
    pub target_stream_key: String,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}

impl fmt::Debug for SrtIngressJobConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SrtIngressJobConfig")
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("source_url", &redact_url_for_debug(&self.source_url))
            .field("target_stream_key", &self.target_stream_key)
            .field("retry_backoff_ms", &self.retry_backoff_ms)
            .field("max_retry_backoff_ms", &self.max_retry_backoff_ms)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// Push SRT egress job: local stream key to target URL.
///
/// 推送 SRT 出口任务：本地流密钥到目标 URL。
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

impl fmt::Debug for SrtEgressJobConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SrtEgressJobConfig")
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("source_stream_key", &self.source_stream_key)
            .field("target_url", &redact_url_for_debug(&self.target_url))
            .field("disable_video", &self.disable_video)
            .field("disable_audio", &self.disable_audio)
            .field("retry_backoff_ms", &self.retry_backoff_ms)
            .field("max_retry_backoff_ms", &self.max_retry_backoff_ms)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// SRT relay job: source URL to target URL through a local stream key.
///
/// SRT 中继任务：通过本地流密钥从源 URL 到目标 URL。
pub struct SrtRelayJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
    pub target_url: String,
    pub stream_key: String,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}

impl fmt::Debug for SrtRelayJobConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SrtRelayJobConfig")
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("source_url", &redact_url_for_debug(&self.source_url))
            .field("target_url", &redact_url_for_debug(&self.target_url))
            .field("stream_key", &self.stream_key)
            .field("retry_backoff_ms", &self.retry_backoff_ms)
            .field("max_retry_backoff_ms", &self.max_retry_backoff_ms)
            .finish()
    }
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
            latency_mul: 4,
            pkt_buf_size: 8192,
            stats_interval_ms: 5_000,
            default_vhost: "__defaultVhost__".to_string(),
            min_peer_srt_version: "1.3.0".to_string(),
            local_srt_version: "1.5.0".to_string(),
            require_peer_version_extension: false,
            payload: SrtPayloadModuleConfig::default(),
            encryption: SrtEncryptionModuleConfig::default(),
            auth: SrtAuthConfig::default(),
            ingress: SrtIngressConfig::default(),
            egress: SrtEgressConfig::default(),
            stream_id: SrtStreamIdModuleConfig::default(),
            fec: SrtFecModuleConfig::default(),
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
            default_mode: "request".to_string(),
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

impl Default for SrtStreamIdModuleConfig {
    fn default() -> Self {
        Self {
            strict_prefix: true,
            strict_resource: true,
            allow_bare_key: false,
            stream_key_vhost_mode: "app_only".to_string(),
        }
    }
}

impl Default for SrtFecModuleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            required: false,
            cols: 10,
            rows: 5,
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
    /// Deserialize from a JSON value.
    ///
    /// 从 JSON 值反序列化。
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// Serialize the default config to JSON.
    ///
    /// 将默认配置序列化为 JSON。
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).unwrap_or_default()
    }

    /// Validate runtime constraints beyond serde parsing.
    ///
    /// 校验 serde 解析之外的运行时约束。
    pub fn validate(&self) -> Result<(), String> {
        if self.payload.kind.eq_ignore_ascii_case("mpegts") {
            // ok
        } else {
            return Err(format!(
                "unsupported srt.payload.kind `{}`; only `mpegts` is supported",
                self.payload.kind
            ));
        }

        let mode = self.ingress.default_mode.to_ascii_lowercase();
        if !matches!(mode.as_str(), "publish" | "request" | "play") {
            return Err(format!(
                "srt.ingress.default_mode must be `publish`, `request`, or `play`, got `{}`",
                self.ingress.default_mode
            ));
        }

        let vhost_mode = self.stream_id.stream_key_vhost_mode.to_ascii_lowercase();
        if !matches!(vhost_mode.as_str(), "app_only" | "vhost_prefix") {
            return Err(format!(
                "srt.stream_id.stream_key_vhost_mode must be `app_only` or `vhost_prefix`, got `{}`",
                self.stream_id.stream_key_vhost_mode
            ));
        }

        parse_srt_version(&self.min_peer_srt_version)
            .map_err(|err| format!("invalid srt.min_peer_srt_version: {err}"))?;
        parse_srt_version(&self.local_srt_version)
            .map_err(|err| format!("invalid srt.local_srt_version: {err}"))?;

        self.fec.validate()?;

        Ok(())
    }
}

impl SrtFecModuleConfig {
    /// Validate FEC matrix parameters.
    ///
    /// 校验 FEC 矩阵参数。
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            if self.required {
                return Err(
                    "srt.fec.enabled must be true when srt.fec.required is true".to_string()
                );
            }
            return Ok(());
        }
        if self.cols == 0 || self.rows == 0 {
            return Err("srt.fec.cols and srt.fec.rows must be > 0 when enabled".to_string());
        }
        if self.cols.saturating_mul(self.rows) > 10_000 {
            return Err("srt.fec matrix is too large".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_encryption_passphrase() {
        let cfg = SrtEncryptionModuleConfig {
            enabled: true,
            passphrase: "super-secret".to_string(),
            key_length: 32,
        };
        let out = format!("{cfg:?}");
        assert!(out.contains("enabled: true"), "enabled missing: {out}");
        assert!(!out.contains("super-secret"), "passphrase leaked: {out}");
    }

    #[test]
    fn debug_redacts_auth_tokens() {
        let cfg = SrtAuthConfig {
            enabled: true,
            publish_token: "pub-token".to_string(),
            request_token: "req-token".to_string(),
            users: vec![SrtAuthUserConfig {
                username: "alice".to_string(),
                token: "user-token".to_string(),
            }],
        };
        let out = format!("{cfg:?}");
        assert!(out.contains("alice"), "username missing: {out}");
        assert!(!out.contains("pub-token"), "publish_token leaked: {out}");
        assert!(!out.contains("req-token"), "request_token leaked: {out}");
        assert!(!out.contains("user-token"), "user token leaked: {out}");
    }

    #[test]
    fn debug_redacts_url_query_secrets_and_userinfo() {
        let job = SrtIngressJobConfig {
            name: "in".to_string(),
            enabled: true,
            source_url: "srt://user:pass@host:9000?passphrase=secret&streamid=live/app".to_string(),
            target_stream_key: "live/app".to_string(),
            retry_backoff_ms: 1_000,
            max_retry_backoff_ms: 30_000,
        };
        let out = format!("{job:?}");
        assert!(!out.contains("user:pass"), "userinfo leaked: {out}");
        assert!(
            !out.contains("passphrase=secret"),
            "passphrase leaked: {out}"
        );
        assert!(
            out.contains("streamid=live/app"),
            "non-secret query dropped: {out}"
        );
    }
}
