//! SMS-compatible record HTTP API surface (request/response models only).
//!
//! The API layer is framework-neutral: the engine's HTTP module wrapper
//! routes a `cheetah_sdk::HttpRequest` to one of these handlers via the
//! `RecordModule::http_service()`. JSON shapes are intentionally close to
//! `vendor-ref/simple-media-server/Src/Api/RecordApi.cpp`.

use std::sync::Arc;

use cheetah_codec::RecordFormat;
use serde::{Deserialize, Serialize};

use crate::metadata::{RecordFileQuery, RecordFormatStr, RecordTaskState};
use crate::registry::{RecordRegistry, RegistryError};
use crate::task::{RecordTaskTemplate, TaskExecutor, TaskExecutorError};

/// `RecordApiError` enumeration.
/// `RecordApiError` 枚举.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RecordApiError {
    /// `InvalidRequest` variant.
    /// `InvalidRequest` 变体.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// `Registry` variant.
    /// `Registry` 变体.
    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),
    /// `Executor` variant.
    /// `Executor` 变体.
    #[error("executor error: {0}")]
    Executor(#[from] TaskExecutorError),
    /// `UnsupportedFormat` variant.
    /// `UnsupportedFormat` 变体.
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
}

/// `POST /api/v1/record/start` body.
#[derive(Debug, Clone, Deserialize)]
pub struct StartRecordRequest {
    /// `format` field of type `String`.
    /// `format` 字段，类型为 `String`.
    pub format: String,
    /// `app` field of type `String`.
    /// `app` 字段，类型为 `String`.
    pub app: String,
    /// `stream` field of type `String`.
    /// `stream` 字段，类型为 `String`.
    pub stream: String,
    /// `uri` field.
    /// `uri` 字段.
    #[serde(default)]
    pub uri: Option<String>,
    /// `task_id` field.
    /// `task_id` 字段.
    #[serde(default)]
    pub task_id: Option<String>,
    /// `record_template` field.
    /// `record_template` 字段.
    #[serde(rename = "recordTemplate", default)]
    pub record_template: Option<RecordTemplate>,
}

/// `RecordTemplate` data structure.
/// `RecordTemplate` 数据结构.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RecordTemplate {
    /// `duration` field.
    /// `duration` 字段.
    #[serde(default)]
    pub duration: Option<u64>,
    /// `segment_duration` field.
    /// `segment_duration` 字段.
    #[serde(rename = "segmentDuration", default)]
    pub segment_duration: Option<u64>,
    /// `segment_count` field.
    /// `segment_count` 字段.
    #[serde(rename = "segmentCount", default)]
    pub segment_count: Option<u32>,
}

/// `StartRecordResponse` data structure.
/// `StartRecordResponse` 数据结构.
#[derive(Debug, Clone, Serialize)]
pub struct StartRecordResponse {
    /// `code` field of type `u16`.
    /// `code` 字段，类型为 `u16`.
    pub code: u16,
    /// `msg` field of type `String`.
    /// `msg` 字段，类型为 `String`.
    pub msg: String,
    /// `task_id` field of type `String`.
    /// `task_id` 字段，类型为 `String`.
    #[serde(rename = "taskId")]
    pub task_id: String,
}

/// `StopRecordRequest` data structure.
/// `StopRecordRequest` 数据结构.
#[derive(Debug, Clone, Deserialize)]
pub struct StopRecordRequest {
    /// `task_id` field of type `String`.
    /// `task_id` 字段，类型为 `String`.
    #[serde(rename = "taskId")]
    pub task_id: String,
}

/// `StopRecordResponse` data structure.
/// `StopRecordResponse` 数据结构.
#[derive(Debug, Clone, Serialize)]
pub struct StopRecordResponse {
    /// `code` field of type `u16`.
    /// `code` 字段，类型为 `u16`.
    pub code: u16,
    /// `msg` field of type `String`.
    /// `msg` 字段，类型为 `String`.
    pub msg: String,
}

