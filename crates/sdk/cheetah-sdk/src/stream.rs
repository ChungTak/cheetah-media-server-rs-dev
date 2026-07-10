use std::sync::Arc;

use async_trait::async_trait;
use cheetah_codec::{AVFrame, TrackInfo};
use serde::{Deserialize, Serialize};

use crate::error::SdkError;
use crate::ids::{StreamId, StreamKey, SubscriberId};

/// Policy for handling subscriber queue overflow.
///
/// 处理订阅者队列溢出的策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackpressurePolicy {
    DropDroppableFirst,
    DropUntilNextKeyframe,
    DisconnectOnOverflow,
}

/// Result of pushing a frame into the dispatch pipeline.
///
/// 将帧推入分发管道后的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DispatchResult {
    Accepted,
    DroppedByPolicy,
    RejectedClosed,
}

/// Strategy for bootstrapping a new subscriber.
///
/// 新订阅者启动策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootstrapMode {
    None,
    LiveTail,
    FullGop,
}

/// Policy controlling how much historical data a new subscriber receives.
///
/// 控制新订阅者接收多少历史数据的策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapPolicy {
    pub mode: BootstrapMode,
    pub max_bootstrap_age_ms: Option<u64>,
    pub max_bootstrap_frames: usize,
    pub wait_for_next_random_access_point: bool,
}

impl BootstrapPolicy {
    /// No bootstrapping; subscriber starts from the next live frame.
    ///
    /// 不启动；订阅者从下一个直播帧开始。
    pub const fn none() -> Self {
        Self {
            mode: BootstrapMode::None,
            max_bootstrap_age_ms: None,
            max_bootstrap_frames: 0,
            wait_for_next_random_access_point: false,
        }
    }

    /// Bootstrap from the live tail bounded by frame count and age.
    ///
    /// 在帧数量和时长限制内从直播尾部启动。
    pub const fn live_tail(max_bootstrap_frames: usize, max_bootstrap_age_ms: Option<u64>) -> Self {
        Self {
            mode: BootstrapMode::LiveTail,
            max_bootstrap_age_ms,
            max_bootstrap_frames,
            wait_for_next_random_access_point: true,
        }
    }

    /// Bootstrap from the most recent complete GOP.
    ///
    /// 从最近一个完整 GOP 开始启动。
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

/// Options for opening a publisher on a stream.
///
/// 在流上打开发布者的选项。
#[derive(Debug, Clone)]
pub struct PublisherOptions {
    pub announce_tracks: bool,
}

impl Default for PublisherOptions {
    fn default() -> Self {
        Self {
            announce_tracks: true,
        }
    }
}

/// Options for opening a subscriber on a stream.
///
/// 在流上打开订阅者的选项。
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

/// Snapshot of a stream for monitoring and inspection APIs.
///
/// 流的快照，用于监控和检查 API。
#[derive(Debug, Clone)]
pub struct StreamSnapshot {
    pub stream_id: StreamId,
    pub key: StreamKey,
    pub publisher_active: bool,
    pub subscriber_count: usize,
    pub tracks: Vec<TrackInfo>,
}

/// Lease token returned when a publisher is acquired.
///
/// The lease must be released when the publisher is done.
///
/// 获取发布者时返回的租约令牌。
///
/// 发布者完成后必须释放此租约。
#[derive(Debug, Clone)]
pub struct PublishLease {
    pub stream_id: StreamId,
    pub stream_key: StreamKey,
    pub lease_id: u64,
}

/// Sink through which a publisher feeds frames into a stream.
///
/// 发布者通过此 sink 将帧送入流。
pub trait PublisherSink: Send + Sync {
    /// Update the track metadata for the published stream.
    ///
    /// 更新已发布流的轨道元数据。
    fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), SdkError>;
    /// Push a frame into the stream dispatch pipeline.
    ///
    /// 将帧推入流分发管道。
    fn push_frame(&self, frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError>;

    /// Close the publisher and release its resources.
    ///
    /// 关闭发布者并释放资源。
    fn close(&self) -> Result<(), SdkError>;
    /// Return the number of pending keyframe requests since the last call.
    ///
    /// Resets the counter atomically. Publishers should check this periodically
    /// and send an IDR if > 0.
    ///
    /// 返回自上次调用以来挂起的关键帧请求数。
    ///
    /// 计数器被原子重置。发布者应定期检查，并在大于 0 时发送 IDR。
    fn take_keyframe_requests(&self) -> u64;
}

/// Source from which a subscriber receives frames.
///
/// 订阅者从中接收帧的源。
#[async_trait]
pub trait SubscriberSource: Send {
    /// Receive the next frame, or `None` if the source is closed.
    ///
    /// 接收下一帧，如果源已关闭则返回 `None`。
    async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, SdkError>;

