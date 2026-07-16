use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::command::*;
use cheetah_media_api::error::{MediaError, Result as MediaResult};
use cheetah_media_api::event::{MediaEventBusApi, MediaEventSender, MediaEventSubscription};
use cheetah_media_api::ids::{
    MediaKey, PlaybackSessionId, ProxyId, RecordFileId, RtpSessionId, SessionId,
};
use cheetah_media_api::image::{ImageArtifact, ImageEncodeApi, ImageEncodeRequest};
use cheetah_media_api::media_file_store::DeleteBatchResult;
use cheetah_media_api::model::{
    AdmissionAction, AdmissionRequest, CloseReason, CloseReport, Decision, OnlineState, Page,
    PlaybackSession, ProxyInfo, PublisherHandle, RecordFile, RecordTask, RtpSession, SessionInfo,
    SnapshotHandle, SnapshotInfo, StreamInfo, SubscriberHandle,
};
use cheetah_media_api::port::{
    MediaControlApi, MediaFacade, MediaRequestContext, PlaybackApi, ProxyApi, PublishSubscribeApi,
    RecordApi, RtpApi, SnapshotApi,
};
use cheetah_media_api::MediaCapabilitySet;
use cheetah_sdk::MediaServices;

/// Engine media facade backed by the runtime `MediaServices` registry.
///
/// 由运行时 `MediaServices` 注册表支撑的引擎媒体 facade。
#[derive(Clone)]
pub struct EngineMediaFacade {
    services: MediaServices,
    media_event_bus: Arc<dyn MediaEventBusApi>,
}

impl EngineMediaFacade {
    /// Build a facade backed by a `MediaServices` registry and a media event bus.
    ///
    /// 使用 `MediaServices` 注册表和媒体事件总线构建 facade。
    pub fn new(services: MediaServices, media_event_bus: Arc<dyn MediaEventBusApi>) -> Self {
        Self {
            services,
            media_event_bus,
        }
    }

    /// Ask the configured admission provider whether a side-effecting media
    /// operation should proceed. A missing provider is treated as allow-all.
    ///
    /// 询问已配置的 admission provider 是否允许执行会产生副作用的媒体操作；
    /// provider 缺失时默认放行。
    async fn check_admission(
        &self,
        ctx: &MediaRequestContext,
        action: AdmissionAction,
        resource: MediaKey,
        protocol: String,
        source_address: Option<String>,
        params: HashMap<String, String>,
    ) -> MediaResult<()> {
        let Some(provider) = self.services.admission() else {
            return Ok(());
        };
        let request = AdmissionRequest {
            action,
            principal: ctx.principal.clone(),
            resource,
            protocol,
            source_address,
            params,
        };
        match provider.authorize(ctx, request).await? {
            Decision::Allow => Ok(()),
            Decision::Deny { code, reason } => Err(MediaError::new(code, reason)),
        }
    }
}

#[async_trait]
impl MediaControlApi for EngineMediaFacade {
    async fn get_media_list(
        &self,
        ctx: &MediaRequestContext,
        mut query: MediaQuery,
    ) -> MediaResult<Page<StreamInfo>> {
        query.clamp_page_size();
        let provider = self
            .services
            .control()
            .ok_or_else(|| MediaError::unavailable("media control"))?;
        provider.get_media_list(ctx, query).await
    }

    async fn get_media(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
    ) -> MediaResult<StreamInfo> {
        let provider = self
            .services
            .control()
            .ok_or_else(|| MediaError::unavailable("media control"))?;
        provider.get_media(ctx, key).await
    }

    async fn is_media_online(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
    ) -> MediaResult<OnlineState> {
        let provider = self
            .services
            .control()
            .ok_or_else(|| MediaError::unavailable("media control"))?;
        provider.is_media_online(ctx, key).await
    }

    async fn list_sessions(
        &self,
        ctx: &MediaRequestContext,
        mut query: SessionQuery,
    ) -> MediaResult<Page<SessionInfo>> {
        query.clamp_page_size();
        let provider = self
            .services
            .control()
            .ok_or_else(|| MediaError::unavailable("media control"))?;
        provider.list_sessions(ctx, query).await
    }

