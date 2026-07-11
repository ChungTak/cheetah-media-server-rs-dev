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

/// Returns whether the first-party capability matrix allows this protocol/direction pair.
///
/// 返回官方能力矩阵是否允许该协议/方向组合。
///
/// Current capability matrix:
/// - RTSP: pull
/// - HTTP-FLV: pull
/// - RTMP: push
/// - WebRTC: push
pub fn supports(protocol: Protocol, direction: Direction) -> bool {
    matches!(
        (protocol, direction),
        (Protocol::Rtsp, Direction::Pull)
            | (Protocol::HttpFlv, Direction::Pull)
            | (Protocol::Rtmp, Direction::Push)
            | (Protocol::WebRtc, Direction::Push)
    )
}
