use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::command::*;
use cheetah_media_api::ids::{MediaKey, ProxyId, RecordFileId, RtpSessionId, SessionId};
use cheetah_media_api::model::{
    CloseReason, CloseReport, OnlineState, Page, ProxyInfo, PublisherHandle, RecordFile,
    RecordTask, RtpSession, SessionInfo, SnapshotHandle, SnapshotInfo, StreamInfo,
    SubscriberHandle,
};
use cheetah_media_api::port::{
    MediaControlApi, MediaFacade, MediaRequestContext, ProxyApi, PublishSubscribeApi, RecordApi,
    RtpApi, SnapshotApi,
};
use cheetah_media_api::{MediaCapability, MediaCapabilitySet};

use super::stream::StreamMediaProvider;
use super::stub::{
    ProxyMediaProvider, RecordMediaProvider, RtpMediaProvider, SnapshotMediaProvider,
};

/// Combined engine media facade.
///
/// 引擎媒体 facade 组合。
#[derive(Clone)]
pub struct EngineMediaFacade {
    control: Arc<dyn MediaControlApi>,
    publish_subscribe: Arc<dyn PublishSubscribeApi>,
    record: Arc<dyn RecordApi>,
    snapshot: Arc<dyn SnapshotApi>,
    proxy: Arc<dyn ProxyApi>,
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
        self.capabilities.remove(MediaCapability::Record);
        self.capabilities.add(MediaCapability::Record, 1);
        self
    }

    /// Set the snapshot provider.
    ///
    /// 设置快照 provider。
    pub fn with_snapshot(mut self, snapshot: Arc<dyn SnapshotApi>) -> Self {
        self.snapshot = snapshot;
        self.capabilities.remove(MediaCapability::Snapshot);
        self.capabilities.add(MediaCapability::Snapshot, 1);
        self
    }

    /// Set the proxy provider.
    ///
    /// 设置代理 provider。
    pub fn with_proxy(mut self, proxy: Arc<dyn ProxyApi>) -> Self {
        self.proxy = proxy;
        self.capabilities.remove(MediaCapability::Proxy);
        self.capabilities.add(MediaCapability::Proxy, 1);
        self
    }

    /// Set the RTP provider.
    ///
    /// 设置 RTP provider。
    pub fn with_rtp(mut self, rtp: Arc<dyn RtpApi>) -> Self {
        self.rtp = rtp;
        self.capabilities.remove(MediaCapability::Rtp);
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
        file_id: &RecordFileId,
        command: RecordPlaybackCommand,
    ) -> cheetah_media_api::error::Result<()> {
        self.record
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