    async fn kick_session(
        &self,
        ctx: &MediaRequestContext,
        id: &SessionId,
        reason: CloseReason,
    ) -> MediaResult<()> {
        let provider = self
            .services
            .control()
            .ok_or_else(|| MediaError::unavailable("media control"))?;
        provider.kick_session(ctx, id, reason).await
    }

    async fn kick_stream(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
        reason: CloseReason,
    ) -> MediaResult<CloseReport> {
        let provider = self
            .services
            .control()
            .ok_or_else(|| MediaError::unavailable("media control"))?;
        provider.kick_stream(ctx, key, reason).await
    }

    async fn request_keyframe(&self, ctx: &MediaRequestContext, key: &MediaKey) -> MediaResult<()> {
        let provider = self
            .services
            .control()
            .ok_or_else(|| MediaError::unavailable("media control"))?;
        provider.request_keyframe(ctx, key).await
    }
}

#[async_trait]
impl PublishSubscribeApi for EngineMediaFacade {
    async fn acquire_publisher(
        &self,
        ctx: &MediaRequestContext,
        request: PublishRequest,
    ) -> MediaResult<PublisherHandle> {
        self.check_admission(
            ctx,
            AdmissionAction::Publish,
            request.media_key.clone(),
            request.protocol.clone(),
            request.origin.clone().or(request.remote_endpoint.clone()),
            request.auth_context.clone(),
        )
        .await?;
        let provider = self
            .services
            .publish_subscribe()
            .ok_or_else(|| MediaError::unavailable("publish"))?;
        provider.acquire_publisher(ctx, request).await
    }

    async fn open_subscriber(
        &self,
        ctx: &MediaRequestContext,
        request: SubscribeRequest,
    ) -> MediaResult<SubscriberHandle> {
        let protocol = if request.protocol.is_empty() {
            request.output_schema.to_string()
        } else {
            request.protocol.clone()
        };
        self.check_admission(
            ctx,
            AdmissionAction::Play,
            request.media_key.clone(),
            protocol,
            request.remote_endpoint.clone(),
            request.auth_context.clone(),
        )
        .await?;
        let provider = self
            .services
            .publish_subscribe()
            .ok_or_else(|| MediaError::unavailable("subscribe"))?;
        provider.open_subscriber(ctx, request).await
    }

    async fn close_handle(
        &self,
        ctx: &MediaRequestContext,
        id: &SessionId,
        reason: CloseReason,
    ) -> MediaResult<()> {
        let provider = self
            .services
            .publish_subscribe()
            .ok_or_else(|| MediaError::unavailable("publish/subscribe"))?;
        provider.close_handle(ctx, id, reason).await
    }
}

#[async_trait]
impl RecordApi for EngineMediaFacade {
    async fn start_record(
        &self,
        ctx: &MediaRequestContext,
        request: StartRecordRequest,
    ) -> MediaResult<RecordTask> {
        let provider = self
            .services
            .record()
            .ok_or_else(|| MediaError::unavailable("record"))?;
        provider.start_record(ctx, request).await
    }

    async fn stop_record(
        &self,
        ctx: &MediaRequestContext,
        request: StopRecordRequest,
    ) -> MediaResult<RecordTask> {
        let provider = self
            .services
            .record()
            .ok_or_else(|| MediaError::unavailable("record"))?;
        provider.stop_record(ctx, request).await
    }

    async fn query_record_tasks(
        &self,
        ctx: &MediaRequestContext,
        mut query: RecordTaskQuery,
    ) -> MediaResult<Page<RecordTask>> {
        query.clamp_page_size();
        let provider = self
            .services
            .record()
            .ok_or_else(|| MediaError::unavailable("record"))?;
        provider.query_record_tasks(ctx, query).await
    }

    async fn query_record_files(
        &self,
        ctx: &MediaRequestContext,
        mut query: RecordFileQuery,
    ) -> MediaResult<Page<RecordFile>> {
        query.clamp_page_size();
        let provider = self
            .services
            .record()
            .ok_or_else(|| MediaError::unavailable("record"))?;
        provider.query_record_files(ctx, query).await
    }

