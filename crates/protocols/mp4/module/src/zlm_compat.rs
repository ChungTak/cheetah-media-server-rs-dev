//! ZLMediaKit-compatible VOD API and URI normalization helpers.
//!
//! Supports the subset of `vendor-ref/ZLMediaKit/server/WebApi.cpp` that
//! drives MP4 file playback: `loadMP4File`, `seekRecordStamp`,
//! `setRecordSpeed`. Also exposes the `mp4:` URI normalization used by the
//! RTMP play-stream handler so any caller can ingest legacy ZLM clients.

use std::sync::Arc;

use cheetah_mp4_core::VodControlCommand;
use serde::Deserialize;

use crate::api::{StartVodRequest, VodApi, VodApiError};
use crate::session_registry::SessionError;

/// `ZlmVodError` enumeration.
/// `ZlmVodError` 枚举.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ZlmVodError {
    /// `InvalidRequest` variant.
    /// `InvalidRequest` 变体.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// `SessionNotFound` variant.
    /// `SessionNotFound` 变体.
    #[error("session not found: {0}")]
    SessionNotFound(String),
    /// `Api` variant.
    /// `Api` 变体.
    #[error("api: {0}")]
    Api(#[from] VodApiError),
}

/// `POST /index/api/loadMP4File` body.
#[derive(Debug, Clone, Deserialize)]
pub struct ZlmLoadMp4 {
    /// `vhost` field.
    /// `vhost` 字段.
    #[serde(default)]
    pub vhost: Option<String>,
    /// `app` field of type `String`.
    /// `app` 字段，类型为 `String`.
    pub app: String,
    /// `stream` field of type `String`.
    /// `stream` 字段，类型为 `String`.
    pub stream: String,
    /// `file_path` field of type `String`.
    /// `file_path` 字段，类型为 `String`.
    pub file_path: String,
    /// `seek_ms` field.
    /// `seek_ms` 字段.
    #[serde(default)]
    pub seek_ms: Option<i64>,
    /// `speed` field.
    /// `speed` 字段.
    #[serde(default)]
    pub speed: Option<f32>,
    /// `file_repeat` field.
    /// `file_repeat` 字段.
    #[serde(default)]
    pub file_repeat: Option<bool>,
}

/// `POST /index/api/seekRecordStamp` body.
#[derive(Debug, Clone, Deserialize)]
pub struct ZlmSeekRecord {
    /// `vhost` field.
    /// `vhost` 字段.
    #[serde(default)]
    pub vhost: Option<String>,
    /// `app` field of type `String`.
    /// `app` 字段，类型为 `String`.
    pub app: String,
    /// `stream` field of type `String`.
    /// `stream` 字段，类型为 `String`.
    pub stream: String,
    /// `stamp` field of type `i64`.
    /// `stamp` 字段，类型为 `i64`.
    pub stamp: i64,
}

/// `POST /index/api/setRecordSpeed` body.
#[derive(Debug, Clone, Deserialize)]
pub struct ZlmSetSpeed {
    /// `vhost` field.
    /// `vhost` 字段.
    #[serde(default)]
    pub vhost: Option<String>,
    /// `app` field of type `String`.
    /// `app` 字段，类型为 `String`.
    pub app: String,
    /// `stream` field of type `String`.
    /// `stream` 字段，类型为 `String`.
    pub stream: String,
    /// `speed` field of type `f32`.
    /// `speed` 字段，类型为 `f32`.
    pub speed: f32,
}

/// ZLM compat surface for the MP4 VOD module.
#[derive(Clone)]
pub struct ZlmVodCompat {
    /// `inner` field.
    /// `inner` 字段.
    inner: Arc<VodApi>,
}

impl ZlmVodCompat {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(inner: Arc<VodApi>) -> Self {
        Self { inner }
    }

    /// `load_mp4` function.
    /// `load_mp4` 函数.
    pub async fn load_mp4(&self, req: ZlmLoadMp4) -> Result<serde_json::Value, ZlmVodError> {
        let session_id = vod_session_id(&req.app, &req.stream);
        let normalized = normalize_rtmp_mp4_uri(&req.file_path);
        let resp = self
            .inner
            .start(StartVodRequest {
                uri: normalized,
                format: Some("mp4".to_string()),
                start_time_ms: req.seek_ms,
                end_time_ms: None,
                loop_count: req.file_repeat.map(|r| if r { u32::MAX } else { 1 }),
                session_id: Some(session_id.clone()),
            })
            .await?;
        if let Some(speed) = req.speed {
            self.inner.control(crate::api::ControlVodRequest {
                session_id: session_id.clone(),
                seek: None,
                pause: None,
                scale: Some(speed),
            })?;
        }
        // ZLM `loadMP4File` returns `{"code":0,"data":{...}}` — flat shape.
        Ok(serde_json::json!({
            "code": 0,
            "data": {
                "sessionId": resp.session_id,
                "duration_ms": 0,
            },
        }))
    }

