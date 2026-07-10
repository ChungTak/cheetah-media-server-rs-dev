use std::collections::HashMap;
use std::sync::Arc;

use cheetah_codec::{Timebase, TimestampNormalizer, TimestampNormalizerConfig, TrackInfo};
use cheetah_rtmp_driver_tokio::RtmpConnectionId;
use cheetah_sdk::{
    CancellationToken, JoinHandle as RuntimeJoinHandle, PublishLease, PublisherSink,
};
use parking_lot::Mutex;

/// `PublishSession` data structure.
/// `PublishSession` 数据结构.
pub struct PublishSession {
    /// `lease` field of type `PublishLease`.
    /// `lease` 字段，类型为 `PublishLease`.
    pub lease: PublishLease,
    /// `sink` field.
    /// `sink` 字段.
    pub sink: Box<dyn PublisherSink>,
    /// `tracks` field of type `PublishTracks`.
    /// `tracks` 字段，类型为 `PublishTracks`.
    pub tracks: PublishTracks,
    /// `timestamp_states` field of type `PublishTimestampStates`.
    /// `timestamp_states` 字段，类型为 `PublishTimestampStates`.
    pub timestamp_states: PublishTimestampStates,
    /// `fps_estimator` field of type `FrameRateEstimator`.
    /// `fps_estimator` 字段，类型为 `FrameRateEstimator`.
    pub fps_estimator: FrameRateEstimator,
}

/// Estimates video frame rate from PTS differences (250 sample average).
#[derive(Debug, Default)]
pub struct FrameRateEstimator {
    /// `last_dts_ms` field.
    /// `last_dts_ms` 字段.
    last_dts_ms: Option<i64>,
    /// `sum_delta_ms` field of type `u64`.
    /// `sum_delta_ms` 字段，类型为 `u64`.
    sum_delta_ms: u64,
    /// `sample_count` field of type `u32`.
    /// `sample_count` 字段，类型为 `u32`.
    sample_count: u32,
    /// `estimated_fps` field.
    /// `estimated_fps` 字段.
    estimated_fps: Option<f64>,
}

impl FrameRateEstimator {
    const MAX_SAMPLES: u32 = 250;

    /// Feed a video frame DTS. Returns estimated FPS once enough samples collected.
    pub fn on_video_frame(&mut self, dts_ms: i64) -> Option<f64> {
        if let Some(last) = self.last_dts_ms {
            let delta = (dts_ms - last).unsigned_abs();
            if delta > 0 && delta < 1000 {
                self.sum_delta_ms += delta;
                self.sample_count += 1;
                if self.sample_count >= Self::MAX_SAMPLES && self.estimated_fps.is_none() {
                    let avg_delta = self.sum_delta_ms as f64 / self.sample_count as f64;
                    if avg_delta > 0.0 {
                        self.estimated_fps = Some((1000.0 / avg_delta).min(120.0));
                    }
                }
            }
        }
        self.last_dts_ms = Some(dts_ms);
        self.estimated_fps
    }
}

/// `PublishTracks` data structure.
/// `PublishTracks` 数据结构.
#[derive(Default)]
pub struct PublishTracks {
    /// `video` field.
    /// `video` 字段.
    pub video: Option<TrackInfo>,
    /// `audio` field.
    /// `audio` 字段.
    pub audio: Option<TrackInfo>,
}

impl PublishTracks {
    /// `list` function.
    /// `list` 函数.
    pub fn list(&self) -> Vec<TrackInfo> {
        let mut tracks: Vec<TrackInfo> = Vec::new();
        if let Some(video) = &self.video {
            tracks.push(video.clone());
        }
        if let Some(audio) = &self.audio {
            tracks.push(audio.clone());
        }
        tracks
    }
}

/// `PublishTrackTimestampState` data structure.
/// `PublishTrackTimestampState` 数据结构.
#[derive(Debug)]
pub struct PublishTrackTimestampState {
    /// `normalizer` field of type `TimestampNormalizer`.
    /// `normalizer` 字段，类型为 `TimestampNormalizer`.
    pub normalizer: TimestampNormalizer,
    /// `repair_count` field of type `u64`.
    /// `repair_count` 字段，类型为 `u64`.
    pub repair_count: u64,
    /// `last_raw_timestamp_ms` field.
    /// `last_raw_timestamp_ms` 字段.
    pub last_raw_timestamp_ms: Option<u32>,
}

impl PublishTrackTimestampState {
    fn new() -> Self {
        let config = TimestampNormalizerConfig::new(
            Timebase::new(1, 1_000),
            Timebase::new(1, 1_000),
            Some(32),
        )
        .unwrap_or_else(|err| {
            unreachable!("rtmp timestamp normalizer config must be valid: {err}")
        });
        Self {
            normalizer: TimestampNormalizer::new(config),
            repair_count: 0,
            last_raw_timestamp_ms: None,
        }
    }
}

/// `PublishTimestampStates` data structure.
/// `PublishTimestampStates` 数据结构.
#[derive(Debug)]
pub struct PublishTimestampStates {
    /// `video` field of type `PublishTrackTimestampState`.
    /// `video` 字段，类型为 `PublishTrackTimestampState`.
    pub video: PublishTrackTimestampState,
    /// `audio` field of type `PublishTrackTimestampState`.
    /// `audio` 字段，类型为 `PublishTrackTimestampState`.
    pub audio: PublishTrackTimestampState,
}

impl Default for PublishTimestampStates {
    fn default() -> Self {
        Self {
            video: PublishTrackTimestampState::new(),
            audio: PublishTrackTimestampState::new(),
        }
    }
}

/// `PlaySession` data structure.
/// `PlaySession` 数据结构.
pub struct PlaySession {
    /// `cancel` field of type `CancellationToken`.
    /// `cancel` 字段，类型为 `CancellationToken`.
    pub cancel: CancellationToken,
    /// `join` field.
    /// `join` 字段.
    pub join: Box<dyn RuntimeJoinHandle>,
}

/// A publish session in keepalive state — publisher disconnected but lease is held
/// for a configurable window to allow seamless reconnection.
pub struct KeepaliveSession {
    /// `lease` field of type `PublishLease`.
    /// `lease` 字段，类型为 `PublishLease`.
    pub lease: PublishLease,
    /// `sink` field.
    /// `sink` 字段.
    pub sink: Box<dyn PublisherSink>,
    /// `tracks` field of type `PublishTracks`.
    /// `tracks` 字段，类型为 `PublishTracks`.
    pub tracks: PublishTracks,
}

/// Returns a copy with `publish_session` set.
/// 返回 一个 copy 带有 `publish_session` 设置.
pub fn with_publish_session<T, F>(
    connection_id: RtmpConnectionId,
    sessions: &Arc<Mutex<HashMap<RtmpConnectionId, PublishSession>>>,
    f: F,
) -> Option<T>
where
    F: FnOnce(&mut PublishSession) -> T,
{
    let mut map = sessions.lock();
    let session = map.get_mut(&connection_id)?;
    Some(f(session))
}
