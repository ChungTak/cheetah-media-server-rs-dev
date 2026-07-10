//! Record file & task metadata records.

use cheetah_codec::RecordFormat;
use serde::{Deserialize, Serialize};

/// Persisted metadata for a record task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordTaskMetadata {
    pub task_id: String,
    pub format: RecordFormatStr,
    pub app: String,
    pub stream: String,
    pub source_stream_key: String,
    pub state: RecordTaskState,
    pub create_time_ms: i64,
    pub duration_limit_ms: u64,
    pub segment_duration_ms: u64,
    pub segment_count_limit: u32,
}

/// State used by `Record Task`.
/// `Record Task` 使用的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordTaskState {
    Pending,
    Running,
    Stopped,
    Failed,
}

/// String alias for `RecordFormat` used in serialized metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordFormatStr {
    Flv,
    Hls,
    Mp4,
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
    pub file_id: String,
    pub task_id: String,
    pub format: RecordFormatStr,
    pub path: String,
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    #[serde(default)]
    pub track_summary: Vec<RecordTrackSummary>,
}

/// `RecordTrackSummary` data structure.
/// `RecordTrackSummary` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordTrackSummary {
    pub kind: String,
    pub codec: String,
}

/// Query criteria for listing record files (subset of SMS's `file/query` body).
#[derive(Debug, Clone, Default)]
pub struct RecordFileQuery {
    pub app: Option<String>,
    pub stream: Option<String>,
    pub format: Option<RecordFormatStr>,
    pub start_time_ms: Option<i64>,
    pub end_time_ms: Option<i64>,
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
