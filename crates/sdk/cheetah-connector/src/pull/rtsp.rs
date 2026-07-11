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

    parse_rtsp_source_peer(url).map_err(|reason| ConnectorError::InvalidUrl {
        protocol: Protocol::Rtsp,
        url: url.to_string(),
        reason,
    })?;

    let mut rtsp_options = match &options.protocol {
        #[cfg(feature = "rtsp")]
        ProtocolPullExtras::Rtsp(opts) => opts.clone(),
        _ => RtspPullOptions::default(),
    };
    rtsp_options.subscriber_options = options.subscriber;

    let cancel = options.cancel.clone().unwrap_or_default().child_token();

    let subscriber = rtsp_open_pull(
        engine.runtime_api(),
        engine.publisher_api(),
        engine.stream_manager_api(),
        url,
        target_stream_key,
        cancel,
        rtsp_options,
    )
    .await
    .map_err(|e| map_sdk_error(Protocol::Rtsp, Operation::Open, e))?;

    Ok(PullHandle::new(Protocol::Rtsp, url.to_string(), subscriber))
}
