use std::sync::Arc;

use cheetah_engine::Engine;
use cheetah_http_flv_module::pull::streaming::{
    open_http_flv_subscriber, HttpFlvSubscriberOptions,
};
use cheetah_runtime_api::CancellationToken;

use crate::error::ConnectorError;
use crate::handles::PullHandle;
use crate::options::{ConnectorPullOptions, ProtocolPullExtras};
use crate::protocol::Protocol;

/// Maximum number of frames buffered inside the connector pull handle.
const DEFAULT_BUFFER_SIZE: usize = 64;

/// Open an HTTP-FLV pull subscriber and wrap it in a `PullHandle`.
///
/// 打开 HTTP-FLV 拉流订阅者并包装为 `PullHandle`。
pub async fn open_http_flv_pull(
    engine: Arc<Engine>,
    endpoint: &str,
    options: ConnectorPullOptions,
) -> Result<PullHandle, ConnectorError> {
    open_http_flv_pull_with_runtime(engine.runtime_api(), endpoint, options).await
}

/// Open an HTTP-FLV pull using only a runtime handle (no full `Engine` required).
///
/// 仅使用 runtime 句柄打开 HTTP-FLV 拉流（不需要完整 `Engine`）。
pub async fn open_http_flv_pull_with_runtime(
    runtime_api: Arc<dyn cheetah_runtime_api::RuntimeApi>,
    endpoint: &str,
    options: ConnectorPullOptions,
) -> Result<PullHandle, ConnectorError> {
    let (reconnect, read_limits, buffer_size) = match options.protocol {
        ProtocolPullExtras::HttpFlv {
            reconnect,
            read_limits,
            buffer_size,
        } => (reconnect, read_limits, buffer_size),
        _ => (None, None, None),
    };

    let buffer_size = buffer_size
        .or(Some(options.subscriber.queue_capacity))
        .unwrap_or(DEFAULT_BUFFER_SIZE);

    let http_flv_options = HttpFlvSubscriberOptions {
        cancel: options.cancel.as_ref().map(CancellationToken::child_token),
        read_limits: read_limits.unwrap_or_default(),
        buffer_size,
        reconnect,
        peer: options.peer,
    };

    let endpoint = endpoint.to_string();
    let subscriber = open_http_flv_subscriber(runtime_api, &endpoint, http_flv_options).await?;

    Ok(PullHandle::new(
        Protocol::HttpFlv,
        endpoint,
        Box::new(subscriber),
    ))
}
