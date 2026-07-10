//! Record module configuration.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `RecordModuleConfig` data structure.
/// `RecordModuleConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordModuleConfig {
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// `root_path` field of type `String`.
    /// `root_path` 字段，类型为 `String`.
    #[serde(default = "default_root_path")]
    pub root_path: String,
    /// `max_tasks` field of type `usize`.
    /// `max_tasks` 字段，类型为 `usize`.
    #[serde(default = "default_max_tasks")]
    pub max_tasks: usize,
    /// `queue_capacity` field of type `usize`.
    /// `queue_capacity` 字段，类型为 `usize`.
    #[serde(default = "default_queue_capacity")]
    pub queue_capacity: usize,
    /// `metadata_flush_interval_ms` field of type `u32`.
    /// `metadata_flush_interval_ms` 字段，类型为 `u32`.
    #[serde(default = "default_metadata_flush_interval_ms")]
    pub metadata_flush_interval_ms: u32,
    /// `cleanup_on_start` field of type `bool`.
    /// `cleanup_on_start` 字段，类型为 `bool`.
    #[serde(default)]
    pub cleanup_on_start: bool,
    /// `formats` field of type `RecordFormatsConfig`.
    /// `formats` 字段，类型为 `RecordFormatsConfig`.
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

/// `RecordFormatsConfig` data structure.
/// `RecordFormatsConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RecordFormatsConfig {
    /// `hls` field of type `HlsRecordConfig`.
    /// `hls` 字段，类型为 `HlsRecordConfig`.
    #[serde(default)]
    pub hls: HlsRecordConfig,
    /// `mp4` field of type `Mp4RecordConfig`.
    /// `mp4` 字段，类型为 `Mp4RecordConfig`.
    #[serde(default)]
    pub mp4: Mp4RecordConfig,
    /// `flv` field of type `FlvRecordConfig`.
    /// `flv` 字段，类型为 `FlvRecordConfig`.
    #[serde(default)]
    pub flv: FlvRecordConfig,
    /// `ps` field of type `PsRecordConfig`.
    /// `ps` 字段，类型为 `PsRecordConfig`.
    #[serde(default)]
    pub ps: PsRecordConfig,
}

/// `HlsRecordConfig` data structure.
/// `HlsRecordConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HlsRecordConfig {
    /// `default_container` field of type `String`.
    /// `default_container` 字段，类型为 `String`.
    #[serde(default = "default_hls_container")]
    pub default_container: String,
    /// `segment_duration_ms` field of type `u32`.
    /// `segment_duration_ms` 字段，类型为 `u32`.
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

/// `Mp4RecordConfig` data structure.
/// `Mp4RecordConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Mp4RecordConfig {
    /// Whether to rewrite the file with `moov` at the front after closing.
    /// Currently the underlying writer ships the standard mdat-first layout
    /// only, so this defaults to false; once `Mp4Writer` learns faststart,
    /// flipping this on will be the wire-up point.
    #[serde(default)]
    pub faststart_on_close: bool,
}

/// `FlvRecordConfig` data structure.
/// `FlvRecordConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlvRecordConfig {
    /// `compat_mode` field of type `String`.
    /// `compat_mode` 字段，类型为 `String`.
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

/// `PsRecordConfig` data structure.
/// `PsRecordConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PsRecordConfig {
    /// `max_tracks` field of type `u8`.
    /// `max_tracks` 字段，类型为 `u8`.
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
    /// Creates `value` from input.
    /// 创建 `值` 来自 输入.
    pub fn from_value(value: Value) -> Result<Self, serde_json::Error> {
        if value.is_null() {
            return Ok(Self::default());
        }
        serde_json::from_value(value)
    }

    /// `default_json` function.
    /// `default_json` 函数.
    pub fn default_json() -> Value {
        serde_json::to_value(Self::default()).expect("default config serializes")
    }

    /// `validate` function.
    /// `validate` 函数.
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
