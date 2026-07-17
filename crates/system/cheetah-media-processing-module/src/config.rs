//! Module configuration for media processing.

use serde::{Deserialize, Serialize};

/// Configuration for `cheetah-media-processing-module`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaProcessingModuleConfig {
    /// Maximum number of concurrent processing jobs.
    pub max_concurrent_jobs: u32,
    /// Maximum input/output image width in pixels.
    pub max_image_width: u32,
    /// Maximum input/output image height in pixels.
    pub max_image_height: u32,
    /// Default JPEG output quality (1–100).
    pub default_jpeg_quality: u8,
    /// Selected avcodec profile name (e.g. `native-free`, `software`).
    pub profile: String,
}

impl Default for MediaProcessingModuleConfig {
    fn default() -> Self {
        Self {
            max_concurrent_jobs: 64,
            max_image_width: 8_192,
            max_image_height: 4_320,
            default_jpeg_quality: 85,
            profile: "native-free".to_string(),
        }
    }
}

impl MediaProcessingModuleConfig {
    /// Validates the configuration and normalizes bounds.
    pub fn validate(&self) -> Result<(), String> {
        if self.max_concurrent_jobs == 0 {
            return Err("max_concurrent_jobs must be > 0".to_string());
        }
        if self.max_image_width == 0 || self.max_image_height == 0 {
            return Err("max_image_width/height must be > 0".to_string());
        }
        if self.default_jpeg_quality == 0 || self.default_jpeg_quality > 100 {
            return Err("default_jpeg_quality must be in 1..=100".to_string());
        }
        Ok(())
    }

    /// Returns the default configuration as a JSON value.
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).expect("default config serializes")
    }

    /// Parses the configuration from a JSON value.
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}
