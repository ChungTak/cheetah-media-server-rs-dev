use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_codec::{AVFrame, TrackInfo};
use cheetah_sdk::{DispatchResult, PublisherSink, SdkError, SubscriberId, SubscriberSource};

use crate::error::{CloseReason, ConnectorError};
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
    ready: Arc<tokio::sync::watch::Receiver<bool>>,
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
    #[cfg(any(feature = "rtmp", feature = "webrtc"))]
    pub(crate) fn new(
        protocol: Protocol,
        url: String,
        inner: Box<dyn PublisherSink>,
        ready: Arc<tokio::sync::watch::Receiver<bool>>,
    ) -> Self {
        Self {
            protocol,
            url,
            inner,
            ready,
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
        let mut ready = (*self.ready).clone();
        if *ready.borrow() {
            return Ok(());
        }
        ready.changed().await.map_err(|_| ConnectorError::Closed {
            protocol: self.protocol,
            reason: CloseReason::Error("publish readiness channel dropped".to_string()),
        })?;
        if *ready.borrow() {
            Ok(())
        } else {
            Err(ConnectorError::Internal(
                "publish readiness channel reported false".to_string(),
            ))
        }
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
    match err {
        SdkError::InvalidArgument(msg) => ConnectorError::InvalidArgument(msg),
        SdkError::NotFound(msg) => ConnectorError::InvalidArgument(msg),
        SdkError::AlreadyExists(msg) => ConnectorError::InvalidArgument(msg),
        SdkError::Conflict(msg) => ConnectorError::InvalidArgument(msg),
        SdkError::Unavailable(msg) => ConnectorError::Connect {
            protocol,
            endpoint: msg.clone(),
            source: Box::new(std::io::Error::other(msg)),
        },
        SdkError::Internal(msg) => ConnectorError::Protocol {
            protocol,
            operation,
            source: Box::new(std::io::Error::other(msg)),
        },
    }
}

/// Pair returned by [`crate::open_in_memory_loopback`].
///
/// 由 [`crate::open_in_memory_loopback`] 返回的对。
#[derive(Debug)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Operation;

    #[test]
    fn map_sdk_error_unavailable_includes_protocol() {
        let err = SdkError::Unavailable("down".to_string());
        let mapped = map_sdk_error(Protocol::HttpFlv, Operation::Connect, err);
        assert!(matches!(
            mapped,
            ConnectorError::Connect {
                protocol: Protocol::HttpFlv,
                ..
            }
        ));
        assert!(mapped.retryable());
    }

    #[test]
    fn map_sdk_error_internal_includes_protocol_and_operation() {
        let err = SdkError::Internal("boom".to_string());
        let mapped = map_sdk_error(Protocol::Rtmp, Operation::Publish, err);
        assert!(matches!(
            mapped,
            ConnectorError::Protocol {
                protocol: Protocol::Rtmp,
                operation: Operation::Publish,
                ..
            }
        ));
        assert!(!mapped.retryable());
    }

    #[test]
    fn map_sdk_error_invalid_argument_passthrough() {
        let err = SdkError::InvalidArgument("bad".to_string());
        let mapped = map_sdk_error(Protocol::WebRtc, Operation::Open, err);
        assert!(matches!(mapped, ConnectorError::InvalidArgument(_)));
    }
}
