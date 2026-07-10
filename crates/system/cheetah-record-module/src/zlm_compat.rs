//! ZLMediaKit-compatible record API.
//!
//! Mirrors the request/response shapes from
//! `vendor-ref/ZLMediaKit/server/WebApi.cpp` for `startRecord`, `stopRecord`,
//! `isRecording`, `getMP4RecordFile`, and `deleteRecordDirectory`. The
//! handlers translate the ZLM payload model into the project's internal
//! `RecordApi`, so both the cheetah-style and ZLM-style clients can drive
//! the same registry.
//!
//! ZLMediaKit 兼容的录制 API。
//!
//! 镜像 `vendor-ref/ZLMediaKit/server/WebApi.cpp` 中 `startRecord`、`stopRecord`、
//! `isRecording`、`getMP4RecordFile` 与 `deleteRecordDirectory` 的请求/响应结构。
//! 处理器将 ZLM 负载模型转换为本项目的内部 `RecordApi`，使 cheetah 风格与 ZLM 风格
//! 的客户端都能驱动同一个注册表。

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use crate::api::{
    FileDeleteRequest, FileQueryRequest, RecordApi, RecordApiError, StartRecordRequest,
    StopRecordRequest,
};

/// Errors returned by ZLMediaKit compatibility handlers.
///
/// ZLMediaKit 兼容处理器返回的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ZlmCompatError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("api error: {0}")]
    Api(#[from] RecordApiError),
    #[error("path not allowed: {0}")]
    PathNotAllowed(String),
}

/// `POST /index/api/startRecord` body.
///
/// ZLM accepts both numeric (0/1/2/3) and string (`"mp4"`/`"hls"`/...) `type`.
///
/// `POST /index/api/startRecord` 请求体。
///
/// ZLM 接受数字（0/1/2/3）和字符串（`"mp4"`/`"hls"`/...）两种 `type`。
#[derive(Debug, Clone, Deserialize)]
pub struct ZlmStartRecord {
    /// ZLM accepts both numeric (0/1/2/3) and string ("mp4"/"hls"/...) types.
    #[serde(rename = "type")]
    pub r#type: Value,
    #[serde(default)]
    pub vhost: Option<String>,
    pub app: String,
    pub stream: String,
    #[serde(rename = "customized_path", default)]
    pub customized_path: Option<String>,
    #[serde(rename = "max_second", default)]
    pub max_second: Option<u64>,
}

/// `POST /index/api/stopRecord` body.
///
/// `POST /index/api/stopRecord` 请求体。
#[derive(Debug, Clone, Deserialize)]
pub struct ZlmStopRecord {
    #[serde(rename = "type")]
    pub r#type: Value,
    #[serde(default)]
    pub vhost: Option<String>,
    pub app: String,
    pub stream: String,
}

/// `GET /index/api/isRecording` query body.
///
/// `GET /index/api/isRecording` 查询体。
#[derive(Debug, Clone, Deserialize)]
pub struct ZlmIsRecording {
    #[serde(rename = "type")]
    pub r#type: Value,
    #[serde(default)]
    pub vhost: Option<String>,
    pub app: String,
    pub stream: String,
}

/// `GET /index/api/getMP4RecordFile` query body.
///
/// `GET /index/api/getMP4RecordFile` 查询体。
#[derive(Debug, Clone, Deserialize)]
pub struct ZlmGetMp4Files {
    #[serde(default)]
    pub vhost: Option<String>,
    pub app: String,
    pub stream: String,
    #[serde(default)]
    pub period: Option<String>,
    #[serde(rename = "customized_path", default)]
    pub customized_path: Option<String>,
}

/// `POST /index/api/deleteRecordDirectory` body.
///
/// `POST /index/api/deleteRecordDirectory` 请求体。
#[derive(Debug, Clone, Deserialize)]
pub struct ZlmDeleteDirectory {
    #[serde(default)]
    pub vhost: Option<String>,
    pub app: String,
    pub stream: String,
    #[serde(default)]
    pub period: Option<String>,
    #[serde(rename = "customized_path", default)]
    pub customized_path: Option<String>,
}

/// ZLMediaKit compatibility layer around `RecordApi`.
///
/// Translates ZLM-style request/response payloads into the internal SMS-style
/// API so the same registry and executor are used for both APIs.
///
/// 围绕 `RecordApi` 的 ZLMediaKit 兼容层。
///
/// 将 ZLM 风格的请求/响应负载转换为内部 SMS 风格 API，使两种 API 共用同一注册表与执行器。
#[derive(Clone)]
pub struct ZlmRecordCompat {
    inner: Arc<RecordApi>,
}

