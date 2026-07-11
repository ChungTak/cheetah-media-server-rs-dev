use cheetah_codec::TrackInfo;
use cheetah_runtime_api::CancellationToken;
use cheetah_sdk::{PublisherOptions, SubscriberOptions};

/// Protocol-specific extras for pull operations.
///
/// pull 操作的协议特定扩展。
#[derive(Debug, Clone, Default)]
pub enum ProtocolPullExtras {
    #[default]
    None,
    #[cfg(feature = "http-flv")]
    HttpFlv {
        reconnect: Option<cheetah_http_flv_module::pull::streaming::ReconnectPolicy>,
    },
}

/// Protocol-specific extras for push operations.
///
/// push 操作的协议特定扩展。
#[derive(Debug, Clone, Default)]
pub enum ProtocolPushExtras {
    #[default]
    None,
}

/// Options for opening a pull handle through the connector.
///
/// 通过 connector 打开 pull 句柄的选项。
#[derive(Debug, Clone, Default)]
pub struct ConnectorPullOptions {
    pub subscriber: SubscriberOptions,
    pub cancel: Option<CancellationToken>,
    pub protocol: ProtocolPullExtras,
}

/// Options for opening a push handle through the connector.
///
/// 通过 connector 打开 push 句柄的选项。
#[derive(Debug, Clone, Default)]
pub struct ConnectorPushOptions {
    pub publisher: PublisherOptions,
    pub cancel: Option<CancellationToken>,
    pub tracks: Vec<TrackInfo>,
    pub protocol: ProtocolPushExtras,
}

/// Options for an in-memory loopback pair.
///
/// 内存 loopback 对的选项。
#[derive(Debug, Clone)]
pub struct LoopbackOptions {
    /// Logical stream name used for app/stream mapping.
    ///
    /// 用于 app/stream 映射的逻辑流名。
    pub stream_name: String,
    /// Tracks for the push side (used to emit sequence headers and metadata).
    ///
    /// push 端的轨道（用于发送序列头与元数据）。
    pub tracks: Vec<TrackInfo>,
    pub subscriber: SubscriberOptions,
    pub publisher: PublisherOptions,
    /// Bounded queue capacity; must be > 0.
    ///
    /// 有界队列容量；必须 > 0。
    pub queue_capacity: usize,
    pub cancel: CancellationToken,
    pub topology: LoopbackTopology,
}

impl Default for LoopbackOptions {
    fn default() -> Self {
        Self {
            stream_name: "loopback".to_string(),
            tracks: Vec::new(),
            subscriber: SubscriberOptions::default(),
            publisher: PublisherOptions::default(),
            queue_capacity: 150,
            cancel: CancellationToken::new(),
            topology: LoopbackTopology::default(),
        }
    }
}

/// Topology of an in-memory loopback pair.
///
/// 内存 loopback 对的拓扑。
#[derive(Debug, Clone)]
pub enum LoopbackTopology {
    /// Same protocol on both ends, if supported.
    ///
    /// 两端使用同一协议（若支持）。
    SameProtocol { protocol: crate::protocol::Protocol },
    /// Cross-protocol: e.g. RTMP push + HTTP-FLV pull.
    ///
    /// 跨协议：例如 RTMP push + HTTP-FLV pull。
    Cross {
        push: crate::protocol::Protocol,
        pull: crate::protocol::Protocol,
    },
}

impl Default for LoopbackTopology {
    fn default() -> Self {
        Self::Cross {
            push: crate::protocol::Protocol::Rtmp,
            pull: crate::protocol::Protocol::HttpFlv,
        }
    }
}

/// Layer at which a loopback test actually operates.
///
/// loopback 测试实际运行的层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopbackLayer {
    EngineOnlyBypassWire,
    ProtocolFraming,
    WebRtcSignalingOnly,
    WebRtcMediaFixture,
    WebRtcLocalUdp,
}

impl LoopbackLayer {
    /// Returns a short string suitable for logging and test labels.
    ///
    /// 返回适合日志与测试标签的短字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EngineOnlyBypassWire => "engine-only-bypass-wire",
            Self::ProtocolFraming => "protocol-framing",
            Self::WebRtcSignalingOnly => "webrtc-signaling-only",
            Self::WebRtcMediaFixture => "webrtc-media-fixture",
            Self::WebRtcLocalUdp => "webrtc-local-udp",
        }
    }
}
