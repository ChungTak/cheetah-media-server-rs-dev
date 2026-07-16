use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cheetah_engine::EngineMediaFacade;
use cheetah_media_api::command::{
    PublishRequest, PullProxyRequest, PushProxyRequest, RetryPolicy, RtpReceiverRequest,
    RtpSenderRequest, SubscribeRequest,
};
use cheetah_media_api::error::{MediaErrorCode, Result as MediaResult};
use cheetah_media_api::event::{
    MediaEvent, MediaEventBusApi, MediaEventSender, MediaEventSubscription,
};
use cheetah_media_api::ids::{MediaKey, MediaSchema, ProxyId, RtpSessionId, SessionId};
use cheetah_media_api::model::{
    AdmissionAction, AdmissionRequest, Decision, MediaUrl, ProxyInfo, ProxyKind, ProxyState,
    PublisherHandle, RtpSession, RtpSessionKind, RtpSessionState, SubscriberHandle,
};
use cheetah_media_api::port::{
    MediaAdmissionApi, MediaRequestContext, ProxyApi, PublishSubscribeApi, RtpApi,
};
use cheetah_sdk::MediaServices;

struct NoopEventBus;

impl MediaEventBusApi for NoopEventBus {
    fn publish(&self, _event: MediaEvent) -> MediaResult<()> {
        Ok(())
    }

    fn subscribe(
        &self,
        _sender: Box<dyn MediaEventSender>,
        _capacity: usize,
    ) -> MediaResult<Box<dyn MediaEventSubscription>> {
        Ok(Box::new(NoopSubscription))
    }

    fn unsubscribe(&self, _id: &str) -> MediaResult<()> {
        Ok(())
    }
}

struct NoopSubscription;

impl MediaEventSubscription for NoopSubscription {
    fn id(&self) -> String {
        "noop".to_string()
    }

    fn unsubscribe(&self) -> MediaResult<()> {
        Ok(())
    }
}

#[derive(Default)]
struct FakePublishSubscribe {
    called: Arc<AtomicBool>,
}

#[async_trait]
impl PublishSubscribeApi for FakePublishSubscribe {
    async fn acquire_publisher(
        &self,
        _ctx: &MediaRequestContext,
        request: PublishRequest,
    ) -> MediaResult<PublisherHandle> {
        self.called.store(true, Ordering::SeqCst);
        Ok(PublisherHandle {
            session_id: SessionId("pub".to_string()),
            media_key: request.media_key,
            lease_token: "token".to_string(),
        })
    }

    async fn open_subscriber(
        &self,
        _ctx: &MediaRequestContext,
        request: SubscribeRequest,
    ) -> MediaResult<SubscriberHandle> {
        self.called.store(true, Ordering::SeqCst);
        Ok(SubscriberHandle {
            session_id: SessionId("sub".to_string()),
            media_key: request.media_key,
            output_schema: request.output_schema,
            url: None,
        })
    }

    async fn close_handle(
        &self,
        _ctx: &MediaRequestContext,
        _id: &SessionId,
        _reason: cheetah_media_api::model::CloseReason,
    ) -> MediaResult<()> {
        Ok(())
    }
}

#[derive(Default)]
struct FakeProxy {
    called: Arc<AtomicBool>,
}

#[async_trait]
impl ProxyApi for FakeProxy {
    async fn create_pull_proxy(
        &self,
        _ctx: &MediaRequestContext,
        request: PullProxyRequest,
    ) -> MediaResult<ProxyInfo> {
        self.called.store(true, Ordering::SeqCst);
        Ok(ProxyInfo {
            proxy_id: ProxyId("pull-1".to_string()),
            kind: ProxyKind::Pull,
            source: request.source_url,
            destination: request.destination,
            state: ProxyState::Created,
            retry_count: 0,
            last_error: None,
            created_at: 0,
            updated_at: 0,
            output_urls: Vec::<MediaUrl>::new(),
        })
    }

    async fn create_push_proxy(
        &self,
        _ctx: &MediaRequestContext,
        request: PushProxyRequest,
    ) -> MediaResult<ProxyInfo> {
        self.called.store(true, Ordering::SeqCst);
        Ok(ProxyInfo {
            proxy_id: ProxyId("push-1".to_string()),
            kind: ProxyKind::Push,
            source: request.destination_url,
            destination: request.source_media_key,
            state: ProxyState::Created,
            retry_count: 0,
            last_error: None,
            created_at: 0,
            updated_at: 0,
            output_urls: Vec::<MediaUrl>::new(),
        })
    }

