use std::net::SocketAddr;
use std::sync::Arc;

use cheetah_codec::MonoTime;
use cheetah_engine::Engine;

use crate::error::ConnectorError;
use crate::handles::LoopbackPair;
use crate::options::{
    ConnectorPullOptions, ConnectorPushOptions, LoopbackLayer, LoopbackOptions, LoopbackTopology,
    ProtocolPullExtras, ProtocolPushExtras,
};
use crate::protocol::{supports, Direction, Protocol};

#[cfg(feature = "webrtc")]
use async_trait::async_trait;
#[cfg(feature = "webrtc")]
use cheetah_sdk::{PublisherSink, SubscriberId, SubscriberSource};

/// Open an in-memory loopback pair.
///
/// 打开一个内存 loopback 对。
///
/// The default topology is RTMP push + HTTP-FLV pull, which is the recommended
/// first L1 path. WebRTC same-protocol fixture loopback is also supported.
///
/// 默认拓扑为 RTMP push + HTTP-FLV pull，这是推荐的第一个 L1 路径。
/// 也支持 WebRTC 同协议 fixture loopback。
pub async fn open_in_memory_loopback(
    engine: Arc<Engine>,
    options: LoopbackOptions,
) -> Result<LoopbackPair, ConnectorError> {
    match options.topology {
        LoopbackTopology::Cross { push, pull } => {
            cross_protocol_loopback(engine, options, push, pull).await
        }
        LoopbackTopology::SameProtocol { protocol } => match protocol {
            Protocol::WebRtc => webrtc_media_loopback(engine, options).await,
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

    let pull_options = ConnectorPullOptions {
        subscriber: options.subscriber.clone(),
        cancel: Some(options.cancel.child_token()),
        protocol: ProtocolPullExtras::HttpFlv { reconnect: None },
    };

    let push_options = ConnectorPushOptions {
        publisher: options.publisher.clone(),
        cancel: Some(options.cancel.child_token()),
        tracks: options.tracks.clone(),
        protocol: ProtocolPushExtras::None,
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
    use crate::handles::{map_sdk_error, PullHandle, PushHandle};

    let runtime = engine.runtime_api();
    let stream_manager = engine.stream_manager_api();
    let stream_key = cheetah_sdk::StreamKey::new("live", &options.stream_name);
    let url = format!("webrtc://loopback/{}", options.stream_name);

    let harness = cheetah_webrtc_module::testing::media_loopback::MediaLoopbackHarness::new(
        runtime,
        stream_manager,
        stream_key,
        options.cancel.clone(),
    )
    .await
    .map_err(|e| map_sdk_error(Protocol::WebRtc, crate::error::Operation::Open, e))?;

    let shared = std::sync::Arc::new(futures::lock::Mutex::new(harness));
    let push = Box::new(MediaLoopbackHarnessHandle(shared.clone()));
    let pull = Box::new(MediaLoopbackHarnessHandle(shared));

    Ok(LoopbackPair {
        publisher: PushHandle::new(Protocol::WebRtc, url.clone(), push),
        subscriber: PullHandle::new(Protocol::WebRtc, url, pull),
        layer: LoopbackLayer::WebRtcMediaFixture,
    })
}

#[cfg(feature = "webrtc")]
struct MediaLoopbackHarnessHandle(
    std::sync::Arc<
        futures::lock::Mutex<cheetah_webrtc_module::testing::media_loopback::MediaLoopbackHarness>,
    >,
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
