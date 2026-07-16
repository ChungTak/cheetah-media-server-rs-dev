use cheetah_media_api::event::{
    EventHeader, MediaEvent, MediaEventBusApi, MediaEventSender, MediaEventSubscription,
    RecordCompleted, SnapshotCompleted, StreamOnlineChanged,
};
use cheetah_media_api::ids::{
    AppName, MediaKey, RecordTaskId, SessionId, SnapshotId, StreamName, VhostName,
};
use cheetah_media_api::model::{OnlineState, SessionKind};
use cheetah_runtime_api::RuntimeApi;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_webhook_dispatcher::config::{
    WebhookDispatcherConfig, WebhookProfile, WebhookProfileMode,
};
use cheetah_webhook_dispatcher::dispatcher::WebhookDispatcher;
use cheetah_webhook_dispatcher::security::WebhookUrlPolicy;
use cheetah_webhook_dispatcher::sender::{
    WebhookHttpRequest, WebhookResponse, WebhookSendError, WebhookSender,
};
use cheetah_webhook_dispatcher::translator::{
    NativeWebhookTranslator, WebhookTranslator, ZlmWebhookTranslator,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

struct FakeSubscription;

impl MediaEventSubscription for FakeSubscription {
    fn id(&self) -> String {
        "sub-1".to_string()
    }

    fn unsubscribe(&self) -> cheetah_media_api::error::Result<()> {
        Ok(())
    }
}

struct FakeEventBus {
    subscribers: Mutex<Vec<Box<dyn MediaEventSender>>>,
}

impl FakeEventBus {
    fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
        }
    }
}

impl MediaEventBusApi for FakeEventBus {
    fn publish(&self, event: MediaEvent) -> cheetah_media_api::error::Result<()> {
        for sub in self.subscribers.lock().iter() {
            let _ = sub.send(event.clone());
        }
        Ok(())
    }

    fn subscribe(
        &self,
        sender: Box<dyn MediaEventSender>,
        _capacity: usize,
    ) -> cheetah_media_api::error::Result<Box<dyn MediaEventSubscription>> {
        self.subscribers.lock().push(sender);
        Ok(Box::new(FakeSubscription))
    }

    fn unsubscribe(&self, _id: &str) -> cheetah_media_api::error::Result<()> {
        Ok(())
    }
}

use std::collections::VecDeque;

struct RecordingSender {
    requests: Arc<Mutex<Vec<WebhookHttpRequest>>>,
    responses: Mutex<VecDeque<Result<WebhookResponse, WebhookSendError>>>,
}

impl RecordingSender {
    fn new(responses: Vec<Result<WebhookResponse, WebhookSendError>>) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            responses: Mutex::new(responses.into_iter().collect()),
        }
    }
}

#[async_trait::async_trait]
impl WebhookSender for RecordingSender {
    async fn send(&self, request: WebhookHttpRequest) -> Result<WebhookResponse, WebhookSendError> {
        self.requests.lock().push(request);
        self.responses
            .lock()
            .pop_front()
            .unwrap_or(Err(WebhookSendError::Timeout))
    }
}

fn zlm_profile(name: &str, events: Vec<&str>) -> WebhookProfile {
    WebhookProfile {
        name: name.to_string(),
        url: "http://127.0.0.1/hook".to_string(),
        mode: WebhookProfileMode::ZlmCompatible,
        events: events.into_iter().map(|s| s.to_string()).collect(),
        allowed_cidrs: vec!["127.0.0.1/32".to_string()],
        ..Default::default()
    }
}

fn native_profile(name: &str, events: Vec<&str>) -> WebhookProfile {
    WebhookProfile {
        name: name.to_string(),
        url: "http://127.0.0.1/hook".to_string(),
        mode: WebhookProfileMode::NativeDomain,
        events: events.into_iter().map(|s| s.to_string()).collect(),
        allowed_cidrs: vec!["127.0.0.1/32".to_string()],
        ..Default::default()
    }
}

