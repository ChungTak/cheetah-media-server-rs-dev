//! Record module configuration.
//!
//! Defines the JSON configuration consumed by `RecordModule` as well as the
//! per-format defaults (HLS, MP4, FLV, PS). Validation keeps the module from
//! starting with nonsensical limits.
//!
//! 录制模块配置。
//!
//! 定义 `RecordModule` 消费的 JSON 配置以及各格式默认值（HLS、MP4、FLV、PS）。
//! 校验逻辑防止模块在启动时使用不合理的限制。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Top-level configuration for the record module.
///
/// Parsed from the `record` namespace in the engine config. All fields carry
/// serde defaults so the module can start with an empty config block.
///
/// 录制模块的顶层配置。
///
/// 从引擎配置的 `record` 命名空间解析。所有字段均带有 serde 默认值，
/// 因此模块可在空配置块下启动。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordModuleConfig {
    /// Whether the module is active at runtime.
    ///
    /// 模块是否在运行时处于激活状态。
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Root directory for all record output.
    ///
    /// 所有录制输出的根目录。
    #[serde(default = "default_root_path")]
    pub root_path: String,
    /// Maximum number of concurrent record tasks.
    ///
    /// 最大并发录制任务数。
    #[serde(default = "default_max_tasks")]
    pub max_tasks: usize,
    /// Per-subscriber queue capacity used for the bootstrap GOP window.
    ///
    /// 用于引导 GOP 窗口的每个订阅者队列容量。
    #[serde(default = "default_queue_capacity")]
    pub queue_capacity: usize,
    /// Interval in milliseconds between on-disk metadata flushes.
    ///
    /// 落盘元数据刷新间隔，单位为毫秒。
    #[serde(default = "default_metadata_flush_interval_ms")]
    pub metadata_flush_interval_ms: u32,
    /// Whether to remove stale files under `root_path` on module start.
    ///
    /// 是否在模块启动时清理 `root_path` 下的陈旧文件。
    #[serde(default)]
    pub cleanup_on_start: bool,
    /// Per-format overrides.
    ///
    /// 各格式覆盖配置。
    #[serde(default)]
    pub formats: RecordFormatsConfig,
}

fn default_enabled() -> bool {
    true
}

fn default_root_path() -> String {
    "./record".to_string()
}

fn default_max_tasks() -> usize {
    256
}

fn default_queue_capacity() -> usize {
    1024
}

fn default_metadata_flush_interval_ms() -> u32 {
    1000
}

/// Format-specific sub-configurations.
///
/// Each field is a self-contained policy object for one container.
///
/// 各格式相关的子配置。
///
/// 每个字段是一个容器对应的独立策略对象。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RecordFormatsConfig {
    /// HLS recording policy.
    ///
    /// HLS 录制策略。
    #[serde(default)]
    pub hls: HlsRecordConfig,
    /// MP4 recording policy.
    ///
    /// MP4 录制策略。
    #[serde(default)]
    pub mp4: Mp4RecordConfig,
    /// FLV recording policy.
    ///
    /// FLV 录制策略。
    #[serde(default)]
    pub flv: FlvRecordConfig,
    /// MPEG-PS recording policy.
    ///
    /// MPEG-PS 录制策略。
    #[serde(default)]
    pub ps: PsRecordConfig,
}

/// HLS recording configuration.
///
/// Controls the default sub-container (`fmp4` vs `ts`) and target segment
/// duration. These values are passed into the HLS writer once it is enabled.
///
/// HLS 录制配置。
///
/// 控制默认子容器（`fmp4` 或 `ts`）以及目标分片时长。
/// 这些值将在 HLS 写入器启用后传入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HlsRecordConfig {
    /// Default sub-container used for HLS segments.
    ///
    /// HLS 分片使用的默认子容器。
    #[serde(default = "default_hls_container")]
    pub default_container: String,
    /// Target segment duration in milliseconds.
    ///
    /// 目标分片时长，单位为毫秒。
    #[serde(default = "default_hls_segment_duration_ms")]
    pub segment_duration_ms: u32,
}

fn default_hls_container() -> String {
    "fmp4".to_string()
}

fn default_hls_segment_duration_ms() -> u32 {
    5_000
}

