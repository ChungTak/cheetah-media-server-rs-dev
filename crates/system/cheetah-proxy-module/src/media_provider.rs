//! `ProxyApi` implementation backed by the in-memory proxy registry.
//!
//! 由内存代理注册表支持的 `ProxyApi` 实现。

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::command::{
    FfmpegProxyRequest, ProxyQuery, PullProxyRequest, PushProxyRequest,
};
use cheetah_media_api::error::{MediaError, MediaErrorCode, Result};
use cheetah_media_api::event::{EventHeader, MediaEvent, MediaEventSender};
use cheetah_media_api::ids::StreamKeyBridge;
use cheetah_media_api::ids::{MediaKey, MediaSchema, ProxyId};
use cheetah_media_api::model::Page;
use cheetah_media_api::model::{MediaUrl, OutputPolicy, ProxyInfo, ProxyKind, ProxyState};
use cheetah_media_api::port::{MediaRequestContext, MediaUrlResolverApi, ProxyApi};
use cheetah_sdk::connector::ConnectorDirection;
use cheetah_sdk::{
    CancellationToken, ConnectorApi, EngineContext, FfmpegApi, FfmpegJob, FfmpegJobSpec,
    PublisherApi, PublisherOptions, RuntimeApi, StreamKey, SubscriberApi, SubscriberOptions,
};

use crate::registry::ProxyRegistry;
use crate::runner;

/// Provider that implements the media-domain `ProxyApi`.
///
/// 实现媒体领域 `ProxyApi` 的 Provider。
pub struct ProxyMediaProvider {
    registry: ProxyRegistry,
    connector_api: Option<Arc<dyn ConnectorApi>>,
    ffmpeg_api: Arc<dyn FfmpegApi>,
    publisher_api: Arc<dyn PublisherApi>,
    subscriber_api: Arc<dyn SubscriberApi>,
    runtime_api: Arc<dyn RuntimeApi>,
    url_resolver_api: Option<Arc<dyn MediaUrlResolverApi>>,
    media_event_sender: Option<Arc<dyn MediaEventSender>>,
}

impl ProxyMediaProvider {
    /// Create a provider from the engine context and a shared registry.
    pub fn new(ctx: &EngineContext, registry: ProxyRegistry) -> Self {
        Self {
            registry,
            connector_api: ctx.connector_api.clone(),
            ffmpeg_api: ctx.ffmpeg_api.clone(),
            publisher_api: ctx.publisher_api.clone(),
            subscriber_api: ctx.subscriber_api.clone(),
            runtime_api: ctx.runtime_api.clone(),
            url_resolver_api: ctx.media_url_resolver_api.clone(),
            media_event_sender: Some(ctx.media_event_sender.clone()),
        }
    }

    /// Create from explicit registry and APIs for tests.
    #[cfg(test)]
    fn with_apis(
        registry: ProxyRegistry,
        connector_api: Option<Arc<dyn ConnectorApi>>,
        ffmpeg_api: Arc<dyn FfmpegApi>,
        publisher_api: Arc<dyn PublisherApi>,
        subscriber_api: Arc<dyn SubscriberApi>,
        runtime_api: Arc<dyn RuntimeApi>,
        url_resolver_api: Option<Arc<dyn MediaUrlResolverApi>>,
    ) -> Self {
        Self {
            registry,
            connector_api,
            ffmpeg_api,
            publisher_api,
            subscriber_api,
            runtime_api,
            url_resolver_api,
            media_event_sender: None,
        }
    }

    fn validate_url(url: &str) -> Result<()> {
        if url.is_empty() {
            return Err(MediaError::invalid_argument("source URL is empty"));
        }
        if !url.contains("://") {
            return Err(MediaError::invalid_argument(format!(
                "invalid URL, missing scheme: {url}"
            )));
        }
        Ok(())
    }

    fn connector(&self) -> Result<&dyn ConnectorApi> {
        self.connector_api
            .as_deref()
            .ok_or_else(|| MediaError::unavailable("connector api not configured"))
    }

    fn check_protocol_support(&self, url: &str, direction: ConnectorDirection) -> Result<()> {
        let scheme = url.split("://").next().unwrap_or("").to_ascii_lowercase();
        let connector = self.connector()?;
        if connector.supports(&scheme, direction) {
            Ok(())
        } else {
            Err(MediaError::new(
                MediaErrorCode::Unsupported,
                format!("unsupported proxy protocol: {scheme}"),
            ))
        }
    }

