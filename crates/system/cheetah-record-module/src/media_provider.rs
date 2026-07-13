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
        let idempotency_key = request.idempotency_key.as_ref().map(|k| k.0.clone());
        let segment_duration = request.segment_duration_ms;
        let max_segments = request.max_segments.or(request.storage_policy.max_segments);
        let format = request.format;

        let internal = crate::api::StartRecordRequest {
            format: format.clone(),
            app: media_key.app.0.clone(),
            stream: media_key.stream.0.clone(),
            uri: None,
            task_id: idempotency_key,
            record_template: Some(RecordTemplate {
                duration: None,
                segment_duration,
                segment_count: max_segments,
            }),
        };
        let response = self.api.start(internal).await.map_err(map_error)?;
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
                started_at: None,
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
            .unwrap_or_else(|| RecordTask {
                task_id: RecordTaskId(task_id.clone()),
                media_key: MediaKey::with_default_vhost("__fallback__", "__fallback__", None)
                    .unwrap_or_else(|_| panic!("fallback media key is invalid")),
                format: format_from_task_id(&task_id),
                state: RecordTaskState::Completed,
                started_at: None,
                ended_at: None,
                duration_ms: 0,
                file_count: 0,
                error: None,
            });
        Ok(task)
    }

    async fn query_record_tasks(
        &self,
        _ctx: &MediaRequestContext,
        query: RecordTaskQuery,
    ) -> Result<Page<RecordTask>> {
        let list = self.api.list();
        let items: Vec<RecordTask> = list
            .data
            .into_iter()
            .filter_map(|t| {
                let task = map_task_brief(&t)?;
                if !filter_task(&task, &query) {
                    return None;
                }
                Some(task)
            })
            .collect();
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
        let internal = crate::api::FileQueryRequest {
            app: query.app.clone(),
            stream: query.stream.clone(),
            format: query.format.clone(),
            start_time_ms: query.start_time_ms,
            end_time_ms: query.end_time_ms,
            limit: Some(query.page_size as u32),
        };
        let response = self.api.query_files(internal).map_err(map_error)?;
        let registry = self.api.registry();
        let items: Vec<RecordFile> = response
            .data
            .into_iter()
            .filter_map(|f| {
                let file = map_file_brief(&f, &registry)?;
                if !filter_file(&file, &query) {
                    return None;
                }
                Some(file)
            })
            .collect();
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
        _command: RecordPlaybackCommand,
    ) -> Result<()> {
        Err(MediaError::unsupported_capability("record playback"))
    }
}

fn map_error(err: RecordApiError) -> MediaError {
    match err {
        RecordApiError::InvalidRequest(msg) => MediaError::invalid_argument(msg),
        RecordApiError::UnsupportedFormat(msg) => {
            MediaError::invalid_argument(format!("unsupported format: {msg}"))
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

fn map_task_brief(t: &crate::api::TaskBrief) -> Option<RecordTask> {
    let media_key = MediaKey::with_default_vhost(&t.app, &t.stream, None).ok()?;
    Some(RecordTask {
        task_id: RecordTaskId(t.task_id.clone()),
        media_key,
        format: t.format.clone(),
        state: parse_task_state(&t.state),
        started_at: None,
        ended_at: None,
        duration_ms: 0,
        file_count: 0,
        error: None,
    })
}

fn map_task_metadata(t: &crate::metadata::RecordTaskMetadata) -> RecordTask {
    let media_key = MediaKey::with_default_vhost(&t.app, &t.stream, None).unwrap_or_else(|_| {
        MediaKey::with_default_vhost("__fallback__", &t.task_id, None)
            .unwrap_or_else(|_| panic!("fallback media key is invalid"))
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

fn map_file_brief(
    f: &crate::api::FileBrief,
    registry: &crate::registry::RecordRegistry,
) -> Option<RecordFile> {
    let media_key = registry
        .get_task(&f.file_id)
        .and_then(|t| MediaKey::with_default_vhost(&t.app, &t.stream, None).ok())
        .or_else(|| MediaKey::with_default_vhost("__fallback__", &f.file_id, None).ok())?;
    Some(RecordFile {
        file_id: RecordFileId(f.file_id.clone()),
        task_id: RecordTaskId(f.file_id.clone()),
        media_key,
        format: f.format.clone(),
        path_handle: FileHandle(f.path.clone()),
        year: 0,
        month: 0,
        day: 0,
        start_time_ms: f.start_time_ms,
        end_time_ms: f.end_time_ms,
        duration_ms: f.duration_ms,
        size_bytes: f.size_bytes,
        download_url: None,
    })
}

fn parse_task_state(s: &str) -> RecordTaskState {
    match s.to_ascii_lowercase().as_str() {
        "pending" => RecordTaskState::Pending,
        "running" => RecordTaskState::Running,
        "stopped" => RecordTaskState::Completed,
        "failed" => RecordTaskState::Failed,
        _ => RecordTaskState::Failed,
    }
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

fn format_from_task_id(task_id: &str) -> String {
    task_id.split('-').next().unwrap_or("mp4").to_string()
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
