use async_trait::async_trait;
use cheetah_media_api::command::*;
use cheetah_media_api::error::MediaError;
use cheetah_media_api::ids::{ProxyId, RecordFileId, RtpSessionId};
use cheetah_media_api::model::{
    Page, ProxyInfo, RecordFile, RecordTask, RtpSession, SnapshotHandle, SnapshotInfo,
};
use cheetah_media_api::port::{MediaRequestContext, ProxyApi, RecordApi, RtpApi, SnapshotApi};

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
        _file_id: &RecordFileId,
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

    async fn list_proxies(
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

    async fn delete_ffmpeg_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _id: &ProxyId,
    ) -> cheetah_media_api::error::Result<()> {
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

    async fn get_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &RtpSessionId,
    ) -> cheetah_media_api::error::Result<RtpSession> {
        Err(MediaError::unsupported_capability("rtp"))
    }
}