    fn build_proxy_info(
        &self,
        kind: ProxyKind,
        source: &str,
        destination: &MediaKey,
        output_urls: Vec<MediaUrl>,
    ) -> Result<ProxyInfo> {
        let now = now_ms();
        Ok(ProxyInfo {
            proxy_id: ProxyId(format!("proxy-{}", generate_id())),
            kind,
            source: source.to_string(),
            destination: destination.clone(),
            state: ProxyState::Connecting,
            retry_count: 0,
            last_error: None,
            created_at: now,
            updated_at: now,
            output_urls,
        })
    }

    fn output_policy_to_schema(policy: &OutputPolicy) -> Option<MediaSchema> {
        match policy {
            OutputPolicy::None => None,
            OutputPolicy::Hls => Some(MediaSchema::Hls),
            OutputPolicy::Mp4 => Some(MediaSchema::Fmp4),
            OutputPolicy::Flv => Some(MediaSchema::HttpFlv),
            OutputPolicy::Fmp4 => Some(MediaSchema::Fmp4),
            OutputPolicy::Rtmp => Some(MediaSchema::Rtmp),
            OutputPolicy::Rtsp => Some(MediaSchema::Rtsp),
        }
    }

    async fn resolve_output_urls(
        &self,
        ctx: &MediaRequestContext,
        destination: &MediaKey,
        policy: &OutputPolicy,
    ) -> Result<Vec<MediaUrl>> {
        let schema = Self::output_policy_to_schema(policy);
        let Some(schema) = schema else {
            return Ok(Vec::new());
        };
        let Some(resolver) = &self.url_resolver_api else {
            return Err(MediaError::unsupported(
                "non-null output_policy requires MediaUrlResolverApi",
            ));
        };
        let urls = resolver.resolve_urls(ctx, destination, &[schema]).await?;
        if urls.iter().any(|u| u.available) {
            Ok(urls)
        } else {
            Err(MediaError::unavailable(format!(
                "no available URL for output policy {:?}",
                policy
            )))
        }
    }

    fn upsert_and_emit(&self, info: ProxyInfo) -> ProxyInfo {
        let inserted = self.registry.upsert_idempotent(info.clone());
        if inserted.proxy_id == info.proxy_id {
            self.emit_state_changed(&inserted);
        }
        inserted
    }

    fn emit_state_changed(&self, info: &ProxyInfo) {
        if let Some(sender) = &self.media_event_sender {
            let header = EventHeader {
                event_id: generate_id(),
                occurred_at: now_ms(),
                sequence: None,
                media_key: Some(info.destination.clone()),
                source: info.source.clone(),
                correlation_id: None,
            };
            let _ = sender.send(MediaEvent::ProxyStateChanged(
                cheetah_media_api::event::ProxyStateChanged {
                    header,
                    proxy_id: info.proxy_id.clone(),
                    state: info.state,
                    last_error: info.last_error.clone(),
                },
            ));
        }
    }
}

#[async_trait]
impl ProxyApi for ProxyMediaProvider {
    async fn create_pull_proxy(
        &self,
        _ctx: &MediaRequestContext,
        request: PullProxyRequest,
    ) -> Result<ProxyInfo> {
        Self::validate_url(&request.source_url)?;
        self.check_protocol_support(&request.source_url, ConnectorDirection::Pull)?;
        let connector = self
            .connector_api
            .as_ref()
            .ok_or_else(|| MediaError::unavailable("connector api not configured"))?
            .clone();

        let destination = media_key_to_stream_key(&request.destination)?;
        let (lease, sink) = self
            .publisher_api
            .acquire_publisher(destination.clone(), PublisherOptions::default())
            .await
            .map_err(map_sdk_error)?;

        let info = self.build_proxy_info(
            ProxyKind::Pull,
            &request.source_url,
            &request.destination,
            Vec::new(),
        )?;
        let info_id = info.proxy_id.clone();
        let inserted = self.upsert_and_emit(info);

        if inserted.proxy_id != info_id {
            let _ = sink.close();
            let _ = self.publisher_api.release_publisher(&lease).await;
            return Ok(inserted);
        }

        let cancel = CancellationToken::new();
        let runner_cancel = cancel.clone();
        let proxy_id = inserted.proxy_id.clone();
        let registry = self.registry.clone();
        let event_sender = self.media_event_sender.clone();
        let publisher_api = self.publisher_api.clone();
        let runtime_api = self.runtime_api.clone();
        let runtime_api_for_runner = runtime_api.clone();
        let url = request.source_url;
        let retry_policy = request.retry_policy;

        let handle = runtime_api.spawn(Box::pin(async move {
            runner::run_pull(
                registry,
                event_sender,
                connector,
                url,
                sink,
                lease,
                publisher_api,
                proxy_id,
                retry_policy,
                runner_cancel,
                runtime_api_for_runner,
            )
            .await;
        }));

        if !self
            .registry
            .attach_task(&inserted.proxy_id, cancel.clone(), handle)
        {
            cancel.cancel();
        }
        Ok(inserted)
    }

