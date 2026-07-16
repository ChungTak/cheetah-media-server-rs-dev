use cheetah_codec::MonoTime;
use cheetah_media_api::event::{
    MediaEvent, MediaEventBusApi, MediaEventSender, MediaEventSubscription,
};
use cheetah_media_api::ids::MediaKey;
use cheetah_runtime_api::RuntimeApi;
use cheetah_sdk::CancellationToken;
use cheetah_sdk::MetricsApi;
use futures::channel::mpsc;
use futures::future::FutureExt;
use futures::select_biased;
use futures::stream::StreamExt;
use parking_lot::RwLock;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::circuit::CircuitBreaker;
use crate::config::{WebhookDispatcherConfig, WebhookProfile, WebhookProfileMode};
use crate::security::WebhookUrlPolicy;
use crate::sender::{WebhookHttpRequest, WebhookSendError, WebhookSender};
use crate::translator::{media_event_type, WebhookDispatch, WebhookTranslator};

/// A single unit of work for a per-target webhook worker.
///
/// 每个目标 webhook worker 的一个任务单元。
#[derive(Debug, Clone)]
pub struct WebhookJob {
    pub event_id: String,
    pub event_type: String,
    pub occurred_at: i64,
    pub resource: Option<MediaKey>,
    pub profile: WebhookProfile,
    pub dispatch: WebhookDispatch,
}

/// Handle returned by [`WebhookDispatcher::start`].  Dropping the handle stops
/// the dispatcher and unsubscribes from the event bus.
///
/// 由 `WebhookDispatcher::start` 返回的句柄。Drop 时会停止分发器并取消订阅。
pub struct WebhookDispatcherHandle {
    cancel: CancellationToken,
    subscription: Option<Box<dyn MediaEventSubscription>>,
}

impl WebhookDispatcherHandle {
    /// Stop accepting events and wait for workers to drain.
    pub fn stop(mut self) {
        self.cancel.cancel();
        if let Some(sub) = self.subscription.take() {
            let _ = sub.unsubscribe();
        }
    }
}

/// Independent outbound webhook dispatcher.
///
/// 独立的出站 webhook 分发器。
#[derive(Clone)]
pub struct WebhookDispatcher {
    config: Arc<RwLock<WebhookDispatcherConfig>>,
    event_bus: Arc<dyn MediaEventBusApi>,
    runtime_api: Arc<dyn RuntimeApi>,
    sender: Arc<dyn WebhookSender>,
    translators: Arc<HashMap<WebhookProfileMode, Arc<dyn WebhookTranslator>>>,
    url_policy: WebhookUrlPolicy,
    metrics: Option<Arc<dyn MetricsApi>>,
}

impl WebhookDispatcher {
    pub fn new(
        config: WebhookDispatcherConfig,
        event_bus: Arc<dyn MediaEventBusApi>,
        runtime_api: Arc<dyn RuntimeApi>,
        sender: Arc<dyn WebhookSender>,
        translators: HashMap<WebhookProfileMode, Arc<dyn WebhookTranslator>>,
        url_policy: WebhookUrlPolicy,
        metrics: Option<Arc<dyn MetricsApi>>,
    ) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            event_bus,
            runtime_api,
            sender,
            translators: Arc::new(translators),
            url_policy,
            metrics,
        }
    }

    /// Replace the active configuration at runtime.
    pub fn set_config(&self, config: WebhookDispatcherConfig) {
        *self.config.write() = config;
    }

    /// Subscribe to the event bus and start dispatching.
    ///
    /// `capacity` is the size of the ingress queue between the bus and the
    /// dispatcher task.
    pub fn start(
        &self,
        capacity: usize,
    ) -> cheetah_media_api::error::Result<WebhookDispatcherHandle> {
        let (tx, rx) = mpsc::channel::<MediaEvent>(capacity);
        let sender = EventIngressSender(tx);
        let subscription = self.event_bus.subscribe(Box::new(sender), capacity)?;
        let cancel = CancellationToken::new();

        let worker = DispatcherWorker {
            config: self.config.clone(),
            event_bus: self.event_bus.clone(),
            runtime_api: self.runtime_api.clone(),
            sender: self.sender.clone(),
            translators: self.translators.clone(),
            url_policy: self.url_policy.clone(),
            metrics: self.metrics.clone(),
            cancel: cancel.child_token(),
        };

        self.runtime_api.spawn(Box::pin(async move {
            worker.run(rx).await;
        }));

        Ok(WebhookDispatcherHandle {
            cancel,
            subscription: Some(subscription),
        })
    }
}

