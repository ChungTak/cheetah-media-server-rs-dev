//! Runtime pull connector for RTSP used by the high-level SDK.
//!
//! 供高层 SDK 使用的 RTSP 拉流运行时连接器。

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cheetah_codec::TrackInfo;
use cheetah_rtsp_driver_tokio::{
    start_http_tunnel_client, start_tcp_client, RtspClientConfig, RtspClientCredentials,
    RtspClientHandle, RtspMethod,
};
use cheetah_sdk::{
    CancellationToken, PublisherApi, PublisherOptions, RuntimeApi, SdkError, StreamKey,
    StreamManagerApi, SubscriberId, SubscriberOptions, SubscriberSource,
};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::{info, warn};

use crate::config::{RtspAlertThresholds, RtspHeartbeatMode, RtspModuleConfig, RtspPullTransport};
use crate::module::client_pull::{
    build_pull_outbound_auth_state, invert_track_controls, rtsp_url_path,
    send_request_with_auth_retry, setup_pull_tracks_and_play, supported_pull_transports,
    tracks_to_map, wait_client_connected, wait_pull_session_end, PullSelectedTransport,
    PullSetupCompletion, PullSetupContext,
};
use crate::module::publish::build_pull_publish_session;
use crate::sdp::parse_announce_sdp;

pub use crate::media::parse_stream_key_from_uri;
pub use crate::module::client_pull::parse_rtsp_source_peer;

/// Options for an on-demand RTSP pull through the high-level connector.
///
/// 通过高层 connector 按需 RTSP 拉流的选项。
#[derive(Debug, Clone)]
pub struct RtspPullOptions {
    /// Transport preference order for the RTSP session.
    ///
    /// 默认仅使用 TCP interleaved，因为 connector 侧通常不需要 UDP。
    pub transport_preference: Vec<RtspPullTransport>,
    /// Optional digest/basic credentials for the RTSP source.
    pub credentials: Option<RtspClientCredentials>,
    /// Request timeout for RTSP round-trips.
    pub request_timeout: Duration,
    /// Keepalive mode for the RTSP session.
    pub heartbeat_mode: RtspHeartbeatMode,
    /// Alert thresholds used when building the internal publish session.
    pub alert_thresholds: RtspAlertThresholds,
    /// Options for the subscriber that is returned to the caller.
    pub subscriber_options: SubscriberOptions,
    /// Options for acquiring the internal publisher lease.
    pub publisher_options: PublisherOptions,
    /// Pre-resolved peer address. When `Some`, the RTSP client connects to this
    /// address instead of re-resolving the URL hostname, which prevents DNS rebinding.
    pub peer: Option<SocketAddr>,
}

impl Default for RtspPullOptions {
    fn default() -> Self {
        Self {
            transport_preference: vec![RtspPullTransport::TcpInterleaved],
            credentials: None,
            request_timeout: Duration::from_secs(5),
            heartbeat_mode: RtspHeartbeatMode::default(),
            alert_thresholds: RtspAlertThresholds::default(),
            subscriber_options: SubscriberOptions::default(),
            publisher_options: PublisherOptions::default(),
            peer: None,
        }
    }
}

