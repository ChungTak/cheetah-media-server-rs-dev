//! Media processing job API and request/result types.
//!
//! 媒体处理任务 API 与请求/结果类型。

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use cheetah_codec::{AVFrame, TrackInfo};
use serde::{Deserialize, Serialize};

use crate::ids::{FileHandle, MediaKey};
use crate::image::ImageFormat;

pub use crate::ids::ProcessingJobId;

/// Processing policy for a stream or proxy.
///
/// 流或代理的处理策略。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ProcessingPolicy {
    #[default]
    Passthrough,
    Auto {
        preset: ProcessingPreset,
    },
    Transcode {
        target: ProcessingTarget,
    },
}

/// Preset for automatic processing decisions.
///
/// 自动处理决策的预设。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingPreset {
    #[default]
    Conservative,
    Balanced,
    Quality,
    LowLatency,
}

/// Explicit processing target.
///
/// 显式处理目标。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProcessingTarget {
    pub video: Option<VideoTarget>,
    pub audio: Option<AudioTarget>,
}

/// Video processing target parameters.
///
/// 视频处理目标参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoTarget {
    pub codec: VideoCodec,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate_num: Option<u32>,
    pub frame_rate_den: Option<u32>,
    pub bit_rate: Option<u64>,
    pub gop_size: Option<u32>,
    pub profile: Option<String>,
}

/// Audio processing target parameters.
///
/// 音频处理目标参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTarget {
    pub codec: AudioCodec,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub bit_rate: Option<u64>,
}

/// Video codec selection for processing jobs.
///
/// 处理任务的视频编解码器选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    H264,
    H265,
    MJPEG,
}

/// Audio codec selection for processing jobs.
///
/// 处理任务的音频编解码器选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    G711A,
    G711U,
    Aac,
    Opus,
    Mp3,
    Pcm,
}

/// Track selection for a processing job.
///
/// 处理任务的轨道选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackSelection {
    #[default]
    All,
    AudioOnly,
    VideoOnly,
}

/// Processing job specification.
///
/// 处理任务规格。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ProcessingJobSpec {
    Transcode {
        source: MediaKey,
        target: MediaKey,
        track_selection: TrackSelection,
        audio: Option<AudioTarget>,
        video: Option<VideoTarget>,
        overlays: Vec<Overlay>,
    },
    AbrLadder {
        source: MediaKey,
        variants: Vec<AbrVariant>,
    },
    AudioMix {
        inputs: Vec<AudioMixInput>,
        target: MediaKey,
        output: AudioTarget,
    },
    VideoMosaic {
        inputs: Vec<VideoMosaicInput>,
        target: MediaKey,
        layout: MosaicLayout,
        audio_mix: Option<AudioMix>,
        overlays: Vec<Overlay>,
    },
    CaptionExtract {
        source: MediaKey,
        target: MediaKey,
        caption: CaptionConfig,
    },
}

/// A single ABR variant.
///
/// 单个 ABR 档位。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbrVariant {
    pub target: MediaKey,
    pub video: VideoTarget,
    pub audio: Option<AudioTarget>,
}

/// Audio mix input reference.
///
/// 音频混音输入引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioMixInput {
    pub source: MediaKey,
    /// Gain in decibels.
    ///
    /// 增益，单位为分贝。
    pub gain_db: Option<i32>,
}

/// Video mosaic input reference.
///
/// 视频宫格输入引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoMosaicInput {
    pub source: MediaKey,
    pub cell: MosaicCell,
    /// Gain in decibels.
    ///
    /// 增益，单位为分贝。
    pub audio_gain_db: Option<i32>,
    /// Tile fit policy. Overrides [`MosaicLayout::fit`] when set.
    #[serde(default)]
    pub fit: Option<MosaicFit>,
    /// Optional per-tile label.
    #[serde(default)]
    pub label: Option<String>,
}

