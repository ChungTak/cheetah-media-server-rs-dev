use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::command::{
    DeleteRecordRequest, OpenPlaybackRequest, PlaybackControl, RecordFileQuery,
    RecordPlaybackCommand, RecordTaskQuery, StartRecordRequest, StopRecordRequest,
};
use cheetah_media_api::error::{MediaError, Result};
use cheetah_media_api::ids::{FileHandle, MediaKey, PlaybackSessionId, RecordFileId, RecordTaskId};
use cheetah_media_api::model::{Page, RecordFile, RecordTask, RecordTaskState};
use cheetah_media_api::port::{MediaRequestContext, PlaybackApi, RecordApi as RecordApiPort};
use cheetah_media_api::MediaFileStoreApi;
use cheetah_sdk::{Deadline, MediaServices};
use parking_lot::Mutex;
use std::collections::HashMap;

use crate::api::{RecordApi, RecordApiError, RecordTemplate};
use crate::metadata::RecordTaskState as InternalRecordTaskState;
use crate::registry::RegistryError;

/// Bridge from the record module's internal `RecordApi` to the media-domain
/// `RecordApi` port.
///
/// 将录制模块内部 `RecordApi` 桥接到媒体领域 `RecordApi` 端口。
#[derive(Clone)]
pub struct RecordMediaProvider {
    api: Arc<RecordApi>,
    file_store: Arc<dyn MediaFileStoreApi>,
    media_services: MediaServices,
    /// file_id -> playback session id (compat shim).
    file_sessions: Arc<Mutex<HashMap<String, String>>>,
}

impl RecordMediaProvider {
    /// Create a provider wrapping the record module's API handle and file store.
    ///
    /// 创建包装录制模块 API 句柄与文件存储的 provider。
    pub fn new(
        api: Arc<RecordApi>,
        file_store: Arc<dyn MediaFileStoreApi>,
        media_services: MediaServices,
    ) -> Self {
        Self {
            api,
            file_store,
            media_services,
            file_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn check_deadline(ctx: &MediaRequestContext) -> Result<()> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))
    }

    fn map_playback_command(command: RecordPlaybackCommand) -> Result<PlaybackControl> {
        Ok(match command {
            RecordPlaybackCommand::Pause => PlaybackControl::Pause,
            RecordPlaybackCommand::Resume => PlaybackControl::Resume,
            RecordPlaybackCommand::Scale { value } => PlaybackControl::SetScale { scale: value },
            RecordPlaybackCommand::Seek { value } => PlaybackControl::Seek { position_ms: value },
        })
    }

    async fn ensure_playback_session(
        &self,
        ctx: &MediaRequestContext,
        playback: &Arc<dyn PlaybackApi>,
        file_id: &str,
        file: &crate::metadata::RecordFileMetadata,
    ) -> Result<PlaybackSessionId> {
        if let Some(id) = self.file_sessions.lock().get(file_id).cloned() {
            return Ok(PlaybackSessionId(id));
        }
        let handle = file
            .file_handle
            .clone()
            .filter(|h| !h.is_empty())
            .map(FileHandle)
            .unwrap_or_else(|| FileHandle(file_id.to_string()));
        // Prefer registered file-store handle; fall back to registering path.
        let resolved = self
            .file_store
            .resolve_for_read(ctx, &handle, None, now_ms());
        let file_handle = match resolved {
            Ok(_) => handle,
            Err(_) if !file.path.is_empty() => {
                let media_key = MediaKey::new(&file.vhost, &file.app, &file.stream, None)
                    .map_err(|e| MediaError::invalid_argument(e.to_string()))?;
                let entry = cheetah_media_api::media_file_store::FileStoreEntry {
                    media_key: media_key.clone(),
                    file_type: "record".to_string(),
                    content_type: "video/mp4".to_string(),
                    size_bytes: file.size_bytes,
                    created_at_ms: file.start_time_ms,
                    expires_at_ms: None,
                    absolute_path: file.path.clone(),
                    owner_principal: None,
                    allowed_principals: Vec::new(),
                };
                self.file_store.register_file(ctx, entry)?
            }
            Err(e) => return Err(e),
        };
        let media_key = MediaKey::new(&file.vhost, &file.app, &file.stream, None)
            .map_err(|e| MediaError::invalid_argument(e.to_string()))?;
        let session = playback
            .open_playback(
                ctx,
                OpenPlaybackRequest {
                    file_handle,
                    media_key,
                    start_position_ms: 0,
                    scale: 1.0,
                },
            )
            .await?;
        self.file_sessions
            .lock()
            .insert(file_id.to_string(), session.session_id.0.clone());
        Ok(session.session_id)
    }
}

