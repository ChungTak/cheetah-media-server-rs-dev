use std::collections::HashMap;

use async_trait::async_trait;
use cheetah_media_api::command::*;
use cheetah_media_api::error::{MediaError, Result as MediaResult};
use cheetah_media_api::event::{MediaEvent, MediaEventSender, MediaEventSubscription};
use cheetah_media_api::ids::*;
use cheetah_media_api::media_file_store::DeleteBatchResult;
use cheetah_media_api::model::*;
use cheetah_media_api::port::{
    MediaControlApi, MediaFacade, MediaRequestContext, PlaybackApi, ProxyApi, PublishSubscribeApi,
    RecordApi, RtpApi, SnapshotApi,
};
use cheetah_media_api::{MediaCapability, MediaCapabilitySet};

/// In-memory test double that implements all media domain ports.
///
/// 实现所有媒体领域端口的内存测试 double。
#[derive(Debug, Default)]
pub struct FakeMediaProvider {
    /// Whether two-way audio (RTP talk) is advertised as supported.
    ///
    /// 是否把双向语音（RTP talk）声明为支持。
    pub talk_supported: bool,

    /// Whether record playback control is advertised as supported.
    ///
    /// 是否把录制回放控制声明为支持。
    pub record_playback_supported: bool,
}

impl FakeMediaProvider {
    /// Build a provider that returns success for every supported capability.
    ///
    /// 构建一个对所有支持的能力返回成功的 provider。
    pub fn new() -> Self {
        Self {
            talk_supported: true,
            record_playback_supported: true,
        }
    }

    /// Disable RTP talk capability in the returned provider.
    ///
    /// 在返回的 provider 中禁用 RTP talk 能力。
    pub fn with_talk(mut self, enabled: bool) -> Self {
        self.talk_supported = enabled;
        self
    }
}

/// Helper to create a default request context for tests.
///
/// 创建测试用默认请求上下文。
pub fn ctx() -> MediaRequestContext {
    MediaRequestContext::default()
}

/// Helper to create a default media key for tests.
///
/// 创建测试用默认媒体键。
pub fn key() -> MediaKey {
    MediaKey::with_default_vhost("live", "test", None).expect("valid test key")
}

/// No-op event sink for tests.
///
/// 测试用的无操作事件接收器。
#[derive(Debug, Default)]
pub struct FakeEventSender;

impl MediaEventSender for FakeEventSender {
    fn send(&self, _event: MediaEvent) -> MediaResult<()> {
        Ok(())
    }

    fn lagged(&self, _dropped: u64) -> MediaResult<()> {
        Ok(())
    }
}

/// No-op subscription handle for tests.
///
/// 测试用的无操作订阅句柄。
#[derive(Debug, Default)]
pub struct FakeSubscription;

impl MediaEventSubscription for FakeSubscription {
    fn id(&self) -> String {
        "fake-subscription".to_string()
    }

    fn unsubscribe(&self) -> MediaResult<()> {
        Ok(())
    }
}

fn empty_page<T: Clone>(page: u64, page_size: u64) -> Page<T> {
    Page {
        items: Vec::new(),
        page,
        page_size,
        total: 0,
        next_cursor: None,
    }
}

#[async_trait]
impl MediaControlApi for FakeMediaProvider {
    async fn get_media_list(
        &self,
        _ctx: &MediaRequestContext,
        query: MediaQuery,
    ) -> MediaResult<Page<StreamInfo>> {
        Ok(empty_page(query.page, query.page_size))
    }

    async fn get_media(
        &self,
        _ctx: &MediaRequestContext,
        key: &MediaKey,
    ) -> MediaResult<StreamInfo> {
        Ok(StreamInfo {
            key: key.clone(),
            origin: None,
            online: OnlineState::Online,
            regist: true,
            created_at: 0,
            last_activity_at: 0,
            readers: 0,
            publishers: 0,
            bytes_in: 0,
            bytes_out: 0,
            duration_ms: 0,
            tracks: Vec::new(),
            urls: Vec::new(),
            metadata: HashMap::new(),
        })
    }

    async fn is_media_online(
        &self,
        _ctx: &MediaRequestContext,
        _key: &MediaKey,
    ) -> MediaResult<OnlineState> {
        Ok(OnlineState::Online)
    }

