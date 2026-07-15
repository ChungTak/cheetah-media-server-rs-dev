use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use async_trait::async_trait;
use cheetah_media_api::command::*;
use cheetah_media_api::error::MediaError;
use cheetah_media_api::ids::*;
use cheetah_media_api::model::*;
use cheetah_media_api::port::{
    MediaControlApi, MediaRequestContext, MediaUrlResolverApi, PublishSubscribeApi,
};
use cheetah_sdk::media_data_plane::{MediaDataPlaneApi, MediaFramePublisher, MediaFrameSubscriber};
use cheetah_sdk::media_session::{MediaSessionDirectoryApi, SessionCloseHandle};
use cheetah_sdk::{SdkError, StreamKey, StreamManagerApi};
use dashmap::DashMap;
use tokio::sync::Mutex;

use super::util::{codec_to_api, media_kind_to_type, now_ms, readiness_to_api};

type PublisherMap = DashMap<SessionId, Arc<Box<dyn MediaFramePublisher>>>;
type SubscriberMap = DashMap<SessionId, Arc<Mutex<Box<dyn MediaFrameSubscriber>>>>;

/// Bridge from `cheetah-sdk` stream primitives to `cheetah-media-api` ports.
///
/// `StreamMediaProvider` implements query, session control, and keyframe request by
/// delegating to the engine's `StreamManagerApi` and `CoreAdaptersApi`. Publish and
/// subscribe are bridged through `MediaDataPlaneApi` and tracked in the shared
/// `MediaSessionDirectoryApi`.
#[derive(Clone)]
pub struct StreamMediaProvider {
    stream_manager: Arc<dyn StreamManagerApi>,
    media_data_plane: Arc<dyn MediaDataPlaneApi>,
    session_directory: Arc<dyn MediaSessionDirectoryApi>,
    url_resolver: Arc<dyn MediaUrlResolverApi>,
    publishers: Arc<PublisherMap>,
    subscribers: Arc<SubscriberMap>,
    next_id: Arc<AtomicU64>,
}