/// `ListTasksResponse` data structure.
/// `ListTasksResponse` 数据结构.
#[derive(Debug, Clone, Serialize)]
pub struct ListTasksResponse {
    /// `code` field of type `u16`.
    /// `code` 字段，类型为 `u16`.
    pub code: u16,
    /// `msg` field of type `String`.
    /// `msg` 字段，类型为 `String`.
    pub msg: String,
    /// `data` field.
    /// `data` 字段.
    pub data: Vec<TaskBrief>,
}

/// `TaskBrief` data structure.
/// `TaskBrief` 数据结构.
#[derive(Debug, Clone, Serialize)]
pub struct TaskBrief {
    /// `task_id` field of type `String`.
    /// `task_id` 字段，类型为 `String`.
    #[serde(rename = "taskId")]
    pub task_id: String,
    /// `format` field of type `String`.
    /// `format` 字段，类型为 `String`.
    pub format: String,
    /// `app` field of type `String`.
    /// `app` 字段，类型为 `String`.
    pub app: String,
    /// `stream` field of type `String`.
    /// `stream` 字段，类型为 `String`.
    pub stream: String,
    /// `state` field of type `String`.
    /// `state` 字段，类型为 `String`.
    pub state: String,
}

/// `FileQueryRequest` data structure.
/// `FileQueryRequest` 数据结构.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct FileQueryRequest {
    /// `app` field.
    /// `app` 字段.
    #[serde(default)]
    pub app: Option<String>,
    /// `stream` field.
    /// `stream` 字段.
    #[serde(default)]
    pub stream: Option<String>,
    /// `format` field.
    /// `format` 字段.
    #[serde(default)]
    pub format: Option<String>,
    /// `start_time_ms` field.
    /// `start_time_ms` 字段.
    #[serde(rename = "startTime", default)]
    pub start_time_ms: Option<i64>,
    /// `end_time_ms` field.
    /// `end_time_ms` 字段.
    #[serde(rename = "endTime", default)]
    pub end_time_ms: Option<i64>,
    /// `limit` field.
    /// `limit` 字段.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `FileQueryResponse` data structure.
/// `FileQueryResponse` 数据结构.
#[derive(Debug, Clone, Serialize)]
pub struct FileQueryResponse {
    /// `code` field of type `u16`.
    /// `code` 字段，类型为 `u16`.
    pub code: u16,
    /// `msg` field of type `String`.
    /// `msg` 字段，类型为 `String`.
    pub msg: String,
    /// `data` field.
    /// `data` 字段.
    pub data: Vec<FileBrief>,
}

/// `FileBrief` data structure.
/// `FileBrief` 数据结构.
#[derive(Debug, Clone, Serialize)]
pub struct FileBrief {
    /// `file_id` field of type `String`.
    /// `file_id` 字段，类型为 `String`.
    #[serde(rename = "fileId")]
    pub file_id: String,
    /// `format` field of type `String`.
    /// `format` 字段，类型为 `String`.
    pub format: String,
    /// `path` field of type `String`.
    /// `path` 字段，类型为 `String`.
    pub path: String,
    /// `duration_ms` field of type `u64`.
    /// `duration_ms` 字段，类型为 `u64`.
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    /// `size_bytes` field of type `u64`.
    /// `size_bytes` 字段，类型为 `u64`.
    #[serde(rename = "sizeBytes")]
    pub size_bytes: u64,
    /// `start_time_ms` field of type `i64`.
    /// `start_time_ms` 字段，类型为 `i64`.
    #[serde(rename = "startTimeMs")]
    pub start_time_ms: i64,
    /// `end_time_ms` field of type `i64`.
    /// `end_time_ms` 字段，类型为 `i64`.
    #[serde(rename = "endTimeMs")]
    pub end_time_ms: i64,
}

