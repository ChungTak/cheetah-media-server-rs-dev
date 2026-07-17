//! `PlaybackApi` provider backed by the MP4 VOD driver.
//!
//! 基于 MP4 VOD 驱动的 `PlaybackApi` provider。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cheetah_media_api::command::{OpenPlaybackRequest, PlaybackControl, PlaybackQuery};
use cheetah_media_api::error::{MediaError, Result};
use cheetah_media_api::ids::{PlaybackSessionId, StreamKeyBridge};
use cheetah_media_api::model::{Page, PlaybackSession, PlaybackSessionState};
use cheetah_media_api::port::{MediaRequestContext, PlaybackApi};
use cheetah_media_api::MediaFileStoreApi;
use cheetah_sdk::{Deadline, StreamKey};
use parking_lot::RwLock;

use crate::api::{ControlVodRequest, StopVodRequest, VodApi, VodApiError};
use crate::session_registry::SessionError;

const ALLOWED_SCALES: [f64; 4] = [0.5, 1.0, 2.0, 4.0];

/// Production `PlaybackApi` implementation over MP4 VOD sessions.
///
/// 基于 MP4 VOD 会话的生产 `PlaybackApi` 实现。
pub struct Mp4PlaybackProvider {
    vod: Arc<VodApi>,
    file_store: Arc<dyn MediaFileStoreApi>,
    sessions: Arc<RwLock<HashMap<String, PlaybackSession>>>,
    /// file_handle.0 -> session_id for record compatibility shims.
    by_file: Arc<RwLock<HashMap<String, String>>>,
}

impl Mp4PlaybackProvider {
    pub fn new(vod: Arc<VodApi>, file_store: Arc<dyn MediaFileStoreApi>) -> Self {
        Self {
            vod,
            file_store,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            by_file: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Stop all sessions (module stop / restart).
    pub fn shutdown_all(&self) {
        let ids: Vec<String> = self.sessions.read().keys().cloned().collect();
        for id in ids {
            let _ = self.vod.stop(StopVodRequest {
                session_id: id.clone(),
            });
            self.sessions.write().remove(&id);
        }
        self.by_file.write().clear();
    }

    /// Locate an existing session for a file handle, if any.
    pub fn session_for_file(&self, file_handle: &str) -> Option<PlaybackSession> {
        let id = self.by_file.read().get(file_handle).cloned()?;
        self.sessions.read().get(&id).cloned()
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    fn check_deadline(ctx: &MediaRequestContext) -> Result<()> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))
    }

    fn validate_scale(scale: f64) -> Result<()> {
        if !scale.is_finite() || !ALLOWED_SCALES.iter().any(|s| (*s - scale).abs() < 1e-9) {
            return Err(MediaError::unsupported(format!(
                "playback scale {scale} not supported; allowed: {:?}",
                ALLOWED_SCALES
            )));
        }
        Ok(())
    }

    fn map_vod_err(err: VodApiError) -> MediaError {
        match err {
            VodApiError::InvalidRequest(m) => MediaError::invalid_argument(m),
            VodApiError::NotFound(m) => MediaError::not_found(m),
            VodApiError::Session(SessionError::NotFound(id)) => {
                MediaError::not_found(format!("playback session not found: {id}"))
            }
            VodApiError::Session(SessionError::Duplicate(id)) => {
                MediaError::already_exists(format!("playback session exists: {id}"))
            }
            VodApiError::Session(SessionError::CapacityExceeded(c)) => {
                MediaError::unavailable(format!("playback capacity exceeded: {c}"))
            }
            VodApiError::Driver(m) => {
                if m.contains("not found") || m.contains("NotFound") {
                    MediaError::not_found(m)
                } else {
                    MediaError::internal(format!("vod driver: {m}"))
                }
            }
        }
    }

    fn touch(session: &mut PlaybackSession) {
        session.updated_at = Self::now_ms();
        session.generation = session.generation.saturating_add(1);
    }
}