    async fn delete_pull_proxy(&self, _ctx: &MediaRequestContext, id: &ProxyId) -> Result<()> {
        if self.registry.delete(id) {
            Ok(())
        } else {
            Err(MediaError::not_found(format!("pull proxy {id}")))
        }
    }

    async fn list_pull_proxies(
        &self,
        _ctx: &MediaRequestContext,
        mut query: ProxyQuery,
    ) -> Result<Page<ProxyInfo>> {
        query.clamp_page_size();
        let all = self.registry.list(query.kind, query.state);
        let total = all.len() as u64;
        let page = query.page.max(1);
        let start = ((page - 1).saturating_mul(query.page_size)) as usize;
        let items = all
            .into_iter()
            .skip(start)
            .take(query.page_size as usize)
            .collect();
        Ok(Page {
            items,
            total,
            page,
            page_size: query.page_size,
            next_cursor: None,
        })
    }

    async fn create_push_proxy(
        &self,
        _ctx: &MediaRequestContext,
        request: PushProxyRequest,
    ) -> Result<ProxyInfo> {
        Self::validate_url(&request.destination_url)?;
        self.check_protocol_support(&request.destination_url, ConnectorDirection::Push)?;
        let connector = self
            .connector_api
            .as_ref()
            .ok_or_else(|| MediaError::unavailable("connector api not configured"))?
            .clone();

        let source_key = media_key_to_stream_key(&request.source_media_key)?;
        let mut source = self
            .subscriber_api
            .subscribe(source_key.clone(), SubscriberOptions::default())
            .await
            .map_err(map_sdk_error)?;

        let info = self.build_proxy_info(
            ProxyKind::Push,
            &request.destination_url,
            &request.source_media_key,
            Vec::new(),
        )?;
        let info_id = info.proxy_id.clone();
        let inserted = self.upsert_and_emit(info);

        if inserted.proxy_id != info_id {
            let _ = source.close().await;
            return Ok(inserted);
        }

        let cancel = CancellationToken::new();
        let runner_cancel = cancel.clone();
        let proxy_id = inserted.proxy_id.clone();
        let registry = self.registry.clone();
        let event_sender = self.media_event_sender.clone();
        let runtime_api = self.runtime_api.clone();
        let runtime_api_for_runner = runtime_api.clone();
        let url = request.destination_url;
        let retry_policy = request.retry_policy;

        let handle = runtime_api.spawn(Box::pin(async move {
            runner::run_push(
                registry,
                event_sender,
                connector,
                url,
                source,
                proxy_id,
                retry_policy,
                runner_cancel,
                runtime_api_for_runner,
            )
            .await;
        }));

        if !self
            .registry
            .attach_task(&inserted.proxy_id, cancel.clone(), handle)
        {
            cancel.cancel();
        }
        Ok(inserted)
    }

    async fn delete_push_proxy(&self, _ctx: &MediaRequestContext, id: &ProxyId) -> Result<()> {
        if self.registry.delete(id) {
            Ok(())
        } else {
            Err(MediaError::not_found(format!("push proxy {id}")))
        }
    }

