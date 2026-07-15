use cheetah_media_api::event::{
    MediaEvent, MediaEventBusApi, MediaEventSender, MediaEventSubscription,
};
use cheetah_runtime_api::RuntimeApi;
use cheetah_sdk::CancellationToken;
use futures::channel::mpsc;
use futures::future::FutureExt;
use futures::select_biased;
use futures::stream::StreamExt;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::circuit::CircuitBreaker;
use crate::config::{WebhookDispatcherConfig, WebhookProfile};
use crate::security::WebhookUrlPolicy;
use crate::sender::{WebhookHttpRequest, WebhookSendError, WebhookSender};
use crate::translator::{WebhookDispatch, WebhookTranslator};

/// A single unit of work for a per-target webhook worker.
///
/// 每个目标 webhook worker 的一个任务单元。
#[derive(Debug, Clone)]
pub struct WebhookJob {
    pub event_id: String,
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
    translator: Arc<dyn WebhookTranslator>,
    url_policy: WebhookUrlPolicy,
}

impl WebhookDispatcher {
    pub fn new(
        config: WebhookDispatcherConfig,
        event_bus: Arc<dyn MediaEventBusApi>,
        runtime_api: Arc<dyn RuntimeApi>,
        sender: Arc<dyn WebhookSender>,
        translator: Arc<dyn WebhookTranslator>,
        url_policy: WebhookUrlPolicy,
    ) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            event_bus,
            runtime_api,
            sender,
            translator,
            url_policy,
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
            translator: self.translator.clone(),
            url_policy: self.url_policy.clone(),
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
    translator: Arc<dyn WebhookTranslator>,
    url_policy: WebhookUrlPolicy,
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
        let dispatches = self.translator.translate(&event);
        let profiles = self.config.read().profiles.clone();

        for dispatch in dispatches {
            for profile in &profiles {
                if !profile.wants_event(&dispatch.hook_name) {
                    continue;
                }

                let event_id = {
                    let mut e = event.clone();
                    e.header_mut().event_id.clone()
                };
                let job = WebhookJob {
                    event_id,
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
                return;
            }
        };

        let body = match serde_json::to_vec(&job.dispatch.payload) {
            Ok(b) => b,
            Err(err) => {
                error!(
                    target = %self.profile_name,
                    event_id = %job.event_id,
                    %err,
                    "failed to serialize webhook body"
                );
                return;
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
            return;
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
            verdict,
            headers,
            body,
            timeout: job.profile.timeout(),
        };

        let mut succeeded = false;
        let mut attempts = 0u32;
        let max_attempts = job.profile.max_retries.saturating_add(1);

        while attempts < max_attempts {
            attempts += 1;
            match self.sender.send(request.clone()).await {
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
                    if crate::util::is_client_error(response.status) {
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
                Err(err) => {
                    warn!(
                        target = %self.profile_name,
                        event_id = %job.event_id,
                        %err,
                        attempt = attempts,
                        "webhook send error"
                    );
                }
            }

            if attempts < max_attempts {
                let deadline = self
                    .runtime_api
                    .now()
                    .as_micros()
                    .saturating_add(job.profile.retry_interval().as_micros() as u64);
                let mut timer = self
                    .runtime_api
                    .sleep_until(cheetah_codec::MonoTime::from_micros(deadline));
                let mut timer_fut = async move { timer.wait().await }.boxed().fuse();
                let mut cancel_fut = self.cancel.cancelled().boxed().fuse();
                select_biased! {
                    _ = timer_fut => {},
                    _ = cancel_fut => return,
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
