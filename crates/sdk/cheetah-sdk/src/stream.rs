use std::sync::Arc;

use async_trait::async_trait;
use cheetah_codec::{AVFrame, TrackInfo};
use serde::{Deserialize, Serialize};

use crate::error::SdkError;
use crate::ids::{StreamId, StreamKey, SubscriberId};

/// Subscriber-side policy when the outbound queue is full.
///
/// 订阅者 outbound 队列满时的反压策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackpressurePolicy {
    DropDroppableFirst,
    DropUntilNextKeyframe,
    DisconnectOnOverflow,
}

/// Result of pushing a frame into the stream distribution pipeline.
///
/// 将帧推入流分发管道后的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DispatchResult {
    Accepted,
    DroppedByPolicy,
    RejectedClosed,
}

/// How a subscriber is bootstrapped when it first connects.
///
/// 订阅者首次连接时的启动方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootstrapMode {
    None,
    LiveTail,
    FullGop,
}

/// Policy controlling how many and how old frames are sent at subscribe time.
///
/// 控制订阅时发送多少帧、帧多旧。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapPolicy {
    pub mode: BootstrapMode,
    pub max_bootstrap_age_ms: Option<u64>,
    pub max_bootstrap_frames: usize,
    pub wait_for_next_random_access_point: bool,
}

impl BootstrapPolicy {
    /// No bootstrapping; subscriber receives only new frames.
    ///
    /// 不启动引导；订阅者只接收新帧。
    pub const fn none() -> Self {
        Self {
            mode: BootstrapMode::None,
            max_bootstrap_age_ms: None,
            max_bootstrap_frames: 0,
            wait_for_next_random_access_point: false,
        }
    }

    /// Bootstrap with the most recent frames up to the configured limits.
    ///
    /// 用最近的帧按配置限制进行引导。
    pub const fn live_tail(max_bootstrap_frames: usize, max_bootstrap_age_ms: Option<u64>) -> Self {
        Self {
            mode: BootstrapMode::LiveTail,
            max_bootstrap_age_ms,
            max_bootstrap_frames,
            wait_for_next_random_access_point: true,
        }
    }

    /// Bootstrap from the most recent random-access point (full GOP).
    ///
    /// 从最近的随机访问点（完整 GOP）开始引导。
    pub const fn full_gop(max_bootstrap_frames: usize, max_bootstrap_age_ms: Option<u64>) -> Self {
        Self {
            mode: BootstrapMode::FullGop,
            max_bootstrap_age_ms,
            max_bootstrap_frames,
            wait_for_next_random_access_point: true,
        }
    }
}

impl Default for BootstrapPolicy {
    fn default() -> Self {
        Self {
            mode: BootstrapMode::LiveTail,
            max_bootstrap_age_ms: Some(1_500),
            max_bootstrap_frames: 150,
            wait_for_next_random_access_point: true,
        }
    }
}

/// Options for opening a publisher.
///
/// 打开发布者时的选项。
#[derive(Debug, Clone)]
pub struct PublisherOptions {
    pub announce_tracks: bool,
    /// Protocol identifier reported on stream-published events (e.g. "rtmp", "rtp").
    pub protocol: String,
    /// Optional remote endpoint of the publisher, for event metadata.
    pub remote_endpoint: Option<String>,
}

impl Default for PublisherOptions {
    fn default() -> Self {
        Self {
            announce_tracks: true,
            protocol: String::new(),
            remote_endpoint: None,
        }
    }
}

/// Options for opening a subscriber.
///
/// 打开订阅者时的选项。
#[derive(Debug, Clone)]
pub struct SubscriberOptions {
    pub queue_capacity: usize,
    pub backpressure: BackpressurePolicy,
    pub bootstrap_policy: BootstrapPolicy,
    pub media_filter: MediaFilter,
}

/// Controls which media types a subscriber receives.
///
/// 控制订阅者接收哪些媒体类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaFilter {
    pub enable_video: bool,
    pub enable_audio: bool,
}

impl Default for MediaFilter {
    fn default() -> Self {
        Self {
            enable_video: true,
            enable_audio: true,
        }
    }
}

impl Default for SubscriberOptions {
    fn default() -> Self {
        Self {
            queue_capacity: 150,
            backpressure: BackpressurePolicy::DropDroppableFirst,
            bootstrap_policy: BootstrapPolicy::default(),
            media_filter: MediaFilter::default(),
        }
    }
}

/// Snapshot of a stream's runtime state.
///
/// 流的运行时状态快照。
#[derive(Debug, Clone)]
pub struct StreamSnapshot {
    pub stream_id: StreamId,
    pub key: StreamKey,
    pub publisher_active: bool,
    pub subscriber_count: usize,
    pub tracks: Vec<TrackInfo>,
}

/// Lease returned when acquiring a publisher, used to release it later.
///
/// 获取发布者时返回的租约，用于后续释放。
#[derive(Debug, Clone)]
pub struct PublishLease {
    pub stream_id: StreamId,
    pub stream_key: StreamKey,
    pub lease_id: u64,
}