/// `FileDeleteRequest` data structure.
/// `FileDeleteRequest` 数据结构.
#[derive(Debug, Clone, Deserialize)]
pub struct FileDeleteRequest {
    /// `file_id` field of type `String`.
    /// `file_id` 字段，类型为 `String`.
    #[serde(rename = "fileId")]
    pub file_id: String,
}

/// Bundles a registry + executor for the HTTP service.
#[derive(Clone)]
pub struct RecordApi {
    /// `registry` field.
    /// `registry` 字段.
    registry: Arc<RecordRegistry>,
    /// `executor` field.
    /// `executor` 字段.
    executor: Arc<dyn TaskExecutor>,
}

impl RecordApi {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(registry: Arc<RecordRegistry>, executor: Arc<dyn TaskExecutor>) -> Self {
        Self { registry, executor }
    }

    /// `registry` function.
    /// `registry` 函数.
    pub fn registry(&self) -> Arc<RecordRegistry> {
        self.registry.clone()
    }

    /// `start` function.
    /// `start` 函数.
    pub async fn start(
        &self,
        req: StartRecordRequest,
    ) -> Result<StartRecordResponse, RecordApiError> {
        let format = RecordFormat::parse(&req.format)
            .ok_or_else(|| RecordApiError::UnsupportedFormat(req.format.clone()))?;
        if req.app.is_empty() || req.stream.is_empty() {
            return Err(RecordApiError::InvalidRequest(
                "app and stream must be non-empty".to_string(),
            ));
        }

        let task_id = req
            .task_id
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{}-{}-{}", req.format, req.app, req.stream));

        let tpl = req.record_template.unwrap_or_default();
        let template = RecordTaskTemplate {
            format,
            app: req.app.clone(),
            stream: req.stream.clone(),
            source_stream_key: req
                .uri
                .clone()
                .unwrap_or_else(|| format!("{}/{}", req.app, req.stream)),
            duration_limit_ms: tpl.duration.unwrap_or(0),
            segment_duration_ms: tpl.segment_duration.unwrap_or(0),
            segment_count_limit: tpl.segment_count.unwrap_or(0),
        };

        let metadata = crate::metadata::RecordTaskMetadata {
            task_id: task_id.clone(),
            format: RecordFormatStr::from(format),
            app: req.app,
            stream: req.stream,
            source_stream_key: template.source_stream_key.clone(),
            state: RecordTaskState::Pending,
            create_time_ms: now_ms(),
            duration_limit_ms: template.duration_limit_ms,
            segment_duration_ms: template.segment_duration_ms,
            segment_count_limit: template.segment_count_limit,
        };
        self.registry.insert_task(metadata)?;
        self.executor
            .spawn(crate::task::RecordTask {
                task_id: task_id.clone(),
                template,
            })
            .await?;
        self.registry
            .update_task_state(&task_id, RecordTaskState::Running)?;
        Ok(StartRecordResponse {
            code: 200,
            msg: "success".to_string(),
            task_id,
        })
    }

    /// `stop` function.
    /// `stop` 函数.
    pub async fn stop(&self, req: StopRecordRequest) -> Result<StopRecordResponse, RecordApiError> {
        self.executor.stop(&req.task_id).await?;
        let _ = self
            .registry
            .update_task_state(&req.task_id, RecordTaskState::Stopped);
        Ok(StopRecordResponse {
            code: 200,
            msg: "success".to_string(),
        })
    }

    /// `list` function.
    /// `list` 函数.
    pub fn list(&self) -> ListTasksResponse {
        let data = self
            .registry
            .list_tasks()
            .into_iter()
            .map(|t| TaskBrief {
                task_id: t.task_id,
                format: format_str_to_string(t.format),
                app: t.app,
                stream: t.stream,
                state: format!("{:?}", t.state).to_lowercase(),
            })
            .collect();
        ListTasksResponse {
            code: 200,
            msg: "success".to_string(),
            data,
        }
    }

