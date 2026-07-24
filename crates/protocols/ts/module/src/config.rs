//! TS module configuration.
//!
//! TS 模块配置。

use std::fmt;

use cheetah_sdk::redact_url_secrets_for_debug;
use serde::{Deserialize, Serialize};

/// Configuration for the TS module.
///
/// TS 模块配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TsModuleConfig {
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
    #[serde(default)]
    pub strict_crc: bool,
    #[serde(default = "default_max_reassembly_bytes")]
    pub max_reassembly_bytes: usize,
    #[serde(default = "default_pat_pmt_interval_ms")]
    pub pat_pmt_interval_ms: u64,
    /// When true, TS mux only runs while at least one player is connected.
    ///
    /// 为 true 时，TS 复用器仅在至少一个播放器连接时运行。
    #[serde(default)]
    pub demand_mode: bool,
    #[serde(default)]
    pub tls: Option<TsTlsConfig>,
    #[serde(default)]
    pub pull_jobs: Vec<TsPullJobConfig>,
}

/// TLS configuration for the TS module.
///
/// TS 模块 TLS 配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TsTlsConfig {
    pub enabled: bool,
    pub listen: String,
    pub cert_path: String,
    pub key_path: String,
    #[serde(default = "default_handshake_timeout_ms")]
    pub handshake_timeout_ms: u64,
}

/// Configuration for a TS pull job.
///
/// TS 拉流任务配置。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct TsPullJobConfig {
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

impl fmt::Debug for TsPullJobConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TsPullJobConfig")
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

impl Default for TsModuleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: "0.0.0.0:8082".to_string(),
            write_queue_capacity: 256,
            read_buffer_size: 65536,
            subscriber_queue_capacity: 256,
            bootstrap_max_frames: 150,
            play_wait_source_timeout_ms: 15000,
            max_tracks: 32,
            strict_crc: false,
            max_reassembly_bytes: 4 * 1024 * 1024,
            pat_pmt_interval_ms: 500,
            demand_mode: false,
            tls: None,
            pull_jobs: Vec::new(),
        }
    }
}

impl TsModuleConfig {
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).unwrap_or_default()
    }

    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// Validate config constraints. Returns a list of errors as a single string.
    ///
    /// 校验配置约束，将错误列表合并为一个字符串返回。
    pub fn validate(&self) -> Result<(), String> {
        let mut errors = Vec::new();

        // Listen address must parse
        if self.listen.parse::<std::net::SocketAddr>().is_err() {
            errors.push(format!("invalid listen address: {}", self.listen));
        }

        // TLS validation
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

        // Queue sizes
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
        if self.max_reassembly_bytes < 188 {
            errors.push("max_reassembly_bytes must be >= 188".to_string());
        }

        // Pull job validation
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

/// Default write queue capacity for TS connections.
///
/// TS 连接默认写队列容量。
fn default_write_queue_capacity() -> usize {
    256
}
/// Default TCP read buffer size.
///
/// 默认 TCP 读缓冲区大小。
fn default_read_buffer_size() -> usize {
    65536
}
/// Default subscriber queue capacity.
///
/// 默认订阅者队列容量。
fn default_subscriber_queue_capacity() -> usize {
    256
}
/// Default max frames for the subscriber bootstrap.
///
/// 订阅者引导默认最大帧数。
fn default_bootstrap_max_frames() -> usize {
    150
}
/// Default timeout for waiting on a source before a play session starts.
///
/// 播放会话等待源的默认超时（毫秒）。
fn default_play_wait_source_timeout_ms() -> u64 {
    15000
}
/// Default maximum number of tracks to mux.
///
/// 复用的最大轨道数默认值。
fn default_max_tracks() -> usize {
    32
}
/// Default maximum reassembly buffer for the TS demuxer.
///
/// TS 解复用器重组缓冲区默认最大值。
fn default_max_reassembly_bytes() -> usize {
    4 * 1024 * 1024
}
/// Default PAT/PMT retransmission interval.
///
/// PAT/PMT 重传间隔默认值（毫秒）。
fn default_pat_pmt_interval_ms() -> u64 {
    500
}
/// Default TLS handshake timeout.
///
/// TLS 握手默认超时（毫秒）。
fn default_handshake_timeout_ms() -> u64 {
    5000
}
/// Default retry backoff for pull jobs.
///
/// 拉流任务默认重试退避（毫秒）。
fn default_retry_backoff_ms() -> u64 {
    500
}
/// Default maximum retry backoff for pull jobs.
///
/// 拉流任务默认最大重试退避（毫秒）。
fn default_max_retry_backoff_ms() -> u64 {
    5000
}
/// Default `true` for serde `#[serde(default)]`.
///
/// 用于 serde `#[serde(default)]` 的默认 `true`。
fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_validates() {
        let config = TsModuleConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn invalid_listen_rejected() {
        let config = TsModuleConfig {
            listen: "not-an-address".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("invalid listen address"));
    }

    #[test]
    fn tls_enabled_empty_cert_rejected() {
        let config = TsModuleConfig {
            tls: Some(TsTlsConfig {
                enabled: true,
                listen: "0.0.0.0:8443".to_string(),
                cert_path: "".to_string(),
                key_path: "/path/to/key".to_string(),
                handshake_timeout_ms: 5000,
            }),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("cert_path must not be empty"));
    }

    #[test]
    fn subscriber_queue_less_than_bootstrap_rejected() {
        let config = TsModuleConfig {
            bootstrap_max_frames: 200,
            subscriber_queue_capacity: 100,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("subscriber_queue_capacity"));
    }

    #[test]
    fn pull_url_invalid_scheme_rejected() {
        let mut config = TsModuleConfig::default();
        config.pull_jobs.push(TsPullJobConfig {
            name: "test".to_string(),
            enabled: true,
            source_url: "ftp://example.com/live/test.ts".to_string(),
            target_stream_key: "live/test".to_string(),
            retry_backoff_ms: 500,
            max_retry_backoff_ms: 5000,
            insecure_tls: false,
        });
        let err = config.validate().unwrap_err();
        assert!(err.contains("http/https/ws/wss scheme"));
    }

    #[test]
    fn pull_url_valid_schemes_accepted() {
        for scheme in &["http://", "https://", "ws://", "wss://"] {
            let mut config = TsModuleConfig::default();
            config.pull_jobs.push(TsPullJobConfig {
                name: "test".to_string(),
                enabled: true,
                source_url: format!("{scheme}example.com/live/test.ts"),
                target_stream_key: "live/test".to_string(),
                retry_backoff_ms: 500,
                max_retry_backoff_ms: 5000,
                insecure_tls: false,
            });
            assert!(config.validate().is_ok(), "scheme {scheme} should be valid");
        }
    }

    #[test]
    fn retry_backoff_exceeds_max_rejected() {
        let mut config = TsModuleConfig::default();
        config.pull_jobs.push(TsPullJobConfig {
            name: "test".to_string(),
            enabled: true,
            source_url: "http://example.com/live/test.ts".to_string(),
            target_stream_key: "live/test".to_string(),
            retry_backoff_ms: 10000,
            max_retry_backoff_ms: 5000,
            insecure_tls: false,
        });
        let err = config.validate().unwrap_err();
        assert!(err.contains("retry_backoff_ms"));
    }

    #[test]
    fn debug_redacts_url_secrets_and_userinfo() {
        let job = TsPullJobConfig {
            name: "test".to_string(),
            enabled: true,
            source_url: "http://user:pass@host/live/test.ts?token=secret&other=ok".to_string(),
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