fn translators() -> HashMap<WebhookProfileMode, Arc<dyn WebhookTranslator>> {
    let mut map: HashMap<WebhookProfileMode, Arc<dyn WebhookTranslator>> = HashMap::new();
    map.insert(
        WebhookProfileMode::ZlmCompatible,
        Arc::new(ZlmWebhookTranslator),
    );
    map.insert(
        WebhookProfileMode::NativeDomain,
        Arc::new(NativeWebhookTranslator),
    );
    map
}

fn sample_header() -> EventHeader {
    EventHeader {
        event_id: "evt-1".to_string(),
        occurred_at: 1,
        sequence: None,
        media_key: Some(MediaKey {
            vhost: VhostName("__defaultVhost__".to_string()),
            app: AppName("live".to_string()),
            stream: StreamName("test".to_string()),
            schema: None,
        }),
        source: "test".to_string(),
        correlation_id: None,
    }
}

#[tokio::test]
async fn zlm_dispatcher_translates_and_sends_event_to_matching_profile() {
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let bus = Arc::new(FakeEventBus::new());

    let recording = Arc::new(RecordingSender::new(vec![Ok(WebhookResponse {
        status: 200,
        body: "ok".to_string(),
        duration_ms: 1,
    })]));

    let config = WebhookDispatcherConfig {
        profiles: vec![zlm_profile("zlm", vec!["on_stream_changed"])],
    };

    let dispatcher = WebhookDispatcher::new(
        config,
        bus.clone(),
        runtime_api,
        recording.clone(),
        translators(),
        WebhookUrlPolicy::default(),
        None,
    );

    let handle = dispatcher.start(8).unwrap();

    bus.publish(MediaEvent::StreamOnlineChanged(StreamOnlineChanged {
        header: sample_header(),
        online: OnlineState::Online,
        schema: None,
    }))
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.stop();

    let requests = recording.requests.lock();
    assert_eq!(requests.len(), 1);
    let body = String::from_utf8_lossy(&requests[0].body);
    let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(payload["app"], "live");
    assert_eq!(payload["stream"], "test");
    assert_eq!(payload["regist"], true);
}

#[tokio::test]
async fn zlm_dispatcher_skips_event_not_in_profile() {
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let bus = Arc::new(FakeEventBus::new());

    let recording = Arc::new(RecordingSender::new(vec![Ok(WebhookResponse {
        status: 200,
        body: "ok".to_string(),
        duration_ms: 1,
    })]));

    let config = WebhookDispatcherConfig {
        profiles: vec![zlm_profile("zlm", vec!["on_publish"])],
    };

    let dispatcher = WebhookDispatcher::new(
        config,
        bus.clone(),
        runtime_api,
        recording.clone(),
        translators(),
        WebhookUrlPolicy::default(),
        None,
    );

    let handle = dispatcher.start(8).unwrap();

    let header = sample_header();
    bus.publish(MediaEvent::SessionOpened(
        cheetah_media_api::event::SessionOpened {
            header,
            kind: SessionKind::Player,
            protocol: "rtsp".to_string(),
            remote_endpoint: Some("10.0.0.1:554".to_string()),
            session_id: SessionId("s1".to_string()),
        },
    ))
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.stop();

    let requests = recording.requests.lock();
    assert!(requests.is_empty());
}

