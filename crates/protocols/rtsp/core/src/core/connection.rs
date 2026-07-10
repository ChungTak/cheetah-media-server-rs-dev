use super::{RtspMessageLimits, RtspMethod};

/// Error returned when an interleaved RTP/RTCP frame exceeds the size limit.
///
/// 当交错 RTP/RTCP 帧超过大小限制时返回的错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtspInterleavedEncodeError {
    #[error("interleaved payload too large: {actual} > {max}")]
    PayloadTooLarge { max: usize, actual: usize },
}
/// Errors that can occur while parsing or serializing an RTSP `Session` header.
///
/// RTSP `Session` 头解析或序列化错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtspSessionError {
    #[error("empty session header")]
    EmptyHeader,
    #[error("missing session id")]
    MissingSessionId,
    #[error("invalid session id: {0}")]
    InvalidSessionId(String),
    #[error("invalid timeout value: {0}")]
    InvalidTimeout(String),
    #[error("invalid session header value")]
    InvalidHeaderValue,
}

/// RTSP `Session` header value.
///
/// Carries the session identifier and optional timeout. The session id is
/// normalized to remove leading/trailing whitespace and rejected if it contains
/// separators.
///
/// RTSP `Session` 头值。
///
/// 携带会话标识符和可选超时。会话 ID 经过去首尾空白归一化，并拒绝包含分隔符的值。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RtspSession {
    pub id: Option<String>,
    pub timeout: Option<u32>,
}

/// RTSP connection-level limits configuration.
///
/// Bridges connection-level configuration to the message parser limits in
/// `RtspMessageLimits`.
///
/// RTSP 连接级限制配置。
///
/// 将连接级配置桥接到 `RtspMessageLimits` 中的消息解析限制。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspConnectionLimits {
    pub max_buffer_size: usize,
    pub max_headers_count: usize,
    pub max_header_line_size: usize,
    pub max_body_size: usize,
    pub max_interleaved_frame_size: usize,
    pub validate_version: bool,
}

/// RTSP protocol-level connection state.
///
/// Only tracks the protocol state machine (INIT → READY → PLAYING/RECORDING).
/// It does not represent module-level business state.
///
/// RTSP 协议级连接状态。
///
/// 仅跟踪协议状态机（INIT → READY → PLAYING/RECORDING），不代表模块级业务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RtspConnectionState {
    #[default]
    Init,
    Ready,
    Playing,
    Recording,
    Disconnected,
}

/// Parsed RTSP interleaved RTP/RTCP frame header.
///
/// The wire format is `$` + channel(1B) + payload_length(2B, BE) + payload.
/// `total_len` equals 4 + payload_len, so callers can decide whether the full
/// frame has been received.
///
/// 解析后的 RTSP 交错 RTP/RTCP 帧头。
///
/// 线格式为 `$` + channel(1B) + payload_length(2B, BE) + payload。`total_len` 等于
/// 4 + payload_len，调用者可据此判断整帧是否已到达。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtspInterleavedFrameHeader {
    pub channel: u8,
    pub payload_len: u16,
    pub total_len: usize,
}

impl RtspSession {
    /// Create an empty session header value.
    ///
    /// 创建空的会话头值。
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a session header with the given id.
    ///
    /// 以给定 id 创建会话头。
    pub fn with_id(id: &str) -> Self {
        Self {
            id: Some(id.to_string()),
            timeout: None,
        }
    }

    /// Parse a `Session` header value of the form `id[;timeout=<seconds>]`.
    ///
    /// 解析形如 `id[;timeout=<seconds>]` 的 `Session` 头值。
    pub fn parse(header_value: &str) -> Result<Self, RtspSessionError> {
        let header_value = header_value.trim();
        if header_value.is_empty() {
            return Err(RtspSessionError::EmptyHeader);
        }
        if header_value.contains('\r') || header_value.contains('\n') {
            return Err(RtspSessionError::InvalidHeaderValue);
        }

        let mut parts = header_value.split(';').map(str::trim);
        let id = parts.next().ok_or(RtspSessionError::EmptyHeader)?;
        let normalized_id = normalize_session_id(id)?;

        let mut session = Self {
            id: Some(normalized_id.to_string()),
            timeout: None,
        };

        for part in parts {
            if part.is_empty() {
                continue;
            }
            if part.eq_ignore_ascii_case("timeout") {
                return Err(RtspSessionError::InvalidTimeout(String::new()));
            }

            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            if !key.trim().eq_ignore_ascii_case("timeout") {
                continue;
            }

            let value = value.trim();
            if value.is_empty() {
                return Err(RtspSessionError::InvalidTimeout(String::new()));
            }
            let timeout = value
                .parse::<u32>()
                .map_err(|_| RtspSessionError::InvalidTimeout(value.to_string()))?;
            session.timeout = Some(timeout);
        }

        Ok(session)
    }