/// Open an RTSP pull subscriber, publish the incoming media into the engine, and
/// return a `SubscriberSource` that receives the normalized `AVFrame`s.
///
/// 打开 RTSP 拉流订阅者，将入站媒体发布到引擎，并返回接收规范化 `AVFrame` 的 `SubscriberSource`。
pub async fn open_rtsp_pull(
    runtime_api: Arc<dyn RuntimeApi>,
    publisher_api: Arc<dyn PublisherApi>,
    stream_manager_api: Arc<dyn StreamManagerApi>,
    source_url: &str,
    target_stream_key: StreamKey,
    cancel: CancellationToken,
    options: RtspPullOptions,
) -> Result<Box<dyn SubscriberSource>, SdkError> {
    let peer = match options.peer {
        Some(peer) => peer,
        None => parse_rtsp_source_peer(source_url).map_err(|reason| {
            SdkError::InvalidArgument(format!("invalid rtsp source url: {reason}"))
        })?,
    };

    let transports = supported_pull_transports(&options.transport_preference)
        .map_err(SdkError::InvalidArgument)?;
    let selected_transport = transports
        .into_iter()
        .next()
        .ok_or_else(|| SdkError::InvalidArgument("no supported rtsp transport".to_string()))?;

    let request_timeout = options.request_timeout;
    let mut client = start_pull_client(
        runtime_api.clone(),
        peer,
        source_url,
        &target_stream_key,
        selected_transport,
        cancel.child_token(),
    )
    .map_err(|err| SdkError::Unavailable(format!("start rtsp pull client failed: {err}")))?;

    let mut auth = build_pull_outbound_auth_state(options.credentials);
    let mut cseq = 1_u32;

    let setup_result = async {
        wait_client_connected(&runtime_api, &mut client, &cancel, request_timeout)
            .await
            .map_err(SdkError::InvalidArgument)?;

        let options_response = send_request_with_auth_retry(
            &runtime_api,
            &mut client,
            &mut auth,
            RtspMethod::Options,
            source_url,
            &mut cseq,
            &[],
            &[],
            &cancel,
            request_timeout,
        )
        .await
        .map_err(SdkError::InvalidArgument)?;
        if options_response.status_code != 200 {
            return Err(SdkError::Unavailable(format!(
                "OPTIONS failed with status {}",
                options_response.status_code
            )));
        }

        let describe_response = send_request_with_auth_retry(
            &runtime_api,
            &mut client,
            &mut auth,
            RtspMethod::Describe,
            source_url,
            &mut cseq,
            &[("Accept", "application/sdp")],
            &[],
            &cancel,
            request_timeout,
        )
        .await
        .map_err(SdkError::InvalidArgument)?;
        if describe_response.status_code != 200 {
            return Err(SdkError::Unavailable(format!(
                "DESCRIBE failed with status {}",
                describe_response.status_code
            )));
        }

        let describe_body = std::str::from_utf8(describe_response.body.as_ref()).map_err(|_| {
            SdkError::InvalidArgument("DESCRIBE response body is not valid utf-8".to_string())
        })?;
        let content_base = describe_response
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("Content-Base"))
            .map(|h| h.value.trim().trim_end_matches('/').to_string());
        let base_url = content_base.as_deref().unwrap_or(source_url);
        let (tracks, control_map) = parse_announce_sdp(describe_body).map_err(|err| {
            SdkError::InvalidArgument(format!("parse DESCRIBE SDP failed: {err}"))
        })?;
        let track_controls = invert_track_controls(&tracks, &control_map);

        let (lease, sink) = publisher_api
            .acquire_publisher(target_stream_key.clone(), options.publisher_options)
            .await?;
        if let Err(err) = sink.update_tracks(tracks.clone()) {
            let _ = publisher_api.release_publisher(&lease).await;
            return Err(SdkError::InvalidArgument(format!(
                "update pull tracks failed: {err}"
            )));
        }

        let config = RtspModuleConfig {
            alert_thresholds: options.alert_thresholds,
            ..RtspModuleConfig::default()
        };
        let publish = build_pull_publish_session(
            &config,
            cancel.child_token(),
            lease,
            sink,
            tracks_to_map(&tracks),
        );

        let setup_completion = match setup_pull_tracks_and_play(
            &mut client,
            &tracks,
            &track_controls,
            PullSetupContext {
                runtime_api: &runtime_api,
                source_url,
                base_url,
                peer,
                transport: selected_transport,
                cancel: &cancel,
                request_timeout,
                auth: &mut auth,
                start_cseq: cseq,
            },
        )
        .await
        {
            Ok(completion) => completion,
            Err(err) => {
                let _ = publisher_api.release_publisher(&publish.lease).await;
                let _ = publish.sink.close();
                return Err(SdkError::Unavailable(format!(
                    "setup pull tracks failed: {err}"
                )));
            }
        };

        Ok::<_, SdkError>((setup_completion, tracks, publish))
    }
    .await;

    let (setup_completion, tracks, mut publish) = match setup_result {
        Ok((completion, tracks, publish)) => (completion, tracks, publish),
        Err(err) => {
            client.shutdown();
            return Err(err);
        }
    };

    let PullSetupCompletion {
        setup,
        mut udp_task_handles,
    } = setup_completion;

    info!(
        stream = %target_stream_key,
        track_count = tracks.len(),
        setup_tracks = setup.interleaved_rtp_channels.len(),
        "rtsp pull control-plane prepared tracks and started RTP ingest"
    );

    let wait_cancel = cancel.child_token();
    let wait_cancel_for_source = wait_cancel.clone();
    let wait_target_stream_key = target_stream_key.clone();
    let wait_runtime_api = runtime_api.clone();
    let wait_source_url = source_url.to_string();
    let wait_publisher_api = publisher_api.clone();
    let wait_setup = setup;
    let heartbeat_mode = options.heartbeat_mode;
    let keep_cancel = cancel.clone();
    let _join = runtime_api.spawn(Box::pin(async move {
        let _keep_cancel = keep_cancel;
        let result = wait_pull_session_end(
            &mut client,
            &wait_cancel,
            &mut publish,
            &wait_setup,
            &wait_source_url,
            &wait_runtime_api,
            &mut auth,
            heartbeat_mode,
        )
        .await;
        for join in udp_task_handles.drain(..) {
            join.abort();
            let _ = join.wait().await;
        }
        client.shutdown();
        if let Err(err) = publish.sink.close() {
            warn!(stream = %wait_target_stream_key, "close pull sink failed: {err}");
        }
        if let Err(err) = wait_publisher_api.release_publisher(&publish.lease).await {
            warn!(stream = %wait_target_stream_key, "release pull lease failed: {err}");
        }
        if let Err(err) = result {
            warn!(stream = %wait_target_stream_key, "rtsp pull session ended: {err}");
        }
    }));

    let subscriber = match stream_manager_api
        .open_subscriber(target_stream_key, options.subscriber_options)
        .await
    {
        Ok(sub) => sub,
        Err(err) => {
            wait_cancel_for_source.cancel();
            return Err(err);
        }
    };

    Ok(Box::new(RtspPullSubscriberSource {
        inner: subscriber,
        cancel: wait_cancel_for_source,
        tracks,
    }))
}

