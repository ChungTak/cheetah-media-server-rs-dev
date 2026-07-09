//! TS module configuration.

use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    pub demand_mode: bool,
    #[serde(default)]
    pub tls: Option<TsTlsConfig>,
    #[serde(default)]
    pub pull_jobs: Vec<TsPullJobConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TsTlsConfig {
    pub enabled: bool,
    pub listen: String,
    pub cert_path: String,
    pub key_path: String,
    #[serde(default = "default_handshake_timeout_ms")]
    pub handshake_timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// Validate config constraints. Returns list of errors.
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
fn default_max_reassembly_bytes() -> usize {
    4 * 1024 * 1024
}
fn default_pat_pmt_interval_ms() -> u64 {
    500
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
        let config = TsModuleConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn invalid_listen_rejected() {
        let mut config = TsModuleConfig::default();
        config.listen = "not-an-address".to_string();
        let err = config.validate().unwrap_err();
        assert!(err.contains("invalid listen address"));
    }

    #[test]
    fn tls_enabled_empty_cert_rejected() {
        let mut config = TsModuleConfig::default();
        config.tls = Some(TsTlsConfig {
            enabled: true,
            listen: "0.0.0.0:8443".to_string(),
            cert_path: "".to_string(),
            key_path: "/path/to/key".to_string(),
            handshake_timeout_ms: 5000,
        });
        let err = config.validate().unwrap_err();
        assert!(err.contains("cert_path must not be empty"));
    }

    #[test]
    fn subscriber_queue_less_than_bootstrap_rejected() {
        let mut config = TsModuleConfig::default();
        config.bootstrap_max_frames = 200;
        config.subscriber_queue_capacity = 100;
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
}