impl ZlmRecordCompat {
    /// Create a new compat wrapper around the internal API.
    ///
    /// 在内部 API 之上创建新的兼容包装器。
    pub fn new(inner: Arc<RecordApi>) -> Self {
        Self { inner }
    }

    /// Start a recording using the ZLM payload shape.
    ///
    /// `max_second` is converted to milliseconds and used as both duration
    /// and segment duration. `customized_path` is validated but not used in V1.
    ///
    /// 使用 ZLM 负载结构启动录制。
    ///
    /// `max_second` 被转换为毫秒并同时作为时长与分片时长。`customized_path` 会校验
    /// 但 V1 中尚未使用。
    pub async fn start_record(&self, req: ZlmStartRecord) -> Result<Value, ZlmCompatError> {
        let format = parse_zlm_type(&req.r#type)?;
        if let Some(path) = &req.customized_path {
            validate_customized_path(path)?;
        }
        let internal = StartRecordRequest {
            format: format.clone(),
            app: req.app.clone(),
            stream: req.stream.clone(),
            uri: None,
            task_id: Some(zlm_task_id(&format, &req.app, &req.stream)),
            record_template: req.max_second.map(|secs| crate::api::RecordTemplate {
                duration: Some(secs.saturating_mul(1000)),
                segment_duration: Some(secs.saturating_mul(1000)),
                segment_count: None,
            }),
        };
        self.inner.start(internal).await?;
        // ZLM `startRecord` returns `{"code":0,"result":true}` — flat shape.
        Ok(serde_json::json!({ "code": 0, "result": true }))
    }

    /// Stop a recording using the ZLM payload shape.
    ///
    /// 使用 ZLM 负载结构停止录制。
    pub async fn stop_record(&self, req: ZlmStopRecord) -> Result<Value, ZlmCompatError> {
        let format = parse_zlm_type(&req.r#type)?;
        let task_id = zlm_task_id(&format, &req.app, &req.stream);
        self.inner.stop(StopRecordRequest { task_id }).await?;
        Ok(serde_json::json!({ "code": 0, "result": true }))
    }

    /// Check whether a recording is currently active for the given ZLM type.
    ///
    /// 检查指定 ZLM 类型是否正在录制。
    pub fn is_recording(&self, req: ZlmIsRecording) -> Result<Value, ZlmCompatError> {
        let format = parse_zlm_type(&req.r#type)?;
        let task_id = zlm_task_id(&format, &req.app, &req.stream);
        let recording = self.inner.registry().get_task(&task_id).is_some();
        Ok(serde_json::json!({ "code": 0, "status": recording }))
    }

    /// List MP4 record file paths matching the ZLM query.
    ///
    /// The `period` parameter is mapped to a millisecond range filter.
    ///
    /// 列出匹配 ZLM 查询的 MP4 录制文件路径。
    ///
    /// `period` 参数会被映射为毫秒级时间范围过滤。
    pub fn get_mp4_files(&self, req: ZlmGetMp4Files) -> Result<Value, ZlmCompatError> {
        if let Some(path) = &req.customized_path {
            validate_customized_path(path)?;
        }
        let mut q = FileQueryRequest {
            app: Some(req.app.clone()),
            stream: Some(req.stream.clone()),
            format: Some("mp4".to_string()),
            ..Default::default()
        };
        if let Some(period) = req.period.as_deref() {
            apply_period(period, &mut q)?;
        }
        let resp = self.inner.query_files(q)?;
        let paths: Vec<String> = resp.data.iter().map(|f| f.path.clone()).collect();
        Ok(serde_json::json!({
            "code": 0,
            "data": {
                "paths": paths,
                "rootPath": "",
            },
        }))
    }

    /// Delete all record files matching the ZLM directory request.
    ///
    /// Maps `period` and `app`/`stream` into an internal file query and then
    /// deletes each file record. Actual on-disk cleanup is not performed here.
    ///
    /// 删除所有匹配 ZLM 目录请求的记录文件。
    ///
    /// 将 `period` 与 `app`/`stream` 映射为内部文件查询，然后删除每条文件记录。
    /// 实际的磁盘清理不在这里执行。
    pub fn delete_record_directory(
        &self,
        req: ZlmDeleteDirectory,
    ) -> Result<Value, ZlmCompatError> {
        if let Some(path) = &req.customized_path {
            validate_customized_path(path)?;
        }
        let mut q = FileQueryRequest {
            app: Some(req.app.clone()),
            stream: Some(req.stream.clone()),
            ..Default::default()
        };
        if let Some(period) = req.period.as_deref() {
            apply_period(period, &mut q)?;
        }
        let listing = self.inner.query_files(q)?;
        for f in listing.data {
            self.inner
                .delete_file(FileDeleteRequest { file_id: f.file_id })?;
        }
        Ok(serde_json::json!({ "code": 0, "result": true }))
    }
}

/// Parse the ZLM `type` field into a normalized format string.
///
/// Numeric values follow ZLM's convention (0=mp4, 1=hls, 2=hls, 3=fmp4).
/// String values are lowercased. `fmp4` is treated as `mp4` for the V1 registry.
///
/// 将 ZLM `type` 字段解析为规范化的格式字符串。
///
/// 数字值遵循 ZLM 约定（0=mp4、1=hls、2=hls、3=fmp4）。字符串值转为小写。
/// V1 注册表将 `fmp4` 视为 `mp4`。
fn parse_zlm_type(value: &Value) -> Result<String, ZlmCompatError> {
    if let Some(num) = value.as_u64() {
        return Ok(match num {
            0 => "mp4".to_string(),
            1 => "hls".to_string(),
            2 => "hls".to_string(), // hls_fmp4 collapses to hls in V1 registry
            3 => "fmp4".to_string(),
            other => {
                return Err(ZlmCompatError::InvalidRequest(format!(
                    "unsupported numeric type {other}"
                )))
            }
        });
    }
    if let Some(s) = value.as_str() {
        return Ok(s.to_lowercase());
    }
    Err(ZlmCompatError::InvalidRequest(
        "type must be number or string".to_string(),
    ))
}

/// Build a deterministic task id for the ZLM API.
///
/// 为 ZLM API 构建确定性任务 ID。
fn zlm_task_id(format: &str, app: &str, stream: &str) -> String {
    format!("{format}-{app}-{stream}")
}

/// Reject path traversal and absolute paths so `customized_path` cannot
/// escape the configured record root.
///
/// 拒绝路径遍历与绝对路径，防止 `customized_path` 逃出配置的记录根目录。
fn validate_customized_path(path: &str) -> Result<(), ZlmCompatError> {
    if path.contains("..") || path.starts_with('/') || path.contains('\\') {
        return Err(ZlmCompatError::PathNotAllowed(path.to_string()));
    }
    Ok(())
}

/// Map ZLM `period=YYYY-MM` (month) or `period=YYYY-MM-DD` (day) into the
/// internal millisecond range filter.
///
/// 将 ZLM `period=YYYY-MM`（月）或 `period=YYYY-MM-DD`（日）映射为内部毫秒范围过滤器。
fn apply_period(period: &str, q: &mut FileQueryRequest) -> Result<(), ZlmCompatError> {
    let parts: Vec<&str> = period.split('-').collect();
    let (start, end) = match parts.len() {
        2 => {
            let y: i32 = parts[0]
                .parse()
                .map_err(|_| ZlmCompatError::InvalidRequest("bad period year".into()))?;
            let m: u32 = parts[1]
                .parse()
                .map_err(|_| ZlmCompatError::InvalidRequest("bad period month".into()))?;
            (month_start_ms(y, m), month_end_ms(y, m))
        }
        3 => {
            let y: i32 = parts[0]
                .parse()
                .map_err(|_| ZlmCompatError::InvalidRequest("bad period year".into()))?;
            let m: u32 = parts[1]
                .parse()
                .map_err(|_| ZlmCompatError::InvalidRequest("bad period month".into()))?;
            let d: u32 = parts[2]
                .parse()
                .map_err(|_| ZlmCompatError::InvalidRequest("bad period day".into()))?;
            (day_start_ms(y, m, d), day_end_ms(y, m, d))
        }
        _ => {
            return Err(ZlmCompatError::InvalidRequest(format!(
                "unsupported period format: {period}"
            )))
        }
    };
    q.start_time_ms = Some(start);
    q.end_time_ms = Some(end);
    Ok(())
}

// Calendar helpers. We only need ms-since-epoch boundaries, not full-blown
// timezone math; the project's record store records `start_time_ms` as
// system clock anyway, so a UTC approximation is good enough for filtering.

/// Start of the month in milliseconds since the Unix epoch.
///
/// 自 Unix 纪元以来该月开始的毫秒数。
fn month_start_ms(year: i32, month: u32) -> i64 {
    days_from_civil(year, month, 1) * 86_400_000
}

/// End of the month in milliseconds since the Unix epoch.
///
/// 自 Unix 纪元以来该月结束的毫秒数。
fn month_end_ms(year: i32, month: u32) -> i64 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    days_from_civil(ny, nm, 1) * 86_400_000 - 1
}

/// Start of the day in milliseconds since the Unix epoch.
///
/// 自 Unix 纪元以来该日开始的毫秒数。
fn day_start_ms(year: i32, month: u32, day: u32) -> i64 {
    days_from_civil(year, month, day) * 86_400_000
}

/// End of the day in milliseconds since the Unix epoch.
///
/// 自 Unix 纪元以来该日结束的毫秒数。
fn day_end_ms(year: i32, month: u32, day: u32) -> i64 {
    day_start_ms(year, month, day) + 86_400_000 - 1
}

/// Howard Hinnant's `days_from_civil` for converting (y, m, d) → days since
/// the Unix epoch. Bounded, branchless, no leap-year edge cases.
///
/// Howard Hinnant 的 `days_from_civil`，将 (y, m, d) 转换为自 Unix 纪元以来的天数。
/// 有界、无分支、无闰年边界问题。
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era as i64) * 146_097 + doe as i64 - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RecordRegistry;
    use crate::task::TaskExecutor;
    use async_trait::async_trait;

