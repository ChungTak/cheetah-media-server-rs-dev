use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::command::*;
use cheetah_media_api::error::MediaError;
use cheetah_media_api::ids::*;
use cheetah_media_api::model::*;
use cheetah_media_api::port::{MediaControlApi, MediaRequestContext, PublishSubscribeApi};
use cheetah_sdk::{CoreAdaptersApi, SdkError, StreamKey, StreamManagerApi};

use super::util::{codec_to_api, media_kind_to_type, now_ms, readiness_to_api};

/// Bridge from `cheetah-sdk` stream primitives to `cheetah-media-api` ports.
///
/// `StreamMediaProvider` implements query, session control, and keyframe request by
/// delegating to the engine's `StreamManagerApi` and `CoreAdaptersApi`. Publish and
/// subscribe are not yet supported through this provider and return `Unsupported`.
/// Other capabilities (record, snapshot, proxy, RTP) are handled by dedicated providers.
///
/// 从 `cheetah-sdk` 流原语到 `cheetah-media-api` 端口的桥接。
///
/// `StreamMediaProvider` 通过委托引擎的 `StreamManagerApi` 和 `CoreAdaptersApi`
/// 实现查询、会话控制和关键帧请求。发布和订阅尚未通过该 provider 支持，返回
/// `Unsupported`。其它能力（录制、快照、代理、RTP）由专用 provider 处理。
#[derive(Clone)]
pub struct StreamMediaProvider {
    stream_manager: Arc<dyn StreamManagerApi>,
    core_adapters: Arc<dyn CoreAdaptersApi>,
}

impl StreamMediaProvider {
    /// Create a new provider backed by the engine stream manager.
    ///
    /// 创建由引擎流管理器支撑的 provider。
    pub fn new(
        stream_manager: Arc<dyn StreamManagerApi>,
        core_adapters: Arc<dyn CoreAdaptersApi>,
    ) -> Self {
        Self {
            stream_manager,
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

    /// Check whether a `SessionInfo` matches the supplied `SessionQuery`.
    ///
    /// 检查 `SessionInfo` 是否匹配给定的 `SessionQuery`。
    fn session_matches_query(session: &SessionInfo, key: &MediaKey, query: &SessionQuery) -> bool {
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
        if let Some(kind) = query.kind {
            if session.kind != kind {
                return false;
            }
        }
        if let Some(state) = query.state {
            if session.state != state {
                return false;
            }
        }
        true
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
                    if let Ok(parsed) = MediaSchema::parse(schema) {
                        if let Some(ref key_schema) = key.schema {
                            if *key_schema != parsed {
                                return false;
                            }
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
                let session = SessionInfo {
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
                };
                if Self::session_matches_query(&session, &key, &query) {
                    items.push(session);
                }
            }
            for i in 0..s.subscriber_count {
                let session = SessionInfo {
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
                };
                if Self::session_matches_query(&session, &key, &query) {
                    items.push(session);
                }
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
        _request: PublishRequest,
    ) -> cheetah_media_api::error::Result<PublisherHandle> {
        Err(MediaError::unsupported_capability("publish"))
    }

    async fn open_subscriber(
        &self,
        _ctx: &MediaRequestContext,
        _request: SubscribeRequest,
    ) -> cheetah_media_api::error::Result<SubscriberHandle> {
        Err(MediaError::unsupported_capability("subscribe"))
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
