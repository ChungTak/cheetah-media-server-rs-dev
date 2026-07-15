//! Outbound webhook dispatcher for [`MediaEvent`] with ZLM-compatible
//! translation, timeout, retry, and circuit breaker.
//!
//! 出站 webhook 分发器：消费 `MediaEvent`，按目标 profile 翻译并发送 HTTP POST。

pub mod circuit;
pub mod config;
pub mod dispatcher;
pub mod module;
pub mod security;
pub mod sender;
pub mod translator;

pub use circuit::{CircuitBreaker, CircuitState};
pub use config::{WebhookDispatcherConfig, WebhookProfile};
pub use dispatcher::{WebhookDispatcher, WebhookDispatcherHandle, WebhookJob};
pub use module::{WebhookModule, WebhookModuleFactory};
pub use security::{WebhookUrlPolicy, WebhookUrlVerdict};
pub use sender::{
    RuntimeHttpClient, WebhookHttpRequest, WebhookResponse, WebhookSendError, WebhookSender,
};
pub use translator::{WebhookDispatch, WebhookTranslator, ZlmWebhookTranslator};
