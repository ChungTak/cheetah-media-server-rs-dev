use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Configuration for the snapshot module.
///
/// 快照模块配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotModuleConfig {
    #[serde(default = "default_root_path")]
    pub root_path: String,
    #[serde(default = "default_max_snapshots")]
    pub max_snapshots: usize,
    #[serde(default = "default_default_timeout_ms")]
    pub default_timeout_ms: u64,
}

impl Default for SnapshotModuleConfig {
    fn default() -> Self {
        Self {
            root_path: default_root_path(),
            max_snapshots: default_max_snapshots(),
            default_timeout_ms: default_default_timeout_ms(),
        }
    }
}

impl SnapshotModuleConfig {
    pub fn from_value(value: Value) -> Result<Self, serde_json::Error> {
        if value.is_null() {
            return Ok(Self::default());
        }
        serde_json::from_value(value)
    }

    pub fn default_json() -> Value {
        serde_json::to_value(Self::default()).expect("default config serializes")
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.root_path.trim().is_empty() {
            return Err("root_path must not be empty".into());
        }
        if self.max_snapshots == 0 {
            return Err("max_snapshots must be > 0".into());
        }
        if self.default_timeout_ms == 0 {
            return Err("default_timeout_ms must be > 0".into());
        }
        Ok(())
    }
}

fn default_root_path() -> String {
    std::env::temp_dir()
        .join("cheetah-snapshots")
        .to_string_lossy()
        .into_owned()
}

fn default_max_snapshots() -> usize {
    10_000
}

fn default_default_timeout_ms() -> u64 {
    10_000
}
