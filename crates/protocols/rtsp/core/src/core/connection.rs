use super::{RtspMessageLimits, RtspMethod};

/// Error returned by `RTSP Interleaved Encode` operations.
/// `RTSP Interleaved Encode` 操作返回的错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtspInterleavedEncodeError {
    #[error("interleaved payload too large: {actual} > {max}")]
    PayloadTooLarge { max: usize, actual: usize },
}
/// Error returned by `RTSP Session` operations.
/// `RTSP Session` 操作返回的错误。
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

/// RTSP Session 头语义。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RtspSession {
    /// 会话 ID。
    pub id: Option<String>,
    /// 会话超时秒数。
    pub timeout: Option<u32>,
}

/// RTSP 连接限制配置。
///
/// 该结构用于承接连接级限制配置，再转换为 `RtspMessageLimits` 注入 core。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspConnectionLimits {
    pub max_buffer_size: usize,
    pub max_headers_count: usize,
    pub max_header_line_size: usize,
    pub max_body_size: usize,
    pub max_interleaved_frame_size: usize,
    pub validate_version: bool,
}

/// RTSP 连接协议状态。
///
/// 仅表达 RTSP 协议层状态，不包含模块业务态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RtspConnectionState {
    /// 初始状态。
    #[default]
    Init,
    /// 已完成 SETUP，进入就绪态。
    Ready,
    /// 已完成 PLAY，进入播放态。
    Playing,
    /// 已完成 RECORD，进入录制态。
    Recording,
    /// 已断开连接。
    Disconnected,
}

/// RTSP interleaved 帧头解析结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtspInterleavedFrameHeader {
    /// interleaved 通道号。
    pub channel: u8,
    /// interleaved 负载长度（不含 4 字节帧头）。
    pub payload_len: u16,
    /// interleaved 总帧长度（4 字节帧头 + 负载）。
    pub total_len: usize,
}

impl RtspSession {
    /// Creates a new `RtspSession` instance.
    /// 创建新的 `RtspSession` 实例。
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a copy with `ID` set.
    /// 返回将 `ID` 设置后的副本。
    pub fn with_id(id: &str) -> Self {
        Self {
            id: Some(id.to_string()),
            timeout: None,
        }
    }

    /// 解析 `Session` 头值。
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

    /// Converts to `header` representation.
    /// 转换为 `header` 表示。
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

/// 解析 interleaved 帧头。
///
/// 输入不满足 `$` 起始或帧头不足 4 字节时返回 `None`，不会 panic。
/// 当帧头完整时返回通道号、负载长度与总帧长度，即使负载尚未全部到齐。
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

/// 编码 interleaved 帧。
///
/// 线格式为 `$` + channel(1B) + payload_length(2B, BE) + payload。
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

/// 返回标准 RTSP 方法集合（用于 `Public` / `Allow` 头）。
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

/// 生成 `Public` 头值，顺序与 `supported_methods` 一致。
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
    /// Converts to `message limits` representation.
    /// 转换为 `message limits` 表示。
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
