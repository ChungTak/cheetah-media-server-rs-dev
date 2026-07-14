//! Bridge `cheetah_media_api::port::RtpApi` to the module's shared
//! `RtpSessionOrchestrator`.
//!
//! 由模块共享的 `RtpSessionOrchestrator` 支撑的 `RtpApi` provider。

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::media_api::command::{
    RtpConnectRequest, RtpQuery, RtpReceiverRequest, RtpSenderRequest, UpdateRtpRequest,
};
use cheetah_sdk::media_api::error::{MediaError, Result};
use cheetah_sdk::media_api::ids::RtpSessionId;
use cheetah_sdk::media_api::model::{Page, RtpSession};
use cheetah_sdk::media_api::port::{MediaRequestContext, RtpApi};

use crate::orchestrator::RtpSessionOrchestrator;

/// Media-domain `RtpApi` provider.
///
/// `RtpApi` provider。
pub struct RtpMediaProvider {
    orchestrator: Arc<RtpSessionOrchestrator>,
}

impl RtpMediaProvider {
    /// Create a provider backed by the shared orchestrator.
    ///
    /// 创建由共享编排器支撑的 provider。
    pub fn new(orchestrator: Arc<RtpSessionOrchestrator>) -> Self {
        Self { orchestrator }
    }

    /// Return the orchestrator so the module can share the same instance with
    /// the HTTP service.
    ///
    /// 返回编排器，以便模块将它与 HTTP 服务共享。
    pub fn orchestrator(&self) -> Arc<RtpSessionOrchestrator> {
        self.orchestrator.clone()
    }
}

#[async_trait]
impl RtpApi for RtpMediaProvider {
    async fn open_rtp_receiver(
        &self,
        _ctx: &MediaRequestContext,
        request: RtpReceiverRequest,
    ) -> Result<RtpSession> {
        self.orchestrator.open_rtp_receiver(request).await
    }

    async fn connect_rtp_receiver(
        &self,
        _ctx: &MediaRequestContext,
        _request: RtpConnectRequest,
    ) -> Result<RtpSession> {
        Err(MediaError::unsupported("active RTP receiver connection"))
    }

    async fn open_rtp_sender(
        &self,
        _ctx: &MediaRequestContext,
        request: RtpSenderRequest,
    ) -> Result<RtpSession> {
        self.orchestrator.open_rtp_sender(request).await
    }

    async fn stop_rtp_session(&self, _ctx: &MediaRequestContext, id: &RtpSessionId) -> Result<()> {
        self.orchestrator.stop_rtp_session(id).await
    }

    async fn list_rtp_sessions(
        &self,
        _ctx: &MediaRequestContext,
        query: RtpQuery,
    ) -> Result<Page<RtpSession>> {
        self.orchestrator.list_rtp_sessions(query)
    }

    async fn update_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        request: UpdateRtpRequest,
    ) -> Result<RtpSession> {
        self.orchestrator.update_rtp_session(request)
    }

    async fn get_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        id: &RtpSessionId,
    ) -> Result<RtpSession> {
        self.orchestrator.get_rtp_session(id)
    }
}