fn start_pull_client(
    runtime_api: Arc<dyn RuntimeApi>,
    peer: SocketAddr,
    source_url: &str,
    target_stream_key: &StreamKey,
    transport: PullSelectedTransport,
    cancel: CancellationToken,
) -> io::Result<RtspClientHandle> {
    match transport {
        PullSelectedTransport::TcpInterleaved | PullSelectedTransport::Udp => {
            start_tcp_client(runtime_api, peer, RtspClientConfig::default(), cancel)
        }
        PullSelectedTransport::HttpTunnel => {
            let path = rtsp_url_path(source_url)
                .map_err(|reason| io::Error::new(io::ErrorKind::InvalidInput, reason))?;
            let session_cookie = format!(
                "cheetah-pull-{}-{}-",
                target_stream_key.namespace, target_stream_key.path
            );
            start_http_tunnel_client(
                runtime_api,
                peer,
                path,
                session_cookie,
                RtspClientConfig::default(),
                cancel,
            )
        }
    }
}

struct RtspPullSubscriberSource {
    inner: Box<dyn SubscriberSource>,
    cancel: CancellationToken,
    tracks: Vec<TrackInfo>,
}

impl Drop for RtspPullSubscriberSource {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[async_trait]
impl SubscriberSource for RtspPullSubscriberSource {
    async fn recv(&mut self) -> Result<Option<Arc<cheetah_codec::AVFrame>>, SdkError> {
        let cancel_fut = self.cancel.cancelled().fuse();
        let recv_fut = self.inner.recv().fuse();
        pin_mut!(cancel_fut, recv_fut);
        select_biased! {
            _ = cancel_fut => Ok(None),
            result = recv_fut => result,
        }
    }

    async fn close(&mut self) -> Result<(), SdkError> {
        self.cancel.cancel();
        self.inner.close().await
    }

    fn id(&self) -> SubscriberId {
        self.inner.id()
    }

    fn tracks(&self) -> Vec<TrackInfo> {
        self.tracks.clone()
    }
}