struct EventIngressSender(mpsc::Sender<MediaEvent>);

impl MediaEventSender for EventIngressSender {
    fn send(&self, event: MediaEvent) -> cheetah_media_api::error::Result<()> {
        // Bounded ingress queue: drop the event if the dispatcher is slow
        // rather than blocking the bus forwarder.
        let mut tx = self.0.clone();
        if tx.try_send(event).is_err() {
            warn!("webhook dispatcher ingress queue full; dropping event");
        }
        Ok(())
    }

    fn lagged(&self, _dropped: u64) -> cheetah_media_api::error::Result<()> {
        // The bus already tracks drops from its own queue; the dispatcher
        // additionally logs ingress drops above.
        Ok(())
    }
}

struct DispatcherWorker {
    config: Arc<RwLock<WebhookDispatcherConfig>>,
    #[allow(dead_code)]
    event_bus: Arc<dyn MediaEventBusApi>,
    runtime_api: Arc<dyn RuntimeApi>,
    sender: Arc<dyn WebhookSender>,
    translators: Arc<HashMap<WebhookProfileMode, Arc<dyn WebhookTranslator>>>,
    url_policy: WebhookUrlPolicy,
    metrics: Option<Arc<dyn MetricsApi>>,
    cancel: CancellationToken,
}

impl DispatcherWorker {
    async fn run(self, mut events: mpsc::Receiver<MediaEvent>) {
        let mut workers: HashMap<String, mpsc::Sender<WebhookJob>> = HashMap::new();
        let cancel = self.cancel.child_token();

        loop {
            select_biased! {
                _ = cancel.cancelled().fuse() => break,
                event = events.next() => {
                    if let Some(event) = event {
                        self.handle_event(event, &mut workers).await;
                    } else {
                        break;
                    }
                }
            }
        }

        // Drop senders so workers finish naturally.
        drop(workers);
    }

    async fn handle_event(
        &self,
        event: MediaEvent,
        workers: &mut HashMap<String, mpsc::Sender<WebhookJob>>,
    ) {
        let header = event.header();
        let event_id = header.event_id.clone();
        let event_type = media_event_type(&event).unwrap_or_default();
        let occurred_at = header.occurred_at;
        let resource = header.media_key.clone();
        let profiles = self.config.read().profiles.clone();

        for profile in &profiles {
            let translator = match self.translators.get(&profile.mode) {
                Some(t) => t,
                None => {
                    warn!(
                        target = %profile.name,
                        mode = ?profile.mode,
                        "no webhook translator registered for profile mode"
                    );
                    continue;
                }
            };

            let dispatches = translator.translate(&event);
            if dispatches.is_empty() {
                self.record_unsupported(&event_type, &profile.name);
                warn!(
                    target = %profile.name,
                    event_type = %event_type,
                    "webhook translator returned no dispatch; unsupported mapping"
                );
                continue;
            }

            for dispatch in dispatches {
                if !profile.wants_event(&dispatch.hook_name) {
                    continue;
                }

                let job = WebhookJob {
                    event_id: event_id.clone(),
                    event_type: event_type.clone(),
                    occurred_at,
                    resource: resource.clone(),
                    profile: profile.clone(),
                    dispatch: dispatch.clone(),
                };

                let sender = workers.entry(profile.name.clone()).or_insert_with(|| {
                    let (tx, rx) = mpsc::channel::<WebhookJob>(128);
                    let url_policy = WebhookUrlPolicy::from_cidr_strings(&profile.allowed_cidrs)
                        .unwrap_or_else(|_| self.url_policy.clone());
                    let worker = TargetWorker {
                        profile_name: profile.name.clone(),
                        sender: self.sender.clone(),
                        url_policy,
                        runtime_api: self.runtime_api.clone(),
                        cancel: self.cancel.child_token(),
                        rx,
                        circuit: CircuitBreaker::new(
                            profile.circuit_failure_threshold,
                            Duration::from_millis(profile.circuit_open_ms),
                        ),
                    };
                    self.runtime_api.spawn(Box::pin(async move {
                        worker.run().await;
                    }));
                    tx
                });

                if sender.try_send(job).is_err() {
                    warn!(
                        target = %profile.name,
                        hook = %dispatch.hook_name,
                        "webhook target queue full; dropping event"
                    );
                }
            }
        }
    }

