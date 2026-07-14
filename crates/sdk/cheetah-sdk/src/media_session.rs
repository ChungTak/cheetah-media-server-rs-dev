use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::command::SessionQuery;
use cheetah_media_api::error::{MediaError, Result as MediaResult};
use cheetah_media_api::ids::{MediaKey, SessionId};
use cheetah_media_api::model::{CloseReason, CloseReport, Page, SessionInfo, SessionState};
use cheetah_media_api::port::MediaRequestContext;

/// Handle used by the session directory to close a registered session.
///
/// 会话目录用来关闭已注册会话的句柄。
#[async_trait]
pub trait SessionCloseHandle: Send + Sync {
    /// Close the session and return its id.
    async fn close(&self, reason: CloseReason) -> MediaResult<SessionId>;
}

/// Runtime-neutral directory of active media sessions.
///
/// 运行时无关的活动媒体会话目录。
#[async_trait]
pub trait MediaSessionDirectoryApi: Send + Sync {
    /// Register a session and return a globally unique session id.
    async fn register_session(
        &self,
        ctx: &MediaRequestContext,
        record: SessionInfo,
        close_handle: Box<dyn SessionCloseHandle>,
    ) -> MediaResult<SessionId>;

    /// Remove a session from the directory without invoking its close handle.
    async fn unregister_session(
        &self,
        ctx: &MediaRequestContext,
        id: &SessionId,
    ) -> MediaResult<()>;

    /// Update the state of an existing session.
    async fn update_state(
        &self,
        ctx: &MediaRequestContext,
        id: &SessionId,
        state: SessionState,
    ) -> MediaResult<()>;

    /// Update the last-seen timestamp for a session.
    async fn touch_session(&self, ctx: &MediaRequestContext, id: &SessionId) -> MediaResult<()>;

    /// Get a single session by id.
    async fn get_session(
        &self,
        ctx: &MediaRequestContext,
        id: &SessionId,
    ) -> MediaResult<Option<SessionInfo>>;

    /// List sessions matching the query, paginated.
    async fn list_sessions(
        &self,
        ctx: &MediaRequestContext,
        query: SessionQuery,
    ) -> MediaResult<Page<SessionInfo>>;

    /// Close a single session by id.
    async fn close_session(
        &self,
        ctx: &MediaRequestContext,
        id: &SessionId,
        reason: CloseReason,
    ) -> MediaResult<CloseReport>;

    /// Close every session associated with a media key.
    async fn close_sessions_for_key(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
        reason: CloseReason,
    ) -> MediaResult<CloseReport>;
}

/// No-op directory used before the engine is fully wired.
///
/// 在引擎完成接线之前使用的空目录。
pub struct NoopMediaSessionDirectory;

#[async_trait]
impl MediaSessionDirectoryApi for NoopMediaSessionDirectory {
    async fn register_session(
        &self,
        _ctx: &MediaRequestContext,
        _record: SessionInfo,
        _close_handle: Box<dyn SessionCloseHandle>,
    ) -> MediaResult<SessionId> {
        Err(MediaError::unavailable("session directory"))
    }

    async fn unregister_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &SessionId,
    ) -> MediaResult<()> {
        Err(MediaError::unavailable("session directory"))
    }

    async fn update_state(
        &self,
        _ctx: &MediaRequestContext,
        _id: &SessionId,
        _state: SessionState,
    ) -> MediaResult<()> {
        Err(MediaError::unavailable("session directory"))
    }

    async fn touch_session(&self, _ctx: &MediaRequestContext, _id: &SessionId) -> MediaResult<()> {
        Err(MediaError::unavailable("session directory"))
    }

    async fn get_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &SessionId,
    ) -> MediaResult<Option<SessionInfo>> {
        Err(MediaError::unavailable("session directory"))
    }

    async fn list_sessions(
        &self,
        _ctx: &MediaRequestContext,
        _query: SessionQuery,
    ) -> MediaResult<Page<SessionInfo>> {
        Err(MediaError::unavailable("session directory"))
    }

    async fn close_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &SessionId,
        _reason: CloseReason,
    ) -> MediaResult<CloseReport> {
        Err(MediaError::unavailable("session directory"))
    }

    async fn close_sessions_for_key(
        &self,
        _ctx: &MediaRequestContext,
        _key: &MediaKey,
        _reason: CloseReason,
    ) -> MediaResult<CloseReport> {
        Err(MediaError::unavailable("session directory"))
    }
}

/// Create a no-op session directory.
pub fn default_session_directory() -> Arc<dyn MediaSessionDirectoryApi> {
    Arc::new(NoopMediaSessionDirectory)
}
