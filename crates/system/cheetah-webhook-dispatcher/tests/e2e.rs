use cheetah_media_api::event::{
    EventHeader, MediaEvent, MediaEventBusApi, MediaEventSender, MediaEventSubscription,
    SessionOpened, StreamOnlineChanged,
};
use cheetah_media_api::ids::{AppName, MediaKey, SessionId, StreamName, VhostName};
use cheetah_media_api::model::{OnlineState, SessionKind};
use cheetah_media_api::port::WebhookApi;
use cheetah_runtime_api::RuntimeApi;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_webhook_dispatcher::config::{
    FailurePolicy, WebhookDispatcherConfig, WebhookProfile, WebhookProfileMode,
};
use cheetah_webhook_dispatcher::decision::WebhookDecisionClient;
use cheetah_webhook_dispatcher::dispatcher::WebhookDispatcher;
use cheetah_webhook_dispatcher::security::WebhookUrlPolicy;
use cheetah_webhook_dispatcher::sender::RuntimeHttpClient;
use cheetah_webhook_dispatcher::translator::{
    NativeWebhookTranslator, WebhookTranslator, ZlmWebhookTranslator,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

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

#[derive(Debug, Clone)]
struct Received {
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn read_request(stream: &mut TcpStream) -> Option<Received> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let mut header_end = None;

    while header_end.is_none() {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(idx) = find_substring(&buf, b"\r\n\r\n") {
            header_end = Some(idx);
        }
    }

    let header_end = header_end?;
    let header_block = String::from_utf8_lossy(&buf[..header_end]);
    let mut headers = HashMap::new();
    let mut content_length = 0usize;

    for line in header_block.split("\r\n").skip(1) {
        if let Some((k, v)) = line.split_once(": ") {
            let name = k.to_lowercase();
            if name == "content-length" {
                content_length = v.parse().unwrap_or(0);
            }
            headers.insert(name, v.to_string());
        }
    }

    let body_start = header_end + 4;
    let mut body = if buf.len() > body_start {
        buf[body_start..].to_vec()
    } else {
        Vec::new()
    };

    while body.len() < content_length {
        let mut tmp = [0u8; 1024];
        let to_read = (content_length - body.len()).min(1024);
        let n = stream.read(&mut tmp[..to_read]).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }

    Some(Received { headers, body })
}

fn find_substring(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Spawn a TCP server that accepts `n` connections, records each request, and
/// replies with the pre-defined response.
fn spawn_fixed_responses(
    listener: TcpListener,
    responses: Vec<&'static [u8]>,
) -> mpsc::Receiver<Received> {
    let (tx, rx) = mpsc::channel::<Received>(16);

    tokio::spawn(async move {
        let responses = responses.into_iter();
        for response in responses {
            if let Ok((mut stream, _)) = listener.accept().await {
                if let Some(received) = read_request(&mut stream).await {
                    let _ = tx.send(received).await;
                }
                let _ = stream.write_all(response).await;
                let _ = stream.flush().await;
                let _ = stream.shutdown().await;
            }
        }
    });

    rx
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

fn native_translators() -> HashMap<WebhookProfileMode, Arc<dyn WebhookTranslator>> {
    let mut map: HashMap<WebhookProfileMode, Arc<dyn WebhookTranslator>> = HashMap::new();
    map.insert(
        WebhookProfileMode::NativeDomain,
        Arc::new(NativeWebhookTranslator),
    );
    map.insert(
        WebhookProfileMode::ZlmCompatible,
        Arc::new(ZlmWebhookTranslator),
    );
    map
}

#[tokio::test]
async fn native_notification_arrives_signed_over_real_http() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    let mut rx = spawn_fixed_responses(listener, vec![response.as_slice()]);

    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let bus = Arc::new(FakeEventBus::new());
    let sender: Arc<dyn cheetah_webhook_dispatcher::sender::WebhookSender> =
        Arc::new(RuntimeHttpClient::new(runtime_api));

    let config = WebhookDispatcherConfig {
        profiles: vec![WebhookProfile {
            name: "native".to_string(),
            url: format!("http://127.0.0.1:{}/hook", addr.port()),
            mode: WebhookProfileMode::NativeDomain,
            events: vec!["stream_online_changed".to_string()],
            allowed_cidrs: vec!["127.0.0.1/8".to_string()],
            secret: Some("hunter2".to_string()),
            timeout_ms: 2000,
            retry_interval_ms: 10,
            max_retry_duration_ms: 1_000,
            max_retries: 2,
            ..Default::default()
        }],
    };

    let dispatcher = WebhookDispatcher::new(
        config,
        bus.clone(),
        Arc::new(TokioRuntime::new()),
        sender,
        native_translators(),
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

    let received = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .expect("server received request");

    handle.stop();

    let body = String::from_utf8_lossy(&received.body);
    let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(envelope["event_type"], "stream_online_changed");
    assert_eq!(envelope["resource"]["app"], "live");

    let signature = received
        .headers
        .get("x-webhook-signature")
        .expect("signature present");
    let expected = cheetah_webhook_dispatcher::util::sign_body(&received.body, "hunter2").unwrap();
    assert_eq!(signature, &expected);
}

#[tokio::test]
async fn dispatcher_retries_on_5xx_over_real_http_and_delivers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let first = b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 4\r\n\r\ndown";
    let second = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    let mut rx = spawn_fixed_responses(listener, vec![first.as_slice(), second.as_slice()]);

    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let bus = Arc::new(FakeEventBus::new());
    let sender: Arc<dyn cheetah_webhook_dispatcher::sender::WebhookSender> =
        Arc::new(RuntimeHttpClient::new(runtime_api));

    let config = WebhookDispatcherConfig {
        profiles: vec![WebhookProfile {
            name: "native".to_string(),
            url: format!("http://127.0.0.1:{}/hook", addr.port()),
            mode: WebhookProfileMode::NativeDomain,
            events: vec!["stream_online_changed".to_string()],
            allowed_cidrs: vec!["127.0.0.1/8".to_string()],
            retry_interval_ms: 50,
            max_retry_duration_ms: 2_000,
            max_retries: 3,
            ..Default::default()
        }],
    };

    let dispatcher = WebhookDispatcher::new(
        config,
        bus.clone(),
        Arc::new(TokioRuntime::new()),
        sender,
        native_translators(),
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

    let first_recv = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let second_recv = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();

    handle.stop();

    let first_envelope: serde_json::Value = serde_json::from_slice(&first_recv.body).unwrap();
    let second_envelope: serde_json::Value = serde_json::from_slice(&second_recv.body).unwrap();
    assert_eq!(first_envelope["event_id"], second_envelope["event_id"]);
    assert_eq!(first_envelope["event_type"], "stream_online_changed");
    assert_eq!(second_envelope["event_type"], "stream_online_changed");
    assert_eq!(first_envelope["attempt"], 1);
    assert_eq!(second_envelope["attempt"], 2);
}

fn play_event() -> MediaEvent {
    MediaEvent::SessionOpened(SessionOpened {
        header: sample_header(),
        kind: SessionKind::Player,
        protocol: "rtmp".to_string(),
        remote_endpoint: Some("10.0.0.1:1935".to_string()),
        session_id: SessionId("s1".to_string()),
    })
}

fn decision_client_for(port: u16, timeout_ms: u64, policy: FailurePolicy) -> WebhookDecisionClient {
    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let sender: Arc<dyn cheetah_webhook_dispatcher::sender::WebhookSender> =
        Arc::new(RuntimeHttpClient::new(runtime_api));

    let config = WebhookDispatcherConfig {
        profiles: vec![WebhookProfile {
            name: "decision".to_string(),
            url: format!("http://127.0.0.1:{}/on_play", port),
            mode: WebhookProfileMode::ZlmCompatible,
            events: Vec::new(),
            decision_events: vec!["on_play".to_string()],
            allowed_cidrs: vec!["127.0.0.1/8".to_string()],
            decision_timeout_ms: timeout_ms,
            decision_failure_policy: policy,
            ..Default::default()
        }],
    };

    WebhookDecisionClient::new(
        config,
        sender,
        Arc::new(ZlmWebhookTranslator),
        WebhookUrlPolicy::default(),
    )
}

#[tokio::test]
async fn decision_webhook_allows_on_code_zero() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 26\r\n\r\n{\"code\":0,\"msg\":\"allowed\"}";
    let mut rx = spawn_fixed_responses(listener, vec![response.as_slice()]);

    let client = decision_client_for(addr.port(), 2000, FailurePolicy::Deny);
    let decision = client.request_decision(play_event()).await.unwrap();
    assert_eq!(decision, cheetah_media_api::Decision::Allow);

    let received = rx.recv().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&received.body).unwrap();
    assert_eq!(body["app"], "live");
    assert_eq!(body["stream"], "test");
    assert_eq!(body["protocol"], "rtmp");
    assert_eq!(body["kind"], "Player");
}

#[tokio::test]
async fn decision_webhook_denies_on_non_zero_code() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let response =
        b"HTTP/1.1 200 OK\r\nContent-Length: 29\r\n\r\n{\"code\":-1,\"msg\":\"forbidden\"}";
    let mut rx = spawn_fixed_responses(listener, vec![response.as_slice()]);

    let client = decision_client_for(addr.port(), 2000, FailurePolicy::Deny);
    let decision = client.request_decision(play_event()).await.unwrap();
    assert!(matches!(decision, cheetah_media_api::Decision::Deny { .. }));

    let received = rx.recv().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&received.body).unwrap();
    assert_eq!(body["app"], "live");
    assert_eq!(body["stream"], "test");
    assert_eq!(body["protocol"], "rtmp");
    assert_eq!(body["kind"], "Player");
}

#[tokio::test]
async fn decision_webhook_timeout_uses_failure_policy() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Server accepts but never responds, causing a timeout.
    tokio::spawn(async move {
        let _ = listener.accept().await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let client = decision_client_for(addr.port(), 50, FailurePolicy::Allow);
    let decision = client.request_decision(play_event()).await.unwrap();
    assert_eq!(decision, cheetah_media_api::Decision::Allow);
}

#[tokio::test]
async fn decision_webhook_timeout_fails_closed() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let _ = listener.accept().await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let client = decision_client_for(addr.port(), 50, FailurePolicy::Deny);
    let decision = client.request_decision(play_event()).await.unwrap();
    assert!(matches!(decision, cheetah_media_api::Decision::Deny { .. }));
}
