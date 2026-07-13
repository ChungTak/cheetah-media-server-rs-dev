use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cheetah_codec::{CodecId, MediaKind, TrackInfo, TrackReadiness as CodecTrackReadiness};
use cheetah_media_api::command::*;
use cheetah_media_api::error::MediaError;
use cheetah_media_api::ids::*;
use cheetah_media_api::model::*;
use cheetah_media_api::port::{
    MediaControlApi, MediaFacade, MediaRequestContext, ProxyApi, PublishSubscribeApi, RecordApi,
    RtpApi, SnapshotApi,
};
use cheetah_media_api::{MediaCapability, MediaCapabilitySet};
use cheetah_sdk::{
    CoreAdaptersApi, PublisherApi, PublisherOptions, SdkError, StreamKey, StreamManagerApi,
    SubscriberApi, SubscriberOptions,
};

/// Bridge from `cheetah-sdk` stream primitives to `cheetah-media-api` ports.
///
/// `StreamMediaProvider` implements query, session control, publish/subscribe, and
/// keyframe request by delegating to the engine's `StreamManagerApi`/`PublisherApi`/
/// `SubscriberApi`. Other capabilities (record, snapshot, proxy, RTP) are left to
/// dedicated providers and return `Unsupported` when not wired.
///
/// 从 `cheetah-sdk` 流原语到 `cheetah-media-api` 端口的桥接。
///
/// `StreamMediaProvider` 通过委托引擎的 `StreamManagerApi`/`PublisherApi`/`SubscriberApi`
/// 实现查询、会话控制、发布/订阅和关键帧请求。其他能力（录制、快照、代理、RTP）
/// 由专用 provider 实现，未接入时返回 `Unsupported`。
#[derive(Clone)]
pub struct StreamMediaProvider {
    stream_manager: Arc<dyn StreamManagerApi>,
    publisher_api: Arc<dyn PublisherApi>,
    subscriber_api: Arc<dyn SubscriberApi>,
    core_adapters: Arc<dyn CoreAdaptersApi>,
}

impl StreamMediaProvider {
    /// Create a new provider backed by the engine stream manager.
    ///
    /// 创建由引擎流管理器支撑的 provider。
    pub fn new(
        stream_manager: Arc<dyn StreamManagerApi>,
        publisher_api: Arc<dyn PublisherApi>,
        subscriber_api: Arc<dyn SubscriberApi>,
        core_adapters: Arc<dyn CoreAdaptersApi>,
    ) -> Self {
        Self {
            stream_manager,
            publisher_api,
            subscriber_api,
            core_adapters,
        }
    }

    /// Convert an SDK `SdkError` to a media-domain `MediaError`.
    ///
    /// 将 SDK `SdkError` 转换为媒体领域 `MediaError`。
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

    /// Convert a `StreamKey` to a `MediaKey` using the reversible bridge.
    ///
    /// 使用可逆桥接将 `StreamKey` 转换为 `MediaKey`。
    fn stream_key_to_media_key(key: &StreamKey) -> MediaKey {
        StreamKeyBridge::from_namespace_path(&key.namespace, &key.path).unwrap_or_else(|_| {
            MediaKey::new("__fallback__", &key.namespace, &key.path, None).unwrap()
        })
    }

    /// Convert a `MediaKey` to a `StreamKey`.
    ///
    /// 将 `MediaKey` 转换为 `StreamKey`。
    fn media_key_to_stream_key(key: &MediaKey) -> StreamKey {
        let (namespace, path) = StreamKeyBridge::to_namespace_path(key);
        StreamKey::new(namespace, path)
    }

