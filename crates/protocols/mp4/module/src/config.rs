//! MP4 VOD module configuration.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mp4ModuleConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_root_path")]
    pub root_path: String,
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
    #[serde(default = "default_read_chunk_bytes")]
    pub read_chunk_bytes: usize,
    #[serde(default = "default_max_box_bytes")]
    pub max_box_bytes: u64,
    #[serde(default = "default_idle_timeout_ms")]
    pub idle_timeout_ms: u64,
}

impl Default for Mp4ModuleConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            root_path: default_root_path(),
            max_sessions: default_max_sessions(),
            read_chunk_bytes: default_read_chunk_bytes(),
            max_box_bytes: default_max_box_bytes(),
            idle_timeout_ms: default_idle_timeout_ms(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn default_root_path() -> String {
    "./record/mp4".to_string()
}

fn default_max_sessions() -> usize {
    256
}

fn default_read_chunk_bytes() -> usize {
    256 * 1024
}

fn default_max_box_bytes() -> u64 {
    8 * 1024 * 1024
}

fn default_idle_timeout_ms() -> u64 {
    15_000
}

impl Mp4ModuleConfig {
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
        if self.max_sessions == 0 {
            return Err("max_sessions must be > 0".into());
        }
        if self.read_chunk_bytes == 0 {
            return Err("read_chunk_bytes must be > 0".into());
        }
        if self.max_box_bytes == 0 {
            return Err("max_box_bytes must be > 0".into());
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
    fn default_config_validates() {
        Mp4ModuleConfig::default().validate().unwrap();
    }
}
