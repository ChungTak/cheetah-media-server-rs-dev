use std::net::SocketAddr;

use cheetah_sdk::{BackpressurePolicy, SdkError};
use serde::{Deserialize, Serialize};

use crate::route::{parse_stream_key_spec, validate_pull_source_url};

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

/// HTTPS-FLV / WSS-FLV TLS configuration.
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
    pub fn from_value(value: serde_json::Value) -> Result<Self, SdkError> {
        let cfg: Self = serde_json::from_value(value)
            .map_err(|err| SdkError::InvalidArgument(format!("invalid http_flv config: {err}")))?;
        cfg.validate()?;
        Ok(cfg)
    }

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