    async fn delete_pull_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _id: &ProxyId,
    ) -> MediaResult<()> {
        Ok(())
    }

    async fn list_pull_proxies(
        &self,
        _ctx: &MediaRequestContext,
        mut _query: cheetah_media_api::command::ProxyQuery,
    ) -> MediaResult<cheetah_media_api::model::Page<ProxyInfo>> {
        unimplemented!()
    }

    async fn get_pull_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _id: &ProxyId,
    ) -> MediaResult<ProxyInfo> {
        unimplemented!()
    }

    async fn list_push_proxies(
        &self,
        _ctx: &MediaRequestContext,
        mut _query: cheetah_media_api::command::ProxyQuery,
    ) -> MediaResult<cheetah_media_api::model::Page<ProxyInfo>> {
        unimplemented!()
    }

    async fn get_push_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _id: &ProxyId,
    ) -> MediaResult<ProxyInfo> {
        unimplemented!()
    }

    async fn delete_push_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _id: &ProxyId,
    ) -> MediaResult<()> {
        Ok(())
    }

    async fn create_ffmpeg_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _request: cheetah_media_api::command::FfmpegProxyRequest,
    ) -> MediaResult<ProxyInfo> {
        unimplemented!()
    }

    async fn delete_ffmpeg_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _id: &ProxyId,
    ) -> MediaResult<()> {
        Ok(())
    }

    async fn get_ffmpeg_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _id: &ProxyId,
    ) -> MediaResult<ProxyInfo> {
        unimplemented!()
    }

    async fn list_ffmpeg_proxies(
        &self,
        _ctx: &MediaRequestContext,
        mut _query: cheetah_media_api::command::ProxyQuery,
    ) -> MediaResult<cheetah_media_api::model::Page<ProxyInfo>> {
        unimplemented!()
    }
}

#[derive(Default)]
struct FakeRtp {
    called: Arc<AtomicBool>,
}

#[async_trait]
impl RtpApi for FakeRtp {
    async fn open_rtp_receiver(
        &self,
        _ctx: &MediaRequestContext,
        request: RtpReceiverRequest,
    ) -> MediaResult<RtpSession> {
        self.called.store(true, Ordering::SeqCst);
        Ok(make_rtp_session(
            request.media_key,
            RtpSessionKind::Receiver,
        ))
    }

    async fn connect_rtp_receiver(
        &self,
        _ctx: &MediaRequestContext,
        _request: cheetah_media_api::command::RtpConnectRequest,
    ) -> MediaResult<RtpSession> {
        unimplemented!()
    }

    async fn open_rtp_sender(
        &self,
        _ctx: &MediaRequestContext,
        request: RtpSenderRequest,
    ) -> MediaResult<RtpSession> {
        self.called.store(true, Ordering::SeqCst);
        Ok(make_rtp_session(request.media_key, RtpSessionKind::Sender))
    }

    async fn stop_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &cheetah_media_api::ids::RtpSessionId,
    ) -> MediaResult<()> {
        Ok(())
    }

    async fn list_rtp_sessions(
        &self,
        _ctx: &MediaRequestContext,
        mut _query: cheetah_media_api::command::RtpQuery,
    ) -> MediaResult<cheetah_media_api::model::Page<RtpSession>> {
        unimplemented!()
    }

    async fn update_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        _request: cheetah_media_api::command::UpdateRtpRequest,
    ) -> MediaResult<RtpSession> {
        unimplemented!()
    }

    async fn get_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &cheetah_media_api::ids::RtpSessionId,
    ) -> MediaResult<RtpSession> {
        unimplemented!()
    }
}

fn make_rtp_session(media_key: MediaKey, kind: RtpSessionKind) -> RtpSession {
    RtpSession {
        session_id: RtpSessionId("rtp-1".to_string()),
        kind,
        media_key,
        local_port: None,
        remote_endpoint: None,
        ssrc: None,
        payload_type: None,
        tcp_mode: None,
        reuse_port: false,
        state: RtpSessionState::Created,
        check_paused: false,
        generation: 1,
        created_at: 0,
        updated_at: 0,
        last_error: None,
    }
}

#[derive(Clone)]
struct FakeAdmission {
    decision: Decision,
    requests: Arc<Mutex<Vec<AdmissionRequest>>>,
}

