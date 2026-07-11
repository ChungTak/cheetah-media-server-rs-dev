use std::sync::Arc;

use cheetah_engine::Engine;
use cheetah_http_flv_module::pull::streaming::{
    open_http_flv_subscriber, HttpFlvSubscriberOptions,
};

use crate::error::ConnectorError;
use crate::handles::PullHandle;
use crate::options::{ConnectorPullOptions, ProtocolPullExtras};
use crate::protocol::Protocol;

/// Open an HTTP-FLV pull handle.
///
/// 打开 HTTP-FLV pull 句柄。
pub async fn open_http_flv_pull(
    engine: Arc<Engine>,
    url: &str,
    options: ConnectorPullOptions,
) -> Result<PullHandle, ConnectorError> {
    let reconnect = match options.protocol {
        ProtocolPullExtras::HttpFlv { reconnect } => reconnect,
        _ => None,
    };

    let subscriber_options = HttpFlvSubscriberOptions {
        read_limits: Default::default(),
        reconnect,
        buffer_size: 64,
        cancel: options.cancel,
    };

    let subscriber = open_http_flv_subscriber(engine.runtime_api(), url, subscriber_options)
        .await
        .map_err(ConnectorError::from)?;

    Ok(PullHandle::new(
        Protocol::HttpFlv,
        url.to_string(),
        Box::new(subscriber),
    ))
}
