use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Top-level configuration for the webhook dispatcher.
///
/// 分发器顶层配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebhookDispatcherConfig {
    pub profiles: Vec<WebhookProfile>,
}

/// A single webhook target.
///
/// 单个 webhook 目标。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookProfile {
    /// Logical name for logs; never sent.
    pub name: String,
    /// POST target URL.
    pub url: String,
    /// Only dispatch events whose hook name is in this list.
    pub events: Vec<String>,
    /// Optional HMAC-SHA256 secret.
    pub secret: Option<String>,
    /// Per-request timeout.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Maximum body size in bytes.
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
    /// Maximum retries for transient failures.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Fixed interval between retries.
    #[serde(default = "default_retry_interval_ms")]
    pub retry_interval_ms: u64,
    /// Consecutive failures before opening the circuit breaker.
    #[serde(default = "default_circuit_failure_threshold")]
    pub circuit_failure_threshold: u32,
    /// Milliseconds the circuit stays open before half-open.
    #[serde(default = "default_circuit_open_ms")]
    pub circuit_open_ms: u64,
    /// Network CIDRs that are explicitly allowed (e.g. ["127.0.0.1/8"]).
    #[serde(default)]
    pub allowed_cidrs: Vec<String>,
}

impl WebhookProfile {
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }

    pub fn retry_interval(&self) -> Duration {
        Duration::from_millis(self.retry_interval_ms)
    }

    pub fn wants_event(&self, hook_name: &str) -> bool {
        self.events.is_empty() || self.events.iter().any(|e| e == hook_name)
    }
}

fn default_timeout_ms() -> u64 {
    5000
}

fn default_max_body_bytes() -> usize {
    64 * 1024
}

fn default_max_retries() -> u32 {
    2
}

fn default_retry_interval_ms() -> u64 {
    1000
}

fn default_circuit_failure_threshold() -> u32 {
    5
}

fn default_circuit_open_ms() -> u64 {
    30_000
}
