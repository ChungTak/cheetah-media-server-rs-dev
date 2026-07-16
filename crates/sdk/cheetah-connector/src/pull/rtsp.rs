//! RTSP pull adapter for the high-level connector.
//!
//! 高层 connector 的 RTSP 拉流适配器。

use std::sync::Arc;

use cheetah_engine::Engine;
use cheetah_rtsp_module::pull::{
    open_rtsp_pull as rtsp_open_pull, parse_rtsp_source_peer, parse_stream_key_from_uri,
    RtspPullOptions,
};

use crate::error::{ConnectorError, Operation};
use crate::handles::{map_sdk_error, PullHandle};
use crate::options::{ConnectorPullOptions, ProtocolPullExtras};
use crate::protocol::Protocol;

/// Open an RTSP pull subscriber for the given `url` and `options`.
///
/// The target engine stream key is derived from the RTSP URI path when possible.
/// Prefer [`open_rtsp_pull_to_stream`] when the destination stream is known
/// explicitly (e.g. proxy destination `MediaKey`).
///
/// 为给定 URL 和选项打开 RTSP 拉流订阅者。
pub async fn open_rtsp_pull(
    engine: Arc<Engine>,
    url: &str,
    options: ConnectorPullOptions,
) -> Result<PullHandle, ConnectorError> {
    let target_stream_key =
        parse_stream_key_from_uri(url).ok_or_else(|| ConnectorError::InvalidUrl {
            protocol: Protocol::Rtsp,
            url: url.to_string(),
            reason: "could not derive stream key from rtsp url".to_string(),
        })?;
    open_rtsp_pull_to_stream(
        engine.runtime_api(),
        engine.publisher_api(),
        engine.stream_manager_api(),
        url,
        target_stream_key,
        options,
    )
    .await
}

/// Open an RTSP pull that publishes into an explicit engine stream key.
///
/// 打开 RTSP 拉流并将媒体发布到指定的引擎流键。
pub async fn open_rtsp_pull_to_stream(
    runtime_api: Arc<dyn cheetah_runtime_api::RuntimeApi>,
    publisher_api: Arc<dyn cheetah_sdk::PublisherApi>,
    stream_manager_api: Arc<dyn cheetah_sdk::StreamManagerApi>,
    url: &str,
    target_stream_key: cheetah_sdk::StreamKey,
    options: ConnectorPullOptions,
) -> Result<PullHandle, ConnectorError> {
    if options.peer.is_none() {
        parse_rtsp_source_peer(url).map_err(|reason| ConnectorError::InvalidUrl {
            protocol: Protocol::Rtsp,
            url: url.to_string(),
            reason,
        })?;
    }

    let mut rtsp_options = match &options.protocol {
        ProtocolPullExtras::Rtsp(opts) => opts.clone(),
        _ => RtspPullOptions::default(),
    };
    rtsp_options.subscriber_options = options.subscriber;
    rtsp_options.peer = options.peer;

    let cancel = options.cancel.clone().unwrap_or_default().child_token();

    let subscriber = rtsp_open_pull(
        runtime_api,
        publisher_api,
        stream_manager_api,
        url,
        target_stream_key,
        cancel,
        rtsp_options,
    )
    .await
    .map_err(|e| map_sdk_error(Protocol::Rtsp, Operation::Open, e))?;

    Ok(PullHandle::new(Protocol::Rtsp, url.to_string(), subscriber))
}
