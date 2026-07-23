//! Rollback guard for RtpMediaProvider resource allocation.
//!
//! If a session is created (port/socket/task allocated) and a later step fails,
//! the guard stops the session on drop unless `commit()` is called.
//!
//! RTP 模块资源分配回滚保护。

use std::sync::Arc;

use cheetah_runtime_api::RuntimeApi;
use cheetah_sdk::media_api::ids::{PlaybackSessionId, RtpSessionId};
use cheetah_sdk::media_api::port::PlaybackApi;
use cheetah_sdk::media_api::MediaRequestContext;
use cheetah_sdk::CancellationToken;
use tracing::debug;

use crate::egress::ActiveEgressMap;
use crate::metrics::RtpModuleMetrics;
use crate::orchestrator::RtpSessionOrchestrator;

pub(crate) struct RollbackGuard {
    orchestrator: Arc<RtpSessionOrchestrator>,
    runtime_api: Arc<dyn RuntimeApi>,
    session_id: RtpSessionId,
    egress_cancel: Option<(ActiveEgressMap, CancellationToken)>,
    playback_stop: Option<(Arc<dyn PlaybackApi>, PlaybackSessionId)>,
    metrics: Option<Arc<RtpModuleMetrics>>,
    committed: bool,
}

impl RollbackGuard {
    pub(crate) fn new(
        orchestrator: Arc<RtpSessionOrchestrator>,
        runtime_api: Arc<dyn RuntimeApi>,
        session_id: RtpSessionId,
        metrics: Option<Arc<RtpModuleMetrics>>,
    ) -> Self {
        Self {
            orchestrator,
            runtime_api,
            session_id,
            egress_cancel: None,
            playback_stop: None,
            metrics,
            committed: false,
        }
    }

    /// Attach an already-spawned egress worker cancellation token so it is also
    /// stopped if the open fails before `commit()`.
    pub(crate) fn with_egress_cancel(
        mut self,
        active_senders: ActiveEgressMap,
        cancel: CancellationToken,
    ) -> Self {
        self.egress_cancel = Some((active_senders, cancel));
        self
    }

    /// Attach a playback session so it is stopped on rollback.
    pub(crate) fn with_playback_stop(
        mut self,
        playback: Arc<dyn PlaybackApi>,
        playback_id: PlaybackSessionId,
    ) -> Self {
        self.playback_stop = Some((playback, playback_id));
        self
    }

    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for RollbackGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Some(metrics) = self.metrics.take() {
            metrics.inc_rollback();
        }
        if let Some((active_senders, cancel)) = self.egress_cancel.take() {
            cancel.cancel();
            active_senders.lock().remove(&self.session_id.0);
        }
        if let Some((playback, playback_id)) = self.playback_stop.take() {
            let _ = self.runtime_api.spawn(Box::pin(async move {
                let ctx = MediaRequestContext::default();
                let _ = playback.stop_playback(&ctx, &playback_id).await;
            }));
        }
        let orchestrator = self.orchestrator.clone();
        let session_id = self.session_id.clone();
        debug!(
            "Rolling back RTP session {} due to incomplete open",
            session_id.0
        );
        let _ = self.runtime_api.spawn(Box::pin(async move {
            let _ = orchestrator.stop_rtp_session(&session_id).await;
        }));
    }
}