    /// Convert codec `TrackInfo` to a media-domain `TrackSummary`.
    ///
    /// 将 codec `TrackInfo` 转换为媒体领域 `TrackSummary`。
    fn track_summary(t: &TrackInfo) -> TrackSummary {
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

    /// Build a `StreamInfo` from a `StreamSnapshot`.
    ///
    /// 从 `StreamSnapshot` 构建 `StreamInfo`。
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
            readers: snapshot.subscriber_count as u64,
            publishers: if snapshot.publisher_active { 1 } else { 0 },
            bytes_in: 0,
            bytes_out: 0,
            duration_ms: 0,
            tracks: snapshot.tracks.iter().map(Self::track_summary).collect(),
            urls: Vec::new(),
            metadata: std::collections::HashMap::new(),
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn media_kind_to_type(k: MediaKind) -> MediaType {
    match k {
        MediaKind::Video => MediaType::Video,
        MediaKind::Audio => MediaType::Audio,
        MediaKind::Data | MediaKind::Subtitle => MediaType::Data,
    }
}

fn codec_to_api(c: CodecId) -> CodecKind {
    match c {
        CodecId::H264 => CodecKind::H264,
        CodecId::H265 => CodecKind::H265,
        CodecId::AV1 => CodecKind::Av1,
        CodecId::AAC => CodecKind::Aac,
        CodecId::Opus => CodecKind::Opus,
        CodecId::G711A => CodecKind::G711A,
        CodecId::G711U => CodecKind::G711U,
        CodecId::MP3 => CodecKind::Mp3,
        _ => CodecKind::Unknown,
    }
}

fn readiness_to_api(r: CodecTrackReadiness) -> TrackReadiness {
    match r {
        CodecTrackReadiness::NotReady => TrackReadiness::Pending,
        CodecTrackReadiness::PendingConfig => TrackReadiness::Pending,
        CodecTrackReadiness::Ready => TrackReadiness::Ready,
    }
}

#[async_trait]
impl MediaControlApi for StreamMediaProvider {
    async fn get_media_list(
        &self,
        _ctx: &MediaRequestContext,
        mut query: MediaQuery,
    ) -> cheetah_media_api::error::Result<Page<StreamInfo>> {
        query.clamp_page_size();
        let snapshots = self
            .stream_manager
            .list_streams()
            .await
            .map_err(Self::map_sdk_error)?;
        let now = now_ms();
        let items: Vec<StreamInfo> = snapshots
            .into_iter()
            .filter(|s| {
                let key = Self::stream_key_to_media_key(&s.key);
                if let Some(ref v) = query.vhost {
                    if key.vhost.0 != *v {
                        return false;
                    }
                }
                if let Some(ref a) = query.app {
                    if key.app.0 != *a {
                        return false;
                    }
                }
                if let Some(ref st) = query.stream {
                    if key.stream.0 != *st {
                        return false;
                    }
                }
                if let Some(ref schema) = query.schema {
                    if let Ok(schema) = MediaSchema::parse(schema) {
                        if key.schema != Some(schema) {
                            return false;
                        }
                    }
                }
                if let Some(online) = query.online {
                    if s.publisher_active != online {
                        return false;
                    }
                }
                true
            })
            .map(|s| Self::stream_info(&s, now))
            .collect();
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
        _ctx: &MediaRequestContext,
        key: &MediaKey,
    ) -> cheetah_media_api::error::Result<StreamInfo> {
        let stream_key = Self::media_key_to_stream_key(key);
        let snapshot = self
            .stream_manager
            .get_stream(&stream_key)
            .await
            .map_err(Self::map_sdk_error)?
            .ok_or_else(|| MediaError::not_found(format!("stream {key}")))?;
        Ok(Self::stream_info(&snapshot, now_ms()))
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
        _ctx: &MediaRequestContext,
        mut query: SessionQuery,
    ) -> cheetah_media_api::error::Result<Page<SessionInfo>> {
        query.clamp_page_size();
        let snapshots = self
            .stream_manager
            .list_streams()
            .await
            .map_err(Self::map_sdk_error)?;
        let mut items = Vec::new();
        for s in snapshots {
            let key = Self::stream_key_to_media_key(&s.key);
            if s.publisher_active {
                items.push(SessionInfo {
                    session_id: SessionId("publisher".to_string()),
                    kind: SessionKind::Publisher,
                    media_key: key.clone(),
                    remote_endpoint: None,
                    local_endpoint: None,
                    protocol: "internal".to_string(),
                    started_at: 0,
                    last_seen_at: now_ms(),
                    bytes_in: 0,
                    bytes_out: 0,
                    state: SessionState::Connected,
                    close_reason: None,
                });
            }
            for i in 0..s.subscriber_count {
                items.push(SessionInfo {
                    session_id: SessionId(format!("player-{i}")),
                    kind: SessionKind::Player,
                    media_key: key.clone(),
                    remote_endpoint: None,
                    local_endpoint: None,
                    protocol: "internal".to_string(),
                    started_at: 0,
                    last_seen_at: now_ms(),
                    bytes_in: 0,
                    bytes_out: 0,
                    state: SessionState::Connected,
                    close_reason: None,
                });
            }
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

    async fn kick_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &SessionId,
        _reason: CloseReason,
    ) -> cheetah_media_api::error::Result<()> {
        Err(MediaError::unsupported("kick_session"))
    }

    async fn kick_stream(
        &self,
        _ctx: &MediaRequestContext,
        key: &MediaKey,
        reason: CloseReason,
    ) -> cheetah_media_api::error::Result<CloseReport> {
        let stream_key = Self::media_key_to_stream_key(key);
        self.core_adapters
            .close_stream(&stream_key)
            .await
            .map_err(Self::map_sdk_error)?;
        Ok(CloseReport {
            media_key: key.clone(),
            closed_sessions: Vec::new(),
            reason,
        })
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

#[async_trait]
impl PublishSubscribeApi for StreamMediaProvider {
    async fn acquire_publisher(
        &self,
        _ctx: &MediaRequestContext,
        request: PublishRequest,
    ) -> cheetah_media_api::error::Result<PublisherHandle> {
        let stream_key = Self::media_key_to_stream_key(&request.media_key);
        let (lease, _sink) = self
            .publisher_api
            .acquire_publisher(stream_key, PublisherOptions::default())
            .await
            .map_err(Self::map_sdk_error)?;
        Ok(PublisherHandle {
            session_id: SessionId(lease.lease_id.to_string()),
            media_key: request.media_key,
            lease_token: lease.lease_id.to_string(),
        })
    }

    async fn open_subscriber(
        &self,
        _ctx: &MediaRequestContext,
        request: SubscribeRequest,
    ) -> cheetah_media_api::error::Result<SubscriberHandle> {
        let stream_key = Self::media_key_to_stream_key(&request.media_key);
        let _source = self
            .subscriber_api
            .subscribe(stream_key, SubscriberOptions::default())
            .await
            .map_err(Self::map_sdk_error)?;
        Ok(SubscriberHandle {
            session_id: SessionId("0".to_string()),
            media_key: request.media_key,
            output_schema: request.output_schema,
            url: None,
        })
    }

    async fn close_handle(
        &self,
        _ctx: &MediaRequestContext,
        _id: &SessionId,
        _reason: CloseReason,
    ) -> cheetah_media_api::error::Result<()> {
        Err(MediaError::unsupported("close_handle"))
    }
}

/// Stub provider for record capabilities. Wired to a dedicated `RecordApi` in the engine.
///
/// 录制能力的存根 provider。在引擎中接入专用 `RecordApi`。
#[derive(Clone)]
pub struct RecordMediaProvider;

#[async_trait]
impl RecordApi for RecordMediaProvider {
    async fn start_record(
        &self,
        _ctx: &MediaRequestContext,
        _request: StartRecordRequest,
    ) -> cheetah_media_api::error::Result<RecordTask> {
        Err(MediaError::unsupported_capability("record"))
    }

    async fn stop_record(
        &self,
        _ctx: &MediaRequestContext,
        _request: StopRecordRequest,
    ) -> cheetah_media_api::error::Result<RecordTask> {
        Err(MediaError::unsupported_capability("record"))
    }

    async fn query_record_tasks(
        &self,
        _ctx: &MediaRequestContext,
        _query: RecordTaskQuery,
    ) -> cheetah_media_api::error::Result<Page<RecordTask>> {
        Err(MediaError::unsupported_capability("record"))
    }

    async fn query_record_files(
        &self,
        _ctx: &MediaRequestContext,
        _query: RecordFileQuery,
    ) -> cheetah_media_api::error::Result<Page<RecordFile>> {
        Err(MediaError::unsupported_capability("record"))
    }

    async fn delete_record_file(
        &self,
        _ctx: &MediaRequestContext,
        _request: DeleteRecordRequest,
    ) -> cheetah_media_api::error::Result<()> {
        Err(MediaError::unsupported_capability("record"))
    }

    async fn control_record_playback(
        &self,
        _ctx: &MediaRequestContext,
        _command: RecordPlaybackCommand,
    ) -> cheetah_media_api::error::Result<()> {
        Err(MediaError::unsupported_capability("record"))
    }
}

/// Stub provider for snapshot capabilities.
///
/// 快照能力的存根 provider。
#[derive(Clone)]
pub struct SnapshotMediaProvider;

#[async_trait]
impl SnapshotApi for SnapshotMediaProvider {
    async fn take_snapshot(
        &self,
        _ctx: &MediaRequestContext,
        _request: SnapshotRequest,
    ) -> cheetah_media_api::error::Result<SnapshotHandle> {
        Err(MediaError::unsupported_capability("snapshot"))
    }

    async fn query_snapshots(
        &self,
        _ctx: &MediaRequestContext,
        _query: SnapshotQuery,
    ) -> cheetah_media_api::error::Result<Page<SnapshotInfo>> {
        Err(MediaError::unsupported_capability("snapshot"))
    }

    async fn delete_snapshot_directory(
        &self,
        _ctx: &MediaRequestContext,
        _request: DeleteSnapshotRequest,
    ) -> cheetah_media_api::error::Result<()> {
        Err(MediaError::unsupported_capability("snapshot"))
    }
}

/// Stub provider for proxy capabilities.
///
/// 代理能力的存根 provider。
#[derive(Clone)]
pub struct ProxyMediaProvider;

#[async_trait]
impl ProxyApi for ProxyMediaProvider {
    async fn create_pull_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _request: PullProxyRequest,
    ) -> cheetah_media_api::error::Result<ProxyInfo> {
        Err(MediaError::unsupported_capability("proxy"))
    }

    async fn delete_pull_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _id: &ProxyId,
    ) -> cheetah_media_api::error::Result<()> {
        Err(MediaError::unsupported_capability("proxy"))
    }

    async fn list_pull_proxies(
        &self,
        _ctx: &MediaRequestContext,
        _query: ProxyQuery,
    ) -> cheetah_media_api::error::Result<Page<ProxyInfo>> {
        Err(MediaError::unsupported_capability("proxy"))
    }

    async fn create_push_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _request: PushProxyRequest,
    ) -> cheetah_media_api::error::Result<ProxyInfo> {
        Err(MediaError::unsupported_capability("proxy"))
    }

    async fn delete_push_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _id: &ProxyId,
    ) -> cheetah_media_api::error::Result<()> {
        Err(MediaError::unsupported_capability("proxy"))
    }

    async fn create_ffmpeg_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _request: FfmpegProxyRequest,
    ) -> cheetah_media_api::error::Result<ProxyInfo> {
        Err(MediaError::unsupported_capability("proxy"))
    }
}

