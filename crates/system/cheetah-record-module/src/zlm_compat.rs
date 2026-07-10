//! ZLMediaKit-compatible record API.
//!
//! Mirrors the request/response shapes from
//! `vendor-ref/ZLMediaKit/server/WebApi.cpp` for `startRecord`, `stopRecord`,
//! `isRecording`, `getMP4RecordFile`, and `deleteRecordDirectory`. The
//! handlers translate the ZLM payload model into the project's internal
//! `RecordApi`, so both the cheetah-style and ZLM-style clients can drive
//! the same registry.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use crate::api::{
    FileDeleteRequest, FileQueryRequest, RecordApi, RecordApiError, StartRecordRequest,
    StopRecordRequest,
};

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

/// Bundles `RecordApi` and exposes ZLM-style handlers.
#[derive(Clone)]
pub struct ZlmRecordCompat {
    inner: Arc<RecordApi>,
}

impl ZlmRecordCompat {
    pub fn new(inner: Arc<RecordApi>) -> Self {
        Self { inner }
    }

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

    pub async fn stop_record(&self, req: ZlmStopRecord) -> Result<Value, ZlmCompatError> {
        let format = parse_zlm_type(&req.r#type)?;
        let task_id = zlm_task_id(&format, &req.app, &req.stream);
        self.inner.stop(StopRecordRequest { task_id }).await?;
        Ok(serde_json::json!({ "code": 0, "result": true }))
    }

    pub fn is_recording(&self, req: ZlmIsRecording) -> Result<Value, ZlmCompatError> {
        let format = parse_zlm_type(&req.r#type)?;
        let task_id = zlm_task_id(&format, &req.app, &req.stream);
        let recording = self.inner.registry().get_task(&task_id).is_some();
        Ok(serde_json::json!({ "code": 0, "status": recording }))
    }

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

fn zlm_task_id(format: &str, app: &str, stream: &str) -> String {
    format!("{format}-{app}-{stream}")
}

/// Reject path traversal and absolute paths so `customized_path` cannot
/// escape the configured record root.
fn validate_customized_path(path: &str) -> Result<(), ZlmCompatError> {
    if path.contains("..") || path.starts_with('/') || path.contains('\\') {
        return Err(ZlmCompatError::PathNotAllowed(path.to_string()));
    }
    Ok(())
}

/// Map ZLM `period=YYYY-MM` (month) or `period=YYYY-MM-DD` (day) into the
/// internal millisecond range filter.
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
fn month_start_ms(year: i32, month: u32) -> i64 {
    days_from_civil(year, month, 1) * 86_400_000
}

fn month_end_ms(year: i32, month: u32) -> i64 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    days_from_civil(ny, nm, 1) * 86_400_000 - 1
}

fn day_start_ms(year: i32, month: u32, day: u32) -> i64 {
    days_from_civil(year, month, day) * 86_400_000
}

fn day_end_ms(year: i32, month: u32, day: u32) -> i64 {
    day_start_ms(year, month, day) + 86_400_000 - 1
}

/// Howard Hinnant's `days_from_civil` for converting (y, m, d) → days since
/// the Unix epoch. Bounded, branchless, no leap-year edge cases.
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
