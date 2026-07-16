//! Webhook administration types.
//!
//! `WebhookAdminApi` lives in `crate::port` next to `WebhookApi` and
//! `MediaAdmissionApi`; this module holds the request/response models shared by
//! the admin provider and HTTP routes.
//!
//! Webhook 管理类型。`WebhookAdminApi` 定义在 `crate::port` 中，本模块保存
//! 管理 provider 与 HTTP 路由共享的请求/响应模型。

use serde::{Deserialize, Serialize};

use crate::ids::{MediaKey, RequestId};
use crate::model::{AdmissionAction, Decision};

/// Stable identifier for a webhook profile.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WebhookProfileId(pub String);

/// Delivery mode for a webhook profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookProfileMode {
    /// Native domain envelope with HMAC-SHA256 signature.
    NativeDomain,
    /// ZLM-compatible hook translation.
    ZlmCompatible,
}

/// Failure policy when the webhook target does not respond.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WebhookFailurePolicy {
    FailClosed,
    FailOpen,
}

/// Webhook profile managed by `WebhookAdminApi`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookProfile {
    pub id: WebhookProfileId,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub mode: WebhookProfileMode,
    pub target_url: String,
    #[serde(default)]
    pub event_filter: Vec<String>,
    #[serde(default)]
    pub admission_actions: Vec<AdmissionAction>,
    pub failure_policy: WebhookFailurePolicy,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Write-only secret used to sign outbound webhook envelopes.
    /// It is stored for internal use but must never be returned through the public view.
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub generation: u64,
}

fn default_true() -> bool {
    true
}

fn default_timeout_ms() -> u64 {
    5_000
}

impl WebhookProfile {
    pub fn view(&self) -> WebhookProfileView {
        WebhookProfileView {
            id: self.id.clone(),
            enabled: self.enabled,
            mode: self.mode,
            target_url: self.target_url.clone(),
            event_filter: self.event_filter.clone(),
            admission_actions: self.admission_actions.clone(),
            failure_policy: self.failure_policy,
            timeout_ms: self.timeout_ms,
            generation: self.generation,
        }
    }
}

/// Public, secret-free view of a `WebhookProfile`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookProfileView {
    pub id: WebhookProfileId,
    pub enabled: bool,
    pub mode: WebhookProfileMode,
    pub target_url: String,
    #[serde(default)]
    pub event_filter: Vec<String>,
    #[serde(default)]
    pub admission_actions: Vec<AdmissionAction>,
    pub failure_policy: WebhookFailurePolicy,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub generation: u64,
}

/// Request to create a webhook profile.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CreateWebhookProfileRequest {
    #[serde(default)]
    pub id: Option<WebhookProfileId>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub mode: WebhookProfileMode,
    pub target_url: String,
    #[serde(default)]
    pub event_filter: Vec<String>,
    #[serde(default)]
    pub admission_actions: Vec<AdmissionAction>,
    pub failure_policy: WebhookFailurePolicy,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    pub secret: String,
}

impl CreateWebhookProfileRequest {
    /// Build a profile with a generated id when the caller did not supply one.
    pub fn into_profile(self, id: WebhookProfileId) -> WebhookProfile {
        WebhookProfile {
            id,
            enabled: self.enabled,
            mode: self.mode,
            target_url: self.target_url,
            event_filter: self.event_filter,
            admission_actions: self.admission_actions,
            failure_policy: self.failure_policy,
            timeout_ms: self.timeout_ms,
            secret: self.secret,
            generation: 0,
        }
    }
}

/// Request to update an existing webhook profile.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UpdateWebhookProfileRequest {
    pub profile: WebhookProfile,
    pub expected_generation: u64,
}

/// Result of testing a webhook profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookTestReport {
    pub dns_resolved: bool,
    pub connected: bool,
    pub http_status: Option<u16>,
    pub body_valid: Option<bool>,
    pub signature_valid: Option<bool>,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// Synthetic test envelope sent by the `test_profile` operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookTest {
    pub event_id: RequestId,
    pub kind: String,
    pub media_key: MediaKey,
    pub payload: String,
}

/// Body of a test request accepted by native routes.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WebhookTestRequest {
    /// Optional explicit target URL to test without modifying the profile.
    #[serde(default)]
    pub target_url: Option<String>,
    /// Optional secret override.
    #[serde(default)]
    pub secret: Option<String>,
}

/// Native route response for a profile test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookTestResponse {
    pub report: WebhookTestReport,
}

/// Response to a `create_profile` call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookProfileResponse {
    pub profile: WebhookProfileView,
}

/// Response to a `list_profiles` call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookProfileListResponse {
    pub profiles: Vec<WebhookProfileView>,
}

/// Translate a `Decision` into the report body-valid flag.
pub fn decision_body_valid(decision: &Decision) -> bool {
    matches!(decision, Decision::Allow)
}
