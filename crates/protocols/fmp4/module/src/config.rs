//! fMP4 module configuration.
//!
//! fMP4 模块配置。

use std::fmt;

use cheetah_sdk::redact_url_secrets_for_debug;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Top-level configuration for the fMP4 module.
///
/// fMP4 模块的顶层配置。
pub struct Fmp4ModuleConfig {
    pub enabled: bool,
    pub listen: String,
    #[serde(default = "default_write_queue_capacity")]
    pub write_queue_capacity: usize,
    #[serde(default = "default_read_buffer_size")]
    pub read_buffer_size: usize,
    #[serde(default = "default_subscriber_queue_capacity")]
    pub subscriber_queue_capacity: usize,
    #[serde(default = "default_bootstrap_max_frames")]
    pub bootstrap_max_frames: usize,
    #[serde(default = "default_play_wait_source_timeout_ms")]
    pub play_wait_source_timeout_ms: u64,
    #[serde(default = "default_max_tracks")]
    pub max_tracks: usize,
    #[serde(default = "default_max_box_bytes")]
    pub max_box_bytes: usize,
    #[serde(default = "default_max_fragment_duration_ms")]
    pub max_fragment_duration_ms: u64,
    #[serde(default = "default_true")]
    pub force_fragment_on_keyframe: bool,
    #[serde(default = "default_true")]
    pub include_styp: bool,
    #[serde(default = "default_true")]
    pub include_sidx: bool,
    #[serde(default)]
    pub demand_mode: bool,
    #[serde(default)]
    pub tls: Option<Fmp4TlsConfig>,
    #[serde(default)]
    pub pull_jobs: Vec<Fmp4PullJobConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// TLS configuration for the fMP4 module listener.
///
/// fMP4 模块监听器的 TLS 配置。
pub struct Fmp4TlsConfig {
    pub enabled: bool,
    pub listen: String,
    pub cert_path: String,
    pub key_path: String,
    #[serde(default = "default_handshake_timeout_ms")]
    pub handshake_timeout_ms: u64,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
/// Pull job that ingests a remote fMP4 source into a local stream key.
///
/// 将远程 fMP4 源拉取到本地流密钥的拉取任务。
pub struct Fmp4PullJobConfig {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub source_url: String,
    pub target_stream_key: String,
    #[serde(default = "default_retry_backoff_ms")]
    pub retry_backoff_ms: u64,
    #[serde(default = "default_max_retry_backoff_ms")]
    pub max_retry_backoff_ms: u64,
    #[serde(default)]
    pub insecure_tls: bool,
}

impl fmt::Debug for Fmp4PullJobConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fmp4PullJobConfig")
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field(
                "source_url",
                &redact_url_secrets_for_debug(&self.source_url),
            )
            .field("target_stream_key", &self.target_stream_key)
            .field("retry_backoff_ms", &self.retry_backoff_ms)
            .field("max_retry_backoff_ms", &self.max_retry_backoff_ms)
            .field("insecure_tls", &self.insecure_tls)
            .finish()
    }
}

/// Default values for `Fmp4ModuleConfig`.
///
/// `Fmp4ModuleConfig` 的默认值。
impl Default for Fmp4ModuleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: "0.0.0.0:8083".to_string(),
            write_queue_capacity: 256,
            read_buffer_size: 65536,
            subscriber_queue_capacity: 256,
            bootstrap_max_frames: 150,
            play_wait_source_timeout_ms: 15000,
            max_tracks: 32,
            max_box_bytes: 4 * 1024 * 1024,
            max_fragment_duration_ms: 1000,
            force_fragment_on_keyframe: true,
            include_styp: true,
            include_sidx: true,
            demand_mode: false,
            tls: None,
            pull_jobs: Vec::new(),
        }
    }
}

