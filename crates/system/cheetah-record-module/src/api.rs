//! SMS-compatible record HTTP API surface (request/response models only).
//!
//! The API layer is framework-neutral: the engine's HTTP module wrapper
//! routes a `cheetah_sdk::HttpRequest` to one of these handlers via the
//! `RecordModule::http_service()`. JSON shapes are intentionally close to
//! `vendor-ref/simple-media-server/Src/Api/RecordApi.cpp`.
//!
//! SMS 兼容的录制 HTTP API 接口（仅请求/响应模型）。
//!
//! API 层框架无关：引擎的 HTTP 模块包装器将 `cheetah_sdk::HttpRequest`
//! 路由到 `RecordModule::http_service()` 中对应处理器。JSON 结构有意与
//! `vendor-ref/simple-media-server/Src/Api/RecordApi.cpp` 保持一致。

use std::sync::Arc;

use cheetah_codec::RecordFormat;
use serde::{Deserialize, Serialize};

use crate::metadata::{RecordFileQuery, RecordFormatStr, RecordTaskState};
use crate::registry::{RecordRegistry, RegistryError};
use crate::task::{RecordTaskTemplate, TaskExecutor, TaskExecutorError};

/// Errors returned by the record API.
///
/// Wraps validation failures, registry errors, executor errors, and unsupported
/// formats.
///
/// 录制 API 返回的错误。
///
/// 封装校验失败、注册表错误、执行器错误以及不支持的格式。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RecordApiError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),
    #[error("executor error: {0}")]
    Executor(#[from] TaskExecutorError),
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
}

/// `POST /api/v1/record/start` body.
///
/// Starts a recording for the given app/stream. The `task_id` field can be
/// provided by the caller; otherwise it is derived from `format-app-stream`.
///
/// `POST /api/v1/record/start` 请求体。
///
/// 为指定 app/stream 启动录制。`task_id` 可由调用方提供；否则根据
/// `format-app-stream` 推导。
#[derive(Debug, Clone, Deserialize)]
pub struct StartRecordRequest {
    pub format: String,
    pub app: String,
    pub stream: String,
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(rename = "recordTemplate", default)]
    pub record_template: Option<RecordTemplate>,
}

/// Optional recording template controlling duration and segmentation.
///
/// 可选的录制模板，用于控制时长和分片。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RecordTemplate {
    #[serde(default)]
    pub duration: Option<u64>,
    #[serde(rename = "segmentDuration", default)]
    pub segment_duration: Option<u64>,
    #[serde(rename = "segmentCount", default)]
    pub segment_count: Option<u32>,
}

/// `POST /api/v1/record/start` response.
///
/// `POST /api/v1/record/start` 响应。
#[derive(Debug, Clone, Serialize)]
pub struct StartRecordResponse {
    pub code: u16,
    pub msg: String,
    #[serde(rename = "taskId")]
    pub task_id: String,
}

/// `POST /api/v1/record/stop` body.
///
/// `POST /api/v1/record/stop` 请求体。
#[derive(Debug, Clone, Deserialize)]
pub struct StopRecordRequest {
    #[serde(rename = "taskId")]
    pub task_id: String,
}

/// `POST /api/v1/record/stop` response.
///
/// `POST /api/v1/record/stop` 响应。
#[derive(Debug, Clone, Serialize)]
pub struct StopRecordResponse {
    pub code: u16,
    pub msg: String,
}

/// `GET /api/v1/record/list` and `GET /api/v1/record/query` response.
///
/// `GET /api/v1/record/list` 与 `GET /api/v1/record/query` 响应。
#[derive(Debug, Clone, Serialize)]
pub struct ListTasksResponse {
    pub code: u16,
    pub msg: String,
    pub data: Vec<TaskBrief>,
}

/// Brief summary of a task returned by the list endpoint.
///
/// 列表端点返回的任务摘要。
#[derive(Debug, Clone, Serialize)]
pub struct TaskBrief {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub format: String,
    pub app: String,
    pub stream: String,
    pub state: String,
}

/// `GET /api/v1/record/file/query` body.
///
/// `GET /api/v1/record/file/query` 请求体。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct FileQueryRequest {
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(rename = "startTime", default)]
    pub start_time_ms: Option<i64>,
    #[serde(rename = "endTime", default)]
    pub end_time_ms: Option<i64>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `GET /api/v1/record/file/query` response.
///
/// `GET /api/v1/record/file/query` 响应。
#[derive(Debug, Clone, Serialize)]
pub struct FileQueryResponse {
    pub code: u16,
    pub msg: String,
    pub data: Vec<FileBrief>,
}

