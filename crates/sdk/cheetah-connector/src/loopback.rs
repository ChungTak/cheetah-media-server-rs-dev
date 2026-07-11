use std::net::SocketAddr;
use std::sync::Arc;

use cheetah_codec::MonoTime;
use cheetah_engine::Engine;
use cheetah_sdk::{PublisherSink, StreamKey, SubscriberSource};

use crate::error::{ConnectorError, Operation};
use crate::handles::{map_sdk_error, LoopbackPair, PullHandle, PushHandle};
use crate::options::{
    ConnectorPullOptions, ConnectorPushOptions, LoopbackLayer, LoopbackOptions, LoopbackTopology,
    ProtocolPullExtras, ProtocolPushExtras, RtmpPushExtras,
};
use crate::protocol::{supports, Direction, Protocol};

#[cfg(feature = "webrtc")]
use async_trait::async_trait;
#[cfg(feature = "webrtc")]
use cheetah_sdk::SubscriberId;
#[cfg(feature = "webrtc")]
use cheetah_webrtc_media_loopback::MediaLoopbackHarness;

/// Open an in-memory loopback pair.
///
/// 打开一个内存 loopback 对。
///
/// The default topology is RTMP push + HTTP-FLV pull with `LoopbackLayer::ProtocolFraming`.
/// `LoopbackLayer::EngineOnlyBypassWire` can be requested to bypass the wire and
/// use the engine `StreamManager` directly.
///
/// 默认拓扑为 RTMP push + HTTP-FLV pull，使用 `LoopbackLayer::ProtocolFraming`。
/// 可以请求 `LoopbackLayer::EngineOnlyBypassWire` 来绕过网线和协议驱动，
/// 直接使用引擎 `StreamManager`。
pub async fn open_in_memory_loopback(
    engine: Arc<Engine>,
    options: LoopbackOptions,
) -> Result<LoopbackPair, ConnectorError> {
    if options.queue_capacity == 0 {
        return Err(ConnectorError::InvalidArgument(
            "loopback queue_capacity must be > 0".to_string(),
        ));
    }

    match options.topology {
        LoopbackTopology::Cross { push, pull } => match options.preferred_layer {
            LoopbackLayer::ProtocolFraming => {
                cross_protocol_loopback(engine, options, push, pull).await
            }
            LoopbackLayer::EngineOnlyBypassWire => {
                engine_only_loopback(engine, options, push, pull).await
            }
            _ => Err(ConnectorError::UnsupportedProtocol {
                protocol: push,
                direction: Direction::Push,
            }),
        },
        LoopbackTopology::SameProtocol { protocol } => match options.preferred_layer {
            LoopbackLayer::WebRtcMediaFixture if protocol == Protocol::WebRtc => {
                webrtc_media_loopback(engine, options).await
            }
            LoopbackLayer::EngineOnlyBypassWire => {
                engine_only_loopback(engine, options, protocol, protocol).await
            }
            _ => Err(ConnectorError::UnsupportedProtocol {
                protocol,
                direction: Direction::Push,
            }),
        },
    }
}