/// Stub provider for RTP capabilities.
///
/// RTP 能力的存根 provider。
#[derive(Clone)]
pub struct RtpMediaProvider;

#[async_trait]
impl RtpApi for RtpMediaProvider {
    async fn open_rtp_receiver(
        &self,
        _ctx: &MediaRequestContext,
        _request: RtpReceiverRequest,
    ) -> cheetah_media_api::error::Result<RtpSession> {
        Err(MediaError::unsupported_capability("rtp"))
    }

    async fn connect_rtp_receiver(
        &self,
        _ctx: &MediaRequestContext,
        _request: RtpConnectRequest,
    ) -> cheetah_media_api::error::Result<RtpSession> {
        Err(MediaError::unsupported_capability("rtp"))
    }

    async fn open_rtp_sender(
        &self,
        _ctx: &MediaRequestContext,
        _request: RtpSenderRequest,
    ) -> cheetah_media_api::error::Result<RtpSession> {
        Err(MediaError::unsupported_capability("rtp"))
    }

    async fn stop_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &RtpSessionId,
    ) -> cheetah_media_api::error::Result<()> {
        Err(MediaError::unsupported_capability("rtp"))
    }

    async fn list_rtp_sessions(
        &self,
        _ctx: &MediaRequestContext,
        _query: RtpQuery,
    ) -> cheetah_media_api::error::Result<Page<RtpSession>> {
        Err(MediaError::unsupported_capability("rtp"))
    }

    async fn update_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        _request: UpdateRtpRequest,
    ) -> cheetah_media_api::error::Result<RtpSession> {
        Err(MediaError::unsupported_capability("rtp"))
    }
}

