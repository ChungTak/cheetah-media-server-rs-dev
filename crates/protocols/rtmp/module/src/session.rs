use std::collections::HashMap;
use std::sync::Arc;

use cheetah_codec::{Timebase, TimestampNormalizer, TimestampNormalizerConfig, TrackInfo};
use cheetah_rtmp_driver_tokio::RtmpConnectionId;
use cheetah_sdk::{
    CancellationToken, JoinHandle as RuntimeJoinHandle, PublishLease, PublisherSink,
};
use parking_lot::Mutex;

/// Mutable state for a single RTMP publish session.
///
/// Holds the publisher lease, the sink used to push frames into the engine, the
/// current track metadata, per-track timestamp normalizers, and a frame-rate estimator.
///
/// 单个 RTMP 发布会话的可变状态。
///
/// 包含发布租约、将帧推入引擎的 sink、当前轨道元数据、
/// 各轨道时间戳归一器以及帧率估算器。
pub struct PublishSession {
    pub lease: PublishLease,
    pub sink: Box<dyn PublisherSink>,
    pub tracks: PublishTracks,
    pub timestamp_states: PublishTimestampStates,
    pub fps_estimator: FrameRateEstimator,
}

/// Estimates video frame rate from DTS differences using a rolling 250-sample average.
///
/// 用滑动 250 样本 DTS 差值均值估算视频帧率。
#[derive(Debug, Default)]
pub struct FrameRateEstimator {
    last_dts_ms: Option<i64>,
    sum_delta_ms: u64,
    sample_count: u32,
    estimated_fps: Option<f64>,
}

impl FrameRateEstimator {
    const MAX_SAMPLES: u32 = 250;

    /// Feeds a video frame DTS and returns the estimated FPS once enough samples are collected.
    ///
    /// 输入视频帧 DTS，样本足够时返回估算 FPS。
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

/// Tracks discovered for a publish session, indexed by media kind.
///
/// 发布会话已发现的轨道，按媒体类型索引。
#[derive(Default)]
pub struct PublishTracks {
    pub video: Option<TrackInfo>,
    pub audio: Option<TrackInfo>,
}

impl PublishTracks {
    /// Returns the current tracks as a vector, ordered video then audio.
    ///
    /// 返回当前轨道向量，顺序为视频在前、音频在后。
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

/// Per-track timestamp normalization state for a publish session.
///
/// Wraps a `TimestampNormalizer` plus bookkeeping for repair counts and the last
/// raw RTMP timestamp, used to detect wraparound and large resets.
///
/// 发布会话单轨道的时间戳归一化状态。
///
/// 包装 `TimestampNormalizer` 并记录修复计数与上一个 raw RTMP 时间戳，
/// 用于检测回绕与大幅重置。
#[derive(Debug)]
pub struct PublishTrackTimestampState {
    pub normalizer: TimestampNormalizer,
    pub repair_count: u64,
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

/// Timestamp normalization state for both video and audio tracks of a publish session.
///
/// 发布会话视频与音频轨道的时间戳归一化状态。
#[derive(Debug)]
pub struct PublishTimestampStates {
    pub video: PublishTrackTimestampState,
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

/// Active RTMP play session with a cancel token and a runtime join handle.
///
/// 活跃的 RTMP 播放会话，包含取消 token 与运行时 join 句柄。
pub struct PlaySession {
    pub cancel: CancellationToken,
    pub join: Box<dyn RuntimeJoinHandle>,
}

/// A publish session retained after the publisher disconnects.
///
/// The lease is held for a configurable window so a seamless reconnection can
/// resume publishing without subscribers noticing the gap.
///
/// 发布者断开连接后保留的发布会话。
///
/// 在可配置的窗口期内保留租约，以便无缝重连并在订阅者无感知的情况下恢复发布。
pub struct KeepaliveSession {
    pub lease: PublishLease,
    pub sink: Box<dyn PublisherSink>,
    pub tracks: PublishTracks,
}

/// Runs a closure against the publish session for a given connection, if any.
///
/// This helper centralizes the lock-and-lookup pattern used by the driver event loop.
///
/// 对指定连接的发布会话（如果存在）执行闭包。
///
/// 该辅助函数集中了驱动事件循环中常用的加锁查找模式。
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