    /// Close the subscriber source.
    ///
    /// 关闭订阅者源。
    async fn close(&mut self) -> Result<(), SdkError>;

    /// Return the subscriber identifier.
    ///
    /// 返回订阅者标识符。
    fn id(&self) -> SubscriberId;
}

/// Engine API for managing publishers, subscribers and stream snapshots.
///
/// 管理发布者、订阅者和流快照的引擎 API。
#[async_trait]
pub trait StreamManagerApi: Send + Sync {
    /// Open a publisher on the given stream key.
    ///
    /// 在给定流键上打开发布者。
    async fn open_publisher(
        &self,
        stream_key: StreamKey,
        options: PublisherOptions,
    ) -> Result<Box<dyn PublisherSink>, SdkError>;

    /// Open a subscriber on the given stream key.
    ///
    /// 在给定流键上打开订阅者。
    async fn open_subscriber(
        &self,
        stream_key: StreamKey,
        options: SubscriberOptions,
    ) -> Result<Box<dyn SubscriberSource>, SdkError>;

    /// List all active streams.
    ///
    /// 列出所有活动流。
    async fn list_streams(&self) -> Result<Vec<StreamSnapshot>, SdkError>;

    /// Return a snapshot for a specific stream, if it exists.
    ///
    /// 返回指定流的快照（如果存在）。
    async fn get_stream(&self, stream_key: &StreamKey) -> Result<Option<StreamSnapshot>, SdkError>;

    /// Request the publisher of a stream to send a keyframe (IDR).
    /// This is best-effort: the publisher may not support or respond to the request.
    /// Used by RTSP subscribers sending RTCP PLI/FIR.
    async fn request_keyframe(&self, stream_key: &StreamKey) -> Result<(), SdkError>;

    /// Close publishers on streams that have had zero subscribers for longer than
    /// `max_idle_secs`. Returns the number of streams closed.
    ///
    /// 关闭在超过 `max_idle_secs` 时间内没有订阅者的流上的发布者。
    /// 返回关闭的流数量。
    async fn close_idle_publishers(&self, max_idle_secs: u64) -> Result<usize, SdkError>;
}

/// API for acquiring and releasing publisher leases.
///
/// 用于获取和释放发布者租约的 API。
#[async_trait]
pub trait PublisherApi: Send + Sync {
    /// Acquire a publisher lease and its sink for a stream.
    ///
    /// 获取流的发布者租约及其 sink。
    async fn acquire_publisher(
        &self,
        stream_key: StreamKey,
        options: PublisherOptions,
    ) -> Result<(PublishLease, Box<dyn PublisherSink>), SdkError>;

    /// Release a previously acquired publisher lease.
    ///
    /// 释放之前获取的发布者租约。
    async fn release_publisher(&self, lease: &PublishLease) -> Result<(), SdkError>;
}

/// API for subscribing to a stream.
///
/// 用于订阅流的 API。
#[async_trait]
pub trait SubscriberApi: Send + Sync {
    /// Subscribe to a stream and return a source that yields frames.
    ///
    /// 订阅流并返回产生帧的源。
    async fn subscribe(
        &self,
        stream_key: StreamKey,
        options: SubscriberOptions,
    ) -> Result<Box<dyn SubscriberSource>, SdkError>;
}

/// Adapter API used by protocol-core modules to publish frames into streams.
///
/// 协议核心模块用于将帧发布到流的适配器 API。
#[async_trait]
pub trait CoreAdaptersApi: Send + Sync {
    /// Publish a frame into the stream with the given key.
    ///
    /// 将帧发布到指定键的流。
    async fn publish_frame(
        &self,
        stream_key: StreamKey,
        frame: Arc<AVFrame>,
    ) -> Result<DispatchResult, SdkError>;

    /// Update the track metadata for the given stream.
    ///
    /// 更新指定流的轨道元数据。
    async fn update_tracks(
        &self,
        stream_key: StreamKey,
        tracks: Vec<TrackInfo>,
    ) -> Result<(), SdkError>;

    /// Close the stream with the given key.
    ///
    /// 关闭指定键的流。
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
