use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use async_trait::async_trait;
use cheetah_media_api::command::SessionQuery;
use cheetah_media_api::error::{MediaError, Result as MediaResult};
use cheetah_media_api::event::{MediaEvent, MediaEventBusApi, SessionClosed, SessionOpened};
use cheetah_media_api::ids::{MediaKey, SessionId};
use cheetah_media_api::model::{CloseReason, CloseReport, Page, SessionInfo, SessionState};
use cheetah_media_api::port::MediaRequestContext;
use cheetah_sdk::media_session::{MediaSessionDirectoryApi, SessionCloseHandle};
use dashmap::DashMap;
use parking_lot::RwLock;

use super::util::{event_header, now_ms};

struct Record {
    info: SessionInfo,
    close_handle: Box<dyn SessionCloseHandle>,
}

/// In-memory session directory backed by the engine.
///
/// 由引擎支撑的内存会话目录。
#[derive(Clone)]
pub struct EngineMediaSessionDirectory {
    inner: Arc<Inner>,
}

struct Inner {
    sessions: DashMap<SessionId, Record>,
    next_id: AtomicU64,
    media_event_bus: RwLock<Option<Arc<dyn MediaEventBusApi>>>,
}

impl EngineMediaSessionDirectory {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                sessions: DashMap::new(),
                next_id: AtomicU64::new(1),
                media_event_bus: RwLock::new(None),
            }),
        }
    }

    /// Attach the typed media event bus so directory changes are published.
    pub fn set_media_event_bus(&self, bus: Arc<dyn MediaEventBusApi>) {
        *self.inner.media_event_bus.write() = Some(bus);
    }

    fn new_id(&self) -> SessionId {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        SessionId(format!("sess-{id:016x}"))
    }

    fn publish(&self, event: MediaEvent) {
        if let Some(bus) = self.inner.media_event_bus.read().as_ref() {
            let _ = bus.publish(event);
        }
    }

    fn emit_session_opened(&self, record: &SessionInfo) {
        let mut header = event_header("session-directory", Some(&record.media_key), None);
        header.correlation_id = Some(record.session_id.0.clone());
        self.publish(MediaEvent::SessionOpened(SessionOpened {
            header,
            kind: record.kind,
            session_id: record.session_id.clone(),
            remote_endpoint: record.remote_endpoint.clone(),
            protocol: record.protocol.clone(),
        }));
    }

    fn emit_session_closed(&self, record: &SessionInfo, reason: CloseReason) {
        let mut header = event_header("session-directory", Some(&record.media_key), None);
        header.correlation_id = Some(record.session_id.0.clone());
        self.publish(MediaEvent::SessionClosed(SessionClosed {
            header,
            kind: record.kind,
            session_id: record.session_id.clone(),
            reason,
        }));
    }

    fn matches(record: &SessionInfo, query: &SessionQuery) -> bool {
        if let Some(ref v) = query.vhost {
            if record.media_key.vhost.0 != *v {
                return false;
            }
        }
        if let Some(ref a) = query.app {
            if record.media_key.app.0 != *a {
                return false;
            }
        }
        if let Some(ref s) = query.stream {
            if record.media_key.stream.0 != *s {
                return false;
            }
        }
        if let Some(kind) = query.kind {
            if record.kind != kind {
                return false;
            }
        }
        if let Some(state) = query.state {
            if record.state != state {
                return false;
            }
        }
        true
    }
}

impl Default for EngineMediaSessionDirectory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MediaSessionDirectoryApi for EngineMediaSessionDirectory {
    async fn register_session(
        &self,
        _ctx: &MediaRequestContext,
        mut record: SessionInfo,
        close_handle: Box<dyn SessionCloseHandle>,
    ) -> MediaResult<SessionId> {
        let id = if record.session_id.0.is_empty() {
            self.new_id()
        } else {
            record.session_id.clone()
        };
        record.session_id = id.clone();
        if record.started_at == 0 {
            record.started_at = now_ms();
        }
        if record.last_seen_at == 0 {
            record.last_seen_at = record.started_at;
        }
        match self.inner.sessions.entry(id.clone()) {
            dashmap::mapref::entry::Entry::Occupied(_) => {
                Err(MediaError::already_exists(format!("session {id}")))
            }
            dashmap::mapref::entry::Entry::Vacant(e) => {
                self.emit_session_opened(&record);
                e.insert(Record {
                    info: record,
                    close_handle,
                });
                Ok(id)
            }
        }
    }