    async fn delete_record_file(
        &self,
        ctx: &MediaRequestContext,
        request: DeleteRecordRequest,
    ) -> MediaResult<()> {
        let provider = self
            .services
            .record()
            .ok_or_else(|| MediaError::unavailable("record"))?;
        provider.delete_record_file(ctx, request).await
    }

    async fn control_record_playback(
        &self,
        ctx: &MediaRequestContext,
        file_id: &RecordFileId,
        command: RecordPlaybackCommand,
    ) -> MediaResult<()> {
        let provider = self
            .services
            .record()
            .ok_or_else(|| MediaError::unavailable("record"))?;
        provider
            .control_record_playback(ctx, file_id, command)
            .await
    }
}

#[async_trait]
impl SnapshotApi for EngineMediaFacade {
    async fn take_snapshot(
        &self,
        ctx: &MediaRequestContext,
        request: SnapshotRequest,
    ) -> MediaResult<SnapshotHandle> {
        let provider = self
            .services
            .snapshot()
            .ok_or_else(|| MediaError::unavailable("snapshot"))?;
        provider.take_snapshot(ctx, request).await
    }

    async fn query_snapshots(
        &self,
        ctx: &MediaRequestContext,
        mut query: SnapshotQuery,
    ) -> MediaResult<Page<SnapshotInfo>> {
        query.clamp_page_size();
        let provider = self
            .services
            .snapshot()
            .ok_or_else(|| MediaError::unavailable("snapshot"))?;
        provider.query_snapshots(ctx, query).await
    }

    async fn delete_snapshots(
        &self,
        ctx: &MediaRequestContext,
        request: DeleteSnapshotRequest,
    ) -> MediaResult<DeleteBatchResult> {
        let provider = self
            .services
            .snapshot()
            .ok_or_else(|| MediaError::unavailable("snapshot"))?;
        provider.delete_snapshots(ctx, request).await
    }
}

#[async_trait]
impl PlaybackApi for EngineMediaFacade {
    async fn open_playback(
        &self,
        ctx: &MediaRequestContext,
        request: OpenPlaybackRequest,
    ) -> MediaResult<PlaybackSession> {
        let provider = self
            .services
            .playback()
            .ok_or_else(|| MediaError::unavailable("playback"))?;
        provider.open_playback(ctx, request).await
    }

    async fn get_playback(
        &self,
        ctx: &MediaRequestContext,
        id: &PlaybackSessionId,
    ) -> MediaResult<PlaybackSession> {
        let provider = self
            .services
            .playback()
            .ok_or_else(|| MediaError::unavailable("playback"))?;
        provider.get_playback(ctx, id).await
    }

    async fn list_playbacks(
        &self,
        ctx: &MediaRequestContext,
        mut query: PlaybackQuery,
    ) -> MediaResult<Page<PlaybackSession>> {
        query.clamp_page_size();
        let provider = self
            .services
            .playback()
            .ok_or_else(|| MediaError::unavailable("playback"))?;
        provider.list_playbacks(ctx, query).await
    }

    async fn control_playback(
        &self,
        ctx: &MediaRequestContext,
        id: &PlaybackSessionId,
        command: PlaybackControl,
    ) -> MediaResult<PlaybackSession> {
        let provider = self
            .services
            .playback()
            .ok_or_else(|| MediaError::unavailable("playback"))?;
        provider.control_playback(ctx, id, command).await
    }

    async fn stop_playback(
        &self,
        ctx: &MediaRequestContext,
        id: &PlaybackSessionId,
    ) -> MediaResult<()> {
        let provider = self
            .services
            .playback()
            .ok_or_else(|| MediaError::unavailable("playback"))?;
        provider.stop_playback(ctx, id).await
    }
}

#[async_trait]
impl ImageEncodeApi for EngineMediaFacade {
    async fn encode(
        &self,
        ctx: &MediaRequestContext,
        request: ImageEncodeRequest,
    ) -> MediaResult<ImageArtifact> {
        let provider = self
            .services
            .image_encode()
            .ok_or_else(|| MediaError::unavailable("image encode"))?;
        provider.encode(ctx, request).await
    }
}