/// Sink used by a publisher to push frames and update tracks.
///
/// 发布者用于推送帧和更新轨道的 sink。
pub trait PublisherSink: Send + Sync {
    fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), SdkError>;
    fn push_frame(&self, frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError>;
    fn close(&self) -> Result<(), SdkError>;
    /// Returns the number of pending keyframe requests since last call.
    /// Resets the counter atomically. Publishers should check this periodically
    /// and send an IDR if > 0.
    fn take_keyframe_requests(&self) -> u64;
}

/// Source used by a subscriber to receive frames.
///
/// 订阅者用于接收帧的 source。
#[async_trait]
pub trait SubscriberSource: Send {
    async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, SdkError>;
    async fn close(&mut self) -> Result<(), SdkError>;
    fn id(&self) -> SubscriberId;

    /// Snapshot of tracks discovered so far. May be empty if the source has not
    /// yet completed track discovery.
    ///
    /// 当前已发现的轨道快照。若源尚未完成轨道发现，可能为空。
    fn tracks(&self) -> Vec<TrackInfo> {
        Vec::new()
    }
}

/// Manager API for publishers, subscribers, and stream lifecycle.
///
/// 发布者、订阅者和流生命周期的管理 API。
#[async_trait]
pub trait StreamManagerApi: Send + Sync {
    async fn open_publisher(
        &self,
        stream_key: StreamKey,
        options: PublisherOptions,
    ) -> Result<Box<dyn PublisherSink>, SdkError>;

    async fn open_subscriber(
        &self,
        stream_key: StreamKey,
        options: SubscriberOptions,
    ) -> Result<Box<dyn SubscriberSource>, SdkError>;

    async fn list_streams(&self) -> Result<Vec<StreamSnapshot>, SdkError>;

    async fn get_stream(&self, stream_key: &StreamKey) -> Result<Option<StreamSnapshot>, SdkError>;

    /// Request the publisher of a stream to send a keyframe (IDR).
    /// This is best-effort: the publisher may not support or respond to the request.
    /// Used by RTSP subscribers sending RTCP PLI/FIR.
    async fn request_keyframe(&self, stream_key: &StreamKey) -> Result<(), SdkError>;

    /// Close publishers on streams that have had zero subscribers for longer than
    /// `max_idle_secs`. Returns the number of streams closed.
    async fn close_idle_publishers(&self, max_idle_secs: u64) -> Result<usize, SdkError>;
}

/// Acquire and release a publisher lease for a stream.
///
/// 获取和释放某流的发布者租约。
#[async_trait]
pub trait PublisherApi: Send + Sync {
    async fn acquire_publisher(
        &self,
        stream_key: StreamKey,
        options: PublisherOptions,
    ) -> Result<(PublishLease, Box<dyn PublisherSink>), SdkError>;

    async fn release_publisher(&self, lease: &PublishLease) -> Result<(), SdkError>;
}

/// Subscribe to a stream.
///
/// 订阅一个流。
#[async_trait]
pub trait SubscriberApi: Send + Sync {
    async fn subscribe(
        &self,
        stream_key: StreamKey,
        options: SubscriberOptions,
    ) -> Result<Box<dyn SubscriberSource>, SdkError>;
}

/// Low-level adapter API for publishing frames and updating tracks.
///
/// 发布帧和更新轨道的底层适配器 API。
#[async_trait]
pub trait CoreAdaptersApi: Send + Sync {
    async fn publish_frame(
        &self,
        stream_key: StreamKey,
        frame: Arc<AVFrame>,
    ) -> Result<DispatchResult, SdkError>;

    async fn update_tracks(
        &self,
        stream_key: StreamKey,
        tracks: Vec<TrackInfo>,
    ) -> Result<(), SdkError>;

    async fn close_stream(&self, stream_key: &StreamKey) -> Result<(), SdkError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscriber_default_uses_live_tail_bootstrap_policy() {
        let options = SubscriberOptions::default();
        assert_eq!(options.bootstrap_policy.mode, BootstrapMode::LiveTail);
        assert_eq!(options.bootstrap_policy.max_bootstrap_age_ms, Some(1_500));
        assert_eq!(options.bootstrap_policy.max_bootstrap_frames, 150);
        assert!(options.bootstrap_policy.wait_for_next_random_access_point);
    }

    #[test]
    fn bootstrap_policy_builders_cover_none_live_tail_and_full_gop() {
        let none = BootstrapPolicy::none();
        assert_eq!(none.mode, BootstrapMode::None);
        assert_eq!(none.max_bootstrap_frames, 0);
        assert!(!none.wait_for_next_random_access_point);

        let live_tail = BootstrapPolicy::live_tail(900, Some(2_000));
        assert_eq!(live_tail.mode, BootstrapMode::LiveTail);
        assert_eq!(live_tail.max_bootstrap_frames, 900);
        assert_eq!(live_tail.max_bootstrap_age_ms, Some(2_000));
        assert!(live_tail.wait_for_next_random_access_point);

        let full_gop = BootstrapPolicy::full_gop(1_200, None);
        assert_eq!(full_gop.mode, BootstrapMode::FullGop);
        assert_eq!(full_gop.max_bootstrap_frames, 1_200);
        assert_eq!(full_gop.max_bootstrap_age_ms, None);
        assert!(full_gop.wait_for_next_random_access_point);
    }
}
