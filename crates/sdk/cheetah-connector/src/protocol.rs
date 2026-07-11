/// Supported wire protocols for the high-level connector.
///
/// 高层 connector 支持的协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    Rtsp,
    HttpFlv,
    Rtmp,
    WebRtc,
}

impl Protocol {
    /// Returns the canonical short string for the protocol.
    ///
    /// 返回协议规范的短字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rtsp => "rtsp",
            Self::HttpFlv => "http-flv",
            Self::Rtmp => "rtmp",
            Self::WebRtc => "webrtc",
        }
    }
}

/// Direction of the media flow requested through the connector.
///
/// 通过 connector 请求的媒体流方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Pull,
    Push,
}

/// Gate flags for adapters that are declared but not yet wired in this build.
///
/// These are internal to `supports()` so the capability matrix stays honest
/// while new adapters are being implemented.
const RTSP_PULL_WIRED: bool = false;
const WEBRTC_PUSH_WIRED: bool = false;

/// Returns whether the first-party capability matrix allows this protocol/direction pair.
///
/// 返回官方能力矩阵是否允许该协议/方向组合。
///
/// The matrix is feature-gated and also requires the adapter to be wired.
/// Currently `rtsp` pull and `webrtc` push adapters are not wired, so this
/// function returns `false` for those pairs even when the feature is enabled.
///
/// 能力矩阵受 feature 控制，并且要求适配器已接线。当前 `rtsp` 拉流和 `webrtc`
/// 推流适配器尚未接线，因此即使 feature 开启，这些组合也会返回 `false`。
pub fn supports(protocol: Protocol, direction: Direction) -> bool {
    match (protocol, direction) {
        (Protocol::Rtsp, Direction::Pull) => cfg!(feature = "rtsp") && RTSP_PULL_WIRED,
        (Protocol::HttpFlv, Direction::Pull) => cfg!(feature = "http-flv"),
        (Protocol::Rtmp, Direction::Push) => cfg!(feature = "rtmp"),
        (Protocol::WebRtc, Direction::Push) => cfg!(feature = "webrtc") && WEBRTC_PUSH_WIRED,
        _ => false,
    }
}