/// Aspect-ratio fitting mode for a mosaic tile.
///
/// 宫格单元的内容填充模式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MosaicFit {
    /// Preserve aspect ratio and pad to the cell (letterbox).
    Contain,
    /// Preserve aspect ratio and crop to fill the cell.
    #[default]
    Cover,
    /// Stretch to fill the cell exactly.
    Stretch,
}

/// Mosaic layout.
///
/// 宫格布局。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MosaicLayout {
    pub columns: u32,
    pub rows: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub background: Option<String>,
    /// Output frame rate numerator.
    #[serde(default)]
    pub frame_rate_num: Option<u32>,
    /// Output frame rate denominator.
    #[serde(default)]
    pub frame_rate_den: Option<u32>,
    /// Output bitrate in bits per second.
    #[serde(default)]
    pub bit_rate: Option<u64>,
    /// Output GOP size in frames.
    #[serde(default)]
    pub gop_size: Option<u32>,
    /// Output video codec.
    #[serde(default)]
    pub video_codec: Option<VideoCodec>,
    /// Default tile fit policy.
    #[serde(default)]
    pub fit: Option<MosaicFit>,
}

/// Mosaic cell assignment.
///
/// 宫格单元分配。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MosaicCell {
    pub column: u32,
    pub row: u32,
    pub z_order: u32,
}

/// Audio mix target description.
///
/// 音频混音目标描述。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioMix {
    pub target: MediaKey,
    pub output: AudioTarget,
}

/// Image or text overlay for video processing.
///
/// 视频处理中的图片或文字水印。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overlay {
    pub kind: OverlayKind,
    pub position: OverlayPosition,
    pub size: Option<OverlaySize>,
    pub opacity: Option<u8>,
}

/// Overlay content kind.
///
/// 水印内容类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OverlayKind {
    Text {
        text: String,
        font_handle: FileHandle,
    },
    Image {
        image_handle: FileHandle,
    },
}

/// Overlay position.
///
/// 水印位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayPosition {
    pub x: i32,
    pub y: i32,
}

/// Overlay size.
///
/// 水印尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlaySize {
    pub width: u32,
    pub height: u32,
}

/// Caption extraction configuration.
///
/// 字幕提取配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptionConfig {
    pub source_streams: Vec<String>,
    pub languages: Vec<String>,
}

/// Processing job lifecycle state.
///
/// 处理任务生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingJobState {
    Pending,
    Starting,
    Running,
    Draining,
    Stopped,
    Failed,
}

/// Processing job summary.
///
/// 处理任务摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessingJob {
    pub job_id: ProcessingJobId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub spec: ProcessingJobSpec,
    pub state: ProcessingJobState,
    pub generation: u64,
    pub profile: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub input_keys: Vec<MediaKey>,
    pub output_keys: Vec<MediaKey>,
    pub ref_count: u64,
    pub restart_count: u32,
    pub frames_in: u64,
    pub frames_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub drops: u64,
    pub pending: u64,
    pub flushes: u64,
    pub resets: u64,
    pub last_error: Option<String>,
}

/// Request to create a processing job.
///
/// 创建处理任务的请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateProcessingJob {
    pub idempotency_key: Option<String>,
    pub deadline_ms: Option<u64>,
    pub spec: ProcessingJobSpec,
}

/// Request to update a processing job.
///
/// 更新处理任务的请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateProcessingJob {
    #[serde(default)]
    pub job_id: ProcessingJobId,
    pub expected_generation: u64,
    pub spec: ProcessingJobSpec,
}

/// Query for processing jobs.
///
/// 处理任务查询。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessingJobQuery {
    #[serde(default)]
    pub vhost: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub state: Option<ProcessingJobState>,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
}

impl ProcessingJobQuery {
    /// Maximum allowed page size.
    pub const MAX_PAGE_SIZE: u64 = 1_000;

    pub fn clamp_page_size(&mut self) {
        if self.page_size == 0 {
            self.page_size = default_page_size();
        }
        self.page_size = self.page_size.min(Self::MAX_PAGE_SIZE);
    }
}

fn default_page_size() -> u64 {
    20
}