    async fn create_ffmpeg_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: FfmpegProxyRequest,
    ) -> Result<ProxyInfo> {
        Self::validate_url(&request.source_url)?;

        let output_urls = self
            .resolve_output_urls(ctx, &request.destination, &request.output_policy)
            .await?;

        let spec = FfmpegJobSpec {
            source_url: request.source_url.clone(),
            destination: request.destination.clone(),
            transcode_policy: request.transcode_policy.clone(),
            output_policy: request.output_policy,
            timeout_ms: request.timeout_ms,
            resource_limits: request.resource_limits.clone(),
            enable_audio: request.enable_audio,
            enable_video: request.enable_video,
        };

        let info = self.build_proxy_info(
            ProxyKind::Ffmpeg,
            &spec.source_url,
            &spec.destination,
            output_urls,
        )?;
        let info_id = info.proxy_id.clone();
        let inserted = self.upsert_and_emit(info);

        if inserted.proxy_id != info_id {
            return Ok(inserted);
        }

        let proxy_id = inserted.proxy_id.clone();
        let job = FfmpegJob {
            job_id: proxy_id.0.clone(),
            proxy_id: proxy_id.0.clone(),
            spec,
        };

        if let Err(e) = self.ffmpeg_api.submit_job(job).await {
            let _ = self.registry.delete(&proxy_id);
            return Err(map_sdk_error(e));
        }

        let cancel = CancellationToken::new();
        let runner_cancel = cancel.clone();
        let registry = self.registry.clone();
        let event_sender = self.media_event_sender.clone();
        let ffmpeg_api = self.ffmpeg_api.clone();
        let runtime_api = self.runtime_api.clone();
        let proxy_id_for_runner = proxy_id.clone();

        let handle = runtime_api.spawn(Box::pin(async move {
            crate::ffmpeg_runner::run(
                registry,
                event_sender,
                ffmpeg_api,
                proxy_id_for_runner,
                runner_cancel,
            )
            .await;
        }));

        if !self
            .registry
            .attach_task(&inserted.proxy_id, cancel.clone(), handle)
        {
            cancel.cancel();
        }

        Ok(inserted)
    }
}

fn media_key_to_stream_key(key: &MediaKey) -> Result<StreamKey> {
    let (namespace, path) = StreamKeyBridge::to_namespace_path(key);
    Ok(StreamKey::new(namespace, path))
}

fn map_sdk_error(e: cheetah_sdk::SdkError) -> MediaError {
    match e {
        cheetah_sdk::SdkError::InvalidArgument(m) => MediaError::invalid_argument(m),
        cheetah_sdk::SdkError::NotFound(m) => MediaError::not_found(m),
        cheetah_sdk::SdkError::AlreadyExists(m) => MediaError::already_exists(m),
        cheetah_sdk::SdkError::Conflict(m) => MediaError::conflict(m),
        cheetah_sdk::SdkError::Unavailable(m) => MediaError::unavailable(m),
        cheetah_sdk::SdkError::Internal(m) => MediaError::internal(m),
    }
}

