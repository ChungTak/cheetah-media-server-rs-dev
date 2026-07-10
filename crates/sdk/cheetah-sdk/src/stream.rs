use std::sync::Arc;

use async_trait::async_trait;
use cheetah_codec::{AVFrame, TrackInfo};
use serde::{Deserialize, Serialize};

use crate::error::SdkError;
use crate::ids::{StreamId, StreamKey, SubscriberId};

/// Action taken when a subscriber queue is full.
///
/// 订阅者队列满时采取的行为。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackpressurePolicy {
    /// Drop frames that are marked `DROPPABLE` first.
    /// 优先丢弃标记为 `DROPPABLE` 的帧。
    DropDroppableFirst,
    /// Drop frames until the next keyframe is received.
    /// 丢弃帧直到收到下一个关键帧。
    DropUntilNextKeyframe,
    /// Disconnect the subscriber when the queue overflows.
    /// 队列溢出时断开订阅者。
    DisconnectOnOverflow,
}

/// Result of dispatching a frame to a subscriber.
///
/// 将帧分发给订阅者的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DispatchResult {
    /// The frame was accepted into the subscriber queue.
    /// 帧已接受进入订阅者队列。
    Accepted,
    /// The frame was dropped by the active backpressure policy.
    /// 帧被当前背压策略丢弃。
    DroppedByPolicy,
    /// The subscriber queue is closed and rejected the frame.
    /// 订阅者队列已关闭并拒绝帧。
    RejectedClosed,
}

/// Mode used when bootstrapping a new subscriber with historical frames.
///
/// 用历史帧引导新订阅者时使用的模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootstrapMode {
    /// No bootstrap; start from the next live frame.
    /// 不引导；从下一个直播帧开始。
    None,
    /// Deliver a short tail of recent frames, then continue live.
    /// 发送一段最近的帧尾，然后继续直播。
    LiveTail,
    /// Deliver the complete GOP ending at the latest random access point.
    /// 发送以最新随机访问点结尾的完整 GOP。
    FullGop,
}

/// Policy controlling how a subscriber is bootstrapped.
///
/// 控制订阅者引导方式的策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapPolicy {
    /// Bootstrap mode.
    /// 引导模式。
    pub mode: BootstrapMode,
    /// Maximum age in milliseconds for bootstrap frames.
    /// 引导帧的最大年龄（毫秒）。
    pub max_bootstrap_age_ms: Option<u64>,
    /// Maximum number of frames to include in the bootstrap.
    /// 引导中包含的最大帧数。
    pub max_bootstrap_frames: usize,
    /// Wait for the next random access point before starting live delivery.
    /// 在开始直播交付前等待下一个随机访问点。
    pub wait_for_next_random_access_point: bool,
}

impl BootstrapPolicy {
    /// No bootstrap.
    /// 不引导。
    pub const fn none() -> Self {
        Self {
            mode: BootstrapMode::None,
            max_bootstrap_age_ms: None,
            max_bootstrap_frames: 0,
            wait_for_next_random_access_point: false,
        }
    }

    /// Bootstrap with a live tail.
    /// 使用直播尾引导。
    pub const fn live_tail(max_bootstrap_frames: usize, max_bootstrap_age_ms: Option<u64>) -> Self {
        Self {
            mode: BootstrapMode::LiveTail,
            max_bootstrap_age_ms,
            max_bootstrap_frames,
            wait_for_next_random_access_point: true,
        }
    }

    /// Bootstrap with a full GOP.
    /// 使用完整 GOP 引导。
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
    /// Whether to announce tracks to subscribers immediately.
    /// 是否立即向订阅者宣告轨道。
    pub announce_tracks: bool,
}

impl Default for PublisherOptions {
    fn default() -> Self {
        Self {
            announce_tracks: true,
        }
    }
}

/// Options for opening a subscriber.
///
/// 打开订阅者时的选项。
#[derive(Debug, Clone)]
pub struct SubscriberOptions {
    /// Maximum number of frames queued for this subscriber.
    /// 为此订阅者排队的最大帧数。
    pub queue_capacity: usize,
    /// Backpressure policy applied when the queue is full.
    /// 队列满时应用的背压策略。
    pub backpressure: BackpressurePolicy,
    /// Bootstrap policy for the subscriber.
    /// 订阅者的引导策略。
    pub bootstrap_policy: BootstrapPolicy,
    /// Which media types (video/audio) the subscriber wants.
    /// 订阅者想要的媒体类型（视频/音频）。
    pub media_filter: MediaFilter,
}

/// Controls which media types a subscriber receives.
///
/// 控制订阅者接收哪些媒体类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaFilter {
    /// Whether to deliver video.
    /// 是否交付视频。
    pub enable_video: bool,
    /// Whether to deliver audio.
    /// 是否交付音频。
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

/// Snapshot of a stream for listing and monitoring.
///
/// 用于列出和监控的流快照。
#[derive(Debug, Clone)]
pub struct StreamSnapshot {
    /// Stream identifier.
    /// 流标识。
    pub stream_id: StreamId,
    /// Logical stream key.
    /// 逻辑流键。
    pub key: StreamKey,
    /// Whether an active publisher is attached.
    /// 是否有活跃的发布者连接。
    pub publisher_active: bool,
    /// Number of active subscribers.
    /// 活跃订阅者数量。
    pub subscriber_count: usize,
    /// Current track metadata.
    /// 当前轨道元数据。
    pub tracks: Vec<TrackInfo>,
}