    async fn unregister_session(
        &self,
        _ctx: &MediaRequestContext,
        id: &SessionId,
    ) -> MediaResult<()> {
        if let Some((_, record)) = self.inner.sessions.remove(id) {
            self.emit_session_closed(&record.info, CloseReason::Normal);
        }
        Ok(())
    }

    async fn update_state(
        &self,
        _ctx: &MediaRequestContext,
        id: &SessionId,
        state: SessionState,
    ) -> MediaResult<()> {
        if let Some(mut entry) = self.inner.sessions.get_mut(id) {
            entry.info.state = state;
            entry.info.last_seen_at = now_ms();
            Ok(())
        } else {
            Err(MediaError::not_found(format!("session {id}")))
        }
    }

    async fn touch_session(&self, _ctx: &MediaRequestContext, id: &SessionId) -> MediaResult<()> {
        if let Some(mut entry) = self.inner.sessions.get_mut(id) {
            entry.info.last_seen_at = now_ms();
            Ok(())
        } else {
            Err(MediaError::not_found(format!("session {id}")))
        }
    }

    async fn get_session(
        &self,
        _ctx: &MediaRequestContext,
        id: &SessionId,
    ) -> MediaResult<Option<SessionInfo>> {
        Ok(self.inner.sessions.get(id).map(|e| e.info.clone()))
    }