/// Combined engine media facade.
///
/// 引擎媒体 facade 组合。
#[derive(Clone)]
pub struct EngineMediaFacade {
    control: Arc<dyn MediaControlApi>,
    publish_subscribe: Arc<dyn PublishSubscribeApi>,
    record: Arc<dyn RecordApi>,
    snapshot: Arc<dyn SnapshotApi>,
    proxy: Arc<dyn cheetah_media_api::port::ProxyApi>,
    rtp: Arc<dyn RtpApi>,
    capabilities: MediaCapabilitySet,
}

impl EngineMediaFacade {
    /// Build a facade with the stream provider and stub providers.
    ///
    /// 使用 stream provider 和存根 provider 构建 facade。
    pub fn new(stream_provider: StreamMediaProvider) -> Self {
        let mut capabilities = MediaCapabilitySet::empty();
        capabilities.add(MediaCapability::Query, 1);
        capabilities.add(MediaCapability::SessionControl, 1);
        capabilities.add(MediaCapability::Publish, 1);
        capabilities.add(MediaCapability::Subscribe, 1);
        Self {
            control: Arc::new(stream_provider.clone()),
            publish_subscribe: Arc::new(stream_provider),
            record: Arc::new(RecordMediaProvider),
            snapshot: Arc::new(SnapshotMediaProvider),
            proxy: Arc::new(ProxyMediaProvider),
            rtp: Arc::new(RtpMediaProvider),
            capabilities,
        }
    }

