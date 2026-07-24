//! Module configuration for media processing.

use serde::{Deserialize, Serialize};

/// Maximum number of concurrent processing jobs allowed by the module.
pub const MAX_CONCURRENT_JOBS: u32 = 4096;

/// Configuration for `cheetah-media-processing-module`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaProcessingModuleConfig {
    /// Maximum number of concurrent processing jobs.
    pub max_concurrent_jobs: u32,
    /// Maximum input/output image width in pixels.
    pub max_image_width: u32,
    /// Maximum input/output image height in pixels.
    pub max_image_height: u32,
    /// Maximum number of input streams for a single processing job.
    pub max_processing_inputs: u32,
    /// Maximum number of overlays on a single processing job.
    pub max_processing_overlays: u32,
    /// Maximum text length for an overlay in characters.
    pub max_overlay_text_length: u32,
    /// Maximum font size for an overlay in pixels.
    pub max_overlay_font_size: u32,
    /// Maximum pixel rate for video processing targets (width * height * fps).
    pub max_video_pixel_rate: u64,
    /// Maximum encoded input frame size in bytes.
    pub max_encoded_frame_bytes: u64,
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
            max_processing_inputs: 16,
            max_processing_overlays: 8,
            max_overlay_text_length: 1_024,
            max_overlay_font_size: 128,
            max_video_pixel_rate: 100_000_000_000,
            max_encoded_frame_bytes: 16 * 1024 * 1024,
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
        if self.max_concurrent_jobs > MAX_CONCURRENT_JOBS {
            return Err(format!(
                "max_concurrent_jobs must be <= {MAX_CONCURRENT_JOBS}"
            ));
        }
        if self.max_image_width == 0 || self.max_image_height == 0 {
            return Err("max_image_width/height must be > 0".to_string());
        }
        if self.max_processing_inputs == 0
            || self.max_processing_overlays == 0
            || self.max_overlay_text_length == 0
            || self.max_overlay_font_size == 0
        {
            return Err("processing input/overlay/text/font bounds must be > 0".to_string());
        }
        if self.max_video_pixel_rate == 0 || self.max_encoded_frame_bytes == 0 {
            return Err("max_video_pixel_rate and max_encoded_frame_bytes must be > 0".to_string());
        }
        if self.default_jpeg_quality == 0 || self.default_jpeg_quality > 100 {
            return Err("default_jpeg_quality must be in 1..=100".to_string());
        }
        let software_enabled = cfg!(feature = "avcodec-profile-software");
        if !matches!(self.profile.as_str(), "native-free" | "software")
            || (self.profile == "software" && !software_enabled)
        {
            return Err(format!(
                "profile must be 'native-free' or 'software' (compiled), got '{}'",
                self.profile
            ));
        }
        Ok(())
    }

    /// Returns the default configuration as a JSON value.
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).unwrap_or_default()
    }

    /// Parses the configuration from a JSON value.
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}