    struct Stub;

    #[async_trait]
    impl TaskExecutor for Stub {
        async fn spawn(
            &self,
            _task: crate::task::RecordTask,
        ) -> Result<(), crate::task::TaskExecutorError> {
            Ok(())
        }
        async fn stop(&self, _id: &str) -> Result<(), crate::task::TaskExecutorError> {
            Ok(())
        }
    }

    fn make() -> ZlmRecordCompat {
        let api = RecordApi::new(Arc::new(RecordRegistry::new(8)), Arc::new(Stub));
        ZlmRecordCompat::new(Arc::new(api))
    }

    #[tokio::test]
    async fn parse_numeric_type_accepts_known_values() {
        assert_eq!(parse_zlm_type(&Value::from(0u64)).unwrap(), "mp4");
        assert_eq!(parse_zlm_type(&Value::from(1u64)).unwrap(), "hls");
        assert!(parse_zlm_type(&Value::from(99u64)).is_err());
    }

    #[tokio::test]
    async fn parse_string_type_lowercases() {
        assert_eq!(parse_zlm_type(&Value::from("MP4")).unwrap(), "mp4");
    }

    #[tokio::test]
    async fn customized_path_rejects_traversal() {
        assert!(validate_customized_path("../etc").is_err());
        assert!(validate_customized_path("/abs").is_err());
        assert!(validate_customized_path("ok/relative").is_ok());
    }