/// Lease returned when a publisher is acquired.
///
/// A publisher lease represents the exclusive right to publish to a `StreamKey`.
///
/// 获取发布者时返回的租约。
///
/// 发布者租约表示对 `StreamKey` 的独占发布权。
#[derive(Debug, Clone)]
pub struct PublishLease {
    /// Stream identifier.
    /// 流标识。
    pub stream_id: StreamId,
    /// Stream key.
    /// 流键。
    pub stream_key: StreamKey,
    /// Opaque lease identifier.
    /// 不透明租约标识。
    pub lease_id: u64,
}

/// Sink for a publisher to push tracks and frames.
///
/// 发布者推送轨道和帧的接收端。
pub trait PublisherSink: Send + Sync {
    /// Update the track metadata for this publisher.
    /// 更新本发布者的轨道元数据。
    fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), SdkError>;
    /// Push a single frame into the stream.
    /// 将单帧推入流。
    fn push_frame(&self, frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError>;
    /// Close the publisher sink.
    /// 关闭发布者 sink。
    fn close(&self) -> Result<(), SdkError>;
    /// Returns the number of pending keyframe requests since last call.
    /// Resets the counter atomically. Publishers should check this periodically
    /// and send an IDR if > 0.
    ///
    /// 返回自上次调用以来挂起的 keyframe 请求数量。
    /// 原子性地重置计数器。发布者应定期检查，若大于 0 则发送 IDR。
    fn take_keyframe_requests(&self) -> u64;
}

/// Source of frames consumed by a subscriber.
///
/// 订阅者消费的帧源。
#[async_trait]
pub trait SubscriberSource: Send {
    /// Receive the next frame, or `None` if the stream ended.
    /// 接收下一帧；若流结束则返回 `None`。
    async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, SdkError>;
    /// Close the subscriber source.
    /// 关闭订阅者源。
    async fn close(&mut self) -> Result<(), SdkError>;
    /// Return the subscriber identifier.
    /// 返回订阅者标识。
    fn id(&self) -> SubscriberId;
}

/// Engine API for managing streams, publishers, and subscribers.
///
/// 用于管理流、发布者和订阅者的引擎 API。
#[async_trait]
pub trait StreamManagerApi: Send + Sync {
    /// Open a publisher for the given stream key.
    /// 为给定流键打开发布者。
    async fn open_publisher(
        &self,
        stream_key: StreamKey,
        options: PublisherOptions,
    ) -> Result<Box<dyn PublisherSink>, SdkError>;

    /// Open a subscriber for the given stream key.
    /// 为给定流键打开订阅者。
    async fn open_subscriber(
        &self,
        stream_key: StreamKey,
        options: SubscriberOptions,
    ) -> Result<Box<dyn SubscriberSource>, SdkError>;

    /// List all active streams.
    /// 列出所有活跃流。
    async fn list_streams(&self) -> Result<Vec<StreamSnapshot>, SdkError>;

    /// Get a snapshot for a specific stream, if it exists.
    /// 获取特定流的快照（如果存在）。
    async fn get_stream(&self, stream_key: &StreamKey) -> Result<Option<StreamSnapshot>, SdkError>;

    /// Request the publisher of a stream to send a keyframe (IDR).
    /// This is best-effort: the publisher may not support or respond to the request.
    /// Used by RTSP subscribers sending RTCP PLI/FIR.
    ///
    /// 请求流发布者发送关键帧（IDR）。
    /// 这是尽力而为：发布者可能不支持或不响应此请求。
    /// 用于发送 RTCP PLI/FIR 的 RTSP 订阅者。
    async fn request_keyframe(&self, stream_key: &StreamKey) -> Result<(), SdkError>;

    /// Close publishers on streams that have had zero subscribers for longer than
    /// `max_idle_secs`. Returns the number of streams closed.
    ///
    /// 关闭超过 `max_idle_secs` 没有订阅者的流上的发布者。返回关闭的流数量。
    async fn close_idle_publishers(&self, max_idle_secs: u64) -> Result<usize, SdkError>;
}

/// Engine API for acquiring and releasing publisher leases.
///
/// 用于获取和释放发布者租约的引擎 API。
#[async_trait]
pub trait PublisherApi: Send + Sync {
    /// Acquire an exclusive publisher lease for a stream key.
    /// 为流键获取独占发布者租约。
    async fn acquire_publisher(
        &self,
        stream_key: StreamKey,
        options: PublisherOptions,
    ) -> Result<(PublishLease, Box<dyn PublisherSink>), SdkError>;

    /// Release a previously acquired publisher lease.
    /// 释放之前获取的发布者租约。
    async fn release_publisher(&self, lease: &PublishLease) -> Result<(), SdkError>;
}

/// Engine API for subscribing to streams.
///
/// 用于订阅流的引擎 API。
#[async_trait]
pub trait SubscriberApi: Send + Sync {
    /// Subscribe to a stream with the given options.
    /// 使用给定选项订阅流。
    async fn subscribe(
        &self,
        stream_key: StreamKey,
        options: SubscriberOptions,
    ) -> Result<Box<dyn SubscriberSource>, SdkError>;
}

/// Adapter API for publishing frames and updating tracks outside the module lifecycle.
///
/// 用于在模块生命周期之外发布帧和更新轨道的适配器 API。
#[async_trait]
pub trait CoreAdaptersApi: Send + Sync {
    /// Publish a frame to a stream.
    /// 向流发布一帧。
    async fn publish_frame(
        &self,
        stream_key: StreamKey,
        frame: Arc<AVFrame>,
    ) -> Result<DispatchResult, SdkError>;

    /// Update track metadata for a stream.
    /// 更新流的轨道元数据。
    async fn update_tracks(
        &self,
        stream_key: StreamKey,
        tracks: Vec<TrackInfo>,
    ) -> Result<(), SdkError>;

    /// Close a stream and notify all subscribers.
    /// 关闭流并通知所有订阅者。
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