    /// `query_files` function.
    /// `query_files` 函数.
    pub fn query_files(&self, req: FileQueryRequest) -> Result<FileQueryResponse, RecordApiError> {
        let format = match req.format.as_deref() {
            Some(s) => Some(parse_format_str(s)?),
            None => None,
        };
        let q = RecordFileQuery {
            app: req.app,
            stream: req.stream,
            format,
            start_time_ms: req.start_time_ms,
            end_time_ms: req.end_time_ms,
            limit: req.limit,
        };
        let data = self
            .registry
            .query_files(&q)
            .into_iter()
            .map(|f| FileBrief {
                file_id: f.file_id,
                format: format_str_to_string(f.format),
                path: f.path,
                duration_ms: f.duration_ms,
                size_bytes: f.size_bytes,
                start_time_ms: f.start_time_ms,
                end_time_ms: f.end_time_ms,
            })
            .collect();
        Ok(FileQueryResponse {
            code: 200,
            msg: "success".to_string(),
            data,
        })
    }

    /// `delete_file` function.
    /// `delete_file` 函数.
    pub fn delete_file(&self, req: FileDeleteRequest) -> Result<(), RecordApiError> {
        // Path traversal guard: file path is metadata-driven, but we still
        // refuse traversal segments in the file id.
        if req.file_id.contains("..") {
            return Err(RecordApiError::InvalidRequest(
                "file_id contains path traversal".to_string(),
            ));
        }
        self.registry.remove_file(&req.file_id)?;
        Ok(())
    }
}

fn parse_format_str(input: &str) -> Result<RecordFormatStr, RecordApiError> {
    match input.to_ascii_lowercase().as_str() {
        "flv" => Ok(RecordFormatStr::Flv),
        "hls" => Ok(RecordFormatStr::Hls),
        "mp4" => Ok(RecordFormatStr::Mp4),
        "ps" => Ok(RecordFormatStr::Ps),
        other => Err(RecordApiError::UnsupportedFormat(other.to_string())),
    }
}

fn format_str_to_string(s: RecordFormatStr) -> String {
    match s {
        RecordFormatStr::Flv => "flv".into(),
        RecordFormatStr::Hls => "hls".into(),
        RecordFormatStr::Mp4 => "mp4".into(),
        RecordFormatStr::Ps => "ps".into(),
    }
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
    use async_trait::async_trait;

    struct MockExecutor;

    #[async_trait]
    impl TaskExecutor for MockExecutor {
        async fn spawn(&self, _task: crate::task::RecordTask) -> Result<(), TaskExecutorError> {
            Ok(())
        }
        async fn stop(&self, _id: &str) -> Result<(), TaskExecutorError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn start_and_list_tasks() {
        let api = RecordApi::new(Arc::new(RecordRegistry::new(8)), Arc::new(MockExecutor));
        let resp = api
            .start(StartRecordRequest {
                format: "mp4".to_string(),
                app: "live".to_string(),
                stream: "test".to_string(),
                uri: None,
                task_id: None,
                record_template: None,
            })
            .await
            .unwrap();
        assert_eq!(resp.code, 200);
        let list = api.list();
        assert_eq!(list.data.len(), 1);
        assert_eq!(list.data[0].format, "mp4");
        assert_eq!(list.data[0].state, "running");
    }

    #[tokio::test]
    async fn unsupported_format_rejected() {
        let api = RecordApi::new(Arc::new(RecordRegistry::new(8)), Arc::new(MockExecutor));
        let err = api
            .start(StartRecordRequest {
                format: "asf".to_string(),
                app: "live".to_string(),
                stream: "test".to_string(),
                uri: None,
                task_id: None,
                record_template: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, RecordApiError::UnsupportedFormat(_)));
    }

    #[tokio::test]
    async fn delete_file_rejects_traversal() {
        let api = RecordApi::new(Arc::new(RecordRegistry::new(8)), Arc::new(MockExecutor));
        let err = api
            .delete_file(FileDeleteRequest {
                file_id: "../etc/passwd".to_string(),
            })
            .unwrap_err();
        assert!(matches!(err, RecordApiError::InvalidRequest(_)));
    }
}