#[async_trait]
impl PlaybackApi for Mp4PlaybackProvider {
    async fn open_playback(
        &self,
        ctx: &MediaRequestContext,
        request: OpenPlaybackRequest,
    ) -> Result<PlaybackSession> {
        Self::check_deadline(ctx)?;
        Self::validate_scale(request.scale)?;
        if request.start_position_ms < 0 {
            return Err(MediaError::invalid_argument(
                "start_position_ms must be >= 0",
            ));
        }

        let now = Self::now_ms();
        let entry = self.file_store.resolve_for_read(
            ctx,
            &request.file_handle,
            Some(&request.media_key),
            now,
        )?;
        if entry.expires_at_ms.is_some_and(|exp| exp > 0 && exp <= now) {
            return Err(MediaError::unavailable(
                "file is expired or pending deletion",
            ));
        }
        let absolute = PathBuf::from(&entry.absolute_path);
        if !absolute.is_absolute() {
            return Err(MediaError::internal(
                "file store returned non-absolute path",
            ));
        }

        let session_id = format!("pb-{}-{}", request.file_handle.0, now & 0xFFFF_FFFF);
        let (ns, path) = StreamKeyBridge::to_namespace_path(&request.media_key);
        let stream_key = StreamKey::new(ns, path);

        self.vod
            .start_absolute(
                session_id.clone(),
                absolute,
                request.file_handle.0.clone(),
                stream_key,
                request.start_position_ms,
                request.scale as f32,
                Some(self.sessions.clone()),
            )
            .await
            .map_err(Self::map_vod_err)?;

        let session = PlaybackSession {
            session_id: PlaybackSessionId(session_id.clone()),
            media_key: request.media_key.clone(),
            file_handle: request.file_handle.clone(),
            state: PlaybackSessionState::Playing,
            duration_ms: 0,
            position_ms: request.start_position_ms,
            scale: request.scale,
            generation: 1,
            output_key: Some(request.media_key),
            last_error: None,
            created_at: now,
            updated_at: now,
        };
        if let Some(existing_id) = self.by_file.read().get(&request.file_handle.0).cloned() {
            // One active session per file handle: stop the previous one.
            let _ = self
                .vod
                .stop(StopVodRequest {
                    session_id: existing_id.clone(),
                })
                .ok();
            self.sessions.write().remove(&existing_id);
        }
        self.sessions
            .write()
            .insert(session_id.clone(), session.clone());
        self.by_file
            .write()
            .insert(request.file_handle.0, session_id);
        Ok(session)
    }

    async fn get_playback(
        &self,
        ctx: &MediaRequestContext,
        id: &PlaybackSessionId,
    ) -> Result<PlaybackSession> {
        Self::check_deadline(ctx)?;
        self.sessions
            .read()
            .get(&id.0)
            .cloned()
            .ok_or_else(|| MediaError::not_found(format!("playback session not found: {}", id.0)))
    }

    async fn list_playbacks(
        &self,
        ctx: &MediaRequestContext,
        mut query: PlaybackQuery,
    ) -> Result<Page<PlaybackSession>> {
        Self::check_deadline(ctx)?;
        query.clamp_page_size();
        let mut items: Vec<PlaybackSession> = self
            .sessions
            .read()
            .values()
            .filter(|s| {
                if let Some(ref v) = query.vhost {
                    if s.media_key.vhost.0 != *v {
                        return false;
                    }
                }
                if let Some(ref a) = query.app {
                    if s.media_key.app.0 != *a {
                        return false;
                    }
                }
                if let Some(ref st) = query.stream {
                    if s.media_key.stream.0 != *st {
                        return false;
                    }
                }
                if let Some(state) = query.state {
                    if s.state != state {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        let total = items.len() as u64;
        let page = query.page.max(1);
        let page_size = query.page_size;
        let start = ((page - 1) * page_size) as usize;
        let paged = items
            .into_iter()
            .skip(start)
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

    async fn control_playback(
        &self,
        ctx: &MediaRequestContext,
        id: &PlaybackSessionId,
        command: PlaybackControl,
    ) -> Result<PlaybackSession> {
        Self::check_deadline(ctx)?;
        let mut session = self.sessions.read().get(&id.0).cloned().ok_or_else(|| {
            MediaError::not_found(format!("playback session not found: {}", id.0))
        })?;

        let mut ctrl = ControlVodRequest {
            session_id: id.0.clone(),
            seek: None,
            pause: None,
            scale: None,
        };
        match command {
            PlaybackControl::Pause => {
                ctrl.pause = Some(true);
                session.state = PlaybackSessionState::Paused;
            }
            PlaybackControl::Resume => {
                ctrl.pause = Some(false);
                session.state = PlaybackSessionState::Playing;
            }
            PlaybackControl::Seek { position_ms } => {
                if position_ms < 0 {
                    return Err(MediaError::invalid_argument("seek position must be >= 0"));
                }
                ctrl.seek = Some(position_ms);
                session.position_ms = position_ms;
                session.state = PlaybackSessionState::Playing;
            }
            PlaybackControl::SetScale { scale } => {
                Self::validate_scale(scale)?;
                ctrl.scale = Some(scale as f32);
                session.scale = scale;
            }
        }
        self.vod.control(ctrl).map_err(Self::map_vod_err)?;
        Self::touch(&mut session);
        self.sessions.write().insert(id.0.clone(), session.clone());
        Ok(session)
    }

    async fn stop_playback(&self, ctx: &MediaRequestContext, id: &PlaybackSessionId) -> Result<()> {
        Self::check_deadline(ctx)?;
        let removed = self.sessions.write().remove(&id.0);
        let had_session = removed.is_some();
        if let Some(s) = removed {
            self.by_file.write().remove(&s.file_handle.0);
        }
        match self.vod.stop(StopVodRequest {
            session_id: id.0.clone(),
        }) {
            Ok(_) => Ok(()),
            Err(VodApiError::Session(SessionError::NotFound(_))) if had_session => Ok(()),
            Err(e) => Err(Self::map_vod_err(e)),
        }
    }
}
