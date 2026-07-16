//! `PlaybackApi` provider backed by the MP4 VOD driver.
//!
//! `PlaybackApi` 的 MP4 VOD 驱动实现。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::media_api::command::{OpenPlaybackRequest, PlaybackControl, PlaybackQuery};
use cheetah_sdk::media_api::error::{MediaError, Result as MediaResult};
use cheetah_sdk::media_api::ids::{FileHandle, MediaKey, PlaybackSessionId};
use cheetah_sdk::media_api::model::{Page, PlaybackSession, PlaybackSessionState};
use cheetah_sdk::media_api::port::{MediaRequestContext, PlaybackApi};
use cheetah_sdk::MediaFileStoreApi;

use crate::api::{ControlVodRequest, StartVodRequest, StopVodRequest, VodApi, VodApiError};
use crate::session_registry::VodSessionRecord;

/// Playback provider that bridges `PlaybackApi` to the MP4 VOD driver.
///
/// `PlaybackApi` 到 MP4 VOD 驱动的桥接实现。
#[derive(Clone)]
pub struct Mp4PlaybackProvider {
    vod: Arc<VodApi>,
    file_store: Arc<dyn MediaFileStoreApi>,
    root_path: PathBuf,
    id_counter: Arc<AtomicU64>,
}

impl Mp4PlaybackProvider {
    /// Create a new playback provider.
    ///
    /// 创建新的回放 provider。
    pub fn new(
        vod: Arc<VodApi>,
        file_store: Arc<dyn MediaFileStoreApi>,
        root_path: impl Into<String>,
    ) -> Self {
        let mut root = PathBuf::from(root_path.into());
        if root.is_relative() {
            if let Ok(cwd) = std::env::current_dir() {
                root = cwd.join(root);
            }
        }
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        Self {
            vod,
            file_store,
            root_path: root,
            id_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    fn generate_id(&self) -> String {
        format!("pb-{}", self.id_counter.fetch_add(1, Ordering::Relaxed))
    }

    fn resolve_uri(
        &self,
        ctx: &MediaRequestContext,
        handle: &FileHandle,
        scope: &MediaKey,
    ) -> MediaResult<String> {
        let entry = self
            .file_store
            .resolve_for_read(ctx, handle, Some(scope), now_ms())
            .map_err(map_file_store_error)?;
        let abs = PathBuf::from(&entry.absolute_path);
        let rel = abs.strip_prefix(&self.root_path).map_err(|_| {
            MediaError::invalid_argument(format!(
                "file {} is outside mp4 root {}",
                entry.absolute_path,
                self.root_path.display()
            ))
        })?;
        let mut rel_str = rel.to_string_lossy().replace('\\', "/");
        // `VodApi::resolve_path` accepts `file/` and `record/` namespace
        // prefixes.  Mirror the convention used by the native routes.
        if !rel_str.starts_with("file/") && !rel_str.starts_with("record/") {
            rel_str = format!("file/{rel_str}");
        }
        if rel_str.contains("..") {
            return Err(MediaError::invalid_argument(
                "playback uri contains traversal",
            ));
        }
        Ok(rel_str)
    }
}

#[async_trait]
impl PlaybackApi for Mp4PlaybackProvider {
    async fn open_playback(
        &self,
        ctx: &MediaRequestContext,
        request: OpenPlaybackRequest,
    ) -> MediaResult<PlaybackSession> {
        validate_scale(request.scale)?;
        let uri = self.resolve_uri(ctx, &request.file_handle, &request.media_key)?;
        let session_id = self.generate_id();
        let pb_id = PlaybackSessionId(session_id.clone());

        self.vod
            .start(StartVodRequest {
                uri,
                format: Some("mp4".to_string()),
                start_time_ms: Some(request.start_position_ms),
                end_time_ms: None,
                loop_count: Some(1),
                session_id: Some(session_id.clone()),
                media_key: Some(request.media_key.clone()),
                file_handle: Some(request.file_handle.clone()),
            })
            .await
            .map_err(map_vod_error)?;

        self.vod
            .control(ControlVodRequest {
                session_id: session_id.clone(),
                seek: None,
                pause: None,
                scale: Some(request.scale as f32),
            })
            .map_err(map_vod_error)?;

        self.get_playback(ctx, &pb_id).await
    }

    async fn get_playback(
        &self,
        _ctx: &MediaRequestContext,
        id: &PlaybackSessionId,
    ) -> MediaResult<PlaybackSession> {
        let record = self
            .vod
            .registry()
            .get(&id.0)
            .ok_or_else(|| MediaError::not_found(format!("playback session {}", id.0)))?;
        Ok(map_record_to_session(&record))
    }

    async fn list_playbacks(
        &self,
        _ctx: &MediaRequestContext,
        mut query: PlaybackQuery,
    ) -> MediaResult<Page<PlaybackSession>> {
        query.clamp_page_size();
        let mut records: Vec<_> = self.vod.registry().list();
        if let Some(vhost) = &query.vhost {
            records.retain(|r| r.media_key.vhost.0 == *vhost);
        }
        if let Some(app) = &query.app {
            records.retain(|r| r.media_key.app.0 == *app);
        }
        if let Some(stream) = &query.stream {
            records.retain(|r| r.media_key.stream.0 == *stream);
        }
        if let Some(state) = query.state {
            records.retain(|r| parse_state(&r.state) == state);
        }

        let total = records.len() as u64;
        let page = query.page.max(1);
        let page_size = query.page_size;
        let start = ((page - 1) * page_size) as usize;
        let items = records
            .into_iter()
            .skip(start)
            .take(page_size as usize)
            .map(|r| map_record_to_session(&r))
            .collect();

        Ok(Page {
            items,
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
    ) -> MediaResult<PlaybackSession> {
        let mut req = ControlVodRequest {
            session_id: id.0.clone(),
            seek: None,
            pause: None,
            scale: None,
        };
        match command {
            PlaybackControl::Pause => req.pause = Some(true),
            PlaybackControl::Resume => req.pause = Some(false),
            PlaybackControl::Seek { position_ms } => req.seek = Some(position_ms),
            PlaybackControl::SetScale { scale } => {
                validate_scale(scale)?;
                req.scale = Some(scale as f32);
            }
        }
        self.vod.control(req).map_err(map_vod_error)?;
        self.get_playback(ctx, id).await
    }

    async fn stop_playback(
        &self,
        _ctx: &MediaRequestContext,
        id: &PlaybackSessionId,
    ) -> MediaResult<()> {
        self.vod
            .stop(StopVodRequest {
                session_id: id.0.clone(),
            })
            .map_err(map_vod_error)?;
        Ok(())
    }
}

fn validate_scale(scale: f64) -> MediaResult<()> {
    const SUPPORTED: [f64; 4] = [0.5, 1.0, 2.0, 4.0];
    if SUPPORTED.contains(&scale) {
        Ok(())
    } else {
        Err(MediaError::unsupported(format!(
            "playback scale {scale}; supported: 0.5, 1, 2, 4"
        )))
    }
}

fn map_record_to_session(record: &VodSessionRecord) -> PlaybackSession {
    PlaybackSession {
        session_id: PlaybackSessionId(record.session_id.clone()),
        media_key: record.media_key.clone(),
        file_handle: record.file_handle.clone(),
        state: parse_state(&record.state),
        duration_ms: 0,
        position_ms: record.start_position_ms,
        scale: record.scale as f64,
        generation: 1,
        output_key: record.output_key.clone(),
        last_error: None,
        created_at: 0,
        updated_at: 0,
    }
}

fn parse_state(state: &str) -> PlaybackSessionState {
    match state {
        "starting" | "pending" => PlaybackSessionState::Pending,
        "playing" => PlaybackSessionState::Playing,
        "paused" => PlaybackSessionState::Paused,
        "seeking" => PlaybackSessionState::Seeking,
        "completed" => PlaybackSessionState::Completed,
        "failed" => PlaybackSessionState::Failed,
        _ => PlaybackSessionState::Playing,
    }
}

fn map_vod_error(err: VodApiError) -> MediaError {
    use crate::session_registry::SessionError;
    match err {
        VodApiError::InvalidRequest(msg) => MediaError::invalid_argument(msg),
        VodApiError::Session(SessionError::NotFound(id)) => MediaError::not_found(id),
        VodApiError::Session(SessionError::Duplicate(id)) => MediaError::already_exists(id),
        VodApiError::Session(SessionError::CapacityExceeded(cap)) => {
            MediaError::unavailable(format!("playback capacity exceeded: {cap}"))
        }
        VodApiError::Driver(msg) if msg.contains("channel closed") => {
            MediaError::not_found(format!("vod session closed: {msg}"))
        }
        VodApiError::Driver(msg) => MediaError::internal(format!("vod driver: {msg}")),
        VodApiError::NotFound(msg) => MediaError::not_found(msg),
    }
}

fn map_file_store_error(err: MediaError) -> MediaError {
    err
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Clone)]
    struct Entry {
        absolute_path: String,
        media_key: MediaKey,
    }

    struct MockFileStore {
        files: Mutex<HashMap<String, Entry>>,
    }

    impl MockFileStore {
        fn with_file(handle: &str, path: &str, media_key: MediaKey) -> Self {
            let mut files = HashMap::new();
            files.insert(
                handle.to_string(),
                Entry {
                    absolute_path: path.to_string(),
                    media_key,
                },
            );
            Self {
                files: Mutex::new(files),
            }
        }
    }

    impl MediaFileStoreApi for MockFileStore {
        fn register_file(
            &self,
            _ctx: &MediaRequestContext,
            _entry: cheetah_sdk::media_api::media_file_store::FileStoreEntry,
        ) -> MediaResult<FileHandle> {
            unimplemented!()
        }

        fn resolve_for_read(
            &self,
            _ctx: &MediaRequestContext,
            handle: &FileHandle,
            _resource_scope: Option<&MediaKey>,
            _now_ms: i64,
        ) -> MediaResult<cheetah_sdk::media_api::media_file_store::FileStoreEntry> {
            let files = self.files.lock().unwrap();
            let entry = files
                .get(&handle.0)
                .ok_or_else(|| MediaError::not_found("file"))?;
            Ok(cheetah_sdk::media_api::media_file_store::FileStoreEntry {
                media_key: entry.media_key.clone(),
                file_type: "mp4".to_string(),
                content_type: "video/mp4".to_string(),
                size_bytes: 1,
                created_at_ms: 0,
                expires_at_ms: None,
                absolute_path: entry.absolute_path.clone(),
                owner_principal: None,
                allowed_principals: Vec::new(),
            })
        }

        fn delete(
            &self,
            _ctx: &MediaRequestContext,
            _handle: &FileHandle,
            _now_ms: i64,
        ) -> MediaResult<()> {
            unimplemented!()
        }

        fn delete_batch(
            &self,
            _ctx: &MediaRequestContext,
            _query: cheetah_sdk::media_api::media_file_store::FileStoreQuery,
            _batch_limit: u32,
            _now_ms: i64,
        ) -> MediaResult<cheetah_sdk::media_api::media_file_store::DeleteBatchResult> {
            unimplemented!()
        }

        fn resolve_download(
            &self,
            _ctx: &MediaRequestContext,
            _handle: &FileHandle,
            _range: Option<cheetah_sdk::media_api::media_file_store::FileRange>,
            _filename: Option<String>,
            _now_ms: i64,
        ) -> MediaResult<cheetah_sdk::media_api::media_file_store::FileDownload> {
            unimplemented!()
        }
    }

    fn provider(root: &str, store: Arc<dyn MediaFileStoreApi>) -> Mp4PlaybackProvider {
        let api = Arc::new(VodApi::new(
            Arc::new(crate::session_registry::VodSessionRegistry::new(16)),
            Arc::new(crate::config::Mp4ModuleConfig {
                root_path: root.to_string(),
                ..Default::default()
            }),
        ));
        Mp4PlaybackProvider::new(api, store, root)
    }

    #[test]
    fn unsupported_scale_is_rejected() {
        let err = validate_scale(3.0).unwrap_err();
        assert!(matches!(
            err.code,
            cheetah_sdk::media_api::error::MediaErrorCode::Unsupported
        ));
    }

    #[test]
    fn supported_scales_pass() {
        for s in [0.5, 1.0, 2.0, 4.0] {
            assert!(validate_scale(s).is_ok());
        }
    }

    #[test]
    fn resolve_uri_rejects_files_outside_root() {
        let root = std::env::temp_dir().join("cheetah-vod-resolve-test-root");
        let other = std::env::temp_dir().join("cheetah-vod-resolve-test-other");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let outside = other.join("secret.mp4");
        std::fs::write(&outside, b"").unwrap();

        let media_key = MediaKey::with_default_vhost("record", "stream", None).unwrap();
        let store = Arc::new(MockFileStore::with_file(
            "h1",
            outside.to_str().unwrap(),
            media_key.clone(),
        ));
        let p = provider(root.to_str().unwrap(), store);

        let err = p
            .resolve_uri(
                &MediaRequestContext::default(),
                &FileHandle("h1".to_string()),
                &media_key,
            )
            .unwrap_err();
        assert!(matches!(
            err.code,
            cheetah_sdk::media_api::error::MediaErrorCode::InvalidArgument
        ));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn resolve_uri_returns_file_prefixed_relative_path() {
        let root = std::env::temp_dir().join("cheetah-vod-resolve-test-in");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("live/test.mp4");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"").unwrap();

        let media_key = MediaKey::with_default_vhost("record", "stream", None).unwrap();
        let store = Arc::new(MockFileStore::with_file(
            "h1",
            file.to_str().unwrap(),
            media_key.clone(),
        ));
        let p = provider(root.to_str().unwrap(), store);

        let uri = p
            .resolve_uri(
                &MediaRequestContext::default(),
                &FileHandle("h1".to_string()),
                &media_key,
            )
            .unwrap();
        assert_eq!(uri, "file/live/test.mp4");

        let _ = std::fs::remove_dir_all(&root);
    }
}