    /// Set the record provider.
    ///
    /// 设置录制 provider。
    pub fn with_record(mut self, record: Arc<dyn RecordApi>) -> Self {
        self.record = record;
        self.capabilities.add(MediaCapability::Record, 1);
        self
    }

    /// Set the snapshot provider.
    ///
    /// 设置快照 provider。
    pub fn with_snapshot(mut self, snapshot: Arc<dyn SnapshotApi>) -> Self {
        self.snapshot = snapshot;
        self.capabilities.add(MediaCapability::Snapshot, 1);
        self
    }

    /// Set the proxy provider.
    ///
    /// 设置代理 provider。
    pub fn with_proxy(mut self, proxy: Arc<dyn cheetah_media_api::port::ProxyApi>) -> Self {
        self.proxy = proxy;
        self.capabilities.add(MediaCapability::Proxy, 1);
        self
    }

    /// Set the RTP provider.
    ///
    /// 设置 RTP provider。
    pub fn with_rtp(mut self, rtp: Arc<dyn RtpApi>) -> Self {
        self.rtp = rtp;
        self.capabilities.add(MediaCapability::Rtp, 1);
        self
    }
}

#[async_trait]
impl MediaControlApi for EngineMediaFacade {
    async fn get_media_list(
        &self,
        ctx: &MediaRequestContext,
        query: MediaQuery,
    ) -> cheetah_media_api::error::Result<Page<StreamInfo>> {
        self.control.get_media_list(ctx, query).await
    }

