use std::net::SocketAddr;

use cheetah_sdk::{BackpressurePolicy, SdkError};
use serde::{Deserialize, Serialize};

use crate::route::{parse_stream_key_spec, validate_pull_source_url};

/// `HttpFlvModuleConfig` data structure.
/// `HttpFlvModuleConfig` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HttpFlvModuleConfig {
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `listen` field of type `String`.
    /// `listen` 字段，类型为 `String`.
    pub listen: String,
    /// `tls` field of type `HttpFlvTlsConfig`.
    /// `tls` 字段，类型为 `HttpFlvTlsConfig`.
    pub tls: HttpFlvTlsConfig,
    /// `write_queue_capacity` field of type `usize`.
    /// `write_queue_capacity` 字段，类型为 `usize`.
    pub write_queue_capacity: usize,
    /// `read_buffer_size` field of type `usize`.
    /// `read_buffer_size` 字段，类型为 `usize`.
    pub read_buffer_size: usize,
    /// `play_wait_source_timeout_ms` field of type `u64`.
    /// `play_wait_source_timeout_ms` 字段，类型为 `u64`.
    pub play_wait_source_timeout_ms: u64,
    /// `subscriber_queue_capacity` field of type `usize`.
    /// `subscriber_queue_capacity` 字段，类型为 `usize`.
    pub subscriber_queue_capacity: usize,
    /// `subscriber_backpressure` field of type `BackpressurePolicy`.
    /// `subscriber_backpressure` 字段，类型为 `BackpressurePolicy`.
    pub subscriber_backpressure: BackpressurePolicy,
    /// `bootstrap_max_frames` field of type `usize`.
    /// `bootstrap_max_frames` 字段，类型为 `usize`.
    pub bootstrap_max_frames: usize,
    /// `enable_add_mute` field of type `bool`.
    /// `enable_add_mute` 字段，类型为 `bool`.
    pub enable_add_mute: bool,
    /// `emit_play_metadata` field of type `bool`.
    /// `emit_play_metadata` 字段，类型为 `bool`.
    pub emit_play_metadata: bool,
    /// `alert_thresholds` field of type `HttpFlvAlertThresholds`.
    /// `alert_thresholds` 字段，类型为 `HttpFlvAlertThresholds`.
    pub alert_thresholds: HttpFlvAlertThresholds,
    /// `pull_jobs` field.
    /// `pull_jobs` 字段.
    pub pull_jobs: Vec<HttpFlvPullJobConfig>,
}

/// HTTPS-FLV / WSS-FLV TLS configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HttpFlvTlsConfig {
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
    pub handshake_timeout_ms: u64,
}

impl Default for HttpFlvTlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "0.0.0.0:8443".to_string(),
            cert_path: String::new(),
            key_path: String::new(),
            handshake_timeout_ms: 5_000,
        }
    }
}

/// `HttpFlvPullJobConfig` data structure.
/// `HttpFlvPullJobConfig` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HttpFlvPullJobConfig {
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

/// `HttpFlvAlertThresholds` data structure.
/// `HttpFlvAlertThresholds` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HttpFlvAlertThresholds {
    /// `startup_timeout_ms` field of type `u64`.
    /// `startup_timeout_ms` 字段，类型为 `u64`.
    pub startup_timeout_ms: u64,
    /// `queue_drop_count` field of type `u64`.
    /// `queue_drop_count` 字段，类型为 `u64`.
    pub queue_drop_count: u64,
}

impl Default for HttpFlvModuleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: "0.0.0.0:8080".to_string(),
            tls: HttpFlvTlsConfig::default(),
            write_queue_capacity: 256,
            read_buffer_size: 64 * 1024,
            play_wait_source_timeout_ms: 15_000,
            subscriber_queue_capacity: 256,
            subscriber_backpressure: BackpressurePolicy::DropUntilNextKeyframe,
            bootstrap_max_frames: 150,
            enable_add_mute: false,
            emit_play_metadata: true,
            alert_thresholds: HttpFlvAlertThresholds::default(),
            pull_jobs: Vec::new(),
        }
    }
}