async fn cross_protocol_loopback(
    engine: Arc<Engine>,
    options: LoopbackOptions,
    push: Protocol,
    pull: Protocol,
) -> Result<LoopbackPair, ConnectorError> {
    if !supports(push, Direction::Push) || !supports(pull, Direction::Pull) {
        return Err(ConnectorError::UnsupportedProtocol {
            protocol: push,
            direction: Direction::Push,
        });
    }

    if !matches!((push, pull), (Protocol::Rtmp, Protocol::HttpFlv)) {
        return Err(ConnectorError::UnsupportedProtocol {
            protocol: push,
            direction: Direction::Push,
        });
    }

    let runtime_api = engine.runtime_api();
    let services = engine.service_registry_api();

    let timeout = 10_000_000_u64; // 10s
    let deadline = MonoTime::from_micros(runtime_api.now().as_micros().saturating_add(timeout));

    let (rtmp_addr, http_flv_addr) = loop {
        let rtmp = services
            .list_services()
            .into_iter()
            .find(|s| s.name == "rtmp");
        let http_flv = services
            .list_services()
            .into_iter()
            .find(|s| s.name == "http-flv");

        if let (Some(rtmp), Some(http_flv)) = (rtmp, http_flv) {
            let rtmp_addr = parse_endpoint_addr(&rtmp.endpoint)?;
            let http_flv_addr = parse_endpoint_addr(&http_flv.endpoint)?;
            break (rtmp_addr, http_flv_addr);
        }

        if runtime_api.now().as_micros() >= deadline.as_micros() {
            return Err(ConnectorError::Internal(
                "rtmp/http-flv loopback endpoints not available in service registry".to_string(),
            ));
        }

        let sleep_deadline = MonoTime::from_micros(
            runtime_api.now().as_micros().saturating_add(100_000), // 100ms
        );
        let mut timer = runtime_api.sleep_until(sleep_deadline);
        timer.wait().await;
    };

    let stream_name = &options.stream_name;
    let pull_url = format!("http://{http_flv_addr}/live/{stream_name}.flv");
    let push_url = format!("rtmp://{rtmp_addr}/live/{stream_name}");

    let cancel = options.cancel.clone().unwrap_or_default().child_token();

    let pull_options = ConnectorPullOptions {
        subscriber: options.subscriber.clone(),
        cancel: Some(cancel.clone()),
        protocol: ProtocolPullExtras::HttpFlv {
            reconnect: None,
            read_limits: None,
            buffer_size: Some(options.queue_capacity),
        },
    };

    let push_options = ConnectorPushOptions {
        publisher: options.publisher.clone(),
        cancel: Some(cancel),
        tracks: options.tracks.clone(),
        protocol: ProtocolPushExtras::Rtmp(RtmpPushExtras {
            command_queue_capacity: Some(options.queue_capacity),
            write_queue_capacity: Some(options.queue_capacity),
            read_buffer_size: None,
            chunk_size: None,
            ack_window_size: None,
        }),
    };

    let subscriber =
        crate::pull::http_flv::open_http_flv_pull(engine.clone(), &pull_url, pull_options).await?;

    let publisher =
        crate::push::rtmp::open_rtmp_push(engine.clone(), &push_url, push_options).await?;

    Ok(LoopbackPair {
        publisher,
        subscriber,
        layer: LoopbackLayer::ProtocolFraming,
    })
}

async fn engine_only_loopback(
    engine: Arc<Engine>,
    options: LoopbackOptions,
    push: Protocol,
    pull: Protocol,
) -> Result<LoopbackPair, ConnectorError> {
    let stream_manager = engine.stream_manager_api();
    let stream_key = StreamKey::new("live", &options.stream_name);
    let url = format!("engine://live/{}", options.stream_name);

    let publisher = stream_manager
        .open_publisher(stream_key.clone(), options.publisher.clone())
        .await
        .map_err(|e| map_sdk_error(push, Operation::Open, e))?;

    let (ready_tx, ready_rx) = tokio::sync::watch::channel(true);
    let ready = Arc::new(ready_rx);
    drop(ready_tx);

    let publisher = PushHandle::new(push, url.clone(), publisher, ready);

    let mut subscriber_options = options.subscriber.clone();
    if options.queue_capacity > 0 {
        subscriber_options.queue_capacity = options.queue_capacity;
    }

    let subscriber = stream_manager
        .open_subscriber(stream_key, subscriber_options)
        .await
        .map_err(|e| map_sdk_error(pull, Operation::Open, e))?;

    let subscriber = PullHandle::new(pull, url, subscriber);

    publisher.update_tracks(options.tracks)?;

    Ok(LoopbackPair {
        publisher,
        subscriber,
        layer: LoopbackLayer::EngineOnlyBypassWire,
    })
}

fn parse_endpoint_addr(endpoint: &str) -> Result<SocketAddr, ConnectorError> {
    let Some((_scheme, rest)) = endpoint.split_once("://") else {
        return Err(ConnectorError::Internal(format!(
            "invalid endpoint: {endpoint}"
        )));
    };
    rest.parse::<SocketAddr>().map_err(|err| {
        ConnectorError::Internal(format!("failed to parse endpoint {endpoint}: {err}"))
    })
}