    #[tokio::test]
    async fn period_parses_month_and_day_forms() {
        let mut q = FileQueryRequest::default();
        apply_period("2026-05", &mut q).unwrap();
        assert!(q.start_time_ms.is_some());
        assert!(q.end_time_ms.is_some());

        let mut q = FileQueryRequest::default();
        apply_period("2026-05-23", &mut q).unwrap();
        let d = q.end_time_ms.unwrap() - q.start_time_ms.unwrap();
        assert_eq!(d, 86_400_000 - 1);
    }

    #[tokio::test]
    async fn start_record_round_trips_to_internal_api() {
        let compat = make();
        let resp = compat
            .start_record(ZlmStartRecord {
                r#type: Value::from(0u64),
                vhost: None,
                app: "live".to_string(),
                stream: "test".to_string(),
                customized_path: None,
                max_second: Some(60),
            })
            .await
            .unwrap();
        assert_eq!(resp["code"], serde_json::json!(0));
    }

    #[tokio::test]
    async fn is_recording_reflects_registry() {
        let compat = make();
        compat
            .start_record(ZlmStartRecord {
                r#type: Value::from(0u64),
                vhost: None,
                app: "live".to_string(),
                stream: "abc".to_string(),
                customized_path: None,
                max_second: None,
            })
            .await
            .unwrap();
        let resp = compat
            .is_recording(ZlmIsRecording {
                r#type: Value::from(0u64),
                vhost: None,
                app: "live".to_string(),
                stream: "abc".to_string(),
            })
            .unwrap();
        assert_eq!(resp["status"], serde_json::json!(true));
    }
}