    fn record_unsupported(&self, event_type: &str, profile_name: &str) {
        if let Some(metrics) = self.metrics.as_ref() {
            metrics.inc(
                &format!(
                    "unsupported_mapping_total{{event_type=\"{}\",profile=\"{}\"}}",
                    event_type, profile_name
                ),
                1,
            );
        }
    }
}

struct TargetWorker {
    profile_name: String,
    sender: Arc<dyn WebhookSender>,
    url_policy: WebhookUrlPolicy,
    runtime_api: Arc<dyn RuntimeApi>,
    cancel: CancellationToken,
    rx: mpsc::Receiver<WebhookJob>,
    circuit: CircuitBreaker,
}

impl TargetWorker {
    async fn run(mut self) {
        loop {
            select_biased! {
                _ = self.cancel.cancelled().fuse() => break,
                job = self.rx.next() => {
                    if let Some(job) = job {
                        self.process(job).await;
                    } else {
                        break;
                    }
                }
            }
        }
    }

    async fn process(&mut self, job: WebhookJob) {
        if !self.circuit.allow() {
            debug!(
                target = %self.profile_name,
                event_id = %job.event_id,
                "circuit breaker open; dropping webhook"
            );
            return;
        }

        let verdict = match self.url_policy.evaluate(&job.profile.url) {
            Ok(v) => v,
            Err(err) => {
                warn!(
                    target = %self.profile_name,
                    event_id = %job.event_id,
                    %err,
                    "webhook URL denied by policy"
                );
                self.circuit.record_failure();
                return;
            }
        };

        let max_attempts = job.profile.max_retries.saturating_add(1);
        let start = self.runtime_api.now();
        let mut attempts = 0u32;
        let mut succeeded = false;

        while attempts < max_attempts {
            attempts += 1;

            if attempts > 1 {
                let elapsed_ms = self
                    .runtime_api
                    .now()
                    .as_micros()
                    .saturating_sub(start.as_micros())
                    / 1000;
                if elapsed_ms >= job.profile.max_retry_duration_ms {
                    warn!(
                        target = %self.profile_name,
                        event_id = %job.event_id,
                        "webhook retry budget exhausted by total duration"
                    );
                    break;
                }
                let delay_ms = backoff_ms(
                    job.profile.retry_interval_ms,
                    attempts,
                    job.profile.max_retry_duration_ms - elapsed_ms,
                );
                let deadline = self
                    .runtime_api
                    .now()
                    .as_micros()
                    .saturating_add(delay_ms * 1000);
                let mut timer = self
                    .runtime_api
                    .sleep_until(MonoTime::from_micros(deadline));
                let mut timer_fut = async move { timer.wait().await }.boxed().fuse();
                let mut cancel_fut = async { self.cancel.cancelled().await }.boxed().fuse();
                select_biased! {
                    _ = timer_fut => {},
                    _ = cancel_fut => return,
                }
            }

            let body = match build_body(&job, attempts) {
                Ok(b) => b,
                Err(err) => {
                    error!(
                        target = %self.profile_name,
                        event_id = %job.event_id,
                        %err,
                        "failed to serialize webhook body"
                    );
                    break;
                }
            };

            if body.len() > job.profile.max_body_bytes {
                warn!(
                    target = %self.profile_name,
                    event_id = %job.event_id,
                    size = body.len(),
                    limit = job.profile.max_body_bytes,
                    "webhook body exceeds max size; dropping"
                );
                break;
            }

            let mut headers = crate::util::webhook_headers(&job.event_id);

            if let Some(secret) = &job.profile.secret {
                match crate::util::sign_body(&body, secret) {
                    Ok(sig) => {
                        headers.insert("X-Webhook-Signature".to_string(), sig);
                    }
                    Err(err) => {
                        warn!(
                            target = %self.profile_name,
                            event_id = %job.event_id,
                            %err,
                            "failed to sign webhook body"
                        );
                    }
                }
            }

            let request = WebhookHttpRequest {
                verdict: verdict.clone(),
                headers,
                body,
                timeout: job.profile.timeout(),
            };

            match self.sender.send(request).await {
                Ok(response) => {
                    if crate::util::is_success(response.status) {
                        succeeded = true;
                        info!(
                            target = %self.profile_name,
                            event_id = %job.event_id,
                            status = response.status,
                            "webhook delivered"
                        );
                        break;
                    }

                    if !should_retry_status(response.status) {
                        warn!(
                            target = %self.profile_name,
                            event_id = %job.event_id,
                            status = response.status,
                            "webhook rejected by target; not retrying"
                        );
                        break;
                    }

                    warn!(
                        target = %self.profile_name,
                        event_id = %job.event_id,
                        status = response.status,
                        attempt = attempts,
                        "webhook failed transiently"
                    );
                }
                Err(WebhookSendError::Policy(reason)) => {
                    warn!(
                        target = %self.profile_name,
                        event_id = %job.event_id,
                        %reason,
                        "webhook denied by sender policy"
                    );
                    break;
                }
                Err(ref err) if should_retry_error(err) => {
                    warn!(
                        target = %self.profile_name,
                        event_id = %job.event_id,
                        %err,
                        attempt = attempts,
                        "webhook send error; will retry"
                    );
                }
                Err(err) => {
                    warn!(
                        target = %self.profile_name,
                        event_id = %job.event_id,
                        %err,
                        attempt = attempts,
                        "webhook send error; not retrying"
                    );
                    break;
                }
            }
        }

        if succeeded {
            self.circuit.record_success();
        } else {
            self.circuit.record_failure();
        }
    }
}