#[async_trait]
impl ProxyApi for EngineMediaFacade {
    async fn create_pull_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: PullProxyRequest,
    ) -> MediaResult<ProxyInfo> {
        let protocol = request
            .source_url
            .split_once("://")
            .map(|(scheme, _)| scheme.to_string())
            .unwrap_or_else(|| "http".to_string());
        self.check_admission(
            ctx,
            AdmissionAction::CreatePullProxy,
            request.destination.clone(),
            protocol,
            Some(request.source_url.clone()),
            HashMap::new(),
        )
        .await?;
        let provider = self
            .services
            .proxy()
            .ok_or_else(|| MediaError::unavailable("proxy"))?;
        provider.create_pull_proxy(ctx, request).await
    }

    async fn delete_pull_proxy(&self, ctx: &MediaRequestContext, id: &ProxyId) -> MediaResult<()> {
        let provider = self
            .services
            .proxy()
            .ok_or_else(|| MediaError::unavailable("proxy"))?;
        provider.delete_pull_proxy(ctx, id).await
    }

    async fn list_pull_proxies(
        &self,
        ctx: &MediaRequestContext,
        mut query: ProxyQuery,
    ) -> MediaResult<Page<ProxyInfo>> {
        query.clamp_page_size();
        let provider = self
            .services
            .proxy()
            .ok_or_else(|| MediaError::unavailable("proxy"))?;
        provider.list_pull_proxies(ctx, query).await
    }

    async fn get_pull_proxy(
        &self,
        ctx: &MediaRequestContext,
        id: &ProxyId,
    ) -> MediaResult<ProxyInfo> {
        let provider = self
            .services
            .proxy()
            .ok_or_else(|| MediaError::unavailable("proxy"))?;
        provider.get_pull_proxy(ctx, id).await
    }

    async fn list_push_proxies(
        &self,
        ctx: &MediaRequestContext,
        mut query: ProxyQuery,
    ) -> MediaResult<Page<ProxyInfo>> {
        query.clamp_page_size();
        let provider = self
            .services
            .proxy()
            .ok_or_else(|| MediaError::unavailable("proxy"))?;
        provider.list_push_proxies(ctx, query).await
    }

    async fn get_push_proxy(
        &self,
        ctx: &MediaRequestContext,
        id: &ProxyId,
    ) -> MediaResult<ProxyInfo> {
        let provider = self
            .services
            .proxy()
            .ok_or_else(|| MediaError::unavailable("proxy"))?;
        provider.get_push_proxy(ctx, id).await
    }

    async fn create_push_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: PushProxyRequest,
    ) -> MediaResult<ProxyInfo> {
        self.check_admission(
            ctx,
            AdmissionAction::CreatePushProxy,
            request.source_media_key.clone(),
            request.protocol.clone(),
            Some(request.destination_url.clone()),
            request.protocol_options.clone(),
        )
        .await?;
        let provider = self
            .services
            .proxy()
            .ok_or_else(|| MediaError::unavailable("proxy"))?;
        provider.create_push_proxy(ctx, request).await
    }

    async fn delete_push_proxy(&self, ctx: &MediaRequestContext, id: &ProxyId) -> MediaResult<()> {
        let provider = self
            .services
            .proxy()
            .ok_or_else(|| MediaError::unavailable("proxy"))?;
        provider.delete_push_proxy(ctx, id).await
    }

    async fn create_ffmpeg_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: FfmpegProxyRequest,
    ) -> MediaResult<ProxyInfo> {
        let protocol = request
            .source_url
            .split_once("://")
            .map(|(scheme, _)| scheme.to_string())
            .unwrap_or_else(|| "http".to_string());
        self.check_admission(
            ctx,
            AdmissionAction::CreateFfmpegProxy,
            request.destination.clone(),
            protocol,
            Some(request.source_url.clone()),
            HashMap::new(),
        )
        .await?;
        let provider = self
            .services
            .proxy()
            .ok_or_else(|| MediaError::unavailable("proxy"))?;
        provider.create_ffmpeg_proxy(ctx, request).await
    }

    async fn delete_ffmpeg_proxy(
        &self,
        ctx: &MediaRequestContext,
        id: &ProxyId,
    ) -> MediaResult<()> {
        let provider = self
            .services
            .proxy()
            .ok_or_else(|| MediaError::unavailable("proxy"))?;
        provider.delete_ffmpeg_proxy(ctx, id).await
    }

    async fn get_ffmpeg_proxy(
        &self,
        ctx: &MediaRequestContext,
        id: &ProxyId,
    ) -> MediaResult<ProxyInfo> {
        let provider = self
            .services
            .proxy()
            .ok_or_else(|| MediaError::unavailable("proxy"))?;
        provider.get_ffmpeg_proxy(ctx, id).await
    }

    async fn list_ffmpeg_proxies(
        &self,
        ctx: &MediaRequestContext,
        mut query: ProxyQuery,
    ) -> MediaResult<Page<ProxyInfo>> {
        query.clamp_page_size();
        let provider = self
            .services
            .proxy()
            .ok_or_else(|| MediaError::unavailable("proxy"))?;
        provider.list_ffmpeg_proxies(ctx, query).await
    }
}