#[async_trait]
impl RecordApiPort for RecordMediaProvider {
    async fn start_record(
        &self,
        ctx: &MediaRequestContext,
        request: StartRecordRequest,
    ) -> Result<RecordTask> {
        Self::check_deadline(ctx)?;
        let media_key = request.media_key;
        let format = request.format;
        let vhost = media_key.vhost.0.clone();
        let app = media_key.app.0.clone();
        let stream = media_key.stream.0.clone();
        let segment_duration = request.segment_duration_ms;
        let max_segments = request.max_segments.or(request.storage_policy.max_segments);
        let now = now_ms();

        // Use the idempotency key as the task id when provided; otherwise
        // generate a unique id so repeated starts do not collide.
        let task_id = request
            .idempotency_key
            .as_ref()
            .map(|k| k.0.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{format}-{vhost}-{app}-{stream}-{now}"));

        // If an idempotency key was supplied, check whether the task already
        // exists and whether the effective request matches.
        if request.idempotency_key.is_some() {
            if let Some(existing) = self.api.registry().get_task(&task_id) {
                if existing.vhost != vhost
                    || existing.app != app
                    || existing.stream != stream
                    || format_str_to_string(existing.format) != format
                    || existing.segment_duration_ms != segment_duration.unwrap_or(0)
                    || existing.segment_count_limit != max_segments.unwrap_or(0)
                {
                    return Err(MediaError::already_exists(format!(
                        "idempotency key {task_id} reused with different parameters"
                    )));
                }
                return Ok(map_task_metadata(&existing));
            }
        }

        let internal = crate::api::StartRecordRequest {
            format: format.clone(),
            vhost: vhost.clone(),
            app: app.clone(),
            stream: stream.clone(),
            uri: None,
            task_id: Some(task_id.clone()),
            record_template: Some(RecordTemplate {
                duration: None,
                segment_duration,
                segment_count: max_segments,
            }),
        };
        let response = match self.api.start(internal).await {
            Ok(resp) => resp,
            Err(RecordApiError::Registry(RegistryError::DuplicateTask(_))) => {
                if let Some(existing) = self.api.registry().get_task(&task_id) {
                    if existing.vhost == vhost
                        && existing.app == app
                        && existing.stream == stream
                        && format_str_to_string(existing.format) == format
                    {
                        return Ok(map_task_metadata(&existing));
                    }
                }
                return Err(MediaError::already_exists(format!(
                    "task already exists: {task_id}"
                )));
            }
            Err(e) => return Err(map_error(e)),
        };
        let task = self
            .api
            .registry()
            .get_task(&response.task_id)
            .map(|t| map_task_metadata(&t))
            .unwrap_or_else(|| RecordTask {
                task_id: RecordTaskId(response.task_id),
                media_key,
                format,
                state: RecordTaskState::Running,
                started_at: Some(now),
                ended_at: None,
                duration_ms: 0,
                file_count: 0,
                error: None,
            });
        Ok(task)
    }

    async fn stop_record(
        &self,
        ctx: &MediaRequestContext,
        request: StopRecordRequest,
    ) -> Result<RecordTask> {
        Self::check_deadline(ctx)?;
        let task_id = request.task_id.0;
        self.api
            .stop(crate::api::StopRecordRequest {
                task_id: task_id.clone(),
            })
            .await
            .map_err(map_error)?;
        let task = self
            .api
            .registry()
            .get_task(&task_id)
            .map(|t| map_task_metadata(&t))
            .ok_or_else(|| MediaError::not_found(format!("task not found: {task_id}")))?;
        Ok(task)
    }

    async fn query_record_tasks(
        &self,
        _ctx: &MediaRequestContext,
        query: RecordTaskQuery,
    ) -> Result<Page<RecordTask>> {
        // The registry capacity bounds the task set, so collecting here is
        // effectively bounded. Sort by start time descending before paging.
        let mut query = query;
        query.clamp_page_size();
        let mut items: Vec<RecordTask> = self
            .api
            .registry()
            .list_tasks()
            .into_iter()
            .map(|t| map_task_metadata(&t))
            .filter(|t| filter_task(t, &query))
            .collect();
        items.sort_by(|a, b| {
            b.started_at
                .unwrap_or(i64::MIN)
                .cmp(&a.started_at.unwrap_or(i64::MIN))
        });
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

    async fn query_record_files(
        &self,
        _ctx: &MediaRequestContext,
        query: RecordFileQuery,
    ) -> Result<Page<RecordFile>> {
        let mut query = query;
        query.clamp_page_size();
        // Bound the internal query so we never load an unbounded file inventory.
        // The registry returns files sorted by start time descending and provides
        // the total number of matching files.
        let limit = query
            .page
            .max(1)
            .saturating_mul(query.page_size)
            .min(RecordFileQuery::MAX_PAGE_SIZE) as u32;
        let internal = crate::api::FileQueryRequest {
            vhost: query.vhost.clone(),
            app: query.app.clone(),
            stream: query.stream.clone(),
            format: query.format.clone(),
            start_time_ms: query.start_time_ms,
            end_time_ms: query.end_time_ms,
            file_id: query.file_id.clone(),
            directory: query.directory.clone(),
            limit: Some(limit),
        };
        let result = self.api.query_files(internal).map_err(map_error)?;
        // All registry-level filters (vhost, app, stream, format, time, file_id,
        // directory) are applied inside the registry, so `result.total` is the
        // true matching count. map_file_brief only converts metadata; it should
        // not fail for valid registry entries.
        let items: Vec<RecordFile> = result
            .files
            .into_iter()
            .filter_map(|f| map_file_brief(&f))
            .collect();
        let total = result.total as u64;
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

    async fn delete_record_file(
        &self,
        ctx: &MediaRequestContext,
        request: DeleteRecordRequest,
    ) -> Result<()> {
        Self::check_deadline(ctx)?;
        let now = now_ms();
        self.file_store
            .delete(ctx, &FileHandle(request.file_id.0.clone()), now)
            .ok();
        self.api
            .delete_file(crate::api::FileDeleteRequest {
                file_id: request.file_id.0,
            })
            .map_err(map_error)
    }

    async fn control_record_playback(
        &self,
        ctx: &MediaRequestContext,
        file_id: &RecordFileId,
        command: RecordPlaybackCommand,
    ) -> Result<()> {
        Self::check_deadline(ctx)?;
        let file = self.api.registry().get_file(&file_id.0).ok_or_else(|| {
            MediaError::not_found(format!("record file not found: {}", file_id.0))
        })?;
        if let Some(playback) = self.media_services.playback() {
            // Validate seek bounds against known duration before opening driver.
            if let RecordPlaybackCommand::Seek { value } = &command {
                if *value < 0 || *value > file.duration_ms as i64 {
                    return Err(MediaError::invalid_argument(format!(
                        "seek {value} is out of range [0, {}]",
                        file.duration_ms
                    )));
                }
            }
            let session_id = self
                .ensure_playback_session(ctx, &playback, &file_id.0, &file)
                .await?;
            let mapped = Self::map_playback_command(command)?;
            playback.control_playback(ctx, &session_id, mapped).await?;
            return Ok(());
        }
        Err(MediaError::unavailable(
            "playback provider not registered; cannot control record playback",
        ))
    }
}

fn map_error(err: RecordApiError) -> MediaError {
    match err {
        RecordApiError::InvalidRequest(msg) => MediaError::invalid_argument(msg),
        RecordApiError::UnsupportedFormat(msg) => {
            MediaError::unsupported(format!("unsupported record format: {msg}"))
        }
        RecordApiError::Registry(e) => match e {
            crate::registry::RegistryError::TaskNotFound(id) => {
                MediaError::not_found(format!("task not found: {id}"))
            }
            crate::registry::RegistryError::FileNotFound(id) => {
                MediaError::not_found(format!("file not found: {id}"))
            }
            crate::registry::RegistryError::DuplicateTask(id) => {
                MediaError::already_exists(format!("task already exists: {id}"))
            }
            crate::registry::RegistryError::CapacityExceeded(cap) => {
                MediaError::unavailable(format!("registry capacity exceeded: {cap}"))
            }
        },
        RecordApiError::Executor(e) => MediaError::internal(format!("executor error: {e}")),
    }
}

fn map_task_metadata(t: &crate::metadata::RecordTaskMetadata) -> RecordTask {
    let media_key = MediaKey::new(&t.vhost, &t.app, &t.stream, None).unwrap_or_else(|_| {
        MediaKey::with_default_vhost(&t.app, &t.stream, None)
            .unwrap_or_else(|_| panic!("media key is invalid"))
    });
    RecordTask {
        task_id: RecordTaskId(t.task_id.clone()),
        media_key,
        format: format_str_to_string(t.format),
        state: map_internal_state(t.state),
        started_at: Some(t.create_time_ms),
        ended_at: None,
        duration_ms: 0,
        file_count: 0,
        error: None,
    }
}

fn map_file_brief(f: &crate::api::FileBrief) -> Option<RecordFile> {
    let media_key = MediaKey::new(&f.vhost, &f.app, &f.stream, None).ok()?;
    let (year, month, day) = ymd_from_ms(f.start_time_ms);
    Some(RecordFile {
        file_id: RecordFileId(f.file_id.clone()),
        task_id: RecordTaskId(f.task_id.clone()),
        media_key,
        format: f.format.clone(),
        path_handle: FileHandle(f.file_handle.clone()),
        year,
        month,
        day,
        start_time_ms: f.start_time_ms,
        end_time_ms: f.end_time_ms,
        duration_ms: f.duration_ms,
        size_bytes: f.size_bytes,
        download_url: None,
    })
}

fn map_internal_state(state: InternalRecordTaskState) -> RecordTaskState {
    match state {
        InternalRecordTaskState::Pending => RecordTaskState::Pending,
        InternalRecordTaskState::Running => RecordTaskState::Running,
        InternalRecordTaskState::Stopped => RecordTaskState::Completed,
        InternalRecordTaskState::Failed => RecordTaskState::Failed,
    }
}

fn format_str_to_string(f: crate::metadata::RecordFormatStr) -> String {
    match f {
        crate::metadata::RecordFormatStr::Flv => "flv".to_string(),
        crate::metadata::RecordFormatStr::Hls => "hls".to_string(),
        crate::metadata::RecordFormatStr::Mp4 => "mp4".to_string(),
        crate::metadata::RecordFormatStr::Ps => "ps".to_string(),
    }
}

fn filter_task(task: &RecordTask, query: &RecordTaskQuery) -> bool {
    if let Some(ref v) = query.vhost {
        if task.media_key.vhost.0 != *v {
            return false;
        }
    }
    if let Some(ref a) = query.app {
        if task.media_key.app.0 != *a {
            return false;
        }
    }
    if let Some(ref s) = query.stream {
        if task.media_key.stream.0 != *s {
            return false;
        }
    }
    if let Some(state) = query.state {
        if task.state != state {
            return false;
        }
    }
    true
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn ymd_from_ms(ms: i64) -> (u32, u32, u32) {
    let secs = (ms / 1_000).max(0);
    let days = secs / 86_400;
    civil_from_days(days)
}

fn civil_from_days(days: i64) -> (u32, u32, u32) {
    // Based on Howard Hinnant's days_from_civil algorithm.
    let days = days + 719_468;
    let era = (if days >= 0 { days } else { days - 146_096 }) / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use cheetah_media_api::command::{RecordFileQuery, RecordTaskQuery, StartRecordRequest};
    use cheetah_media_api::event::{
        MediaEvent, MediaEventBusApi, MediaEventSender, MediaEventSubscription,
    };
    use cheetah_media_api::ids::{IdempotencyKey, MediaKey};
    use cheetah_media_api::model::StoragePolicy;
    use parking_lot::Mutex;

    struct MockExecutor;
    struct MockFileStore;
    struct MockSubscription;

    impl MediaEventSubscription for MockSubscription {
        fn id(&self) -> String {
            "mock-sub".to_string()
        }

        fn unsubscribe(&self) -> cheetah_media_api::error::Result<()> {
            Ok(())
        }
    }

    struct MockBus {
        events: Mutex<Vec<MediaEvent>>,
    }

    impl MediaEventBusApi for MockBus {
        fn publish(&self, event: MediaEvent) -> cheetah_media_api::error::Result<()> {
            self.events.lock().push(event);
            Ok(())
        }

        fn subscribe(
            &self,
            _sender: Box<dyn MediaEventSender>,
            _capacity: usize,
        ) -> cheetah_media_api::error::Result<Box<dyn MediaEventSubscription>> {
            Ok(Box::new(MockSubscription))
        }

        fn unsubscribe(&self, _id: &str) -> cheetah_media_api::error::Result<()> {
            Ok(())
        }
    }

    impl cheetah_media_api::MediaFileStoreApi for MockFileStore {
        fn register_file(
            &self,
            _ctx: &cheetah_media_api::port::MediaRequestContext,
            _entry: cheetah_media_api::FileStoreEntry,
        ) -> cheetah_media_api::error::Result<cheetah_media_api::ids::FileHandle> {
            Ok(cheetah_media_api::ids::FileHandle("mock".to_string()))
        }

        fn resolve_for_read(
            &self,
            _ctx: &cheetah_media_api::port::MediaRequestContext,
            _handle: &cheetah_media_api::ids::FileHandle,
            _resource_scope: Option<&cheetah_media_api::ids::MediaKey>,
            _now_ms: i64,
        ) -> cheetah_media_api::error::Result<cheetah_media_api::FileStoreEntry> {
            Err(cheetah_media_api::error::MediaError::not_found("mock"))
        }

        fn delete(
            &self,
            _ctx: &cheetah_media_api::port::MediaRequestContext,
            _handle: &cheetah_media_api::ids::FileHandle,
            _now_ms: i64,
        ) -> cheetah_media_api::error::Result<()> {
            Ok(())
        }

        fn delete_batch(
            &self,
            _ctx: &cheetah_media_api::port::MediaRequestContext,
            _query: cheetah_media_api::FileStoreQuery,
            _batch_limit: u32,
            _now_ms: i64,
        ) -> cheetah_media_api::error::Result<cheetah_media_api::DeleteBatchResult> {
            Ok(cheetah_media_api::DeleteBatchResult {
                matched: 0,
                deleted: 0,
                failed: 0,
                failures: Vec::new(),
            })
        }

        fn resolve_download(
            &self,
            _ctx: &cheetah_media_api::port::MediaRequestContext,
            _handle: &cheetah_media_api::ids::FileHandle,
            _range: Option<cheetah_media_api::FileRange>,
            _filename: Option<String>,
            _now_ms: i64,
        ) -> cheetah_media_api::error::Result<cheetah_media_api::FileDownload> {
            Err(cheetah_media_api::error::MediaError::not_found("mock"))
        }
    }

    #[async_trait]
    impl crate::task::TaskExecutor for MockExecutor {
        async fn spawn(
            &self,
            _task: crate::task::RecordTask,
        ) -> std::result::Result<(), crate::task::TaskExecutorError> {
            Ok(())
        }
        async fn stop(&self, _id: &str) -> std::result::Result<(), crate::task::TaskExecutorError> {
            Ok(())
        }
    }

    fn provider() -> RecordMediaProvider {
        let bus = Arc::new(MockBus {
            events: Mutex::new(Vec::new()),
        });
        RecordMediaProvider::new(
            Arc::new(crate::api::RecordApi::new(
                Arc::new(crate::registry::RecordRegistry::new(16)),
                Arc::new(MockExecutor),
                bus,
            )),
            Arc::new(MockFileStore),
            MediaServices::unavailable(),
        )
    }

    #[tokio::test]
    async fn start_record_preserves_vhost_and_idempotency_key() {
        let provider = provider();
        let ctx = MediaRequestContext::default();
        let media_key = MediaKey::new("custom", "live", "test", None).unwrap();
        let req = StartRecordRequest {
            media_key: media_key.clone(),
            format: "mp4".to_string(),
            template: Default::default(),
            segment_duration_ms: None,
            max_segments: None,
            storage_policy: StoragePolicy::default(),
            idempotency_key: Some(IdempotencyKey("idemp-1".to_string())),
        };
        let task = provider.start_record(&ctx, req.clone()).await.unwrap();
        assert_eq!(task.media_key, media_key);
        assert_eq!(task.task_id.0, "idemp-1");

        // Repeating the same effective request returns the original task.
        let task2 = provider.start_record(&ctx, req).await.unwrap();
        assert_eq!(task2.task_id, task.task_id);
        assert_eq!(task2.media_key, media_key);
    }

    #[tokio::test]
    async fn idempotency_key_reused_with_different_params_is_conflict() {
        let provider = provider();
        let ctx = MediaRequestContext::default();
        let key = MediaKey::new("custom", "live", "test", None).unwrap();
        let req1 = StartRecordRequest {
            media_key: key.clone(),
            format: "mp4".to_string(),
            template: Default::default(),
            segment_duration_ms: None,
            max_segments: None,
            storage_policy: StoragePolicy::default(),
            idempotency_key: Some(IdempotencyKey("idemp-2".to_string())),
        };
        let mut req2 = req1.clone();
        req2.format = "flv".to_string();
        provider.start_record(&ctx, req1).await.unwrap();
        let err = provider.start_record(&ctx, req2).await.unwrap_err();
        assert!(err.to_string().contains("idempotency"));
    }

    #[tokio::test]
    async fn query_tasks_and_files_pages_inside_capacity() {
        let provider = provider();
        let ctx = MediaRequestContext::default();
        for i in 0..6 {
            let key =
                MediaKey::new("__defaultVhost__", "live", format!("stream-{i}"), None).unwrap();
            let req = StartRecordRequest {
                media_key: key,
                format: "mp4".to_string(),
                template: Default::default(),
                segment_duration_ms: None,
                max_segments: None,
                storage_policy: StoragePolicy::default(),
                idempotency_key: None,
            };
            provider.start_record(&ctx, req).await.unwrap();
        }
        let page = provider
            .query_record_tasks(
                &ctx,
                RecordTaskQuery {
                    page: 1,
                    page_size: 4,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 6);
        assert_eq!(page.items.len(), 4);

        let files = provider
            .query_record_files(
                &ctx,
                RecordFileQuery {
                    page: 1,
                    page_size: 10,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(files.items.len(), 0);
    }

    #[test]
    fn ymd_from_ms_has_correct_year_for_january_and_february() {
        // 2026-01-15 is 20468 days since 1970-01-01.
        assert_eq!(civil_from_days(20468), (2026, 1, 15));
        // 2026-02-28 is 20512 days since 1970-01-01.
        assert_eq!(civil_from_days(20512), (2026, 2, 28));
        // 2026-05-23 (existing executor test case) is 20596 days.
        assert_eq!(civil_from_days(20596), (2026, 5, 23));
    }

    #[tokio::test]
    async fn query_record_files_reports_total_and_pages_descending() {
        let provider = provider();
        let ctx = MediaRequestContext::default();
        for i in 0..5 {
            provider
                .api
                .registry()
                .insert_file(crate::metadata::RecordFileMetadata {
                    file_id: format!("f{i}"),
                    task_id: format!("t{i}"),
                    format: crate::metadata::RecordFormatStr::Mp4,
                    vhost: cheetah_media_api::ids::DEFAULT_VHOST.to_string(),
                    app: "live".to_string(),
                    stream: format!("stream-{i}"),
                    path: format!("/rec/live/stream-{i}/2026/f{i}.mp4"),
                    file_handle: None,
                    duration_ms: 1_000,
                    size_bytes: 1000,
                    start_time_ms: i as i64 * 1000,
                    end_time_ms: (i as i64 + 1) * 1000,
                    track_summary: vec![],
                })
                .unwrap();
        }
        let page = provider
            .query_record_files(
                &ctx,
                RecordFileQuery {
                    page: 1,
                    page_size: 2,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 5);
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].file_id.0, "f4");
        assert_eq!(page.items[1].file_id.0, "f3");

        let page = provider
            .query_record_files(
                &ctx,
                RecordFileQuery {
                    page: 2,
                    page_size: 2,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 5);
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].file_id.0, "f2");
        assert_eq!(page.items[1].file_id.0, "f1");

        // Zero page size is clamped to the default.
        let page = provider
            .query_record_files(
                &ctx,
                RecordFileQuery {
                    page: 1,
                    page_size: 0,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 5);
        assert_eq!(page.items.len(), 5);
    }

    #[tokio::test]
    async fn query_record_files_filters_by_file_id_and_total_is_accurate() {
        let provider = provider();
        let ctx = MediaRequestContext::default();
        for i in 0..3 {
            provider
                .api
                .registry()
                .insert_file(crate::metadata::RecordFileMetadata {
                    file_id: format!("f{i}"),
                    task_id: format!("t{i}"),
                    format: crate::metadata::RecordFormatStr::Mp4,
                    vhost: cheetah_media_api::ids::DEFAULT_VHOST.to_string(),
                    app: "live".to_string(),
                    stream: format!("stream-{i}"),
                    path: format!("/rec/live/stream-{i}/2026/f{i}.mp4"),
                    file_handle: None,
                    duration_ms: 1_000,
                    size_bytes: 1000,
                    start_time_ms: i as i64 * 1000,
                    end_time_ms: (i as i64 + 1) * 1000,
                    track_summary: vec![],
                })
                .unwrap();
        }
        let page = provider
            .query_record_files(
                &ctx,
                RecordFileQuery {
                    file_id: Some("f1".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].file_id.0, "f1");

        let page = provider
            .query_record_files(
                &ctx,
                RecordFileQuery {
                    directory: Some("stream-2".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].file_id.0, "f2");
    }

    #[tokio::test]
    async fn control_playback_requires_playback_provider() {
        let provider = provider();
        let ctx = MediaRequestContext::default();
        provider
            .api
            .registry()
            .insert_file(crate::metadata::RecordFileMetadata {
                file_id: "f1".to_string(),
                task_id: "t1".to_string(),
                format: crate::metadata::RecordFormatStr::Mp4,
                vhost: cheetah_media_api::ids::DEFAULT_VHOST.to_string(),
                app: "live".to_string(),
                stream: "test".to_string(),
                path: "/rec/live/test/2026/f1-1.mp4".to_string(),
                file_handle: None,
                duration_ms: 10_000,
                size_bytes: 1_000_000,
                start_time_ms: 1_000,
                end_time_ms: 11_000,
                track_summary: vec![],
            })
            .unwrap();

        let err = provider
            .control_record_playback(
                &ctx,
                &RecordFileId("f1".to_string()),
                RecordPlaybackCommand::Pause,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("playback provider"));

        let err = provider
            .control_record_playback(
                &ctx,
                &RecordFileId("missing".to_string()),
                RecordPlaybackCommand::Pause,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
