use std::net::SocketAddr;

use cheetah_sdk::{BackpressurePolicy, SdkError};
use serde::{Deserialize, Serialize};

use crate::route::{parse_stream_key_spec, validate_pull_source_url};

/// Runtime configuration for the HTTP-FLV module.
///
/// Drives the TCP/HTTP listener, TLS listener, subscriber queue policy,
/// bootstrap policy, and pull jobs.  Values are validated by
/// [`HttpFlvModuleConfig::validate`].
///
/// HTTP-FLV 模块的运行时配置。
///
/// 驱动 TCP/HTTP 监听器、TLS 监听器、订阅者队列策略、启动策略和拉流任务。
/// 由 [`HttpFlvModuleConfig::validate`] 进行校验。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HttpFlvModuleConfig {
    pub enabled: bool,
    pub listen: String,
    pub tls: HttpFlvTlsConfig,
    pub write_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub play_wait_source_timeout_ms: u64,
    pub subscriber_queue_capacity: usize,
    pub subscriber_backpressure: BackpressurePolicy,
    pub bootstrap_max_frames: usize,
    pub enable_add_mute: bool,
    pub emit_play_metadata: bool,
    pub alert_thresholds: HttpFlvAlertThresholds,
    pub pull_jobs: Vec<HttpFlvPullJobConfig>,
}

/// TLS / HTTPS-FLV / WSS-FLV listener configuration.
///
/// When enabled, `cert_path` and `key_path` must be non-empty and
/// `listen` must be a valid `SocketAddr`.
///
/// TLS / HTTPS-FLV / WSS-FLV 监听器配置。
///
/// 启用时，`cert_path` 和 `key_path` 必须非空，`listen` 必须是有效的 `SocketAddr`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HttpFlvTlsConfig {
    pub enabled: bool,
    pub listen: String,
    pub cert_path: String,
    pub key_path: String,
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

/// Pull job description.
///
/// The module repeatedly pulls an HTTP or WebSocket FLV source and
/// injects the resulting stream into the engine under `target_stream_key`.
///
/// 拉流任务描述。
///
/// 模块会反复从 HTTP 或 WebSocket FLV 源拉流，并将得到的流注入到
/// 引擎中的 `target_stream_key` 下。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HttpFlvPullJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
    pub target_stream_key: String,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}

/// Operational thresholds used to diagnose startup stalls and queue drops.
///
/// 用于诊断启动卡死与队列丢包的运行阈值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HttpFlvAlertThresholds {
    pub startup_timeout_ms: u64,
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
    /// Parse a JSON value into a validated HTTP-FLV module configuration.
    ///
    /// 将 JSON 值解析为已校验的 HTTP-FLV 模块配置。
    pub fn from_value(value: serde_json::Value) -> Result<Self, SdkError> {
        let cfg: Self = serde_json::from_value(value)
            .map_err(|err| SdkError::InvalidArgument(format!("invalid http_flv config: {err}")))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate the configuration fields.
    ///
    /// Checks that listen addresses are valid `SocketAddr`s, TLS prerequisites
    /// are satisfied when TLS is enabled, queue sizes are positive, and that
    /// `startup_timeout_ms` does not exceed `play_wait_source_timeout_ms`. Each
    /// enabled pull job must have a non-empty name, a valid HTTP/WS source URL,
    /// a valid target stream key, and positive backoff values.
    ///
    /// 校验配置字段。
    ///
    /// 检查监听地址是否为有效的 `SocketAddr`；TLS 启用时是否满足 TLS 前提条件；
    /// 队列大小是否为正；`startup_timeout_ms` 是否不超过 `play_wait_source_timeout_ms`。
    /// 每个已启用的拉流任务必须有非空名称、有效的 HTTP/WS 源 URL、有效的目标流 Key
    /// 以及正的退避值。
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

    /// Return the default configuration as a JSON value for the schema registry.
    ///
    /// 将默认配置以 JSON 值形式返回，用于 schema 注册。
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