    async fn get_media(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
    ) -> cheetah_media_api::error::Result<StreamInfo> {
        self.control.get_media(ctx, key).await
    }

    async fn is_media_online(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
    ) -> cheetah_media_api::error::Result<OnlineState> {
        self.control.is_media_online(ctx, key).await
    }

    async fn list_sessions(
        &self,
        ctx: &MediaRequestContext,
        query: SessionQuery,
    ) -> cheetah_media_api::error::Result<Page<SessionInfo>> {
        self.control.list_sessions(ctx, query).await
    }

    async fn kick_session(
        &self,
        ctx: &MediaRequestContext,
        id: &SessionId,
        reason: CloseReason,
    ) -> cheetah_media_api::error::Result<()> {
        self.control.kick_session(ctx, id, reason).await
    }

    async fn kick_stream(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
        reason: CloseReason,
    ) -> cheetah_media_api::error::Result<CloseReport> {
        self.control.kick_stream(ctx, key, reason).await
    }

    async fn request_keyframe(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
    ) -> cheetah_media_api::error::Result<()> {
        self.control.request_keyframe(ctx, key).await
    }
}

#[async_trait]
impl PublishSubscribeApi for EngineMediaFacade {
    async fn acquire_publisher(
        &self,
        ctx: &MediaRequestContext,
        request: PublishRequest,
    ) -> cheetah_media_api::error::Result<PublisherHandle> {
        self.publish_subscribe.acquire_publisher(ctx, request).await
    }

    async fn open_subscriber(
        &self,
        ctx: &MediaRequestContext,
        request: SubscribeRequest,
    ) -> cheetah_media_api::error::Result<SubscriberHandle> {
        self.publish_subscribe.open_subscriber(ctx, request).await
    }

    async fn close_handle(
        &self,
        ctx: &MediaRequestContext,
        id: &SessionId,
        reason: CloseReason,
    ) -> cheetah_media_api::error::Result<()> {
        self.publish_subscribe.close_handle(ctx, id, reason).await
    }
}

#[async_trait]
impl RecordApi for EngineMediaFacade {
    async fn start_record(
        &self,
        ctx: &MediaRequestContext,
        request: StartRecordRequest,
    ) -> cheetah_media_api::error::Result<RecordTask> {
        self.record.start_record(ctx, request).await
    }

    async fn stop_record(
        &self,
        ctx: &MediaRequestContext,
        request: StopRecordRequest,
    ) -> cheetah_media_api::error::Result<RecordTask> {
        self.record.stop_record(ctx, request).await
    }

    async fn query_record_tasks(
        &self,
        ctx: &MediaRequestContext,
        query: RecordTaskQuery,
    ) -> cheetah_media_api::error::Result<Page<RecordTask>> {
        self.record.query_record_tasks(ctx, query).await
    }

    async fn query_record_files(
        &self,
        ctx: &MediaRequestContext,
        query: RecordFileQuery,
    ) -> cheetah_media_api::error::Result<Page<RecordFile>> {
        self.record.query_record_files(ctx, query).await
    }

    async fn delete_record_file(
        &self,
        ctx: &MediaRequestContext,
        request: DeleteRecordRequest,
    ) -> cheetah_media_api::error::Result<()> {
        self.record.delete_record_file(ctx, request).await
    }

    async fn control_record_playback(
        &self,
        ctx: &MediaRequestContext,
        command: RecordPlaybackCommand,
    ) -> cheetah_media_api::error::Result<()> {
        self.record.control_record_playback(ctx, command).await
    }
}

#[async_trait]
impl SnapshotApi for EngineMediaFacade {
    async fn take_snapshot(
        &self,
        ctx: &MediaRequestContext,
        request: SnapshotRequest,
    ) -> cheetah_media_api::error::Result<SnapshotHandle> {
        self.snapshot.take_snapshot(ctx, request).await
    }