impl StreamMediaProvider {
    pub fn new(
        stream_manager: Arc<dyn StreamManagerApi>,
        media_data_plane: Arc<dyn MediaDataPlaneApi>,
        session_directory: Arc<dyn MediaSessionDirectoryApi>,
        url_resolver: Arc<dyn MediaUrlResolverApi>,
    ) -> Self {
        Self {
            stream_manager,
            media_data_plane,
            session_directory,
            url_resolver,
            publishers: Arc::new(PublisherMap::new()),
            subscribers: Arc::new(SubscriberMap::new()),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    fn map_sdk_error(err: SdkError) -> MediaError {
        match err {
            SdkError::NotFound(msg) => MediaError::not_found(msg),
            SdkError::AlreadyExists(msg) => MediaError::already_exists(msg),
            SdkError::InvalidArgument(msg) => MediaError::invalid_argument(msg),
            SdkError::Conflict(msg) => MediaError::conflict(msg),
            SdkError::Unavailable(msg) => MediaError::unavailable(msg),
            SdkError::Internal(msg) => MediaError::internal(msg),
        }
    }

    fn stream_key_to_media_key(key: &StreamKey) -> MediaKey {
        cheetah_sdk::media_api::ids::StreamKeyBridge::from_namespace_path(&key.namespace, &key.path)
            .unwrap_or_else(|_| {
                MediaKey::new("__fallback__", &key.namespace, &key.path, None).unwrap()
            })
    }

    fn media_key_to_stream_key(key: &MediaKey) -> StreamKey {
        let (namespace, path) =
            cheetah_sdk::media_api::ids::StreamKeyBridge::to_namespace_path(key);
        StreamKey::new(namespace, path)
    }

    fn track_summary(t: &cheetah_codec::TrackInfo) -> TrackSummary {
        TrackSummary {
            track_id: t.track_id.0.to_string(),
            media_type: media_kind_to_type(t.media_kind),
            codec: codec_to_api(t.codec),
            clock_rate: t.clock_rate,
            sample_rate: t.sample_rate,
            channels: t.channels,
            width: t.width,
            height: t.height,
            bitrate: t.bitrate.map(u64::from),
            parameter_set_available: !matches!(t.extradata, cheetah_codec::CodecExtradata::None),
            readiness: readiness_to_api(t.readiness),
        }
    }

    fn stream_info(snapshot: &cheetah_sdk::StreamSnapshot, now_ms: i64) -> StreamInfo {
        let key = Self::stream_key_to_media_key(&snapshot.key);
        let online = if snapshot.publisher_active {
            OnlineState::Online
        } else {
            OnlineState::Offline
        };
        StreamInfo {
            key,
            origin: None,
            online,
            regist: snapshot.publisher_active,
            created_at: now_ms,
            last_activity_at: now_ms,
            readers: 0,
            publishers: 0,
            bytes_in: 0,
            bytes_out: 0,
            duration_ms: 0,
            tracks: snapshot.tracks.iter().map(Self::track_summary).collect(),
            urls: Vec::new(),
            metadata: std::collections::HashMap::new(),
        }
    }

    async fn enrich_stream_info(
        &self,
        ctx: &MediaRequestContext,
        snapshot: &cheetah_sdk::StreamSnapshot,
        now_ms: i64,
    ) -> cheetah_media_api::error::Result<StreamInfo> {
        let mut info = Self::stream_info(snapshot, now_ms);
        let key = info.key.clone();
        let mut query = SessionQuery {
            vhost: Some(key.vhost.0.clone()),
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            page_size: SessionQuery::MAX_PAGE_SIZE,
            ..SessionQuery::default()
        };
        query.clamp_page_size();
        let page = self.session_directory.list_sessions(ctx, query).await?;

        let mut started_at: Option<i64> = None;
        let mut last_seen_at: Option<i64> = None;
        let mut readers = 0u64;
        let mut publishers = 0u64;
        for s in page.items {
            match s.kind {
                SessionKind::Publisher | SessionKind::RtpReceiver => publishers += 1,
                SessionKind::Player | SessionKind::Proxy | SessionKind::RtpSender => readers += 1,
            }
            info.bytes_in += s.bytes_in;
            info.bytes_out += s.bytes_out;
            if started_at.is_none_or(|t| s.started_at < t) {
                started_at = Some(s.started_at);
            }
            if last_seen_at.is_none_or(|t| s.last_seen_at > t) {
                last_seen_at = Some(s.last_seen_at);
            }
            if info.origin.is_none() {
                info.origin = s.remote_endpoint;
            }
        }
        info.readers = readers;
        info.publishers = publishers;
        if let Some(t) = started_at {
            info.created_at = t;
        }
        if let Some(t) = last_seen_at {
            info.last_activity_at = t;
        }
        if let (Some(start), Some(end)) = (started_at, last_seen_at) {
            info.duration_ms = (end - start).max(0) as u64;
        }
        if let Ok(urls) = self.url_resolver.resolve_urls(ctx, &key, &[]).await {
            info.urls = urls;
        }
        Ok(info)
    }

    fn new_session_id(&self, kind: &str) -> SessionId {
        let n = self.next_id.fetch_add(1, Ordering::Relaxed);
        SessionId(format!("stream-{kind}-{n:016x}"))
    }
}

#[async_trait]
impl MediaControlApi for StreamMediaProvider {
    async fn get_media_list(
        &self,
        ctx: &MediaRequestContext,
        mut query: MediaQuery,
    ) -> cheetah_media_api::error::Result<Page<StreamInfo>> {
        query.clamp_page_size();
        let snapshots = self
            .stream_manager
            .list_streams()
            .await
            .map_err(Self::map_sdk_error)?;
        let now = now_ms();
        if query.schema.is_some() {
            return Err(MediaError::invalid_argument(
                "schema filter is not supported by the stream provider".to_string(),
            ));
        }
        let mut items: Vec<StreamInfo> = Vec::with_capacity(snapshots.len());
        for s in snapshots {
            let key = Self::stream_key_to_media_key(&s.key);
            if let Some(ref v) = query.vhost {
                if key.vhost.0 != *v {
                    continue;
                }
            }
            if let Some(ref a) = query.app {
                if key.app.0 != *a {
                    continue;
                }
            }
            if let Some(ref st) = query.stream {
                if key.stream.0 != *st {
                    continue;
                }
            }
            if let Some(online) = query.online {
                if s.publisher_active != online {
                    continue;
                }
            }
            let info = self.enrich_stream_info(ctx, &s, now).await?;
            items.push(info);
        }
        let total = items.len() as u64;
        let page = query.page.max(1);
        let page_size = query.page_size;
        let start = ((page - 1) * page_size) as usize;
        let paged = items
            .into_iter()
            .skip(start)
            .take(page_size as usize)
            .collect();
        Ok(Page {
            items: paged,
            page,
            page_size,
            total,
            next_cursor: None,
        })
    }

    async fn get_media(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
    ) -> cheetah_media_api::error::Result<StreamInfo> {
        let stream_key = Self::media_key_to_stream_key(key);
        let snapshot = self
            .stream_manager
            .get_stream(&stream_key)
            .await
            .map_err(Self::map_sdk_error)?
            .ok_or_else(|| MediaError::not_found(format!("stream {key}")))?;
        self.enrich_stream_info(ctx, &snapshot, now_ms()).await
    }

    async fn is_media_online(
        &self,
        _ctx: &MediaRequestContext,
        key: &MediaKey,
    ) -> cheetah_media_api::error::Result<OnlineState> {
        let stream_key = Self::media_key_to_stream_key(key);
        let snapshot = self
            .stream_manager
            .get_stream(&stream_key)
            .await
            .map_err(Self::map_sdk_error)?;
        Ok(match snapshot {
            Some(s) if s.publisher_active => OnlineState::Online,
            Some(_) => OnlineState::Offline,
            None => OnlineState::Unknown,
        })
    }

    async fn list_sessions(
        &self,
        ctx: &MediaRequestContext,
        mut query: SessionQuery,
    ) -> cheetah_media_api::error::Result<Page<SessionInfo>> {
        query.clamp_page_size();
        self.session_directory.list_sessions(ctx, query).await
    }

    async fn kick_session(
        &self,
        ctx: &MediaRequestContext,
        id: &SessionId,
        reason: CloseReason,
    ) -> cheetah_media_api::error::Result<()> {
        self.session_directory
            .close_session(ctx, id, reason)
            .await
            .map(|_| ())
    }

    async fn kick_stream(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
        reason: CloseReason,
    ) -> cheetah_media_api::error::Result<CloseReport> {
        self.session_directory
            .close_sessions_for_key(ctx, key, reason)
            .await
    }

    async fn request_keyframe(
        &self,
        _ctx: &MediaRequestContext,
        key: &MediaKey,
    ) -> cheetah_media_api::error::Result<()> {
        let stream_key = Self::media_key_to_stream_key(key);
        self.stream_manager
            .request_keyframe(&stream_key)
            .await
            .map_err(Self::map_sdk_error)
    }
}

struct PublisherCloseHandle {
    publishers: Arc<PublisherMap>,
    id: SessionId,
    publisher: Arc<Box<dyn MediaFramePublisher>>,
}

#[async_trait]
impl SessionCloseHandle for PublisherCloseHandle {
    async fn close(&self, _reason: CloseReason) -> cheetah_media_api::error::Result<SessionId> {
        self.publishers.remove(&self.id);
        self.publisher.close().await?;
        Ok(self.id.clone())
    }
}

struct SubscriberCloseHandle {
    subscribers: Arc<SubscriberMap>,
    id: SessionId,
}

#[async_trait]
impl SessionCloseHandle for SubscriberCloseHandle {
    async fn close(&self, _reason: CloseReason) -> cheetah_media_api::error::Result<SessionId> {
        if let Some((_, sub)) = self.subscribers.remove(&self.id) {
            let mut guard = sub.lock().await;
            guard.close().await?;
        }
        Ok(self.id.clone())
    }
}

#[async_trait]
impl PublishSubscribeApi for StreamMediaProvider {
    async fn acquire_publisher(
        &self,
        ctx: &MediaRequestContext,
        request: PublishRequest,
    ) -> cheetah_media_api::error::Result<PublisherHandle> {
        let publisher = self
            .media_data_plane
            .open_frame_publisher(ctx, request.clone())
            .await?;
        let id = self.new_session_id("pub");
        let publisher = Arc::new(publisher);
        self.publishers.insert(id.clone(), publisher.clone());
        let close_handle = Box::new(PublisherCloseHandle {
            publishers: self.publishers.clone(),
            id: id.clone(),
            publisher: publisher.clone(),
        });
        let record = SessionInfo {
            session_id: id.clone(),
            kind: SessionKind::Publisher,
            media_key: request.media_key.clone(),
            remote_endpoint: request.remote_endpoint,
            local_endpoint: None,
            protocol: request.protocol,
            started_at: now_ms(),
            last_seen_at: now_ms(),
            bytes_in: 0,
            bytes_out: 0,
            state: SessionState::Connected,
            close_reason: None,
            owner_module: "stream".to_string(),
        };
        if let Err(e) = self
            .session_directory
            .register_session(ctx, record, close_handle)
            .await
        {
            self.publishers.remove(&id);
            let _ = publisher.close().await;
            return Err(e);
        }
        Ok(PublisherHandle {
            session_id: id.clone(),
            media_key: request.media_key,
            lease_token: id.0,
        })
    }

    async fn open_subscriber(
        &self,
        ctx: &MediaRequestContext,
        request: SubscribeRequest,
    ) -> cheetah_media_api::error::Result<SubscriberHandle> {
        let subscriber = self
            .media_data_plane
            .open_frame_subscriber(ctx, request.clone())
            .await?;
        let id = self.new_session_id("sub");
        let subscriber = Arc::new(Mutex::new(subscriber));
        self.subscribers.insert(id.clone(), subscriber.clone());
        let close_handle = Box::new(SubscriberCloseHandle {
            subscribers: self.subscribers.clone(),
            id: id.clone(),
        });
        let record = SessionInfo {
            session_id: id.clone(),
            kind: SessionKind::Player,
            media_key: request.media_key.clone(),
            remote_endpoint: None,
            local_endpoint: None,
            protocol: request.subscriber_kind.clone(),
            started_at: now_ms(),
            last_seen_at: now_ms(),
            bytes_in: 0,
            bytes_out: 0,
            state: SessionState::Connected,
            close_reason: None,
            owner_module: "stream".to_string(),
        };
        if let Err(e) = self
            .session_directory
            .register_session(ctx, record, close_handle)
            .await
        {
            self.subscribers.remove(&id);
            let mut guard = subscriber.lock().await;
            let _ = guard.close().await;
            return Err(e);
        }
        Ok(SubscriberHandle {
            session_id: id,
            media_key: request.media_key,
            output_schema: request.output_schema,
            url: None,
        })
    }

    async fn close_handle(
        &self,
        ctx: &MediaRequestContext,
        id: &SessionId,
        reason: CloseReason,
    ) -> cheetah_media_api::error::Result<()> {
        self.session_directory
            .close_session(ctx, id, reason)
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::media_provider::{EngineMediaDataPlane, EngineMediaSessionDirectory};
    use crate::stream::{DispatcherMode, StreamManager};
    use cheetah_media_api::command::{PublishRequest, SessionQuery, SubscribeRequest};
    use cheetah_media_api::ids::{MediaKey, MediaSchema};
    use cheetah_media_api::model::{CloseReason, SessionKind};
    use cheetah_runtime_tokio::TokioRuntime;

    struct EmptyConfig;
    impl cheetah_sdk::ConfigProvider for EmptyConfig {
        fn global(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn module(&self, _module_id: &cheetah_sdk::ModuleId) -> serde_json::Value {
            serde_json::Value::Null
        }
        fn version(&self) -> u64 {
            0
        }
    }

    fn test_provider() -> StreamMediaProvider {
        let runtime: Arc<dyn cheetah_sdk::RuntimeApi> = Arc::new(TokioRuntime::new());
        let manager = Arc::new(StreamManager::new(DispatcherMode::PerStream, 128, runtime));
        let publisher_api: Arc<dyn cheetah_sdk::PublisherApi> = manager.clone();
        let subscriber_api: Arc<dyn cheetah_sdk::SubscriberApi> = manager.clone();
        let data_plane: Arc<dyn MediaDataPlaneApi> =
            Arc::new(EngineMediaDataPlane::new(publisher_api, subscriber_api));
        let directory: Arc<dyn MediaSessionDirectoryApi> =
            Arc::new(EngineMediaSessionDirectory::new());
        let media_services = cheetah_sdk::MediaServices::unavailable();
        media_services.register_output_registry(Arc::new(
            cheetah_sdk::output::InMemoryMediaOutputRegistry::new(),
        )
            as Arc<dyn cheetah_media_api::port::MediaOutputRegistryApi>);
        let url_resolver: Arc<dyn MediaUrlResolverApi> =
            Arc::new(crate::media_provider::EngineMediaUrlResolver::new(
                media_services,
                Arc::new(EmptyConfig),
            ));
        StreamMediaProvider::new(manager, data_plane, directory, url_resolver)
    }

    fn publish_request(key: &MediaKey) -> PublishRequest {
        PublishRequest {
            media_key: key.clone(),
            protocol: "test".to_string(),
            origin: None,
            remote_endpoint: None,
            lease_token: None,
            auth_context: Default::default(),
            metadata: Default::default(),
        }
    }

    fn subscribe_request(key: &MediaKey) -> SubscribeRequest {
        SubscribeRequest {
            media_key: key.clone(),
            output_schema: MediaSchema::Webrtc,
            subscriber_kind: "webrtc".to_string(),
            start_policy: Default::default(),
            auth_context: Default::default(),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn acquire_publisher_registers_session() {
        let provider = test_provider();
        let key = MediaKey::with_default_vhost("live", "session-test", None).unwrap();
        let ctx = MediaRequestContext::default();

        let handle = provider
            .acquire_publisher(&ctx, publish_request(&key))
            .await
            .unwrap();
        assert_eq!(handle.media_key, key);

        let mut query = SessionQuery::default();
        query.kind = Some(SessionKind::Publisher);
        let page = provider.list_sessions(&ctx, query).await.unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].kind, SessionKind::Publisher);

        provider
            .kick_session(&ctx, &handle.session_id, CloseReason::Kicked)
            .await
            .unwrap();
        let mut query = SessionQuery::default();
        query.kind = Some(SessionKind::Publisher);
        let page = provider.list_sessions(&ctx, query).await.unwrap();
        assert!(page.items.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn second_publisher_on_same_key_is_rejected() {
        let provider = test_provider();
        let key = MediaKey::with_default_vhost("live", "exclusive-pub", None).unwrap();
        let ctx = MediaRequestContext::default();

        let _ = provider
            .acquire_publisher(&ctx, publish_request(&key))
            .await
            .unwrap();
        let err = provider
            .acquire_publisher(&ctx, publish_request(&key))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already exists") || err.to_string().contains("conflict"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_subscriber_registers_player_session() {
        let provider = test_provider();
        let key = MediaKey::with_default_vhost("live", "sub-test", None).unwrap();
        let ctx = MediaRequestContext::default();

        let _pub_handle = provider
            .acquire_publisher(&ctx, publish_request(&key))
            .await
            .unwrap();
        let sub = provider
            .open_subscriber(&ctx, subscribe_request(&key))
            .await
            .unwrap();
        assert_eq!(sub.media_key, key);

        let mut query = SessionQuery::default();
        query.kind = Some(SessionKind::Player);
        let page = provider.list_sessions(&ctx, query).await.unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].kind, SessionKind::Player);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn kick_stream_closes_all_sessions_for_key() {
        let provider = test_provider();
        let key = MediaKey::with_default_vhost("live", "kick-stream", None).unwrap();
        let ctx = MediaRequestContext::default();

        let _pub_handle = provider
            .acquire_publisher(&ctx, publish_request(&key))
            .await
            .unwrap();
        let _sub_handle = provider
            .open_subscriber(&ctx, subscribe_request(&key))
            .await
            .unwrap();

        let report = provider
            .kick_stream(&ctx, &key, CloseReason::Kicked)
            .await
            .unwrap();
        assert_eq!(report.closed_sessions.len(), 2);

        let page = provider
            .list_sessions(&ctx, SessionQuery::default())
            .await
            .unwrap();
        assert!(page.items.is_empty());
    }
}