/// Brief summary of a file returned by the file query endpoint.
///
/// 文件查询端点返回的文件摘要。
#[derive(Debug, Clone, Serialize)]
pub struct FileBrief {
    #[serde(rename = "fileId")]
    pub file_id: String,
    pub format: String,
    pub path: String,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    #[serde(rename = "sizeBytes")]
    pub size_bytes: u64,
    #[serde(rename = "startTimeMs")]
    pub start_time_ms: i64,
    #[serde(rename = "endTimeMs")]
    pub end_time_ms: i64,
}

/// `POST /api/v1/record/file/delete` body.
///
/// `POST /api/v1/record/file/delete` 请求体。
#[derive(Debug, Clone, Deserialize)]
pub struct FileDeleteRequest {
    #[serde(rename = "fileId")]
    pub file_id: String,
}

/// Bundles a registry + executor for the HTTP service.
///
/// The API layer owns no I/O; it translates HTTP requests into registry
/// and executor operations.
///
/// 将注册表与执行器组合后供 HTTP 服务使用。
///
/// API 层不持有 I/O；它把 HTTP 请求翻译成注册表与执行器操作。
#[derive(Clone)]
pub struct RecordApi {
    registry: Arc<RecordRegistry>,
    executor: Arc<dyn TaskExecutor>,
}

impl RecordApi {
    /// Construct a new API handle around the registry and executor.
    ///
    /// 基于注册表和执行器构造新的 API 句柄。
    pub fn new(registry: Arc<RecordRegistry>, executor: Arc<dyn TaskExecutor>) -> Self {
        Self { registry, executor }
    }

    /// Return a clone of the registry handle.
    ///
    /// 返回注册表句柄的克隆。
    pub fn registry(&self) -> Arc<RecordRegistry> {
        self.registry.clone()
    }

    /// Start a new recording task from the SMS-style request.
    ///
    /// Validates the format, registers a Pending task, spawns the executor,
    /// and transitions the task to Running.
    ///
    /// 从 SMS 风格请求启动新的录制任务。
    ///
    /// 校验格式、注册 Pending 任务、派生执行器，并将任务转为 Running。
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

    /// Stop a recording task and mark it Stopped in the registry.
    ///
    /// 停止录制任务并在注册表中将状态标记为 Stopped。
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

    /// List all registered tasks as brief records.
    ///
    /// 将所有已注册任务以简要记录形式返回。
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

    /// Query files from the registry using the public request shape.
    ///
    /// 使用公共请求结构从注册表查询文件。
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

    /// Delete a file record from the registry.
    ///
    /// Path traversal in the file id is rejected before delegating to the
    /// registry, since the registry path is metadata-driven.
    ///
    /// 从注册表删除文件记录。
    ///
    /// 由于注册表路径由元数据驱动，file_id 中的路径遍历会在委托给注册表前被拒绝。
    pub fn delete_file(&self, req: FileDeleteRequest) -> Result<(), RecordApiError> {
        if req.file_id.contains("..") {
            return Err(RecordApiError::InvalidRequest(
                "file_id contains path traversal".to_string(),
            ));
        }
        self.registry.remove_file(&req.file_id)?;
        Ok(())
    }
}

/// Parse a client-supplied format string into the internal enum.
///
/// 将客户端提供的格式字符串解析为内部枚举。
fn parse_format_str(input: &str) -> Result<RecordFormatStr, RecordApiError> {
    match input.to_ascii_lowercase().as_str() {
        "flv" => Ok(RecordFormatStr::Flv),
        "hls" => Ok(RecordFormatStr::Hls),
        "mp4" => Ok(RecordFormatStr::Mp4),
        "ps" => Ok(RecordFormatStr::Ps),
        other => Err(RecordApiError::UnsupportedFormat(other.to_string())),
    }
}

/// Convert the internal format enum to a lowercase string for JSON output.
///
/// 将内部格式枚举转换为小写字符串用于 JSON 输出。
fn format_str_to_string(s: RecordFormatStr) -> String {
    match s {
        RecordFormatStr::Flv => "flv".into(),
        RecordFormatStr::Hls => "hls".into(),
        RecordFormatStr::Mp4 => "mp4".into(),
        RecordFormatStr::Ps => "ps".into(),
    }
}

/// Current wall-clock time in milliseconds since the Unix epoch.
///
/// 自 Unix 纪元以来的当前墙上时间（毫秒）。
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