    /// `seek_record` function.
    /// `seek_record` 函数.
    pub fn seek_record(&self, req: ZlmSeekRecord) -> Result<serde_json::Value, ZlmVodError> {
        let session_id = vod_session_id(&req.app, &req.stream);
        let handle = self
            .inner
            .registry()
            .handle(&session_id)
            .ok_or_else(|| ZlmVodError::SessionNotFound(session_id.clone()))?;
        handle
            .send_control(VodControlCommand::Seek {
                position_us: req.stamp.saturating_mul(1_000),
            })
            .map_err(|e| ZlmVodError::InvalidRequest(e.to_string()))?;
        Ok(serde_json::json!({ "code": 0, "result": true }))
    }

    /// Sets the `speed` value.
    /// Sets `speed` 值.
    pub fn set_speed(&self, req: ZlmSetSpeed) -> Result<serde_json::Value, ZlmVodError> {
        if !(0.1..=20.0).contains(&req.speed) {
            return Err(ZlmVodError::InvalidRequest(format!(
                "speed {} out of [0.1, 20.0]",
                req.speed
            )));
        }
        let session_id = vod_session_id(&req.app, &req.stream);
        let handle = self
            .inner
            .registry()
            .handle(&session_id)
            .ok_or_else(|| ZlmVodError::SessionNotFound(session_id.clone()))?;
        handle
            .send_control(VodControlCommand::Scale(req.speed))
            .map_err(|e| ZlmVodError::InvalidRequest(e.to_string()))?;
        Ok(serde_json::json!({ "code": 0, "result": true }))
    }
}

fn vod_session_id(app: &str, stream: &str) -> String {
    format!("zlm-{app}-{stream}")
}

impl From<SessionError> for ZlmVodError {
    fn from(value: SessionError) -> Self {
        ZlmVodError::SessionNotFound(value.to_string())
    }
}

/// Normalize ZLM-style RTMP `mp4:` stream IDs into plain file paths.
///
/// VLC, ffplay, and mpv clients sometimes emit `mp4:0` or `mp4:0.mp4` when
/// playing `rtmp://host/record/0.mp4`. ZLM strips the leading `mp4:` prefix
/// and adds `.mp4` if missing. This mirrors that behaviour.
pub fn normalize_rtmp_mp4_uri(input: &str) -> String {
    let trimmed = input.trim_start_matches("mp4:").trim_start_matches("flv:");
    if trimmed.ends_with(".mp4") || trimmed.ends_with(".flv") || trimmed.ends_with(".m4a") {
        trimmed.to_string()
    } else {
        format!("{trimmed}.mp4")
    }
}

/// Expand a VOD URI list:
///
/// * Semicolon-separated paths: split and return verbatim.
/// * Single file ending in `.mp4`: return as-is.
/// * Directory path: callers are expected to expand at the driver layer
///   when reading the file system. This helper only handles the textual
///   parts so unit tests can stay deterministic.
pub fn expand_uri_list(uri: &str) -> Vec<String> {
    if uri.contains(';') {
        return uri
            .split(';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
    }
    vec![uri.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtmp_mp4_prefix_is_stripped() {
        assert_eq!(normalize_rtmp_mp4_uri("mp4:0"), "0.mp4");
        assert_eq!(normalize_rtmp_mp4_uri("mp4:0.mp4"), "0.mp4");
        assert_eq!(normalize_rtmp_mp4_uri("file/0.mp4"), "file/0.mp4");
        assert_eq!(normalize_rtmp_mp4_uri("0"), "0.mp4");
    }

    #[test]
    fn semicolon_uri_expands_to_list() {
        let list = expand_uri_list("a.mp4;b.mp4;c.mp4");
        assert_eq!(list, vec!["a.mp4", "b.mp4", "c.mp4"]);
    }

    #[test]
    fn single_uri_returns_one_entry() {
        assert_eq!(expand_uri_list("file/0.mp4"), vec!["file/0.mp4"]);
    }

    #[test]
    fn empty_segments_dropped_in_uri_list() {
        let list = expand_uri_list("a.mp4;;b.mp4;");
        assert_eq!(list, vec!["a.mp4", "b.mp4"]);
    }
}
