//! Snapshot module configuration.
//!
//! 截图模块配置。

use serde::{Deserialize, Serialize};

/// Configuration for the snapshot module.
///
/// 截图模块配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotModuleConfig {
    /// Root directory where snapshot files are stored.
    #[serde(default = "default_root_path")]
    pub root_path: String,

    /// Default image format extension (e.g. "jpg", "png").
    #[serde(default = "default_format")]
    pub default_format: String,

    /// Default timeout for receiving a keyframe, in milliseconds.
    #[serde(default = "default_timeout_ms")]
    pub default_timeout_ms: u64,

    /// Maximum number of snapshots kept per media key. `0` means unlimited.
    #[serde(default = "default_max_snapshots_per_key")]
    pub max_snapshots_per_key: u32,

    /// Global maximum number of snapshot metadata entries kept in memory.
    #[serde(default = "default_max_total_snapshots")]
    pub max_total_snapshots: u32,
}

impl Default for SnapshotModuleConfig {
    fn default() -> Self {
        Self {
            root_path: default_root_path(),
            default_format: default_format(),
            default_timeout_ms: default_timeout_ms(),
            max_snapshots_per_key: default_max_snapshots_per_key(),
            max_total_snapshots: default_max_total_snapshots(),
        }
    }
}

impl SnapshotModuleConfig {
    /// Validate the configuration and report the first error found.
    pub fn validate(&self) -> Result<(), String> {
        if self.root_path.is_empty() {
            return Err("snapshot root_path must not be empty".to_string());
        }
        if self.default_format.is_empty() {
            return Err("snapshot default_format must not be empty".to_string());
        }
        if self.default_timeout_ms == 0 {
            return Err("snapshot default_timeout_ms must be greater than 0".to_string());
        }
        if self.max_total_snapshots == 0 {
            return Err("snapshot max_total_snapshots must be greater than 0".to_string());
        }
        Ok(())
    }

    /// Return the configuration as a JSON value.
    pub fn to_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }

    /// Parse a JSON value into a configuration.
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// Return the default configuration as a JSON value.
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).expect("default config serializes")
    }
}

fn default_root_path() -> String {
    "./snap".to_string()
}

fn default_format() -> String {
    "jpg".to_string()
}

fn default_timeout_ms() -> u64 {
    10_000
}

fn default_max_snapshots_per_key() -> u32 {
    100
}

fn default_max_total_snapshots() -> u32 {
    10_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_passes_validation() {
        let cfg = SnapshotModuleConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn empty_root_path_is_rejected() {
        let cfg = SnapshotModuleConfig {
            root_path: String::new(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }
}