/// `Fmp4ModuleConfig` helpers for serialization, deserialization, and validation.
///
/// `Fmp4ModuleConfig` 的序列化、反序列化与校验辅助。
impl Fmp4ModuleConfig {
    /// Serialize the default config to JSON.
    ///
    /// 将默认配置序列化为 JSON。
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).unwrap_or_default()
    }

    /// Deserialize from a JSON value.
    ///
    /// 从 JSON 值反序列化。
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// Validate the module and job configuration.
    ///
    /// 校验模块与任务配置。
    pub fn validate(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        if self.listen.parse::<std::net::SocketAddr>().is_err() {
            errors.push(format!("invalid listen address: {}", self.listen));
        }
        if let Some(tls) = &self.tls {
            if tls.enabled {
                if tls.cert_path.is_empty() {
                    errors.push("tls.cert_path must not be empty when TLS is enabled".to_string());
                }
                if tls.key_path.is_empty() {
                    errors.push("tls.key_path must not be empty when TLS is enabled".to_string());
                }
                if tls.listen.parse::<std::net::SocketAddr>().is_err() {
                    errors.push(format!("invalid tls.listen address: {}", tls.listen));
                }
            }
        }
        if self.write_queue_capacity < 1 {
            errors.push("write_queue_capacity must be >= 1".to_string());
        }
        let min_sub_queue = self.bootstrap_max_frames.max(1);
        if self.subscriber_queue_capacity < min_sub_queue {
            errors.push(format!(
                "subscriber_queue_capacity ({}) must be >= bootstrap_max_frames.max(1) ({})",
                self.subscriber_queue_capacity, min_sub_queue
            ));
        }
        if self.max_tracks < 1 {
            errors.push("max_tracks must be >= 1".to_string());
        }
        for job in &self.pull_jobs {
            if job.retry_backoff_ms > job.max_retry_backoff_ms {
                errors.push(format!(
                    "pull job '{}': retry_backoff_ms ({}) > max_retry_backoff_ms ({})",
                    job.name, job.retry_backoff_ms, job.max_retry_backoff_ms
                ));
            }
            let url = &job.source_url;
            if !(url.starts_with("http://")
                || url.starts_with("https://")
                || url.starts_with("ws://")
                || url.starts_with("wss://"))
            {
                errors.push(format!(
                    "pull job '{}': source_url must use http/https/ws/wss scheme",
                    job.name
                ));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

fn default_write_queue_capacity() -> usize {
    256
}
fn default_read_buffer_size() -> usize {
    65536
}
fn default_subscriber_queue_capacity() -> usize {
    256
}
fn default_bootstrap_max_frames() -> usize {
    150
}
fn default_play_wait_source_timeout_ms() -> u64 {
    15000
}
fn default_max_tracks() -> usize {
    32
}
fn default_max_box_bytes() -> usize {
    4 * 1024 * 1024
}
fn default_max_fragment_duration_ms() -> u64 {
    1000
}
fn default_handshake_timeout_ms() -> u64 {
    5000
}
fn default_retry_backoff_ms() -> u64 {
    500
}
fn default_max_retry_backoff_ms() -> u64 {
    5000
}
fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_validates() {
        assert!(Fmp4ModuleConfig::default().validate().is_ok());
    }

    #[test]
    fn invalid_listen_rejected() {
        let config = Fmp4ModuleConfig {
            listen: "not-an-address".to_string(),
            ..Default::default()
        };
        assert!(config
            .validate()
            .unwrap_err()
            .contains("invalid listen address"));
    }

    #[test]
    fn tls_enabled_empty_cert_rejected() {
        let config = Fmp4ModuleConfig {
            tls: Some(Fmp4TlsConfig {
                enabled: true,
                listen: "0.0.0.0:8445".to_string(),
                cert_path: "".to_string(),
                key_path: "/path/to/key".to_string(),
                handshake_timeout_ms: 5000,
            }),
            ..Default::default()
        };
        assert!(config
            .validate()
            .unwrap_err()
            .contains("cert_path must not be empty"));
    }

    #[test]
    fn pull_url_invalid_scheme_rejected() {
        let mut config = Fmp4ModuleConfig::default();
        config.pull_jobs.push(Fmp4PullJobConfig {
            name: "test".to_string(),
            enabled: true,
            source_url: "ftp://example.com/live/test.mp4".to_string(),
            target_stream_key: "live/test".to_string(),
            retry_backoff_ms: 500,
            max_retry_backoff_ms: 5000,
            insecure_tls: false,
        });
        assert!(config
            .validate()
            .unwrap_err()
            .contains("http/https/ws/wss scheme"));
    }

    #[test]
    fn debug_redacts_url_secrets_and_userinfo() {
        let job = Fmp4PullJobConfig {
            name: "test".to_string(),
            enabled: true,
            source_url: "http://user:pass@host/live/test.mp4?token=secret&other=ok".to_string(),
            target_stream_key: "live/test".to_string(),
            retry_backoff_ms: 500,
            max_retry_backoff_ms: 5_000,
            insecure_tls: false,
        };
        let out = format!("{job:?}");
        assert!(!out.contains("user:pass"), "{out}");
        assert!(!out.contains("token=secret"), "{out}");
        assert!(out.contains("other=ok"), "{out}");
    }
}