impl Default for HlsRecordConfig {
    fn default() -> Self {
        Self {
            default_container: default_hls_container(),
            segment_duration_ms: default_hls_segment_duration_ms(),
        }
    }
}

/// MP4 recording configuration.
///
/// MP4 录制配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Mp4RecordConfig {
    /// Whether to rewrite the file with `moov` at the front after closing.
    /// Currently the underlying writer ships the standard mdat-first layout
    /// only, so this defaults to false; once `Mp4Writer` learns faststart,
    /// flipping this on will be the wire-up point.
    ///
    /// 是否在关闭后将文件重写为 `moov` 前置的标准 faststart 布局。
    /// 当前底层写入器仅输出 mdat 在前的标准布局，因此默认 false；
    /// 一旦 `Mp4Writer` 支持 faststart，打开此项即可生效。
    #[serde(default)]
    pub faststart_on_close: bool,
}

/// FLV recording configuration.
///
/// The `compat_mode` selects the FLV writer behavior for legacy clients.
///
/// FLV 录制配置。
///
/// `compat_mode` 用于选择针对 legacy 客户端的 FLV 写入器行为。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlvRecordConfig {
    /// Compatibility mode for FLV output (`auto`, `legacy`, `strict`, etc.).
    ///
    /// FLV 输出的兼容模式（`auto`、`legacy`、`strict` 等）。
    #[serde(default = "default_compat_mode")]
    pub compat_mode: String,
}

fn default_compat_mode() -> String {
    "auto".to_string()
}

impl Default for FlvRecordConfig {
    fn default() -> Self {
        Self {
            compat_mode: default_compat_mode(),
        }
    }
}

/// MPEG-PS recording configuration.
///
/// MPEG-PS 录制配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PsRecordConfig {
    /// Maximum number of tracks multiplexed into a PS file.
    ///
    /// 单个 PS 文件中复用的最大轨道数。
    #[serde(default = "default_max_ps_tracks")]
    pub max_tracks: u8,
}

fn default_max_ps_tracks() -> u8 {
    16
}

impl Default for PsRecordConfig {
    fn default() -> Self {
        Self {
            max_tracks: default_max_ps_tracks(),
        }
    }
}

impl Default for RecordModuleConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            root_path: default_root_path(),
            max_tasks: default_max_tasks(),
            queue_capacity: default_queue_capacity(),
            metadata_flush_interval_ms: default_metadata_flush_interval_ms(),
            cleanup_on_start: false,
            formats: RecordFormatsConfig::default(),
        }
    }
}

impl RecordModuleConfig {
    /// Build a config from a JSON value, defaulting to an empty config.
    ///
    /// 从 JSON 值构造配置；传入 null 时使用默认配置。
    pub fn from_value(value: Value) -> Result<Self, serde_json::Error> {
        if value.is_null() {
            return Ok(Self::default());
        }
        serde_json::from_value(value)
    }

    /// Return the default configuration as a JSON value.
    ///
    /// 以 JSON 值形式返回默认配置。
    pub fn default_json() -> Value {
        serde_json::to_value(Self::default()).unwrap_or_default()
    }

    /// Validate the loaded configuration, returning an error message on mismatch.
    ///
    /// 校验加载后的配置，若不合理则返回错误信息。
    pub fn validate(&self) -> Result<(), String> {
        if self.max_tasks == 0 {
            return Err("max_tasks must be > 0".into());
        }
        if self.queue_capacity == 0 {
            return Err("queue_capacity must be > 0".into());
        }
        if self.formats.hls.segment_duration_ms == 0 {
            return Err("formats.hls.segment_duration_ms must be > 0".into());
        }
        if self.formats.ps.max_tracks == 0 {
            return Err("formats.ps.max_tracks must be > 0".into());
        }
        if self.root_path.is_empty() {
            return Err("root_path must not be empty".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_passes_validation() {
        RecordModuleConfig::default().validate().unwrap();
    }

    #[test]
    fn empty_root_path_rejected() {
        let mut cfg = RecordModuleConfig::default();
        cfg.root_path.clear();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn from_value_supports_partial_updates() {
        let json = serde_json::json!({"enabled": false});
        let cfg = RecordModuleConfig::from_value(json).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.root_path, "./record");
    }
}
