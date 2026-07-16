use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

/// Failure policy for synchronous decision hooks when the target does not
/// respond in time or returns a malformed response.
///
/// 同步决策 webhook 超时或响应异常时的失败策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    #[default]
    Deny,
    Allow,
}

/// Dispatch mode for a webhook target.
///
/// webhook 目标的投递模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookProfileMode {
    /// Native domain envelope with HMAC-SHA256 signing.
    #[default]
    NativeDomain,
    /// ZLMediaKit-compatible hook translation.
    ZlmCompatible,
}

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
#[derive(Clone, Serialize, Deserialize)]
pub struct WebhookProfile {
    /// Logical name for logs; never sent.
    pub name: String,
    /// POST target URL.
    pub url: String,
    /// Dispatch mode: native domain envelope or ZLM-compatible translation.
    #[serde(default)]
    pub mode: WebhookProfileMode,
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
    /// Maximum response body size in bytes.
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
    /// Maximum retries for transient failures.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Base interval between retries; doubled on each attempt (exponential backoff).
    #[serde(default = "default_retry_interval_ms")]
    pub retry_interval_ms: u64,
    /// Maximum total time spent retrying a single webhook before giving up.
    #[serde(default = "default_max_retry_duration_ms")]
    pub max_retry_duration_ms: u64,
    /// Consecutive failures before opening the circuit breaker.
    #[serde(default = "default_circuit_failure_threshold")]
    pub circuit_failure_threshold: u32,
    /// Milliseconds the circuit stays open before half-open.
    #[serde(default = "default_circuit_open_ms")]
    pub circuit_open_ms: u64,
    /// Network CIDRs that are explicitly allowed (e.g. ["127.0.0.1/8"]).
    #[serde(default)]
    pub allowed_cidrs: Vec<String>,
    /// Hook names that should be treated as synchronous decision hooks for
    /// this target (e.g. ["on_publish", "on_play"]).
    #[serde(default)]
    pub decision_events: Vec<String>,
    /// Short per-request timeout for synchronous decision hooks.
    #[serde(default = "default_decision_timeout_ms")]
    pub decision_timeout_ms: u64,
    /// Failure policy for synchronous decision hooks (default deny).
    #[serde(default = "default_decision_failure_policy")]
    pub decision_failure_policy: FailurePolicy,
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

    pub fn wants_decision(&self, hook_name: &str) -> bool {
        self.decision_events.iter().any(|e| e == hook_name)
    }

    pub fn decision_timeout(&self) -> Duration {
        Duration::from_millis(self.decision_timeout_ms)
    }
}

impl fmt::Debug for WebhookProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebhookProfile")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("mode", &self.mode)
            .field("events", &self.events)
            .field("secret", &self.secret.as_deref().map(|_| "***"))
            .field("timeout_ms", &self.timeout_ms)
            .field("max_body_bytes", &self.max_body_bytes)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("max_retries", &self.max_retries)
            .field("retry_interval_ms", &self.retry_interval_ms)
            .field("max_retry_duration_ms", &self.max_retry_duration_ms)
            .field("circuit_failure_threshold", &self.circuit_failure_threshold)
            .field("circuit_open_ms", &self.circuit_open_ms)
            .field("allowed_cidrs", &self.allowed_cidrs)
            .field("decision_events", &self.decision_events)
            .field("decision_timeout_ms", &self.decision_timeout_ms)
            .field("decision_failure_policy", &self.decision_failure_policy)
            .finish()
    }
}

impl Default for WebhookProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            url: String::new(),
            mode: WebhookProfileMode::default(),
            events: Vec::new(),
            secret: None,
            timeout_ms: default_timeout_ms(),
            max_body_bytes: default_max_body_bytes(),
            max_response_bytes: default_max_response_bytes(),
            max_retries: default_max_retries(),
            retry_interval_ms: default_retry_interval_ms(),
            max_retry_duration_ms: default_max_retry_duration_ms(),
            circuit_failure_threshold: default_circuit_failure_threshold(),
            circuit_open_ms: default_circuit_open_ms(),
            allowed_cidrs: Vec::new(),
            decision_events: Vec::new(),
            decision_timeout_ms: default_decision_timeout_ms(),
            decision_failure_policy: default_decision_failure_policy(),
        }
    }
}

fn default_timeout_ms() -> u64 {
    5000
}

fn default_max_body_bytes() -> usize {
    64 * 1024
}

fn default_max_response_bytes() -> usize {
    DEFAULT_MAX_RESPONSE_BYTES
}

pub(crate) const DEFAULT_MAX_RESPONSE_BYTES: usize = 1024 * 1024;

fn default_max_retries() -> u32 {
    2
}

fn default_retry_interval_ms() -> u64 {
    1000
}

fn default_max_retry_duration_ms() -> u64 {
    60_000
}

fn default_circuit_failure_threshold() -> u32 {
    5
}

fn default_circuit_open_ms() -> u64 {
    30_000
}

fn default_decision_timeout_ms() -> u64 {
    2000
}

fn default_decision_failure_policy() -> FailurePolicy {
    FailurePolicy::Deny
}