fn build_body(job: &WebhookJob, attempt: u32) -> Result<Vec<u8>, serde_json::Error> {
    match job.profile.mode {
        WebhookProfileMode::NativeDomain => {
            let envelope = json!({
                "event_id": &job.event_id,
                "event_type": &job.event_type,
                "occurred_at": job.occurred_at,
                "resource": &job.resource,
                "payload": &job.dispatch.payload,
                "attempt": attempt,
            });
            serde_json::to_vec(&envelope)
        }
        WebhookProfileMode::ZlmCompatible => serde_json::to_vec(&job.dispatch.payload),
    }
}

fn backoff_ms(base_ms: u64, attempt: u32, cap_remaining_ms: u64) -> u64 {
    let shift = (attempt.saturating_sub(1)).min(63);
    let raw = base_ms.saturating_mul(1u64.checked_shl(shift).unwrap_or(u64::MAX));
    raw.min(cap_remaining_ms)
}

fn should_retry_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

fn should_retry_error(err: &WebhookSendError) -> bool {
    matches!(
        err,
        WebhookSendError::Io(_) | WebhookSendError::Timeout | WebhookSendError::InvalidResponse
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_each_attempt_and_caps_at_remaining() {
        assert_eq!(backoff_ms(1000, 1, 10_000), 1000);
        assert_eq!(backoff_ms(1000, 2, 10_000), 2000);
        assert_eq!(backoff_ms(1000, 3, 2500), 2500);
    }

    #[test]
    fn should_retry_only_429_and_5xx() {
        assert!(!should_retry_status(200));
        assert!(!should_retry_status(301));
        assert!(!should_retry_status(400));
        assert!(!should_retry_status(404));
        assert!(should_retry_status(429));
        assert!(should_retry_status(500));
        assert!(should_retry_status(503));
    }
}
