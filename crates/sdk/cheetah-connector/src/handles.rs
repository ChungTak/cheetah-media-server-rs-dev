use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_codec::{AVFrame, TrackInfo};
use cheetah_sdk::{DispatchResult, PublisherSink, SdkError, SubscriberId, SubscriberSource};

use crate::error::ConnectorError;
use crate::protocol::Protocol;

/// A handle returned by [`RuntimeConnector::open_pull`].
///
/// Wraps an internal `SubscriberSource` and maps `SdkError` to `ConnectorError`.
///
/// 由 [`RuntimeConnector::open_pull`] 返回的句柄。
///
/// 包装内部 `SubscriberSource` 并将 `SdkError` 映射为 `ConnectorError`。
pub struct PullHandle {
    protocol: Protocol,
    url: String,
    inner: Box<dyn SubscriberSource>,
}

impl fmt::Debug for PullHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PullHandle")
            .field("protocol", &self.protocol)
            .field("url", &self.url)
            .field("id", &self.id())
            .finish_non_exhaustive()
    }
}

impl PullHandle {
    pub(crate) fn new(protocol: Protocol, url: String, inner: Box<dyn SubscriberSource>) -> Self {
        Self {
            protocol,
            url,
            inner,
        }
    }

    /// Returns the protocol for this pull handle.
    ///
    /// 返回本 pull 句柄的协议。
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Returns the URL used to open this pull handle.
    ///
    /// 返回打开本 pull 句柄所用的 URL。
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the subscriber id of the underlying source.
    ///
    /// 返回底层源的订阅者 id。
    pub fn id(&self) -> SubscriberId {
        self.inner.id()
    }

    /// Snapshot of tracks discovered so far. May be empty if the protocol has
    /// not yet completed track discovery.
    ///
    /// 返回目前已发现的轨道快照。若协议尚未完成轨道发现，可能为空。
    pub fn tracks(&self) -> Vec<TrackInfo> {
        self.inner.tracks()
    }

    /// Receive the next frame from the source.
    ///
    /// 从源接收下一帧。
    pub async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, ConnectorError> {
        self.inner
            .recv()
            .await
            .map_err(|e| map_sdk_error(self.protocol, crate::error::Operation::Read, e))
    }

    /// Close the pull handle.
    ///
    /// 关闭 pull 句柄。
    pub async fn close(&mut self) -> Result<(), ConnectorError> {
        self.inner
            .close()
            .await
            .map_err(|e| map_sdk_error(self.protocol, crate::error::Operation::Close, e))
    }

    /// Expose the underlying source if the caller needs a `&dyn SubscriberSource`.
    ///
    /// 暴露底层源，供调用者需要 `&dyn SubscriberSource` 时使用。
    pub fn as_subscriber(&mut self) -> &mut (dyn SubscriberSource + 'static) {
        self.inner.as_mut()
    }
}

/// A handle returned by [`RuntimeConnector::open_push`].
///
/// Wraps an internal `PublisherSink` and maps `SdkError` to `ConnectorError`.
///
/// 由 [`RuntimeConnector::open_push`] 返回的句柄。
///
/// 包装内部 `PublisherSink` 并将 `SdkError` 映射为 `ConnectorError`。
pub struct PushHandle {
    protocol: Protocol,
    url: String,
    inner: Box<dyn PublisherSink>,
}

impl fmt::Debug for PushHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PushHandle")
            .field("protocol", &self.protocol)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl PushHandle {
    pub(crate) fn new(protocol: Protocol, url: String, inner: Box<dyn PublisherSink>) -> Self {
        Self {
            protocol,
            url,
            inner,
        }
    }

    /// Returns the protocol for this push handle.
    ///
    /// 返回本 push 句柄的协议。
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Returns the URL used to open this push handle.
    ///
    /// 返回打开本 push 句柄所用的 URL。
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Update the tracks published on this stream.
    ///
    /// 更新本流发布的轨道。
    pub fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), ConnectorError> {
        self.inner
            .update_tracks(tracks)
            .map_err(|e| map_sdk_error(self.protocol, crate::error::Operation::Open, e))
    }

    /// Push a frame into the stream.
    ///
    /// 将帧推入流。
    pub fn push_frame(&self, frame: Arc<AVFrame>) -> Result<DispatchResult, ConnectorError> {
        self.inner
            .push_frame(frame)
            .map_err(|e| map_sdk_error(self.protocol, crate::error::Operation::Write, e))
    }

    /// Returns the number of pending keyframe requests since the last call.
    ///
    /// 返回自上次调用以来待处理的关键帧请求数。
    pub fn take_keyframe_requests(&self) -> u64 {
        self.inner.take_keyframe_requests()
    }

    /// Wait until the publish session is ready.
    ///
    /// 等待发布会话就绪。
    pub async fn wait_ready(&self) -> Result<(), ConnectorError> {
        // TODO: wire protocol-specific readiness signalling.
        Ok(())
    }

    /// Close the push handle.
    ///
    /// 关闭 push 句柄。
    pub fn close(&self) -> Result<(), ConnectorError> {
        self.inner
            .close()
            .map_err(|e| map_sdk_error(self.protocol, crate::error::Operation::Close, e))
    }

    /// Expose the underlying sink if the caller needs a `&dyn PublisherSink`.
    ///
    /// 暴露底层 sink，供调用者需要 `&dyn PublisherSink` 时使用。
    pub fn as_sink(&self) -> &dyn PublisherSink {
        self.inner.as_ref()
    }
}

pub(crate) fn map_sdk_error(
    protocol: Protocol,
    operation: crate::error::Operation,
    err: SdkError,
) -> ConnectorError {
    let connector_err: ConnectorError = err.into();
    // Ensure the protocol context is attached for variants that do not carry it.
    match connector_err {
        // `SdkError::Unavailable` maps to `Connect` with a default RTMP protocol in
        // `From<SdkError>`. Override the protocol when the handle knows a different one.
        ConnectorError::Connect {
            protocol: Protocol::Rtmp,
            endpoint,
            source,
        } if protocol != Protocol::Rtmp => ConnectorError::Connect {
            protocol,
            endpoint,
            source,
        },
        ConnectorError::Internal(msg) => ConnectorError::Protocol {
            protocol,
            operation,
            source: Box::new(std::io::Error::other(msg)),
        },
        ConnectorError::InvalidArgument(_) => connector_err,
        _ => connector_err,
    }
}

/// Pair returned by [`crate::open_in_memory_loopback`].
///
/// 由 [`crate::open_in_memory_loopback`] 返回的对。
pub struct LoopbackPair {
    pub publisher: PushHandle,
    pub subscriber: PullHandle,
    pub layer: crate::options::LoopbackLayer,
}

#[async_trait]
impl SubscriberSource for PullHandle {
    async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, SdkError> {
        self.inner.recv().await
    }

    async fn close(&mut self) -> Result<(), SdkError> {
        self.inner.close().await
    }

    fn id(&self) -> SubscriberId {
        self.inner.id()
    }

    fn tracks(&self) -> Vec<TrackInfo> {
        self.inner.tracks()
    }
}

impl PublisherSink for PushHandle {
    fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), SdkError> {
        self.inner.update_tracks(tracks)
    }

    fn push_frame(&self, frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError> {
        self.inner.push_frame(frame)
    }

    fn close(&self) -> Result<(), SdkError> {
        self.inner.close()
    }

    fn take_keyframe_requests(&self) -> u64 {
        self.inner.take_keyframe_requests()
    }
}
