//! Bridge `cheetah_media_api::port::RtpApi` to the module's shared
//! `RtpSessionOrchestrator`.
//!
//! 由模块共享的 `RtpSessionOrchestrator` 支撑的 `RtpApi` provider。

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_rtp_driver_tokio::RtpDriverHandle;
use cheetah_sdk::media_api::command::{
    RtpConnectRequest, RtpQuery, RtpReceiverRequest, RtpSenderRequest, UpdateRtpRequest,
};
use cheetah_sdk::media_api::error::Result;
use cheetah_sdk::media_api::ids::StreamKeyBridge;
use cheetah_sdk::media_api::ids::{MediaKey, RtpSessionId};
use cheetah_sdk::media_api::model::{Page, RtpSession};
use cheetah_sdk::media_api::port::{MediaRequestContext, RtpApi};
use cheetah_sdk::{CancellationToken, EngineContext, StreamKey};

use crate::egress::{run_egress_session, ActiveEgressMap, EgressCleanup};
use crate::orchestrator::RtpSessionOrchestrator;

/// Media-domain `RtpApi` provider.
///
/// `RtpApi` provider。
pub struct RtpMediaProvider {
    orchestrator: Arc<RtpSessionOrchestrator>,
    engine: EngineContext,
    module_cancel: CancellationToken,
    /// Active sender egress tasks keyed by session key so `stop_rtp_session` can cancel them.
    active_senders: ActiveEgressMap,
}

impl RtpMediaProvider {
    /// Create a provider backed by the shared orchestrator.
    ///
    /// 创建由共享编排器支撑的 provider。
    pub fn new(
        orchestrator: Arc<RtpSessionOrchestrator>,
        engine: EngineContext,
        module_cancel: CancellationToken,
    ) -> Self {
        Self {
            orchestrator,
            engine,
            module_cancel,
            active_senders: ActiveEgressMap::default(),
        }
    }

    /// Return the orchestrator so the module can share the same instance with
    /// the HTTP service.
    ///
    /// 返回编排器，以便模块将它与 HTTP 服务共享。
    pub fn orchestrator(&self) -> Arc<RtpSessionOrchestrator> {
        self.orchestrator.clone()
    }

    fn driver(&self) -> Result<Arc<RtpDriverHandle>> {
        self.orchestrator.driver()
    }

    /// Build the `StreamKey` that the engine uses for a given `MediaKey`.
    fn stream_key_for_media_key(media_key: &MediaKey) -> StreamKey {
        let (namespace, path) = StreamKeyBridge::to_namespace_path(media_key);
        StreamKey::new(namespace, path)
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
        request: RtpConnectRequest,
    ) -> Result<RtpSession> {
        self.orchestrator.connect_rtp_receiver(request).await
    }

    async fn open_rtp_sender(
        &self,
        _ctx: &MediaRequestContext,
        request: RtpSenderRequest,
    ) -> Result<RtpSession> {
        // Create the driver-side sender session first.
        let session = self.orchestrator.open_rtp_sender(request.clone()).await?;
        let session_key = session.session_id.0.clone();

        // Determine the engine stream we need to subscribe to.
        let stream_key = Self::stream_key_for_media_key(&request.media_key);

        let driver = self.driver()?;
        let cancel = self.module_cancel.child_token();
        self.active_senders
            .lock()
            .insert(session_key.clone(), cancel.clone());

        let engine = self.engine.clone();
        let orchestrator = self.orchestrator.clone();
        let cleanup = EgressCleanup::new(self.active_senders.clone(), session_key.clone());
        let runtime_api = self.engine.runtime_api.clone();
        runtime_api.spawn(Box::pin(async move {
            run_egress_session(
                engine,
                driver,
                vec![session_key],
                stream_key,
                cancel,
                Some(orchestrator),
                Some(cleanup),
            )
            .await;
        }));

        Ok(session)
    }

    async fn stop_rtp_session(&self, _ctx: &MediaRequestContext, id: &RtpSessionId) -> Result<()> {
        if let Some(cancel) = self.active_senders.lock().remove(&id.0) {
            cancel.cancel();
        }
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