impl Default for HttpFlvPullJobConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            source_url: String::new(),
            target_stream_key: String::new(),
            retry_backoff_ms: 500,
            max_retry_backoff_ms: 5_000,
        }
    }
}

impl Default for HttpFlvAlertThresholds {
    fn default() -> Self {
        Self {
            startup_timeout_ms: 3_000,
            queue_drop_count: 64,
        }
    }
}

impl HttpFlvModuleConfig {
    /// Creates `value` from input.
    /// 创建 `值` 来自 输入.
    pub fn from_value(value: serde_json::Value) -> Result<Self, SdkError> {
        let cfg: Self = serde_json::from_value(value)
            .map_err(|err| SdkError::InvalidArgument(format!("invalid http_flv config: {err}")))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// `validate` function.
    /// `validate` 函数.
    pub fn validate(&self) -> Result<(), SdkError> {
        self.listen
            .parse::<SocketAddr>()
            .map_err(|err| SdkError::InvalidArgument(format!("invalid http_flv.listen: {err}")))?;
        if self.tls.enabled {
            self.tls.listen.parse::<SocketAddr>().map_err(|err| {
                SdkError::InvalidArgument(format!("invalid http_flv.tls.listen: {err}"))
            })?;
            if self.tls.cert_path.is_empty() {
                return Err(SdkError::InvalidArgument(
                    "http_flv.tls.cert_path must not be empty when tls is enabled".to_string(),
                ));
            }
            if self.tls.key_path.is_empty() {
                return Err(SdkError::InvalidArgument(
                    "http_flv.tls.key_path must not be empty when tls is enabled".to_string(),
                ));
            }
        }
        if self.write_queue_capacity == 0 {
            return Err(SdkError::InvalidArgument(
                "http_flv.write_queue_capacity must be > 0".to_string(),
            ));
        }
        if self.read_buffer_size == 0 {
            return Err(SdkError::InvalidArgument(
                "http_flv.read_buffer_size must be > 0".to_string(),
            ));
        }
        if self.subscriber_queue_capacity == 0 {
            return Err(SdkError::InvalidArgument(
                "http_flv.subscriber_queue_capacity must be > 0".to_string(),
            ));
        }
        if self.bootstrap_max_frames == 0 {
            return Err(SdkError::InvalidArgument(
                "http_flv.bootstrap_max_frames must be > 0".to_string(),
            ));
        }
        if self.alert_thresholds.startup_timeout_ms == 0 {
            return Err(SdkError::InvalidArgument(
                "http_flv.alert_thresholds.startup_timeout_ms must be > 0".to_string(),
            ));
        }
        if self.alert_thresholds.queue_drop_count == 0 {
            return Err(SdkError::InvalidArgument(
                "http_flv.alert_thresholds.queue_drop_count must be > 0".to_string(),
            ));
        }
        if self.play_wait_source_timeout_ms > 0
            && self.alert_thresholds.startup_timeout_ms > self.play_wait_source_timeout_ms
        {
            return Err(SdkError::InvalidArgument(
                "http_flv.alert_thresholds.startup_timeout_ms must be <= http_flv.play_wait_source_timeout_ms when timeout is enabled".to_string(),
            ));
        }

        for (idx, job) in self.pull_jobs.iter().enumerate() {
            if !job.enabled {
                continue;
            }
            if job.name.trim().is_empty() {
                return Err(SdkError::InvalidArgument(format!(
                    "http_flv.pull_jobs[{idx}].name must not be empty"
                )));
            }
            if !validate_pull_source_url(job.source_url.trim()) {
                return Err(SdkError::InvalidArgument(format!(
                    "http_flv.pull_jobs[{idx}].source_url is invalid"
                )));
            }
            if parse_stream_key_spec(job.target_stream_key.trim()).is_none() {
                return Err(SdkError::InvalidArgument(format!(
                    "http_flv.pull_jobs[{idx}].target_stream_key is invalid"
                )));
            }
            if job.retry_backoff_ms == 0 || job.max_retry_backoff_ms == 0 {
                return Err(SdkError::InvalidArgument(format!(
                    "http_flv.pull_jobs[{idx}] backoff must be > 0"
                )));
            }
        }

        Ok(())
    }

    /// `default_json` function.
    /// `default_json` 函数.
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).expect("serialize default http_flv config")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_pull_job() -> HttpFlvPullJobConfig {
        HttpFlvPullJobConfig {
            name: "job-a".to_string(),
            enabled: true,
            source_url: "http://localhost/live/stream.flv".to_string(),
            target_stream_key: "live/stream".to_string(),
            retry_backoff_ms: 500,
            max_retry_backoff_ms: 2_000,
        }
    }

