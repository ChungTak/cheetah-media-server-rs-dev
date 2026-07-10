//! Record file & task metadata records.

use cheetah_codec::RecordFormat;
use serde::{Deserialize, Serialize};

/// Persisted metadata for a record task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordTaskMetadata {
    /// `task_id` field of type `String`.
    /// `task_id` 字段，类型为 `String`.
    pub task_id: String,
    /// `format` field of type `RecordFormatStr`.
    /// `format` 字段，类型为 `RecordFormatStr`.
    pub format: RecordFormatStr,
    /// `app` field of type `String`.
    /// `app` 字段，类型为 `String`.
    pub app: String,
    /// `stream` field of type `String`.
    /// `stream` 字段，类型为 `String`.
    pub stream: String,
    /// `source_stream_key` field of type `String`.
    /// `source_stream_key` 字段，类型为 `String`.
    pub source_stream_key: String,
    /// `state` field of type `RecordTaskState`.
    /// `state` 字段，类型为 `RecordTaskState`.
    pub state: RecordTaskState,
    /// `create_time_ms` field of type `i64`.
    /// `create_time_ms` 字段，类型为 `i64`.
    pub create_time_ms: i64,
    /// `duration_limit_ms` field of type `u64`.
    /// `duration_limit_ms` 字段，类型为 `u64`.
    pub duration_limit_ms: u64,
    /// `segment_duration_ms` field of type `u64`.
    /// `segment_duration_ms` 字段，类型为 `u64`.
    pub segment_duration_ms: u64,
    /// `segment_count_limit` field of type `u32`.
    /// `segment_count_limit` 字段，类型为 `u32`.
    pub segment_count_limit: u32,
}

/// `RecordTaskState` enumeration.
/// `RecordTaskState` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordTaskState {
    /// `Pending` variant.
    /// `Pending` 变体.
    Pending,
    /// `Running` variant.
    /// `Running` 变体.
    Running,
    /// `Stopped` variant.
    /// `Stopped` 变体.
    Stopped,
    /// `Failed` variant.
    /// `Failed` 变体.
    Failed,
}

/// String alias for `RecordFormat` used in serialized metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordFormatStr {
    /// `Flv` variant.
    /// `Flv` 变体.
    Flv,
    /// `Hls` variant.
    /// `Hls` 变体.
    Hls,
    /// `Mp4` variant.
    /// `Mp4` 变体.
    Mp4,
    /// `Ps` variant.
    /// `Ps` 变体.
    Ps,
}

impl From<RecordFormat> for RecordFormatStr {
    fn from(value: RecordFormat) -> Self {
        match value {
            RecordFormat::Flv => RecordFormatStr::Flv,
            RecordFormat::Hls => RecordFormatStr::Hls,
            RecordFormat::Mp4 => RecordFormatStr::Mp4,
            RecordFormat::Ps => RecordFormatStr::Ps,
        }
    }
}

impl From<RecordFormatStr> for RecordFormat {
    fn from(value: RecordFormatStr) -> Self {
        match value {
            RecordFormatStr::Flv => RecordFormat::Flv,
            RecordFormatStr::Hls => RecordFormat::Hls,
            RecordFormatStr::Mp4 => RecordFormat::Mp4,
            RecordFormatStr::Ps => RecordFormat::Ps,
        }
    }
}

/// Persisted metadata for a record file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordFileMetadata {
    /// `file_id` field of type `String`.
    /// `file_id` 字段，类型为 `String`.
    pub file_id: String,
    /// `task_id` field of type `String`.
    /// `task_id` 字段，类型为 `String`.
    pub task_id: String,
    /// `format` field of type `RecordFormatStr`.
    /// `format` 字段，类型为 `RecordFormatStr`.
    pub format: RecordFormatStr,
    /// `path` field of type `String`.
    /// `path` 字段，类型为 `String`.
    pub path: String,
    /// `duration_ms` field of type `u64`.
    /// `duration_ms` 字段，类型为 `u64`.
    pub duration_ms: u64,
    /// `size_bytes` field of type `u64`.
    /// `size_bytes` 字段，类型为 `u64`.
    pub size_bytes: u64,
    /// `start_time_ms` field of type `i64`.
    /// `start_time_ms` 字段，类型为 `i64`.
    pub start_time_ms: i64,
    /// `end_time_ms` field of type `i64`.
    /// `end_time_ms` 字段，类型为 `i64`.
    pub end_time_ms: i64,
    /// `track_summary` field.
    /// `track_summary` 字段.
    #[serde(default)]
    pub track_summary: Vec<RecordTrackSummary>,
}

/// `RecordTrackSummary` data structure.
/// `RecordTrackSummary` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordTrackSummary {
    /// `kind` field of type `String`.
    /// `kind` 字段，类型为 `String`.
    pub kind: String,
    /// `codec` field of type `String`.
    /// `codec` 字段，类型为 `String`.
    pub codec: String,
}

/// Query criteria for listing record files (subset of SMS's `file/query` body).
#[derive(Debug, Clone, Default)]
pub struct RecordFileQuery {
    /// `app` field.
    /// `app` 字段.
    pub app: Option<String>,
    /// `stream` field.
    /// `stream` 字段.
    pub stream: Option<String>,
    /// `format` field.
    /// `format` 字段.
    pub format: Option<RecordFormatStr>,
    /// `start_time_ms` field.
    /// `start_time_ms` 字段.
    pub start_time_ms: Option<i64>,
    /// `end_time_ms` field.
    /// `end_time_ms` 字段.
    pub end_time_ms: Option<i64>,
    /// `limit` field.
    /// `limit` 字段.
    pub limit: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_format_str_roundtrips() {
        for fmt in [
            RecordFormat::Flv,
            RecordFormat::Hls,
            RecordFormat::Mp4,
            RecordFormat::Ps,
        ] {
            let s: RecordFormatStr = fmt.into();
            let back: RecordFormat = s.into();
            assert_eq!(back, fmt);
        }
    }
}