impl FakeAdmission {
    fn new(decision: Decision) -> Self {
        Self {
            decision,
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn take_requests(&self) -> Vec<AdmissionRequest> {
        self.requests.lock().unwrap().drain(..).collect()
    }
}

#[async_trait]
impl MediaAdmissionApi for FakeAdmission {
    async fn authorize(
        &self,
        _ctx: &MediaRequestContext,
        request: AdmissionRequest,
    ) -> MediaResult<Decision> {
        self.requests.lock().unwrap().push(request);
        Ok(self.decision.clone())
    }
}

fn key() -> MediaKey {
    MediaKey::with_default_vhost("live", "test", None).expect("valid key")
}

fn ctx() -> MediaRequestContext {
    MediaRequestContext::default()
}

fn facade(
    admission: Option<Arc<dyn MediaAdmissionApi>>,
    ps: Arc<dyn PublishSubscribeApi>,
    proxy: Arc<dyn ProxyApi>,
    rtp: Arc<dyn RtpApi>,
) -> EngineMediaFacade {
    let services = MediaServices::unavailable();
    services.register_publish_subscribe(ps);
    services.register_proxy(proxy);
    services.register_rtp(rtp);
    if let Some(a) = admission {
        services.register_admission(a);
    }
    EngineMediaFacade::new(services, Arc::new(NoopEventBus))
}

fn deny() -> Decision {
    Decision::Deny {
        code: MediaErrorCode::PermissionDenied,
        reason: "not allowed".to_string(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn admission_deny_blocks_publisher_and_leaves_no_lease() {
    let admission = Arc::new(FakeAdmission::new(deny()));
    let ps = Arc::new(FakePublishSubscribe::default());
    let proxy = Arc::new(FakeProxy::default());
    let rtp = Arc::new(FakeRtp::default());
    let facade = facade(
        Some(admission.clone()),
        ps.clone(),
        proxy.clone(),
        rtp.clone(),
    );

    let err = facade
        .acquire_publisher(
            &ctx(),
            PublishRequest {
                media_key: key(),
                protocol: "rtmp".to_string(),
                origin: None,
                remote_endpoint: Some("1.2.3.4:1234".to_string()),
                lease_token: None,
                auth_context: HashMap::new(),
                metadata: HashMap::new(),
            },
        )
        .await
        .expect_err("denied publisher should fail");

    assert_eq!(err.code, MediaErrorCode::PermissionDenied);
    assert!(
        !ps.called.load(Ordering::SeqCst),
        "provider must not be called after deny"
    );

    let requests = admission.take_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].action, AdmissionAction::Publish);
    assert_eq!(requests[0].resource, key());
}

#[tokio::test(flavor = "current_thread")]
async fn admission_allow_lets_publisher_proceed() {
    let admission = Arc::new(FakeAdmission::new(Decision::Allow));
    let ps = Arc::new(FakePublishSubscribe::default());
    let proxy = Arc::new(FakeProxy::default());
    let rtp = Arc::new(FakeRtp::default());
    let facade = facade(
        Some(admission.clone()),
        ps.clone(),
        proxy.clone(),
        rtp.clone(),
    );

    let _handle = facade
        .acquire_publisher(
            &ctx(),
            PublishRequest {
                media_key: key(),
                protocol: "rtmp".to_string(),
                origin: None,
                remote_endpoint: None,
                lease_token: None,
                auth_context: HashMap::new(),
                metadata: HashMap::new(),
            },
        )
        .await
        .expect("allowed publisher should succeed");

    assert!(ps.called.load(Ordering::SeqCst));
    assert_eq!(admission.take_requests().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn admission_deny_blocks_subscriber() {
    let admission = Arc::new(FakeAdmission::new(deny()));
    let ps = Arc::new(FakePublishSubscribe::default());
    let proxy = Arc::new(FakeProxy::default());
    let rtp = Arc::new(FakeRtp::default());
    let facade = facade(Some(admission), ps.clone(), proxy.clone(), rtp.clone());

    let err = facade
        .open_subscriber(
            &ctx(),
            SubscribeRequest {
                media_key: key(),
                output_schema: MediaSchema::Hls,
                subscriber_kind: "".to_string(),
                start_policy: "".to_string(),
                auth_context: HashMap::new(),
            },
        )
        .await
        .expect_err("denied subscriber should fail");

    assert_eq!(err.code, MediaErrorCode::PermissionDenied);
    assert!(!ps.called.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn admission_deny_blocks_pull_proxy() {
    let admission = Arc::new(FakeAdmission::new(deny()));
    let ps = Arc::new(FakePublishSubscribe::default());
    let proxy = Arc::new(FakeProxy::default());
    let rtp = Arc::new(FakeRtp::default());
    let facade = facade(Some(admission), ps.clone(), proxy.clone(), rtp.clone());

    let err = facade
        .create_pull_proxy(
            &ctx(),
            PullProxyRequest {
                source_url: "http://example.com/live.flv".to_string(),
                destination: key(),
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_delay_ms: 0,
                    max_retry_delay_ms: 0,
                },
                heartbeat_ms: None,
                timeout_ms: 0,
                transcode_policy: Default::default(),
                output_policy: Default::default(),
                record_policy: None,
            },
        )
        .await
        .expect_err("denied pull proxy should fail");

    assert_eq!(err.code, MediaErrorCode::PermissionDenied);
    assert!(!proxy.called.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn admission_deny_blocks_push_proxy() {
    let admission = Arc::new(FakeAdmission::new(deny()));
    let ps = Arc::new(FakePublishSubscribe::default());
    let proxy = Arc::new(FakeProxy::default());
    let rtp = Arc::new(FakeRtp::default());
    let facade = facade(Some(admission), ps.clone(), proxy.clone(), rtp.clone());

    let err = facade
        .create_push_proxy(
            &ctx(),
            PushProxyRequest {
                source_media_key: key(),
                destination_url: "rtmp://example.com/live/push".to_string(),
                protocol: "rtmp".to_string(),
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_delay_ms: 0,
                    max_retry_delay_ms: 0,
                },
                protocol_options: HashMap::new(),
            },
        )
        .await
        .expect_err("denied push proxy should fail");

    assert_eq!(err.code, MediaErrorCode::PermissionDenied);
    assert!(!proxy.called.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn admission_deny_blocks_rtp_receiver() {
    let admission = Arc::new(FakeAdmission::new(deny()));
    let ps = Arc::new(FakePublishSubscribe::default());
    let proxy = Arc::new(FakeProxy::default());
    let rtp = Arc::new(FakeRtp::default());
    let facade = facade(Some(admission), ps.clone(), proxy.clone(), rtp.clone());

    let err = facade
        .open_rtp_receiver(
            &ctx(),
            RtpReceiverRequest {
                media_key: key(),
                port: None,
                ip: Some("127.0.0.1".to_string()),
                ssrc: None,
                enable_rtcp: false,
                tcp_mode: None,
                payload_type: None,
                codec_hint: None,
                reuse_port: false,
                timeout_ms: 0,
            },
        )
        .await
        .expect_err("denied RTP receiver should fail");

    assert_eq!(err.code, MediaErrorCode::PermissionDenied);
    assert!(!rtp.called.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn admission_deny_blocks_rtp_sender() {
    let admission = Arc::new(FakeAdmission::new(deny()));
    let ps = Arc::new(FakePublishSubscribe::default());
    let proxy = Arc::new(FakeProxy::default());
    let rtp = Arc::new(FakeRtp::default());
    let facade = facade(Some(admission), ps.clone(), proxy.clone(), rtp.clone());

    let err = facade
        .open_rtp_sender(
            &ctx(),
            RtpSenderRequest {
                media_key: key(),
                destination_endpoint: "127.0.0.1:10000".to_string(),
                ssrc: None,
                payload_type: None,
                codec_hint: None,
                mode: Default::default(),
                transport_options: HashMap::new(),
            },
        )
        .await
        .expect_err("denied RTP sender should fail");

    assert_eq!(err.code, MediaErrorCode::PermissionDenied);
    assert!(!rtp.called.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn missing_admission_skips_check_and_allows_operation() {
    let ps = Arc::new(FakePublishSubscribe::default());
    let proxy = Arc::new(FakeProxy::default());
    let rtp = Arc::new(FakeRtp::default());
    let facade = facade(None, ps.clone(), proxy.clone(), rtp.clone());

    let _handle = facade
        .acquire_publisher(
            &ctx(),
            PublishRequest {
                media_key: key(),
                protocol: "rtmp".to_string(),
                origin: None,
                remote_endpoint: None,
                lease_token: None,
                auth_context: HashMap::new(),
                metadata: HashMap::new(),
            },
        )
        .await
        .expect("operation should succeed when admission is not configured");

    assert!(ps.called.load(Ordering::SeqCst));
}
