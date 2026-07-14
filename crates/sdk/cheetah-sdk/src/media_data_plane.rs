use std::sync::Arc;

use crate::ids::SubscriberId;
use async_trait::async_trait;
use cheetah_codec::{AVFrame, TrackInfo};
use cheetah_media_api::command::{PublishRequest, SubscribeRequest};
use cheetah_media_api::error::{MediaError, Result as MediaResult};
use cheetah_media_api::port::MediaRequestContext;

/// Runtime-neutral publisher for raw media frames.
///
/// 运行时无关的原始媒体帧发布者。
#[async_trait]
pub trait MediaFramePublisher: Send + Sync {
    /// Replace the published track metadata.
    fn update_tracks(&self, tracks: Vec<TrackInfo>) -> MediaResult<()>;

    /// Push a frame into the engine.
    fn push_frame(&self, frame: Arc<AVFrame>) -> MediaResult<()>;

    /// Close the publisher and release the stream lease.
    async fn close(&self) -> MediaResult<()>;

    /// Return the number of pending keyframe requests since the last call.
    fn take_keyframe_requests(&self) -> u64;
}

/// Runtime-neutral subscriber for raw media frames.
///
/// 运行时无关的原始媒体帧订阅者。
#[async_trait]
pub trait MediaFrameSubscriber: Send {
    /// Receive the next frame, or `None` when the subscriber is closed.
    async fn recv(&mut self) -> MediaResult<Option<Arc<AVFrame>>>;

    /// Close the subscriber.
    async fn close(&mut self) -> MediaResult<()>;

    /// Unique subscriber id assigned by the engine.
    fn id(&self) -> SubscriberId;

    /// Tracks discovered so far.
    fn tracks(&self) -> Vec<TrackInfo> {
        Vec::new()
    }
}

/// Runtime-neutral data plane API for in-process frame publishing and subscribing.
///
/// 用于进程内帧发布/订阅的运行时无关数据面 API。
#[async_trait]
pub trait MediaDataPlaneApi: Send + Sync {
    /// Open a frame publisher for the requested media key.
    async fn open_frame_publisher(
        &self,
        ctx: &MediaRequestContext,
        request: PublishRequest,
    ) -> MediaResult<Box<dyn MediaFramePublisher>>;

    /// Open a frame subscriber for the requested media key.
    async fn open_frame_subscriber(
        &self,
        ctx: &MediaRequestContext,
        request: SubscribeRequest,
    ) -> MediaResult<Box<dyn MediaFrameSubscriber>>;
}

/// No-op data plane used before the engine is fully wired.
///
/// 在引擎完成接线之前使用的空数据面。
pub struct NoopMediaDataPlane;

#[async_trait]
impl MediaDataPlaneApi for NoopMediaDataPlane {
    async fn open_frame_publisher(
        &self,
        _ctx: &MediaRequestContext,
        _request: PublishRequest,
    ) -> MediaResult<Box<dyn MediaFramePublisher>> {
        Err(MediaError::unavailable("media data plane"))
    }

    async fn open_frame_subscriber(
        &self,
        _ctx: &MediaRequestContext,
        _request: SubscribeRequest,
    ) -> MediaResult<Box<dyn MediaFrameSubscriber>> {
        Err(MediaError::unavailable("media data plane"))
    }
}

/// Create a no-op media data plane.
pub fn default_media_data_plane() -> Arc<dyn MediaDataPlaneApi> {
    Arc::new(NoopMediaDataPlane)
}