// ----------------------------------------------------------------------
// WebRTC same-protocol in-process media loopback
// ----------------------------------------------------------------------

#[cfg(feature = "webrtc")]
async fn webrtc_media_loopback(
    engine: Arc<Engine>,
    options: LoopbackOptions,
) -> Result<LoopbackPair, ConnectorError> {
    let runtime = engine.runtime_api();
    let stream_manager = engine.stream_manager_api();
    let stream_key = StreamKey::new("live", &options.stream_name);
    let url = format!("webrtc://loopback/{}", options.stream_name);
    let cancel = options.cancel.clone().unwrap_or_default();

    let harness = MediaLoopbackHarness::new(runtime, stream_manager, stream_key, cancel)
        .await
        .map_err(|e| map_sdk_error(Protocol::WebRtc, Operation::Open, e))?;

    let shared = std::sync::Arc::new(futures::lock::Mutex::new(harness));
    let push = Box::new(MediaLoopbackHarnessHandle(shared.clone()));
    let pull = Box::new(MediaLoopbackHarnessHandle(shared));

    let (ready_tx, ready_rx) = tokio::sync::watch::channel(true);
    let ready = Arc::new(ready_rx);
    drop(ready_tx);

    Ok(LoopbackPair {
        publisher: PushHandle::new(Protocol::WebRtc, url.clone(), push, ready),
        subscriber: PullHandle::new(Protocol::WebRtc, url, pull),
        layer: LoopbackLayer::WebRtcMediaFixture,
    })
}

#[cfg(feature = "webrtc")]
struct MediaLoopbackHarnessHandle(
    std::sync::Arc<futures::lock::Mutex<cheetah_webrtc_media_loopback::MediaLoopbackHarness>>,
);

#[cfg(feature = "webrtc")]
impl PublisherSink for MediaLoopbackHarnessHandle {
    fn update_tracks(
        &self,
        tracks: Vec<cheetah_codec::TrackInfo>,
    ) -> Result<(), cheetah_sdk::SdkError> {
        futures::executor::block_on(async { self.0.lock().await.update_tracks(tracks) })
    }

    fn push_frame(
        &self,
        frame: std::sync::Arc<cheetah_codec::AVFrame>,
    ) -> Result<cheetah_sdk::DispatchResult, cheetah_sdk::SdkError> {
        futures::executor::block_on(async { self.0.lock().await.push_frame(frame) })
    }

    fn close(&self) -> Result<(), cheetah_sdk::SdkError> {
        futures::executor::block_on(async { self.0.lock().await.close_sink() })
    }

    fn take_keyframe_requests(&self) -> u64 {
        futures::executor::block_on(async { self.0.lock().await.take_keyframe_requests() })
    }
}

#[cfg(feature = "webrtc")]
#[async_trait]
impl SubscriberSource for MediaLoopbackHarnessHandle {
    async fn recv(
        &mut self,
    ) -> Result<Option<std::sync::Arc<cheetah_codec::AVFrame>>, cheetah_sdk::SdkError> {
        self.0.lock().await.recv().await
    }

    async fn close(&mut self) -> Result<(), cheetah_sdk::SdkError> {
        self.0.lock().await.close().await
    }

    fn id(&self) -> SubscriberId {
        futures::executor::block_on(async { self.0.lock().await.id() })
    }

    fn tracks(&self) -> Vec<cheetah_codec::TrackInfo> {
        futures::executor::block_on(async { self.0.lock().await.tracks() })
    }
}

#[cfg(not(feature = "webrtc"))]
async fn webrtc_media_loopback(
    _engine: Arc<Engine>,
    _options: LoopbackOptions,
) -> Result<LoopbackPair, ConnectorError> {
    Err(ConnectorError::FeatureDisabled {
        protocol: Protocol::WebRtc,
        feature: "webrtc",
    })
}
