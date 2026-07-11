use std::error::Error;
use std::fmt;

use cheetah_codec::CodecId;
use cheetah_sdk::SdkError;

use crate::protocol::{Direction, Protocol};

/// Operation at which the connector error occurred.
///
/// connector 错误发生的操作阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Open,
    Connect,
    Handshake,
    Publish,
    Play,
    Read,
    Write,
    Negotiate,
    Close,
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Open => "open",
            Self::Connect => "connect",
            Self::Handshake => "handshake",
            Self::Publish => "publish",
            Self::Play => "play",
            Self::Read => "read",
            Self::Write => "write",
            Self::Negotiate => "negotiate",
            Self::Close => "close",
        };
        write!(f, "{s}")
    }
}

/// Reason for a closed connector handle.
///
/// connector 句柄关闭的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    User,
    Remote,
    Cancelled,
    Error(String),
}

/// Errors returned by the high-level connector facade.
///
/// 高层 connector facade 返回的错误。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConnectorError {
    #[error("invalid url for {protocol:?}: {url}: {reason}")]
    InvalidUrl {
        protocol: Protocol,
        url: String,
        reason: String,
    },

    #[error("unsupported protocol {protocol:?} for {direction:?}")]
    UnsupportedProtocol {
        protocol: Protocol,
        direction: Direction,
    },

    #[error("feature disabled for {protocol:?}: {feature}")]
    FeatureDisabled {
        protocol: Protocol,
        feature: &'static str,
    },

    #[error("connect failed for {protocol:?} endpoint={endpoint}")]
    Connect {
        protocol: Protocol,
        endpoint: String,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },

    #[error("protocol error {protocol:?} op={operation}")]
    Protocol {
        protocol: Protocol,
        operation: Operation,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },

    #[error("media error codec={codec:?}")]
    Media {
        codec: Option<CodecId>,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },

    #[error("backpressure on {protocol:?}")]
    Backpressure { protocol: Protocol },

    #[error("closed {protocol:?}: {reason:?}")]
    Closed {
        protocol: Protocol,
        reason: CloseReason,
    },

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("internal: {0}")]
    Internal(String),
}

impl ConnectorError {
    /// Returns the protocol associated with the error, if any.
    ///
    /// 返回错误关联的协议（如有）。
    pub fn protocol(&self) -> Option<Protocol> {
        match self {
            Self::InvalidUrl { protocol, .. }
            | Self::UnsupportedProtocol { protocol, .. }
            | Self::FeatureDisabled { protocol, .. }
            | Self::Connect { protocol, .. }
            | Self::Protocol { protocol, .. }
            | Self::Backpressure { protocol }
            | Self::Closed { protocol, .. } => Some(*protocol),
            Self::Media { .. } | Self::InvalidArgument(_) | Self::Internal(_) => None,
        }
    }

    /// Returns `true` if the error is transient and the operation may be retried.
    ///
    /// 返回错误是否为瞬时错误，操作可重试。
    pub fn retryable(&self) -> bool {
        match self {
            Self::InvalidUrl { .. }
            | Self::UnsupportedProtocol { .. }
            | Self::FeatureDisabled { .. }
            | Self::InvalidArgument(_) => false,
            Self::Connect { .. } | Self::Backpressure { .. } => true,
            Self::Protocol { source, .. } => retryable_protocol_source(source.as_ref()),
            Self::Media { .. } => false,
            Self::Closed { reason, .. } => matches!(reason, CloseReason::Error(_)),
            Self::Internal(_) => false,
        }
    }
}

impl From<SdkError> for ConnectorError {
    fn from(err: SdkError) -> Self {
        match err {
            SdkError::InvalidArgument(msg) => Self::InvalidArgument(msg),
            SdkError::NotFound(msg) => Self::InvalidArgument(msg),
            SdkError::AlreadyExists(msg) => Self::InvalidArgument(msg),
            SdkError::Conflict(msg) => Self::InvalidArgument(msg),
            // `SdkError::Unavailable` is intentionally not mapped to a hard-coded RTMP
            // `Connect` error. Handle adapters wrap `SdkError` through `map_sdk_error`
            // so the correct protocol is attached.
            SdkError::Unavailable(msg) => Self::Internal(msg),
            SdkError::Internal(msg) => Self::Internal(msg),
        }
    }
}

fn retryable_protocol_source(source: &(dyn Error + Send + Sync + 'static)) -> bool {
    if let Some(io) = source.downcast_ref::<std::io::Error>() {
        return matches!(
            io.kind(),
            std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::Interrupted
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::UnexpectedEof
        );
    }

    #[cfg(feature = "http-flv")]
    if let Some(err) = source.downcast_ref::<cheetah_http_flv_module::pull::HttpFlvPullError>() {
        use cheetah_http_flv_module::pull::HttpFlvPullError as E;
        return match err {
            E::ReadBody(_) | E::WriteRequest(_) => true,
            E::BadStatusCode { status_code } => (500..600).contains(status_code),
            _ => false,
        };
    }

    false
}

#[cfg(feature = "http-flv")]
impl From<cheetah_http_flv_module::pull::HttpFlvPullError> for ConnectorError {
    fn from(err: cheetah_http_flv_module::pull::HttpFlvPullError) -> Self {
        use cheetah_http_flv_module::pull::HttpFlvPullError as E;
        let protocol = Protocol::HttpFlv;
        match err {
            E::InvalidUrl(reason) => Self::InvalidUrl {
                protocol,
                url: String::new(),
                reason,
            },
            E::UnsupportedScheme { scheme } => Self::InvalidUrl {
                protocol,
                url: scheme,
                reason: "unsupported scheme".to_string(),
            },
            E::Resolve(endpoint) | E::Connect(endpoint) => Self::Connect {
                protocol,
                endpoint: endpoint.clone(),
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    endpoint,
                )),
            },
            E::Cancelled => Self::Closed {
                protocol,
                reason: CloseReason::Cancelled,
            },
            E::FlvDemux(_) | E::Ingress(_) => Self::Media {
                codec: None,
                source: Box::new(err),
            },
            E::WriteRequest(_) => Self::Protocol {
                protocol,
                operation: Operation::Write,
                source: Box::new(err),
            },
            E::ReadBody(_) => Self::Protocol {
                protocol,
                operation: Operation::Read,
                source: Box::new(err),
            },
            E::InvalidChunkedEncoding(_) => Self::Protocol {
                protocol,
                operation: Operation::Read,
                source: Box::new(err),
            },
            E::BadStatusCode { .. } => Self::Protocol {
                protocol,
                operation: Operation::Play,
                source: Box::new(err),
            },
            E::ResponseHeaderIncomplete
            | E::ResponseHeaderTooLarge { .. }
            | E::InvalidStatusLine
            | E::InvalidWebSocketAccept
            | E::WebSocketProtocol(_) => Self::Protocol {
                protocol,
                operation: Operation::Handshake,
                source: Box::new(err),
            },
        }
    }
}