    #[test]
    fn default_config_is_valid() {
        HttpFlvModuleConfig::default()
            .validate()
            .expect("default config should validate");
    }

    #[test]
    fn reject_invalid_listen() {
        let cfg = HttpFlvModuleConfig {
            listen: "invalid-listen".to_string(),
            ..HttpFlvModuleConfig::default()
        };
        let err = cfg.validate().expect_err("must reject invalid listen");
        assert!(err.to_string().contains("http_flv.listen"));
    }

    #[test]
    fn reject_zero_queue_capacity() {
        let cfg = HttpFlvModuleConfig {
            write_queue_capacity: 0,
            ..HttpFlvModuleConfig::default()
        };
        let err = cfg.validate().expect_err("must reject zero queue");
        assert!(err.to_string().contains("http_flv.write_queue_capacity"));
    }

    #[test]
    fn reject_empty_pull_job_name() {
        let cfg = HttpFlvModuleConfig {
            pull_jobs: vec![HttpFlvPullJobConfig {
                name: " ".to_string(),
                ..enabled_pull_job()
            }],
            ..HttpFlvModuleConfig::default()
        };
        let err = cfg.validate().expect_err("must reject empty pull job name");
        assert!(err.to_string().contains("pull_jobs[0].name"));
    }

    #[test]
    fn reject_invalid_pull_job_source_url() {
        let cfg = HttpFlvModuleConfig {
            pull_jobs: vec![HttpFlvPullJobConfig {
                source_url: "rtmp://localhost/live/stream".to_string(),
                ..enabled_pull_job()
            }],
            ..HttpFlvModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject invalid pull source url");
        assert!(err.to_string().contains("pull_jobs[0].source_url"));
    }

    #[test]
    fn reject_unsupported_tls_pull_job_source_url() {
        let cfg = HttpFlvModuleConfig {
            pull_jobs: vec![HttpFlvPullJobConfig {
                source_url: "https://localhost/live/stream.flv".to_string(),
                ..enabled_pull_job()
            }],
            ..HttpFlvModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject unsupported tls pull source url");
        assert!(err.to_string().contains("pull_jobs[0].source_url"));

        let cfg = HttpFlvModuleConfig {
            pull_jobs: vec![HttpFlvPullJobConfig {
                source_url: "wss://localhost/live/stream.flv".to_string(),
                ..enabled_pull_job()
            }],
            ..HttpFlvModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject unsupported tls ws pull source url");
        assert!(err.to_string().contains("pull_jobs[0].source_url"));
    }

    #[test]
    fn reject_invalid_pull_job_target_stream_key() {
        let cfg = HttpFlvModuleConfig {
            pull_jobs: vec![HttpFlvPullJobConfig {
                target_stream_key: "no-slash".to_string(),
                ..enabled_pull_job()
            }],
            ..HttpFlvModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject invalid target stream key");
        assert!(err.to_string().contains("pull_jobs[0].target_stream_key"));
    }

    #[test]
    fn reject_zero_pull_job_backoff() {
        let cfg = HttpFlvModuleConfig {
            pull_jobs: vec![HttpFlvPullJobConfig {
                retry_backoff_ms: 0,
                ..enabled_pull_job()
            }],
            ..HttpFlvModuleConfig::default()
        };
        let err = cfg.validate().expect_err("must reject zero retry backoff");
        assert!(err.to_string().contains("pull_jobs[0] backoff"));
    }
}