    async fn query_snapshots(
        &self,
        ctx: &MediaRequestContext,
        query: SnapshotQuery,
    ) -> cheetah_media_api::error::Result<Page<SnapshotInfo>> {
        self.snapshot.query_snapshots(ctx, query).await
    }

    async fn delete_snapshot_directory(
        &self,
        ctx: &MediaRequestContext,
        request: DeleteSnapshotRequest,
    ) -> cheetah_media_api::error::Result<()> {
        self.snapshot.delete_snapshot_directory(ctx, request).await
    }
}

#[async_trait]
impl ProxyApi for EngineMediaFacade {
    async fn create_pull_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: PullProxyRequest,
    ) -> cheetah_media_api::error::Result<ProxyInfo> {
        self.proxy.create_pull_proxy(ctx, request).await
    }

    async fn delete_pull_proxy(
        &self,
        ctx: &MediaRequestContext,
        id: &ProxyId,
    ) -> cheetah_media_api::error::Result<()> {
        self.proxy.delete_pull_proxy(ctx, id).await
    }

    async fn list_pull_proxies(
        &self,
        ctx: &MediaRequestContext,
        query: ProxyQuery,
    ) -> cheetah_media_api::error::Result<Page<ProxyInfo>> {
        self.proxy.list_pull_proxies(ctx, query).await
    }

    async fn create_push_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: PushProxyRequest,
    ) -> cheetah_media_api::error::Result<ProxyInfo> {
        self.proxy.create_push_proxy(ctx, request).await
    }

    async fn delete_push_proxy(
        &self,
        ctx: &MediaRequestContext,
        id: &ProxyId,
    ) -> cheetah_media_api::error::Result<()> {
        self.proxy.delete_push_proxy(ctx, id).await
    }

    async fn create_ffmpeg_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: FfmpegProxyRequest,
    ) -> cheetah_media_api::error::Result<ProxyInfo> {
        self.proxy.create_ffmpeg_proxy(ctx, request).await
    }
}

#[async_trait]
impl RtpApi for EngineMediaFacade {
    async fn open_rtp_receiver(
        &self,
        ctx: &MediaRequestContext,
        request: RtpReceiverRequest,
    ) -> cheetah_media_api::error::Result<RtpSession> {
        self.rtp.open_rtp_receiver(ctx, request).await
    }

    async fn connect_rtp_receiver(
        &self,
        ctx: &MediaRequestContext,
        request: RtpConnectRequest,
    ) -> cheetah_media_api::error::Result<RtpSession> {
        self.rtp.connect_rtp_receiver(ctx, request).await
    }

    async fn open_rtp_sender(
        &self,
        ctx: &MediaRequestContext,
        request: RtpSenderRequest,
    ) -> cheetah_media_api::error::Result<RtpSession> {
        self.rtp.open_rtp_sender(ctx, request).await
    }

    async fn stop_rtp_session(
        &self,
        ctx: &MediaRequestContext,
        id: &RtpSessionId,
    ) -> cheetah_media_api::error::Result<()> {
        self.rtp.stop_rtp_session(ctx, id).await
    }

    async fn list_rtp_sessions(
        &self,
        ctx: &MediaRequestContext,
        query: RtpQuery,
    ) -> cheetah_media_api::error::Result<Page<RtpSession>> {
        self.rtp.list_rtp_sessions(ctx, query).await
    }

    async fn update_rtp_session(
        &self,
        ctx: &MediaRequestContext,
        request: UpdateRtpRequest,
    ) -> cheetah_media_api::error::Result<RtpSession> {
        self.rtp.update_rtp_session(ctx, request).await
    }
}

#[async_trait]
impl MediaFacade for EngineMediaFacade {
    fn capabilities(&self) -> MediaCapabilitySet {
        self.capabilities.clone()
    }

    fn subscribe_events(
        &self,
        _sender: Box<dyn cheetah_media_api::event::MediaEventSender>,
    ) -> cheetah_media_api::error::Result<()> {
        Ok(())
    }
}
