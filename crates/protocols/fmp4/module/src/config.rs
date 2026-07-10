//! fMP4 module configuration.

use serde::{Deserialize, Serialize};

/// `Fmp4ModuleConfig` data structure.
/// `Fmp4ModuleConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fmp4ModuleConfig {
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `listen` field of type `String`.
    /// `listen` 字段，类型为 `String`.
    pub listen: String,
    /// `write_queue_capacity` field of type `usize`.
    /// `write_queue_capacity` 字段，类型为 `usize`.
    #[serde(default = "default_write_queue_capacity")]
    pub write_queue_capacity: usize,
    /// `read_buffer_size` field of type `usize`.
    /// `read_buffer_size` 字段，类型为 `usize`.
    #[serde(default = "default_read_buffer_size")]
    pub read_buffer_size: usize,
    /// `subscriber_queue_capacity` field of type `usize`.
    /// `subscriber_queue_capacity` 字段，类型为 `usize`.
    #[serde(default = "default_subscriber_queue_capacity")]
    pub subscriber_queue_capacity: usize,
    /// `bootstrap_max_frames` field of type `usize`.
    /// `bootstrap_max_frames` 字段，类型为 `usize`.
    #[serde(default = "default_bootstrap_max_frames")]
    pub bootstrap_max_frames: usize,
    /// `play_wait_source_timeout_ms` field of type `u64`.
    /// `play_wait_source_timeout_ms` 字段，类型为 `u64`.
    #[serde(default = "default_play_wait_source_timeout_ms")]
    pub play_wait_source_timeout_ms: u64,
    /// `max_tracks` field of type `usize`.
    /// `max_tracks` 字段，类型为 `usize`.
    #[serde(default = "default_max_tracks")]
    pub max_tracks: usize,
    /// `max_box_bytes` field of type `usize`.
    /// `max_box_bytes` 字段，类型为 `usize`.
    #[serde(default = "default_max_box_bytes")]
    pub max_box_bytes: usize,
    /// `max_fragment_duration_ms` field of type `u64`.
    /// `max_fragment_duration_ms` 字段，类型为 `u64`.
    #[serde(default = "default_max_fragment_duration_ms")]
    pub max_fragment_duration_ms: u64,
    /// `force_fragment_on_keyframe` field of type `bool`.
    /// `force_fragment_on_keyframe` 字段，类型为 `bool`.
    #[serde(default = "default_true")]
    pub force_fragment_on_keyframe: bool,
    /// `include_styp` field of type `bool`.
    /// `include_styp` 字段，类型为 `bool`.
    #[serde(default = "default_true")]
    pub include_styp: bool,
    /// `include_sidx` field of type `bool`.
    /// `include_sidx` 字段，类型为 `bool`.
    #[serde(default = "default_true")]
    pub include_sidx: bool,
    /// `demand_mode` field of type `bool`.
    /// `demand_mode` 字段，类型为 `bool`.
    #[serde(default)]
    pub demand_mode: bool,
    /// `tls` field.
    /// `tls` 字段.
    #[serde(default)]
    pub tls: Option<Fmp4TlsConfig>,
    /// `pull_jobs` field.
    /// `pull_jobs` 字段.
    #[serde(default)]
    pub pull_jobs: Vec<Fmp4PullJobConfig>,
}

/// `Fmp4TlsConfig` data structure.
/// `Fmp4TlsConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fmp4TlsConfig {
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `listen` field of type `String`.
    /// `listen` 字段，类型为 `String`.
    pub listen: String,
    /// `cert_path` field of type `String`.
    /// `cert_path` 字段，类型为 `String`.
    pub cert_path: String,
    /// `key_path` field of type `String`.
    /// `key_path` 字段，类型为 `String`.
    pub key_path: String,
    /// `handshake_timeout_ms` field of type `u64`.
    /// `handshake_timeout_ms` 字段，类型为 `u64`.
    #[serde(default = "default_handshake_timeout_ms")]
    pub handshake_timeout_ms: u64,
}

/// `Fmp4PullJobConfig` data structure.
/// `Fmp4PullJobConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fmp4PullJobConfig {
    /// `name` field of type `String`.
    /// `name` 字段，类型为 `String`.
    pub name: String,
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `source_url` field of type `String`.
    /// `source_url` 字段，类型为 `String`.
    pub source_url: String,
    /// `target_stream_key` field of type `String`.
    /// `target_stream_key` 字段，类型为 `String`.
    pub target_stream_key: String,
    /// `retry_backoff_ms` field of type `u64`.
    /// `retry_backoff_ms` 字段，类型为 `u64`.
    #[serde(default = "default_retry_backoff_ms")]
    pub retry_backoff_ms: u64,
    /// `max_retry_backoff_ms` field of type `u64`.
    /// `max_retry_backoff_ms` 字段，类型为 `u64`.
    #[serde(default = "default_max_retry_backoff_ms")]
    pub max_retry_backoff_ms: u64,
    /// `insecure_tls` field of type `bool`.
    /// `insecure_tls` 字段，类型为 `bool`.
    #[serde(default)]
    pub insecure_tls: bool,
}

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

impl Fmp4ModuleConfig {
    /// `default_json` function.
    /// `default_json` 函数.
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).unwrap_or_default()
    }

    /// Creates `value` from input.
    /// 创建 `值` 来自 输入.
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// `validate` function.
    /// `validate` 函数.
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
}