    async fn list_sessions(
        &self,
        _ctx: &MediaRequestContext,
        query: SessionQuery,
    ) -> MediaResult<Page<SessionInfo>> {
        Ok(empty_page(query.page, query.page_size))
    }

    async fn kick_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &SessionId,
        _reason: CloseReason,
    ) -> MediaResult<()> {
        Ok(())
    }

    async fn kick_stream(
        &self,
        _ctx: &MediaRequestContext,
        key: &MediaKey,
        reason: CloseReason,
    ) -> MediaResult<CloseReport> {
        Ok(CloseReport {
            media_key: key.clone(),
            closed_sessions: Vec::new(),
            reason,
        })
    }

    async fn request_keyframe(
        &self,
        _ctx: &MediaRequestContext,
        _key: &MediaKey,
    ) -> MediaResult<()> {
        Ok(())
    }
}

#[async_trait]
impl PublishSubscribeApi for FakeMediaProvider {
    async fn acquire_publisher(
        &self,
        _ctx: &MediaRequestContext,
        request: PublishRequest,
    ) -> MediaResult<PublisherHandle> {
        Ok(PublisherHandle {
            session_id: SessionId("pub-1".to_string()),
            media_key: request.media_key,
            lease_token: "lease".to_string(),
        })
    }

    async fn open_subscriber(
        &self,
        _ctx: &MediaRequestContext,
        request: SubscribeRequest,
    ) -> MediaResult<SubscriberHandle> {
        Ok(SubscriberHandle {
            session_id: SessionId("sub-1".to_string()),
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
    ) -> MediaResult<()> {
        Ok(())
    }
}

#[async_trait]
impl RecordApi for FakeMediaProvider {
    async fn start_record(
        &self,
        _ctx: &MediaRequestContext,
        request: StartRecordRequest,
    ) -> MediaResult<RecordTask> {
        Ok(RecordTask {
            task_id: RecordTaskId("rec-1".to_string()),
            media_key: request.media_key,
            format: request.format,
            state: RecordTaskState::Running,
            started_at: Some(0),
            ended_at: None,
            duration_ms: 0,
            file_count: 0,
            error: None,
        })
    }

    async fn stop_record(
        &self,
        _ctx: &MediaRequestContext,
        request: StopRecordRequest,
    ) -> MediaResult<RecordTask> {
        Ok(RecordTask {
            task_id: request.task_id,
            media_key: key(),
            format: "mp4".to_string(),
            state: RecordTaskState::Completed,
            started_at: Some(0),
            ended_at: Some(0),
            duration_ms: 0,
            file_count: 0,
            error: None,
        })
    }

    async fn query_record_tasks(
        &self,
        _ctx: &MediaRequestContext,
        query: RecordTaskQuery,
    ) -> MediaResult<Page<RecordTask>> {
        Ok(empty_page(query.page, query.page_size))
    }

    async fn query_record_files(
        &self,
        _ctx: &MediaRequestContext,
        query: RecordFileQuery,
    ) -> MediaResult<Page<RecordFile>> {
        Ok(empty_page(query.page, query.page_size))
    }

    async fn delete_record_file(
        &self,
        _ctx: &MediaRequestContext,
        _request: DeleteRecordRequest,
    ) -> MediaResult<()> {
        Ok(())
    }

    async fn control_record_playback(
        &self,
        _ctx: &MediaRequestContext,
        _file_id: &RecordFileId,
        _command: RecordPlaybackCommand,
    ) -> MediaResult<()> {
        if self.record_playback_supported {
            Ok(())
        } else {
            Err(MediaError::unsupported("record playback"))
        }
    }
}

#[async_trait]
impl SnapshotApi for FakeMediaProvider {
    async fn take_snapshot(
        &self,
        _ctx: &MediaRequestContext,
        request: SnapshotRequest,
    ) -> MediaResult<SnapshotHandle> {
        Ok(SnapshotHandle {
            snapshot_id: SnapshotId("snap-1".to_string()),
            media_key: request.media_key,
            state: SnapshotState::Completed,
            path_handle: FileHandle("snap.jpg".to_string()),
            download_url: None,
            created_at: 0,
        })
    }

    async fn query_snapshots(
        &self,
        _ctx: &MediaRequestContext,
        query: SnapshotQuery,
    ) -> MediaResult<Page<SnapshotInfo>> {
        Ok(empty_page(query.page, query.page_size))
    }

    async fn delete_snapshots(
        &self,
        _ctx: &MediaRequestContext,
        _request: DeleteSnapshotRequest,
    ) -> MediaResult<DeleteBatchResult> {
        Ok(DeleteBatchResult {
            matched: 0,
            deleted: 0,
            failed: 0,
            failures: Vec::new(),
        })
    }
}

#[async_trait]
impl PlaybackApi for FakeMediaProvider {
    async fn open_playback(
        &self,
        _ctx: &MediaRequestContext,
        request: OpenPlaybackRequest,
    ) -> MediaResult<PlaybackSession> {
        Ok(PlaybackSession {
            session_id: PlaybackSessionId("pb-1".to_string()),
            media_key: request.media_key,
            file_handle: request.file_handle,
            state: PlaybackSessionState::Playing,
            duration_ms: 0,
            position_ms: request.start_position_ms,
            scale: request.scale,
            generation: 1,
            output_key: None,
            last_error: None,
            created_at: 0,
            updated_at: 0,
        })
    }

    async fn get_playback(
        &self,
        _ctx: &MediaRequestContext,
        _id: &PlaybackSessionId,
    ) -> MediaResult<PlaybackSession> {
        Err(MediaError::not_found("playback session"))
    }

    async fn list_playbacks(
        &self,
        _ctx: &MediaRequestContext,
        query: PlaybackQuery,
    ) -> MediaResult<Page<PlaybackSession>> {
        Ok(empty_page(query.page, query.page_size))
    }

    async fn control_playback(
        &self,
        _ctx: &MediaRequestContext,
        _id: &PlaybackSessionId,
        command: PlaybackControl,
    ) -> MediaResult<PlaybackSession> {
        let mut session = PlaybackSession {
            session_id: PlaybackSessionId("pb-1".to_string()),
            media_key: key(),
            file_handle: FileHandle("pb.mp4".to_string()),
            state: PlaybackSessionState::Playing,
            duration_ms: 0,
            position_ms: 0,
            scale: 1.0,
            generation: 1,
            output_key: None,
            last_error: None,
            created_at: 0,
            updated_at: 0,
        };
        match command {
            PlaybackControl::Pause => session.state = PlaybackSessionState::Paused,
            PlaybackControl::Resume => session.state = PlaybackSessionState::Playing,
            PlaybackControl::Seek { position_ms } => session.position_ms = position_ms,
            PlaybackControl::SetScale { scale } => session.scale = scale,
        }
        Ok(session)
    }

    async fn stop_playback(
        &self,
        _ctx: &MediaRequestContext,
        _id: &PlaybackSessionId,
    ) -> MediaResult<()> {
        Ok(())
    }
}

#[async_trait]
impl ProxyApi for FakeMediaProvider {
    async fn create_pull_proxy(
        &self,
        _ctx: &MediaRequestContext,
        request: PullProxyRequest,
    ) -> MediaResult<ProxyInfo> {
        Ok(ProxyInfo {
            proxy_id: ProxyId("proxy-1".to_string()),
            kind: ProxyKind::Pull,
            source: request.source_url,
            destination: request.destination,
            state: ProxyState::Created,
            retry_count: 0,
            last_error: None,
            created_at: 0,
            updated_at: 0,
            output_urls: Vec::new(),
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
        query: ProxyQuery,
    ) -> MediaResult<Page<ProxyInfo>> {
        Ok(empty_page(query.page, query.page_size))
    }

    async fn create_push_proxy(
        &self,
        _ctx: &MediaRequestContext,
        request: PushProxyRequest,
    ) -> MediaResult<ProxyInfo> {
        Ok(ProxyInfo {
            proxy_id: ProxyId("proxy-1".to_string()),
            kind: ProxyKind::Push,
            source: request.destination_url,
            destination: request.source_media_key,
            state: ProxyState::Created,
            retry_count: 0,
            last_error: None,
            created_at: 0,
            updated_at: 0,
            output_urls: Vec::new(),
        })
    }

    async fn delete_push_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _id: &ProxyId,
    ) -> MediaResult<()> {
        Ok(())
    }
}

#[async_trait]
impl RtpApi for FakeMediaProvider {
    async fn open_rtp_receiver(
        &self,
        _ctx: &MediaRequestContext,
        request: RtpReceiverRequest,
    ) -> MediaResult<RtpSession> {
        Ok(RtpSession {
            session_id: RtpSessionId("rtp-recv-1".to_string()),
            kind: RtpSessionKind::Receiver,
            media_key: request.media_key,
            local_port: request.port,
            remote_endpoint: None,
            ssrc: request.ssrc,
            payload_type: request.payload_type,
            tcp_mode: request.tcp_mode,
            reuse_port: request.reuse_port,
            state: RtpSessionState::Listening,
            check_paused: false,
            generation: 1,
            created_at: 0,
            updated_at: 0,
            last_error: None,
        })
    }

    async fn connect_rtp_receiver(
        &self,
        _ctx: &MediaRequestContext,
        request: RtpConnectRequest,
    ) -> MediaResult<RtpSession> {
        Ok(RtpSession {
            session_id: request.session_id,
            kind: RtpSessionKind::Receiver,
            media_key: key(),
            local_port: None,
            remote_endpoint: Some(request.remote_endpoint),
            ssrc: request.ssrc,
            payload_type: None,
            tcp_mode: None,
            reuse_port: false,
            state: RtpSessionState::Connected,
            check_paused: false,
            generation: 1,
            created_at: 0,
            updated_at: 0,
            last_error: None,
        })
    }

    async fn open_rtp_sender(
        &self,
        _ctx: &MediaRequestContext,
        request: RtpSenderRequest,
    ) -> MediaResult<RtpSession> {
        if request.mode == RtpSenderMode::Talk && !self.talk_supported {
            return Err(MediaError::unsupported("rtp talk"));
        }
        Ok(RtpSession {
            session_id: RtpSessionId("rtp-send-1".to_string()),
            kind: if request.mode == RtpSenderMode::Talk {
                RtpSessionKind::Talk
            } else {
                RtpSessionKind::Sender
            },
            media_key: request.media_key,
            local_port: None,
            remote_endpoint: Some(request.destination_endpoint),
            ssrc: request.ssrc,
            payload_type: request.payload_type,
            tcp_mode: None,
            reuse_port: false,
            state: RtpSessionState::Created,
            check_paused: false,
            generation: 1,
            created_at: 0,
            updated_at: 0,
            last_error: None,
        })
    }

    async fn stop_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &RtpSessionId,
    ) -> MediaResult<()> {
        Ok(())
    }

    async fn get_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        id: &RtpSessionId,
    ) -> MediaResult<RtpSession> {
        Ok(RtpSession {
            session_id: id.clone(),
            kind: RtpSessionKind::Receiver,
            media_key: key(),
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
        })
    }

    async fn list_rtp_sessions(
        &self,
        _ctx: &MediaRequestContext,
        query: RtpQuery,
    ) -> MediaResult<Page<RtpSession>> {
        Ok(empty_page(query.page, query.page_size))
    }

    async fn update_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        request: UpdateRtpRequest,
    ) -> MediaResult<RtpSession> {
        Ok(RtpSession {
            session_id: request.session_id,
            kind: RtpSessionKind::Receiver,
            media_key: key(),
            local_port: None,
            remote_endpoint: None,
            ssrc: request.ssrc,
            payload_type: request.payload_type,
            tcp_mode: None,
            reuse_port: false,
            state: RtpSessionState::Connected,
            check_paused: false,
            generation: 1,
            created_at: 0,
            updated_at: 0,
            last_error: None,
        })
    }
}

#[async_trait]
impl MediaFacade for FakeMediaProvider {
    fn capabilities(&self) -> MediaCapabilitySet {
        let mut set = MediaCapabilitySet::empty();
        set.add(MediaCapability::Query, 1);
        set.add(MediaCapability::SessionControl, 1);
        set.add(MediaCapability::Publish, 1);
        set.add(MediaCapability::Subscribe, 1);
        set.add(MediaCapability::Record, 1);
        set.add(MediaCapability::Snapshot, 1);
        set.add(MediaCapability::Proxy, 1);
        set.add(MediaCapability::Rtp, 1);
        set.add(MediaCapability::Playback, 1);
        set
    }

    fn subscribe_events(
        &self,
        _sender: Box<dyn MediaEventSender>,
    ) -> MediaResult<Box<dyn MediaEventSubscription>> {
        Ok(Box::new(FakeSubscription))
    }
}