#[tokio::test]
async fn native_dispatcher_sends_signed_envelope_with_attempt() {
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let bus = Arc::new(FakeEventBus::new());

    let recording = Arc::new(RecordingSender::new(vec![Ok(WebhookResponse {
        status: 200,
        body: "ok".to_string(),
        duration_ms: 1,
    })]));

    let config = WebhookDispatcherConfig {
        profiles: vec![WebhookProfile {
            name: "native".to_string(),
            url: "http://127.0.0.1/hook".to_string(),
            mode: WebhookProfileMode::NativeDomain,
            events: vec!["stream_online_changed".to_string()],
            allowed_cidrs: vec!["127.0.0.1/32".to_string()],
            secret: Some("hunter2".to_string()),
            ..Default::default()
        }],
    };

    let dispatcher = WebhookDispatcher::new(
        config,
        bus.clone(),
        runtime_api,
        recording.clone(),
        translators(),
        WebhookUrlPolicy::default(),
        None,
    );

    let handle = dispatcher.start(8).unwrap();

    bus.publish(MediaEvent::StreamOnlineChanged(StreamOnlineChanged {
        header: sample_header(),
        online: OnlineState::Online,
        schema: None,
    }))
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.stop();

    let requests = recording.requests.lock();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    let body = String::from_utf8_lossy(&req.body);
    let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(envelope["event_id"], "evt-1");
    assert_eq!(envelope["event_type"], "stream_online_changed");
    assert_eq!(envelope["occurred_at"], 1);
    assert_eq!(envelope["attempt"], 1);
    assert_eq!(envelope["resource"]["app"], "live");
    assert_eq!(envelope["payload"]["online"], "online");

    let signature = req
        .headers
        .get("X-Webhook-Signature")
        .expect("signature header present");
    let expected =
        cheetah_webhook_dispatcher::util::sign_body(req.body.as_slice(), "hunter2").unwrap();
    assert_eq!(signature, &expected);
}

#[tokio::test]
async fn dispatcher_retries_on_5xx_and_429() {
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let bus = Arc::new(FakeEventBus::new());

    let recording = Arc::new(RecordingSender::new(vec![
        Ok(WebhookResponse {
            status: 503,
            body: "down".to_string(),
            duration_ms: 1,
        }),
        Ok(WebhookResponse {
            status: 429,
            body: "slow".to_string(),
            duration_ms: 1,
        }),
        Ok(WebhookResponse {
            status: 200,
            body: "ok".to_string(),
            duration_ms: 1,
        }),
    ]));

    let config = WebhookDispatcherConfig {
        profiles: vec![WebhookProfile {
            name: "retry".to_string(),
            url: "http://127.0.0.1/hook".to_string(),
            mode: WebhookProfileMode::ZlmCompatible,
            events: vec!["on_stream_changed".to_string()],
            allowed_cidrs: vec!["127.0.0.1/32".to_string()],
            retry_interval_ms: 10,
            max_retry_duration_ms: 5_000,
            max_retries: 3,
            ..Default::default()
        }],
    };

    let dispatcher = WebhookDispatcher::new(
        config,
        bus.clone(),
        runtime_api,
        recording.clone(),
        translators(),
        WebhookUrlPolicy::default(),
        None,
    );

    let handle = dispatcher.start(8).unwrap();

    bus.publish(MediaEvent::StreamOnlineChanged(StreamOnlineChanged {
        header: sample_header(),
        online: OnlineState::Online,
        schema: None,
    }))
    .unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.stop();

    let requests = recording.requests.lock();
    assert_eq!(requests.len(), 3);
}

#[tokio::test]
async fn dispatcher_does_not_retry_4xx_other_than_429() {
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let bus = Arc::new(FakeEventBus::new());

    let recording = Arc::new(RecordingSender::new(vec![Ok(WebhookResponse {
        status: 404,
        body: "not found".to_string(),
        duration_ms: 1,
    })]));

    let config = WebhookDispatcherConfig {
        profiles: vec![zlm_profile("zlm", vec!["on_stream_changed"])],
    };

    let dispatcher = WebhookDispatcher::new(
        config,
        bus.clone(),
        runtime_api,
        recording.clone(),
        translators(),
        WebhookUrlPolicy::default(),
        None,
    );

    let handle = dispatcher.start(8).unwrap();

    bus.publish(MediaEvent::StreamOnlineChanged(StreamOnlineChanged {
        header: sample_header(),
        online: OnlineState::Online,
        schema: None,
    }))
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.stop();

    let requests = recording.requests.lock();
    assert_eq!(requests.len(), 1);
}