/// Preflight report for a processing operation.
///
/// 处理操作预检报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessingPreflightReport {
    pub profile: String,
    pub available: bool,
    pub operations: Vec<String>,
    pub diagnostics: HashMap<String, String>,
}

/// Input to an image process operation.
///
/// 图片处理操作的输入。
#[derive(Debug, Clone)]
pub enum ImageInput {
    Encoded {
        data: Bytes,
        format: ImageFormat,
    },
    Frame {
        frame: Arc<AVFrame>,
        track: TrackInfo,
    },
}

/// Single image processing operation.
///
/// 单个图片处理算子。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ImageOperation {
    Crop {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    Resize {
        width: u32,
        height: u32,
    },
    Fit {
        width: u32,
        height: u32,
    },
    Rotate {
        degrees: i32,
    },
    Flip {
        horizontal: bool,
        vertical: bool,
    },
    Pad {
        top: u32,
        bottom: u32,
        left: u32,
        right: u32,
        color: Option<String>,
    },
    Csc {
        format: String,
    },
    Blend {
        overlay: Bytes,
        x: i32,
        y: i32,
        opacity: Option<u8>,
    },
    Text {
        text: String,
        font_handle: FileHandle,
        x: i32,
        y: i32,
        size: u32,
        color: Option<String>,
    },
    ResizePad {
        width: u32,
        height: u32,
        color: Option<String>,
    },
}

/// Request to process an image.
///
/// 图片处理请求。
#[derive(Debug, Clone)]
pub struct ImageProcessRequest {
    pub input: ImageInput,
    pub operations: Vec<ImageOperation>,
    pub output_format: ImageFormat,
    pub quality: u8,
}

impl ImageProcessRequest {
    pub fn new(input: ImageInput, output_format: ImageFormat) -> Self {
        Self {
            input,
            operations: Vec::new(),
            output_format,
            quality: 80,
        }
    }

    pub fn with_operations(mut self, operations: Vec<ImageOperation>) -> Self {
        self.operations = operations;
        self
    }

    pub fn with_quality(mut self, quality: u8) -> Self {
        self.quality = quality.clamp(1, 100);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processing_policy_round_trips() {
        let policy = ProcessingPolicy::Auto {
            preset: ProcessingPreset::Balanced,
        };
        let json = serde_json::to_string(&policy).unwrap();
        let de: ProcessingPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, de);
    }

    #[test]
    fn processing_job_spec_tagged_serialization() {
        let spec = ProcessingJobSpec::Transcode {
            source: MediaKey::new("__defaultVhost__", "live", "src", None).unwrap(),
            target: MediaKey::new("__defaultVhost__", "live", "dst", None).unwrap(),
            track_selection: TrackSelection::All,
            audio: Some(AudioTarget {
                codec: AudioCodec::Aac,
                sample_rate: Some(48_000),
                channels: Some(2),
                bit_rate: Some(128_000),
            }),
            video: Some(VideoTarget {
                codec: VideoCodec::H264,
                width: Some(1280),
                height: Some(720),
                frame_rate_num: Some(30),
                frame_rate_den: Some(1),
                bit_rate: Some(2_000_000),
                gop_size: Some(60),
                profile: None,
            }),
            overlays: Vec::new(),
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("\"kind\":\"transcode\""));
        let de: ProcessingJobSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, de);
    }

    #[test]
    fn processing_job_query_clamps_page_size() {
        let mut query = ProcessingJobQuery {
            page_size: 10_000,
            ..Default::default()
        };
        query.clamp_page_size();
        assert_eq!(query.page_size, ProcessingJobQuery::MAX_PAGE_SIZE);
    }

    #[test]
    fn image_process_request_quality_is_clamped() {
        let input = ImageInput::Encoded {
            data: Bytes::new(),
            format: ImageFormat::Jpeg,
        };
        let request = ImageProcessRequest::new(input, ImageFormat::Jpeg).with_quality(150);
        assert_eq!(request.quality, 100);
    }
}