    /// Serialize this session back to a `Session` header value.
    ///
    /// 将该会话序列化回 `Session` 头值。
    pub fn to_header(&self) -> Result<Option<String>, RtspSessionError> {
        let Some(id) = self.id.as_deref() else {
            return Ok(None);
        };
        let id = normalize_session_id(id)?;
        Ok(Some(if let Some(timeout) = self.timeout {
            format!("{id};timeout={timeout}")
        } else {
            id.to_string()
        }))
    }
}

/// Validate and trim a session id, rejecting empty values and separators.
///
/// 校验并裁剪会话 ID，拒绝空值和分隔符。
fn normalize_session_id(value: &str) -> Result<&str, RtspSessionError> {
    let id = value.trim();
    if id.is_empty() {
        return Err(RtspSessionError::MissingSessionId);
    }
    if id.contains(';') || id.contains('\r') || id.contains('\n') {
        return Err(RtspSessionError::InvalidSessionId(id.to_string()));
    }
    Ok(id)
}

/// Parse the 4-byte interleaved frame header from raw bytes.
///
/// Returns `None` if the buffer is shorter than 4 bytes or does not start with
/// `$`. When a complete header is present, the channel, payload length, and
/// total frame length are returned even if the payload has not yet arrived.
///
/// 从原始字节中解析 4 字节交错帧头。
///
/// 若缓冲区短于 4 字节或未以 `$` 开头则返回 `None`。当存在完整帧头时，即使负载尚未
/// 到达，也会返回通道号、负载长度和总帧长度。
pub fn parse_interleaved_frame(data: &[u8]) -> Option<RtspInterleavedFrameHeader> {
    if data.len() < 4 || data[0] != b'$' {
        return None;
    }

    let channel = data[1];
    let payload_len = u16::from_be_bytes([data[2], data[3]]);
    let total_len = 4 + usize::from(payload_len);

    Some(RtspInterleavedFrameHeader {
        channel,
        payload_len,
        total_len,
    })
}

