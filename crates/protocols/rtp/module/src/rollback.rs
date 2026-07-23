//! Rollback guard for RtpMediaProvider resource allocation.
//!
//! If a session is created (port/socket/task allocated) and a later step fails,
//! the guard stops the session on drop unless `commit()` is called.
//!
//! RTP 模块资源分配回滚保护。

use std::sync::Arc;

use cheetah_runtime_api::RuntimeApi;
use cheetah_sdk::media_api::ids::RtpSessionId;
use tracing::debug;

use crate::orchestrator::RtpSessionOrchestrator;

pub(crate) struct RollbackGuard {
    orchestrator: Arc<RtpSessionOrchestrator>,
    runtime_api: Arc<dyn RuntimeApi>,
    session_id: RtpSessionId,
    committed: bool,
}

impl RollbackGuard {
    pub(crate) fn new(
        orchestrator: Arc<RtpSessionOrchestrator>,
        runtime_api: Arc<dyn RuntimeApi>,
        session_id: RtpSessionId,
    ) -> Self {
        Self {
            orchestrator,
            runtime_api,
            session_id,
            committed: false,
        }
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