pub(crate) fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub(crate) fn generate_id() -> String {
    let mut buf = [0u8; 8];
    if getrandom::getrandom(&mut buf).is_err() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let now = now_ms() as u64;
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        return format!("{:x}{:x}", now, seq);
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use std::result::Result as StdResult;

    use super::*;
    use cheetah_media_api::command::FfmpegProxyRequest;
    use cheetah_media_api::ids::MediaKey;
    use cheetah_sdk::{
        FfmpegApi, FfmpegJob, FfmpegJobOutcome, PublishLease, PublisherSink, SdkError,
        SubscriberSource,
    };

    fn fake_key(stream: &str) -> MediaKey {
        MediaKey::with_default_vhost("live", stream, None).unwrap()
    }

    fn provider() -> ProxyMediaProvider {
        ProxyMediaProvider::with_apis(
            ProxyRegistry::new(10),
            None,
            Arc::new(FakeFfmpegApi),
            Arc::new(FakePublisherApi),
            Arc::new(FakeSubscriberApi),
            Arc::new(FakeRuntime),
            None,
        )
    }

    #[tokio::test]
    async fn create_pull_proxy_rejects_invalid_url() {
        let p = provider();
        let req = PullProxyRequest {
            source_url: "not-a-url".to_string(),
            destination: fake_key("s"),
            retry_policy: Default::default(),
            heartbeat_ms: None,
            timeout_ms: 30_000,
            transcode_policy: Default::default(),
            output_policy: Default::default(),
            record_policy: None,
        };
        let ctx = MediaRequestContext::default();
        let err = p.create_pull_proxy(&ctx, req).await.unwrap_err();
        assert_eq!(err.code, MediaErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn create_pull_proxy_without_connector_is_unavailable() {
        let p = provider();
        let req = PullProxyRequest {
            source_url: "rtsp://example/stream".to_string(),
            destination: fake_key("s"),
            retry_policy: Default::default(),
            heartbeat_ms: None,
            timeout_ms: 30_000,
            transcode_policy: Default::default(),
            output_policy: Default::default(),
            record_policy: None,
        };
        let ctx = MediaRequestContext::default();
        let err = p.create_pull_proxy(&ctx, req).await.unwrap_err();
        assert_eq!(err.code, MediaErrorCode::Unavailable);
    }

    struct FakePublisherApi;

    #[async_trait]
    impl PublisherApi for FakePublisherApi {
        async fn acquire_publisher(
            &self,
            _stream_key: StreamKey,
            _options: PublisherOptions,
        ) -> StdResult<(PublishLease, Box<dyn PublisherSink>), SdkError> {
            Err(SdkError::Unavailable("no publisher".to_string()))
        }

        async fn release_publisher(&self, _lease: &PublishLease) -> StdResult<(), SdkError> {
            Ok(())
        }
    }

    struct FakeSubscriberApi;

    #[async_trait]
    impl SubscriberApi for FakeSubscriberApi {
        async fn subscribe(
            &self,
            _stream_key: StreamKey,
            _options: SubscriberOptions,
        ) -> StdResult<Box<dyn SubscriberSource>, SdkError> {
            Err(SdkError::Unavailable("no subscriber".to_string()))
        }
    }

    struct FakeFfmpegApi;

    #[async_trait]
    impl FfmpegApi for FakeFfmpegApi {
        async fn submit_job(&self, _job: FfmpegJob) -> StdResult<(), SdkError> {
            Err(SdkError::Unavailable("no ffmpeg".to_string()))
        }

        async fn cancel_job(&self, _job_id: &str) -> StdResult<(), SdkError> {
            Ok(())
        }

        async fn wait_job(&self, _job_id: &str) -> StdResult<FfmpegJobOutcome, SdkError> {
            Err(SdkError::Unavailable("no ffmpeg".to_string()))
        }

        fn list_jobs(&self) -> Vec<FfmpegJob> {
            Vec::new()
        }
    }

    struct FakeRuntime;

    impl RuntimeApi for FakeRuntime {
        fn now(&self) -> cheetah_codec::MonoTime {
            cheetah_codec::MonoTime::from_micros(0)
        }

        fn spawn(
            &self,
            _fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Box<dyn cheetah_sdk::JoinHandle> {
            Box::new(FakeJoinHandle)
        }

        fn spawn_local(
            &self,
            _fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>>,
        ) -> StdResult<Box<dyn cheetah_sdk::JoinHandle>, cheetah_sdk::SpawnError> {
            Ok(Box::new(FakeJoinHandle))
        }

        fn bind_udp(
            &self,
            _addr: std::net::SocketAddr,
        ) -> std::io::Result<Box<dyn cheetah_sdk::AsyncUdpSocket>> {
            unimplemented!()
        }

        fn connect_tcp(
            &self,
            _addr: std::net::SocketAddr,
        ) -> std::io::Result<Box<dyn cheetah_sdk::AsyncTcpStream>> {
            unimplemented!()
        }

        fn bind_tcp(
            &self,
            _addr: std::net::SocketAddr,
        ) -> std::io::Result<Box<dyn cheetah_sdk::AsyncTcpListener>> {
            unimplemented!()
        }

        fn wrap_udp_socket(
            &self,
            _socket: std::net::UdpSocket,
        ) -> std::io::Result<Box<dyn cheetah_sdk::AsyncUdpSocket>> {
            unimplemented!()
        }

        fn wrap_tcp_listener(
            &self,
            _listener: std::net::TcpListener,
        ) -> std::io::Result<Box<dyn cheetah_sdk::AsyncTcpListener>> {
            unimplemented!()
        }

        fn wrap_tcp_stream(
            &self,
            _stream: std::net::TcpStream,
        ) -> std::io::Result<Box<dyn cheetah_sdk::AsyncTcpStream>> {
            unimplemented!()
        }

        fn sleep_until(
            &self,
            _deadline: cheetah_codec::MonoTime,
        ) -> Box<dyn cheetah_sdk::AsyncTimer> {
            unimplemented!()
        }
    }

    struct FakeJoinHandle;

    impl cheetah_sdk::JoinHandle for FakeJoinHandle {
        fn abort(&self) {}
        fn is_finished(&self) -> bool {
            true
        }
        fn wait(
            self: Box<Self>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = StdResult<(), cheetah_sdk::TaskJoinError>>
                    + Send
                    + 'static,
            >,
        > {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn create_ffmpeg_proxy_rejects_non_null_output_policy() {
        let p = provider();
        let req = FfmpegProxyRequest {
            source_url: "http://example/stream".to_string(),
            destination: fake_key("s"),
            transcode_policy: Default::default(),
            output_policy: OutputPolicy::Mp4,
            timeout_ms: 5000,
            resource_limits: Default::default(),
            enable_audio: true,
            enable_video: true,
        };
        let ctx = MediaRequestContext::default();
        let err = p.create_ffmpeg_proxy(&ctx, req).await.unwrap_err();
        assert_eq!(err.code, MediaErrorCode::Unsupported);
    }
}