/// Encode RTP/RTCP payload into an interleaved `$` frame.
///
/// Wire format: `$` + channel(1B) + payload_length(2B, BE) + payload.
///
/// 将 RTP/RTCP 负载编码为交错的 `$` 帧。
///
/// 线格式：`$` + channel(1B) + payload_length(2B, BE) + payload。
pub fn encode_interleaved_frame(
    channel: u8,
    payload: &[u8],
) -> Result<Vec<u8>, RtspInterleavedEncodeError> {
    let payload_len = payload.len();
    let max_payload_len = usize::from(u16::MAX);
    if payload_len > max_payload_len {
        return Err(RtspInterleavedEncodeError::PayloadTooLarge {
            max: max_payload_len,
            actual: payload_len,
        });
    }

    let mut frame = Vec::with_capacity(4 + payload_len);
    frame.push(b'$');
    frame.push(channel);
    frame.extend_from_slice(&(payload_len as u16).to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

/// Return the standard RTSP method set for `Public` / `Allow` headers.
///
/// 返回用于 `Public` / `Allow` 头的标准 RTSP 方法集合。
pub fn supported_methods() -> Vec<RtspMethod> {
    vec![
        RtspMethod::Options,
        RtspMethod::Describe,
        RtspMethod::Announce,
        RtspMethod::Setup,
        RtspMethod::Play,
        RtspMethod::Pause,
        RtspMethod::Teardown,
        RtspMethod::GetParameter,
        RtspMethod::SetParameter,
        RtspMethod::Redirect,
        RtspMethod::Record,
    ]
}

/// Build the `Public` header value from `supported_methods`.
///
/// 从 `supported_methods` 生成 `Public` 头值。
pub fn public_header_value() -> String {
    supported_methods()
        .iter()
        .map(RtspMethod::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

impl Default for RtspConnectionLimits {
    fn default() -> Self {
        Self {
            max_buffer_size: 1024 * 1024,
            max_headers_count: 64,
            max_header_line_size: 8 * 1024,
            max_body_size: 512 * 1024,
            max_interleaved_frame_size: 64 * 1024,
            validate_version: true,
        }
    }
}

impl RtspConnectionLimits {
    /// Convert connection limits to message parser limits.
    ///
    /// 将连接限制转换为消息解析器限制。
    pub fn to_message_limits(&self) -> RtspMessageLimits {
        self.clone().into()
    }
}

impl From<RtspConnectionLimits> for RtspMessageLimits {
    fn from(value: RtspConnectionLimits) -> Self {
        Self {
            max_buffer_size: value.max_buffer_size,
            max_headers_count: value.max_headers_count,
            max_header_line_size: value.max_header_line_size,
            max_body_size: value.max_body_size,
            max_interleaved_frame_size: value.max_interleaved_frame_size,
            validate_version: value.validate_version,
        }
    }
}

impl From<RtspMessageLimits> for RtspConnectionLimits {
    fn from(value: RtspMessageLimits) -> Self {
        Self {
            max_buffer_size: value.max_buffer_size,
            max_headers_count: value.max_headers_count,
            max_header_line_size: value.max_header_line_size,
            max_body_size: value.max_body_size,
            max_interleaved_frame_size: value.max_interleaved_frame_size,
            validate_version: value.validate_version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        encode_interleaved_frame, parse_interleaved_frame, public_header_value, supported_methods,
        RtspConnectionLimits, RtspConnectionState, RtspInterleavedEncodeError,
        RtspInterleavedFrameHeader, RtspMessageLimits, RtspSession, RtspSessionError,
    };
    use crate::core::RtspMethod;

    #[test]
    fn connection_limits_default_matches_vendor_semantics() {
        let limits = RtspConnectionLimits::default();
        assert_eq!(limits.max_buffer_size, 1024 * 1024);
        assert_eq!(limits.max_headers_count, 64);
        assert_eq!(limits.max_header_line_size, 8 * 1024);
        assert_eq!(limits.max_body_size, 512 * 1024);
        assert_eq!(limits.max_interleaved_frame_size, 64 * 1024);
        assert!(limits.validate_version);
    }

    #[test]
    fn converts_to_message_limits() {
        let limits = RtspConnectionLimits {
            max_buffer_size: 1234,
            max_headers_count: 6,
            max_header_line_size: 320,
            max_body_size: 2048,
            max_interleaved_frame_size: 4096,
            validate_version: false,
        };
        let message_limits = limits.to_message_limits();
        assert_eq!(
            message_limits,
            RtspMessageLimits {
                max_buffer_size: 1234,
                max_headers_count: 6,
                max_header_line_size: 320,
                max_body_size: 2048,
                max_interleaved_frame_size: 4096,
                validate_version: false,
            }
        );
    }

    #[test]
    fn converts_from_message_limits() {
        let message_limits = RtspMessageLimits {
            max_buffer_size: 4096,
            max_headers_count: 7,
            max_header_line_size: 512,
            max_body_size: 8192,
            max_interleaved_frame_size: 32 * 1024,
            validate_version: true,
        };
        let connection_limits = RtspConnectionLimits::from(message_limits.clone());
        assert_eq!(
            connection_limits,
            RtspConnectionLimits {
                max_buffer_size: 4096,
                max_headers_count: 7,
                max_header_line_size: 512,
                max_body_size: 8192,
                max_interleaved_frame_size: 32 * 1024,
                validate_version: true,
            }
        );
        assert_eq!(RtspMessageLimits::from(connection_limits), message_limits);
    }

    #[test]
    fn connection_state_default_is_init() {
        assert_eq!(RtspConnectionState::default(), RtspConnectionState::Init);
    }

    #[test]
    fn connection_state_variants_match_vendor_semantics() {
        let states = [
            RtspConnectionState::Init,
            RtspConnectionState::Ready,
            RtspConnectionState::Playing,
            RtspConnectionState::Recording,
            RtspConnectionState::Disconnected,
        ];
        assert_eq!(states.len(), 5);
        assert_ne!(states[0], states[4]);
    }

    #[test]
    fn session_parse_and_to_header_roundtrip() {
        let session = RtspSession::parse("abc123;timeout=60").expect("parse session header");
        assert_eq!(session.id.as_deref(), Some("abc123"));
        assert_eq!(session.timeout, Some(60));
        assert_eq!(
            session.to_header().expect("encode session header"),
            Some("abc123;timeout=60".to_string())
        );
    }

    #[test]
    fn session_parse_without_timeout() {
        let session = RtspSession::parse("abc123").expect("parse session header");
        assert_eq!(session.id.as_deref(), Some("abc123"));
        assert_eq!(session.timeout, None);
        assert_eq!(
            session.to_header().expect("encode session header"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn session_parse_rejects_invalid_timeout() {
        let err = RtspSession::parse("abc123;timeout=NaN").expect_err("invalid timeout");
        assert!(matches!(
            err,
            RtspSessionError::InvalidTimeout(ref value) if value == "NaN"
        ));
    }

    #[test]
    fn session_parse_rejects_empty_id() {
        let err = RtspSession::parse(" ;timeout=60").expect_err("missing session id");
        assert!(matches!(err, RtspSessionError::MissingSessionId));
    }

    #[test]
    fn session_to_header_rejects_invalid_id() {
        let session = RtspSession {
            id: Some("a;b".to_string()),
            timeout: Some(30),
        };
        let err = session.to_header().expect_err("invalid session id");
        assert!(matches!(
            err,
            RtspSessionError::InvalidSessionId(ref value) if value == "a;b"
        ));
    }

    #[test]
    fn parse_interleaved_frame_parses_header() {
        let parsed =
            parse_interleaved_frame(b"$\x02\x01\x00rest").expect("must parse interleaved header");
        assert_eq!(
            parsed,
            RtspInterleavedFrameHeader {
                channel: 2,
                payload_len: 256,
                total_len: 260,
            }
        );
    }

    #[test]
    fn parse_interleaved_frame_accepts_partial_payload() {
        let parsed = parse_interleaved_frame(b"$\x01\x00\x04AB").expect("must parse frame header");
        assert_eq!(parsed.channel, 1);
        assert_eq!(parsed.payload_len, 4);
        assert_eq!(parsed.total_len, 8);
    }

    #[test]
    fn parse_interleaved_frame_rejects_non_interleaved_prefix() {
        assert_eq!(parse_interleaved_frame(b"R"), None);
        assert_eq!(parse_interleaved_frame(b"RTSP/1.0 200 OK\r\n"), None);
    }

    #[test]
    fn parse_interleaved_frame_rejects_short_header() {
        assert_eq!(parse_interleaved_frame(b"$"), None);
        assert_eq!(parse_interleaved_frame(b"$\x01\x00"), None);
    }

    #[test]
    fn encode_interleaved_frame_builds_wire_bytes() {
        let encoded =
            encode_interleaved_frame(2, b"ABCD").expect("encode interleaved frame must succeed");
        assert_eq!(encoded, b"$\x02\x00\x04ABCD");
    }

    #[test]
    fn encode_interleaved_frame_rejects_payload_too_large() {
        let payload = vec![0_u8; usize::from(u16::MAX) + 1];
        let err = encode_interleaved_frame(0, &payload).expect_err("oversized interleaved frame");
        assert_eq!(
            err,
            RtspInterleavedEncodeError::PayloadTooLarge {
                max: usize::from(u16::MAX),
                actual: usize::from(u16::MAX) + 1,
            }
        );
    }

    #[test]
    fn supported_methods_matches_vendor_order() {
        let methods = supported_methods();
        assert_eq!(
            methods,
            vec![
                RtspMethod::Options,
                RtspMethod::Describe,
                RtspMethod::Announce,
                RtspMethod::Setup,
                RtspMethod::Play,
                RtspMethod::Pause,
                RtspMethod::Teardown,
                RtspMethod::GetParameter,
                RtspMethod::SetParameter,
                RtspMethod::Redirect,
                RtspMethod::Record,
            ]
        );
    }

    #[test]
    fn public_header_value_matches_vendor_semantics() {
        assert_eq!(
            public_header_value(),
            "OPTIONS, DESCRIBE, ANNOUNCE, SETUP, PLAY, PAUSE, TEARDOWN, GET_PARAMETER, SET_PARAMETER, REDIRECT, RECORD"
        );
    }
}
