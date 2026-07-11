use crate::protocol::Protocol;

#[cfg(feature = "rtsp")]
use cheetah_rtsp_module::pull::RtspPullOptions;

/// Options for opening a pull subscriber through the connector.
///
/// 通过 connector 打开拉流订阅者的选项。
#[derive(Debug, Clone, Default)]
pub struct ConnectorPullOptions {
    pub subscriber: cheetah_sdk::SubscriberOptions,
    pub cancel: Option<cheetah_runtime_api::CancellationToken>,
    pub protocol: ProtocolPullExtras,
}

/// Per-protocol pull extras.
///
/// 按协议区分的拉流额外选项。
#[derive(Debug, Clone, Default)]
pub enum ProtocolPullExtras {
    /// No protocol-specific extras.
    ///
    /// 没有协议特定的额外选项。
    #[default]
    None,

    /// HTTP-FLV pull-specific options.
    ///
    /// HTTP-FLV 拉流相关选项。
    #[cfg(feature = "http-flv")]
    HttpFlv {
        /// Optional reconnect policy for the HTTP-FLV streaming driver.
        reconnect: Option<cheetah_http_flv_module::pull::streaming::ReconnectPolicy>,
        /// Optional per-protocol read limits.
        read_limits: Option<cheetah_http_flv_module::pull::PullReadLimits>,
        /// Optional buffer size for the incoming frame channel.
        buffer_size: Option<usize>,
    },

    /// RTSP pull-specific options.
    ///
    /// RTSP 拉流相关选项。
    #[cfg(feature = "rtsp")]
    Rtsp(RtspPullOptions),
}

/// Options for opening a push publisher through the connector.
///
/// 通过 connector 打开推流发布者的选项。
#[derive(Debug, Clone, Default)]
pub struct ConnectorPushOptions {
    pub publisher: cheetah_sdk::PublisherOptions,
    pub cancel: Option<cheetah_runtime_api::CancellationToken>,
    pub tracks: Vec<cheetah_codec::TrackInfo>,
    pub protocol: ProtocolPushExtras,
}

/// Per-protocol push extras.
///
/// 按协议区分的推流额外选项。
#[derive(Debug, Clone, Default)]
pub enum ProtocolPushExtras {
    /// No protocol-specific extras.
    ///
    /// 没有协议特定的额外选项。
    #[default]
    None,

    /// RTMP push-specific options.
    ///
    /// RTMP 推流相关选项。
    #[cfg(feature = "rtmp")]
    Rtmp(RtmpPushExtras),
}

/// RTMP push-specific options that are forwarded to the RTMP client driver.
///
/// RTMP 推流特有选项，会透传给 RTMP 客户端驱动。
#[cfg(feature = "rtmp")]
#[derive(Debug, Clone, Default)]
pub struct RtmpPushExtras {
    /// Capacity of the internal command queue between the connector and the driver.
    pub command_queue_capacity: Option<usize>,
    /// Capacity of the TCP write queue.
    pub write_queue_capacity: Option<usize>,
    /// Size of the TCP read buffer.
    pub read_buffer_size: Option<usize>,
    /// RTMP chunk size used for outgoing messages.
    pub chunk_size: Option<usize>,
    /// RTMP acknowledgement window size.
    pub ack_window_size: Option<usize>,
}

/// How an in-memory loopback should be routed.
///
/// 内存 loopback 的路由方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopbackTopology {
    /// Cross-protocol loopback: push to `push`, pull from `pull`.
    ///
    /// 跨协议 loopback：从 `push` 推，从 `pull` 拉。
    Cross { push: Protocol, pull: Protocol },
    /// Same-protocol loopback: push and pull on the same protocol.
    ///
    /// 同协议 loopback：在同一协议上推拉。
    SameProtocol { protocol: Protocol },
}

impl Default for LoopbackTopology {
    fn default() -> Self {
        Self::Cross {
            push: Protocol::Rtmp,
            pull: Protocol::HttpFlv,
        }
    }
}

/// Which layer the loopback should bypass to.
///
/// Loopback 应 bypass 到哪一层。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopbackLayer {
    /// Full protocol framing (default). For the default cross-protocol pair this
    /// runs RTMP push over a localhost TCP server and HTTP-FLV pull over the
    /// corresponding localhost TCP server.
    ///
    /// 完整协议帧层（默认）。对于默认的跨协议组合，会走 RTMP 推流到 localhost TCP
    /// 服务器，并从对应的 HTTP-FLV localhost TCP 服务器拉流。
    #[default]
    ProtocolFraming,
    /// Bypass the wire and the protocol drivers; use the engine `StreamManager` directly.
    ///
    /// 绕过网线和协议驱动，直接使用引擎 `StreamManager`。
    EngineOnlyBypassWire,
    /// WebRTC media loopback fixture (WHIP/WHEP player through the engine).
    ///
    /// WebRTC 媒体 loopback fixture（通过引擎的 WHIP/WHEP player）。
    WebRtcMediaFixture,
    /// WebRTC signaling-only loopback (not yet implemented).
    ///
    /// WebRTC 仅信令 loopback（尚未实现）。
    WebRtcSignalingOnly,
    /// WebRTC local UDP loopback (not yet implemented).
    ///
    /// WebRTC 本地 UDP loopback（尚未实现）。
    WebRtcLocalUdp,
}

/// Options for `RuntimeConnector::open_in_memory_loopback`.
///
/// `RuntimeConnector::open_in_memory_loopback` 的选项。
#[derive(Debug, Clone)]
pub struct LoopbackOptions {
    pub stream_name: String,
    pub topology: LoopbackTopology,
    pub preferred_layer: LoopbackLayer,
    pub tracks: Vec<cheetah_codec::TrackInfo>,
    pub subscriber: cheetah_sdk::SubscriberOptions,
    pub publisher: cheetah_sdk::PublisherOptions,
    pub cancel: Option<cheetah_runtime_api::CancellationToken>,
    pub queue_capacity: usize,
}

impl Default for LoopbackOptions {
    fn default() -> Self {
        Self {
            stream_name: String::new(),
            topology: LoopbackTopology::default(),
            preferred_layer: LoopbackLayer::default(),
            tracks: Vec::new(),
            subscriber: cheetah_sdk::SubscriberOptions::default(),
            publisher: cheetah_sdk::PublisherOptions::default(),
            cancel: None,
            queue_capacity: 150,
        }
    }
}
