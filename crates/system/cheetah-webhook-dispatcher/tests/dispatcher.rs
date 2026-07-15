use cheetah_media_api::event::{
    MediaEvent, MediaEventBusApi, MediaEventSender, MediaEventSubscription,
};
use cheetah_media_api::ids::{AppName, MediaKey, SessionId, StreamName, VhostName};
use cheetah_media_api::model::{OnlineState, SessionKind};
use cheetah_runtime_api::RuntimeApi;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_webhook_dispatcher::config::{WebhookDispatcherConfig, WebhookProfile};
use cheetah_webhook_dispatcher::dispatcher::WebhookDispatcher;
use cheetah_webhook_dispatcher::security::WebhookUrlPolicy;
use cheetah_webhook_dispatcher::sender::{
    WebhookHttpRequest, WebhookResponse, WebhookSendError, WebhookSender,
};
use cheetah_webhook_dispatcher::translator::ZlmWebhookTranslator;
use parking_lot::Mutex;
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

struct RecordingSender {
    requests: Arc<Mutex<Vec<WebhookHttpRequest>>>,
    response: WebhookResponse,
}

impl RecordingSender {
    fn new(response: WebhookResponse) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            response,
        }
    }
}

#[async_trait::async_trait]
impl WebhookSender for RecordingSender {
    async fn send(&self, request: WebhookHttpRequest) -> Result<WebhookResponse, WebhookSendError> {
        self.requests.lock().push(request);
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn dispatcher_translates_and_sends_event_to_matching_profile() {
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let bus = Arc::new(FakeEventBus::new());

    let recording = Arc::new(RecordingSender::new(WebhookResponse {
        status: 200,
        body: "ok".to_string(),
        duration_ms: 1,
    }));

    let config = WebhookDispatcherConfig {
        profiles: vec![WebhookProfile {
            name: "zlm".to_string(),
            url: "http://127.0.0.1/hook".to_string(),
            events: vec!["on_stream_changed".to_string()],
            allowed_cidrs: vec!["127.0.0.1/32".to_string()],
            ..Default::default()
        }],
    };

    let dispatcher = WebhookDispatcher::new(
        config,
        bus.clone(),
        runtime_api,
        recording.clone(),
        Arc::new(ZlmWebhookTranslator),
        WebhookUrlPolicy::default(),
    );

    let handle = dispatcher.start(8).unwrap();

    let header = cheetah_media_api::event::EventHeader {
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
    };

    bus.publish(MediaEvent::StreamOnlineChanged(
        cheetah_media_api::event::StreamOnlineChanged {
            header,
            online: OnlineState::Online,
            schema: None,
        },
    ))
    .unwrap();

    // Give the dispatcher a moment to forward the event.
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
async fn dispatcher_skips_event_not_in_profile() {
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let bus = Arc::new(FakeEventBus::new());

    let recording = Arc::new(RecordingSender::new(WebhookResponse {
        status: 200,
        body: "ok".to_string(),
        duration_ms: 1,
    }));

    let config = WebhookDispatcherConfig {
        profiles: vec![WebhookProfile {
            name: "zlm".to_string(),
            url: "http://127.0.0.1/hook".to_string(),
            events: vec!["on_publish".to_string()],
            allowed_cidrs: vec!["127.0.0.1/32".to_string()],
            ..Default::default()
        }],
    };

    let dispatcher = WebhookDispatcher::new(
        config,
        bus.clone(),
        runtime_api,
        recording.clone(),
        Arc::new(ZlmWebhookTranslator),
        WebhookUrlPolicy::default(),
    );

    let handle = dispatcher.start(8).unwrap();

    let header = cheetah_media_api::event::EventHeader {
        event_id: "evt-2".to_string(),
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
    };

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