    async fn list_sessions(
        &self,
        _ctx: &MediaRequestContext,
        mut query: SessionQuery,
    ) -> MediaResult<Page<SessionInfo>> {
        query.clamp_page_size();
        let mut items: Vec<SessionInfo> = self
            .inner
            .sessions
            .iter()
            .map(|e| e.value().info.clone())
            .filter(|info| Self::matches(info, &query))
            .collect();
        let total = items.len() as u64;
        let page = query.page.max(1);
        let page_size = query.page_size;
        let start = ((page - 1) * page_size) as usize;
        let paged = items
            .drain(start.min(items.len())..)
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

    async fn close_session(
        &self,
        _ctx: &MediaRequestContext,
        id: &SessionId,
        reason: CloseReason,
    ) -> MediaResult<CloseReport> {
        let record = self.inner.sessions.remove(id).map(|(_, r)| r);
        match record {
            Some(Record { info, close_handle }) => {
                let key = info.media_key.clone();
                let closed_id = close_handle.close(reason.clone()).await?;
                self.emit_session_closed(&info, reason.clone());
                Ok(CloseReport {
                    media_key: key,
                    closed_sessions: vec![closed_id],
                    reason,
                })
            }
            None => Err(MediaError::not_found(format!("session {id}"))),
        }
    }

    async fn close_sessions_for_key(
        &self,
        _ctx: &MediaRequestContext,
        key: &MediaKey,
        reason: CloseReason,
    ) -> MediaResult<CloseReport> {
        let mut closed = Vec::new();
        let mut handles: Vec<(SessionId, SessionInfo, Box<dyn SessionCloseHandle>)> = Vec::new();
        {
            let keys_to_remove: Vec<SessionId> = self
                .inner
                .sessions
                .iter()
                .filter(|e| e.value().info.media_key == *key)
                .map(|e| e.key().clone())
                .collect();
            for id in keys_to_remove {
                if let Some((_, record)) = self.inner.sessions.remove(&id) {
                    handles.push((id, record.info, record.close_handle));
                }
            }
        }
        for (id, info, handle) in handles {
            match handle.close(reason.clone()).await {
                Ok(closed_id) => {
                    self.emit_session_closed(&info, reason.clone());
                    closed.push(closed_id);
                }
                Err(_) => closed.push(id),
            }
        }
        Ok(CloseReport {
            media_key: key.clone(),
            closed_sessions: closed,
            reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_provider::LocalMediaEventBus;
    use cheetah_media_api::event::{MediaEvent, MediaEventBusApi, MediaEventSender};
    use cheetah_media_api::model::SessionKind;
    use cheetah_runtime_tokio::TokioRuntime;
    use cheetah_sdk::media_session::MediaSessionDirectoryApi;
    use cheetah_sdk::RuntimeApi;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
    use tokio::time::{timeout, Duration};

    fn dummy_record() -> SessionInfo {
        SessionInfo {
            session_id: SessionId("".to_string()),
            kind: SessionKind::Publisher,
            media_key: MediaKey::new("__defaultVhost__", "live", "test", None).unwrap(),
            remote_endpoint: None,
            local_endpoint: None,
            protocol: "internal".to_string(),
            started_at: 0,
            last_seen_at: 0,
            bytes_in: 0,
            bytes_out: 0,
            state: SessionState::Connected,
            close_reason: None,
            owner_module: "test".to_string(),
        }
    }

    struct DummyCloseHandle;

    #[async_trait]
    impl SessionCloseHandle for DummyCloseHandle {
        async fn close(&self, _reason: CloseReason) -> MediaResult<SessionId> {
            Ok(SessionId("closed".to_string()))
        }
    }

    #[tokio::test]
    async fn register_and_list_sessions() {
        let dir = EngineMediaSessionDirectory::new();
        let mut record = dummy_record();
        record.media_key = MediaKey::new("__defaultVhost__", "live", "alpha", None).unwrap();
        let id = dir
            .register_session(
                &MediaRequestContext::default(),
                record,
                Box::new(DummyCloseHandle),
            )
            .await
            .unwrap();
        let page = dir
            .list_sessions(&MediaRequestContext::default(), SessionQuery::default())
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].session_id, id);
    }

    #[tokio::test]
    async fn close_session_returns_closed_id() {
        let dir = EngineMediaSessionDirectory::new();
        let record = dummy_record();
        let id = dir
            .register_session(
                &MediaRequestContext::default(),
                record,
                Box::new(DummyCloseHandle),
            )
            .await
            .unwrap();
        let report = dir
            .close_session(&MediaRequestContext::default(), &id, CloseReason::Kicked)
            .await
            .unwrap();
        assert_eq!(report.closed_sessions.len(), 1);
        assert!(dir
            .get_session(&MediaRequestContext::default(), &id)
            .await
            .unwrap()
            .is_none());
    }

    struct CollectingMediaEventSender(UnboundedSender<MediaEvent>);

    impl MediaEventSender for CollectingMediaEventSender {
        fn send(&self, event: MediaEvent) -> cheetah_media_api::error::Result<()> {
            let _ = self.0.send(event);
            Ok(())
        }

        fn lagged(&self, _dropped: u64) -> cheetah_media_api::error::Result<()> {
            Ok(())
        }
    }

    async fn recv_event(
        rx: &mut UnboundedReceiver<MediaEvent>,
        deadline: Duration,
    ) -> Option<MediaEvent> {
        timeout(deadline, rx.recv()).await.ok().flatten()
    }

    #[tokio::test]
    async fn media_event_session_lifecycle() {
        let runtime: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let bus = Arc::new(LocalMediaEventBus::new(runtime));
        let (tx, mut rx) = unbounded_channel();
        let _sub = bus
            .subscribe(Box::new(CollectingMediaEventSender(tx)), 8)
            .unwrap();

        let dir = EngineMediaSessionDirectory::new();
        dir.set_media_event_bus(bus);

        let mut record = dummy_record();
        record.media_key =
            MediaKey::new("__defaultVhost__", "live", "session-event", None).unwrap();
        record.remote_endpoint = Some("5.6.7.8:10000".to_string());
        record.protocol = "rtp".to_string();

        let id = dir
            .register_session(
                &MediaRequestContext::default(),
                record,
                Box::new(DummyCloseHandle),
            )
            .await
            .unwrap();

        let opened = recv_event(&mut rx, Duration::from_millis(100))
            .await
            .expect("SessionOpened event");
        assert!(matches!(opened, MediaEvent::SessionOpened(_)));

        let report = dir
            .close_session(&MediaRequestContext::default(), &id, CloseReason::Kicked)
            .await
            .unwrap();
        assert!(!report.closed_sessions.is_empty());

        let closed = recv_event(&mut rx, Duration::from_millis(100))
            .await
            .expect("SessionClosed event");
        assert!(matches!(closed, MediaEvent::SessionClosed(_)));
    }
}
