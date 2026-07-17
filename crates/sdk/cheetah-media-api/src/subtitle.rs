//! WebVTT subtitle types for processing and caption extraction.
//!
//! WebVTT 字幕类型，用于处理与字幕提取。

use serde::{Deserialize, Serialize};

/// A single WebVTT cue.
///
/// 单个 WebVTT cue。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebVttCue {
    pub id: Option<String>,
    pub start_ms: u64,
    pub end_ms: u64,
    pub payload: String,
    pub settings: Option<String>,
}

/// A collection of WebVTT cues carrying a subtitle track segment.
///
/// WebVTT cue 集合，表示一个字幕轨道片段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebVttFrame {
    pub cues: Vec<WebVttCue>,
    pub styles: Vec<String>,
    pub regions: Vec<String>,
}

impl WebVttFrame {
    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cues.len()
    }
}
