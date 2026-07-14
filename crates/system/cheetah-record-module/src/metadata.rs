//! Record file & task metadata records.
//!
//! Holds the serializable metadata structures stored in the registry and used
//! by the HTTP API to report task/file inventories. `RecordFormatStr` is the
//! metadata-side alias for `cheetah_codec::RecordFormat`.
//!
//! 录制文件与任务元数据记录。
//!
//! 保存注册表中可序列化的元数据结构，用于 HTTP API 报告任务/文件清单。
//! `RecordFormatStr` 是 `cheetah_codec::RecordFormat` 在元数据侧的别名。

use cheetah_codec::RecordFormat;
use serde::{Deserialize, Serialize};

/// Persisted metadata for a record task.
///
/// Captures the task id, source stream, requested format, and lifecycle state.
///
/// 录制任务的持久化元数据。
///
/// 记录任务 ID、源流、请求格式以及生命周期状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordTaskMetadata {
    pub task_id: String,
    pub format: RecordFormatStr,
    pub vhost: String,
    pub app: String,
    pub stream: String,
    pub source_stream_key: String,
    pub state: RecordTaskState,
    pub create_time_ms: i64,
    pub duration_limit_ms: u64,
    pub segment_duration_ms: u64,
    pub segment_count_limit: u32,
}

/// Lifecycle state of a record task.
///
/// Mirrors the standard state machine: Pending -> Running -> Stopped/Failed.
///
/// 录制任务的生命周期状态。
///
/// 映射标准状态机：Pending -> Running -> Stopped/Failed。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordTaskState {
    Pending,
    Running,
    Stopped,
    Failed,
}

/// String alias for `RecordFormat` used in serialized metadata.
///
/// Keeps the wire shape lowercase (`mp4`, `hls`, `flv`, `ps`) while the
/// runtime uses `cheetah_codec::RecordFormat` internally.
///
/// 序列化元数据中使用的 `RecordFormat` 字符串别名。
///
/// 对外保持小写（`mp4`、`hls`、`flv`、`ps`），运行时内部仍使用
/// `cheetah_codec::RecordFormat`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordFormatStr {
    Flv,
    Hls,
    Mp4,
    Ps,
}

impl From<RecordFormat> for RecordFormatStr {
    /// Convert the codec enum into the metadata string enum.
    ///
    /// 将 codec 枚举转换为元数据字符串枚举。
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
    /// Convert the metadata string enum back into the codec enum.
    ///
    /// 将元数据字符串枚举转换回 codec 枚举。
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
///
/// Records the on-disk path, time range, size, and a summary of tracks.
/// The `file_handle` field is the public `FileHandle` token returned to
/// clients; `path` is kept internal for directory filtering and cleanup.
///
/// 录制文件的持久化元数据。
/// `file_handle` 是返回给客户端的公共 `FileHandle` 令牌；`path` 仅内部用于目录过滤与清理。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordFileMetadata {
    pub file_id: String,
    pub task_id: String,
    pub format: RecordFormatStr,
    pub vhost: String,
    pub app: String,
    pub stream: String,
    pub path: String,
    #[serde(default)]
    pub file_handle: Option<String>,
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    #[serde(default)]
    pub track_summary: Vec<RecordTrackSummary>,
}

/// Track summary recorded alongside a file.
///
/// Provides a lightweight summary for clients without full `TrackInfo`.
///
/// 与文件一并记录的轨道摘要。
///
/// 为客户端提供轻量摘要，无需完整 `TrackInfo`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordTrackSummary {
    pub kind: String,
    pub codec: String,
}

/// Query criteria for listing record files (subset of SMS's `file/query` body).
///
/// All fields are optional; missing ones are treated as wildcards.
///
/// 列出录制文件的查询条件（SMS `file/query` 请求体的子集）。
///
/// 所有字段均为可选；缺失字段视为通配。
#[derive(Debug, Clone, Default)]
pub struct RecordFileQuery {
    pub vhost: Option<String>,
    pub app: Option<String>,
    pub stream: Option<String>,
    pub format: Option<RecordFormatStr>,
    pub start_time_ms: Option<i64>,
    pub end_time_ms: Option<i64>,
    pub file_id: Option<String>,
    pub directory: Option<String>,
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
