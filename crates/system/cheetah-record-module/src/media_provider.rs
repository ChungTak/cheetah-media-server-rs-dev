use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::command::{
    DeleteRecordRequest, RecordFileQuery, RecordPlaybackCommand, RecordTaskQuery,
    StartRecordRequest, StopRecordRequest,
};
use cheetah_media_api::error::{MediaError, Result};
use cheetah_media_api::ids::{FileHandle, MediaKey, RecordFileId, RecordTaskId};
use cheetah_media_api::model::{Page, RecordFile, RecordTask, RecordTaskState};
use cheetah_media_api::port::{MediaRequestContext, RecordApi as RecordApiPort};

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
}

impl RecordMediaProvider {
    /// Create a provider wrapping the record module's API handle.
    ///
    /// 创建包装录制模块 API 句柄的 provider。
    pub fn new(api: Arc<RecordApi>) -> Self {
        Self { api }
    }
}

#[async_trait]
impl RecordApiPort for RecordMediaProvider {
    async fn start_record(
        &self,
        _ctx: &MediaRequestContext,
        request: StartRecordRequest,
    ) -> Result<RecordTask> {
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
        _ctx: &MediaRequestContext,
        request: StopRecordRequest,
    ) -> Result<RecordTask> {
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
        // Bound the internal query so we never load an unbounded file inventory.
        let limit = query
            .page
            .max(1)
            .saturating_mul(query.page_size)
            .min(RecordFileQuery::MAX_PAGE_SIZE) as u32;
        let internal = crate::api::FileQueryRequest {
            app: query.app.clone(),
            stream: query.stream.clone(),
            format: query.format.clone(),
            start_time_ms: query.start_time_ms,
            end_time_ms: query.end_time_ms,
            limit: Some(limit),
        };
        let response = self.api.query_files(internal).map_err(map_error)?;
        let mut items: Vec<RecordFile> = response
            .data
            .into_iter()
            .filter_map(|f| {
                let file = map_file_brief(&f)?;
                if !filter_file(&file, &query) {
                    return None;
                }
                Some(file)
            })
            .collect();
        items.sort_by(|a, b| b.start_time_ms.cmp(&a.start_time_ms));
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

    async fn delete_record_file(
        &self,
        _ctx: &MediaRequestContext,
        request: DeleteRecordRequest,
    ) -> Result<()> {
        self.api
            .delete_file(crate::api::FileDeleteRequest {
                file_id: request.file_id.0,
            })
            .map_err(map_error)
    }

    async fn control_record_playback(
        &self,
        _ctx: &MediaRequestContext,
        _file_id: &RecordFileId,
        _command: RecordPlaybackCommand,
    ) -> Result<()> {
        Err(MediaError::unsupported_capability("record playback"))
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
        path_handle: FileHandle(f.file_id.clone()),
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

fn filter_file(file: &RecordFile, query: &RecordFileQuery) -> bool {
    if let Some(ref v) = query.vhost {
        if file.media_key.vhost.0 != *v {
            return false;
        }
    }
    if let Some(ref file_id) = query.file_id {
        if file.file_id.0 != *file_id {
            return false;
        }
    }
    if let Some(ref directory) = query.directory {
        if !file.path_handle.0.contains(directory) {
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
    (y as u32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use cheetah_media_api::command::{RecordFileQuery, RecordTaskQuery, StartRecordRequest};
    use cheetah_media_api::ids::{IdempotencyKey, MediaKey};
    use cheetah_media_api::model::StoragePolicy;

    struct MockExecutor;

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
        RecordMediaProvider::new(Arc::new(crate::api::RecordApi::new(
            Arc::new(crate::registry::RecordRegistry::new(16)),
            Arc::new(MockExecutor),
        )))
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
                MediaKey::new("__defaultVhost__", "live", &format!("stream-{i}"), None).unwrap();
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
}