#[async_trait]
impl RtpApi for EngineMediaFacade {
    async fn open_rtp_receiver(
        &self,
        ctx: &MediaRequestContext,
        request: RtpReceiverRequest,
    ) -> MediaResult<RtpSession> {
        self.check_admission(
            ctx,
            AdmissionAction::OpenRtpReceiver,
            request.media_key.clone(),
            "rtp".to_string(),
            request.ip.clone(),
            HashMap::new(),
        )
        .await?;
        let provider = self
            .services
            .rtp()
            .ok_or_else(|| MediaError::unavailable("rtp"))?;
        provider.open_rtp_receiver(ctx, request).await
    }

    async fn connect_rtp_receiver(
        &self,
        ctx: &MediaRequestContext,
        request: RtpConnectRequest,
    ) -> MediaResult<RtpSession> {
        let provider = self
            .services
            .rtp()
            .ok_or_else(|| MediaError::unavailable("rtp"))?;
        provider.connect_rtp_receiver(ctx, request).await
    }

    async fn open_rtp_sender(
        &self,
        ctx: &MediaRequestContext,
        request: RtpSenderRequest,
    ) -> MediaResult<RtpSession> {
        self.check_admission(
            ctx,
            AdmissionAction::OpenRtpSender,
            request.media_key.clone(),
            "rtp".to_string(),
            Some(request.destination_endpoint.clone()),
            HashMap::new(),
        )
        .await?;
        let provider = self
            .services
            .rtp()
            .ok_or_else(|| MediaError::unavailable("rtp"))?;
        provider.open_rtp_sender(ctx, request).await
    }

    async fn stop_rtp_session(
        &self,
        ctx: &MediaRequestContext,
        id: &RtpSessionId,
    ) -> MediaResult<()> {
        let provider = self
            .services
            .rtp()
            .ok_or_else(|| MediaError::unavailable("rtp"))?;
        provider.stop_rtp_session(ctx, id).await
    }

    async fn list_rtp_sessions(
        &self,
        ctx: &MediaRequestContext,
        mut query: RtpQuery,
    ) -> MediaResult<Page<RtpSession>> {
        query.clamp_page_size();
        let provider = self
            .services
            .rtp()
            .ok_or_else(|| MediaError::unavailable("rtp"))?;
        provider.list_rtp_sessions(ctx, query).await
    }

    async fn update_rtp_session(
        &self,
        ctx: &MediaRequestContext,
        request: UpdateRtpRequest,
    ) -> MediaResult<RtpSession> {
        let provider = self
            .services
            .rtp()
            .ok_or_else(|| MediaError::unavailable("rtp"))?;
        provider.update_rtp_session(ctx, request).await
    }

    async fn get_rtp_session(
        &self,
        ctx: &MediaRequestContext,
        id: &RtpSessionId,
    ) -> MediaResult<RtpSession> {
        let provider = self
            .services
            .rtp()
            .ok_or_else(|| MediaError::unavailable("rtp"))?;
        provider.get_rtp_session(ctx, id).await
    }
}

impl MediaFacade for EngineMediaFacade {
    fn capabilities(&self) -> MediaCapabilitySet {
        self.services.capabilities()
    }

    fn subscribe_events(
        &self,
        sender: Box<dyn MediaEventSender>,
    ) -> MediaResult<Box<dyn MediaEventSubscription>> {
        self.media_event_bus.subscribe(sender, 256)
    }
}
