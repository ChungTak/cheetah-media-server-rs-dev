//! WebVTT subtitle types and CEA closed-caption extraction helpers.
//!
//! 字幕/WebVTT 类型与 CEA 闭字幕提取辅助。

use crate::prelude::*;
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
