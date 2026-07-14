//! Lightweight playback-state controller used by the record module to bridge
//! VOD-style control commands.
//!
//! This is not a full demux/player; it tracks the per-file playback state
//! (`paused`, `scale`, `position_ms`) so that `RecordApi::control_record_playback`
//! can validate and reflect `Pause`, `Resume`, `Scale`, and `Seek` commands.
//!
//! 轻量级回放状态控制器，用于录制模块桥接 VOD 风格的控制命令。
//! 它并非完整解复用/播放器，仅按文件记录回放状态，以便
//! `RecordApi::control_record_playback` 能校验并反映暂停、恢复、倍速和定位。

use std::collections::HashMap;

use cheetah_media_api::command::RecordPlaybackCommand;
use cheetah_media_api::error::{MediaError, Result};
use parking_lot::Mutex;

const DEFAULT_SCALE: f64 = 1.0;
const MIN_SCALE: f64 = 0.25;
const MAX_SCALE: f64 = 16.0;

/// Current playback state for a single file.
///
/// 单个文件的当前回放状态。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PlaybackState {
    pub paused: bool,
    pub scale: f64,
    pub position_ms: i64,
    pub duration_ms: u64,
}

/// In-memory registry of playback sessions keyed by file id.
///
/// 按文件 id 索引的回放会话内存注册表。
pub struct PlaybackRegistry {
    states: Mutex<HashMap<String, PlaybackState>>,
}

impl Default for PlaybackRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackRegistry {
    pub fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }

    /// Apply a playback command to the session for `file_id`.
    ///
    /// `duration_ms` is the known media duration used to bound `Seek`.
    /// Returns the updated state.
    ///
    /// 将回放控制命令应用到 `file_id` 的会话。
    pub fn apply(
        &self,
        file_id: &str,
        duration_ms: u64,
        command: RecordPlaybackCommand,
    ) -> Result<PlaybackState> {
        let mut states = self.states.lock();
        let state = states.entry(file_id.to_string()).or_insert(PlaybackState {
            scale: DEFAULT_SCALE,
            ..Default::default()
        });
        state.duration_ms = duration_ms;

        match command {
            RecordPlaybackCommand::Pause => state.paused = true,
            RecordPlaybackCommand::Resume => state.paused = false,
            RecordPlaybackCommand::Scale { value } => {
                if !value.is_finite() || !(MIN_SCALE..=MAX_SCALE).contains(&value) {
                    return Err(MediaError::invalid_argument(format!(
                        "scale must be finite and in [{MIN_SCALE}, {MAX_SCALE}]"
                    )));
                }
                state.scale = value;
            }
            RecordPlaybackCommand::Seek { value } => {
                if value < 0 || value > duration_ms as i64 {
                    return Err(MediaError::invalid_argument(format!(
                        "seek {value} is out of range [0, {duration_ms}]"
                    )));
                }
                state.position_ms = value;
            }
        }
        Ok(*state)
    }

    /// Return the current playback state for `file_id`, if any.
    ///
    /// 返回 `file_id` 当前的回放状态（如有）。
    pub fn get(&self, file_id: &str) -> Option<PlaybackState> {
        self.states.lock().get(file_id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_resume_change_state() {
        let r = PlaybackRegistry::new();
        let s = r.apply("f1", 10_000, RecordPlaybackCommand::Pause).unwrap();
        assert!(s.paused);

        let s = r
            .apply("f1", 10_000, RecordPlaybackCommand::Resume)
            .unwrap();
        assert!(!s.paused);
    }

    #[test]
    fn scale_is_bounded_and_finite() {
        let r = PlaybackRegistry::new();
        let s = r
            .apply("f1", 10_000, RecordPlaybackCommand::Scale { value: 2.0 })
            .unwrap();
        assert_eq!(s.scale, 2.0);

        let err = r
            .apply(
                "f1",
                10_000,
                RecordPlaybackCommand::Scale {
                    value: f64::INFINITY,
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("scale"));

        let err = r
            .apply("f1", 10_000, RecordPlaybackCommand::Scale { value: 0.1 })
            .unwrap_err();
        assert!(err.to_string().contains("scale"));
    }

    #[test]
    fn seek_is_bounded_by_duration() {
        let r = PlaybackRegistry::new();
        let s = r
            .apply("f1", 10_000, RecordPlaybackCommand::Seek { value: 5_000 })
            .unwrap();
        assert_eq!(s.position_ms, 5_000);

        let err = r
            .apply("f1", 10_000, RecordPlaybackCommand::Seek { value: 20_000 })
            .unwrap_err();
        assert!(err.to_string().contains("seek"));

        let err = r
            .apply("f1", 10_000, RecordPlaybackCommand::Seek { value: -1 })
            .unwrap_err();
        assert!(err.to_string().contains("seek"));
    }
}