struct MetricsRecorder {
    counters: Mutex<Vec<(String, u64)>>,
}

impl cheetah_sdk::MetricsApi for MetricsRecorder {
    fn inc(&self, key: &str, value: u64) {
        self.counters.lock().push((key.to_string(), value));
    }

    fn render(&self) -> String {
        String::new()
    }
}

#[tokio::test]
async fn zlm_dispatcher_records_unsupported_mapping_metric() {
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let bus = Arc::new(FakeEventBus::new());

    let metrics = Arc::new(MetricsRecorder {
        counters: Mutex::new(Vec::new()),
    });

    let recording = Arc::new(RecordingSender::new(vec![]));

    // ProxyStateChanged has no ZLM mapping, so it should record an unsupported
    // mapping metric when the profile is in ZLM-compatible mode.
    let config = WebhookDispatcherConfig {
        profiles: vec![zlm_profile("zlm", vec![])],
    };

    let dispatcher = WebhookDispatcher::new(
        config,
        bus.clone(),
        runtime_api,
        recording.clone(),
        translators(),
        WebhookUrlPolicy::default(),
        Some(metrics.clone()),
    );

    let handle = dispatcher.start(8).unwrap();

    bus.publish(MediaEvent::ProxyStateChanged(
        cheetah_media_api::event::ProxyStateChanged {
            header: sample_header(),
            proxy_id: cheetah_media_api::ids::ProxyId("p1".to_string()),
            state: cheetah_media_api::model::ProxyState::Created,
            last_error: None,
        },
    ))
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.stop();

    let counters = metrics.counters.lock();
    assert!(counters
        .iter()
        .any(|(k, _)| k.contains("unsupported_mapping_total")));
    assert!(counters
        .iter()
        .any(|(k, _)| k.contains("proxy_state_changed")));
    assert!(counters.iter().any(|(k, _)| k.contains("zlm")));
}

#[tokio::test]
async fn dispatcher_delivers_snapshot_completed_and_record_completed() {
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let bus = Arc::new(FakeEventBus::new());

    let recording = Arc::new(RecordingSender::new(vec![Ok(WebhookResponse {
        status: 200,
        body: "ok".to_string(),
        duration_ms: 1,
    })]));

    let config = WebhookDispatcherConfig {
        profiles: vec![native_profile("native", vec![])],
    };

    let dispatcher = WebhookDispatcher::new(
        config,
        bus.clone(),
        runtime_api,
        recording.clone(),
        translators(),
        WebhookUrlPolicy::default(),
        None,
    );

    let handle = dispatcher.start(8).unwrap();

    let snapshot_header = sample_header();
    bus.publish(MediaEvent::SnapshotCompleted(SnapshotCompleted {
        header: snapshot_header,
        snapshot_id: SnapshotId("snap-1".to_string()),
        path_handle: cheetah_media_api::ids::FileHandle("/tmp/1.jpg".to_string()),
        url: Some("http://x/1.jpg".to_string()),
        format: "jpg".to_string(),
        width: 1920,
        height: 1080,
        size_bytes: 1234,
    }))
    .unwrap();

    let record_header = sample_header();
    bus.publish(MediaEvent::RecordCompleted(RecordCompleted {
        header: record_header,
        task_id: RecordTaskId("task-1".to_string()),
        format: "mp4".to_string(),
        file_path: "/tmp/1.mp4".to_string(),
        file_size: 1024,
        time_len_ms: 15000,
        folder: "/tmp".to_string(),
        url: Some("http://x/1.mp4".to_string()),
    }))
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.stop();

    let requests = recording.requests.lock();
    assert_eq!(requests.len(), 2);

    let bodies: Vec<serde_json::Value> = requests
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    let types: Vec<&str> = bodies
        .iter()
        .map(|b| b["event_type"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"snapshot_completed"));
    assert!(types.contains(&"record_completed"));
}
