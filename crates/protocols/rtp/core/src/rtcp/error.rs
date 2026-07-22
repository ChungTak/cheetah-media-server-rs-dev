use thiserror::Error;

/// Errors encountered while parsing RTCP packets.
///
/// 解析 RTCP 包时遇到的错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RtcpParseError {
    #[error("rtcp packet too short")]
    TooShort,
    #[error("truncated rtcp {pt} packet")]
    Truncated { pt: u8 },
    #[error("invalid rtcp version: {version}")]
    InvalidVersion { version: u8 },
    #[error("invalid sdes item length")]
    InvalidSdes,
    #[error("invalid padding count: {count} for pt {pt}")]
    InvalidPadding { pt: u8, count: u8 },
}

/// Errors encountered while encoding RTCP packets.
///
/// 编码 RTCP 包时遇到的错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RtcpEncodeError {
    #[error("sdes item text too long: {length}")]
    SdesItemTooLong { length: usize },
    #[error("bye reason too long: {length}")]
    ByeReasonTooLong { length: usize },
    #[error("too many report blocks: {count}")]
    TooManyReportBlocks { count: usize },
    #[error("too many sdes chunks: {count}")]
    TooManySdesChunks { count: usize },
    #[error("too many bye ssrcs: {count}")]
    TooManyByeSsrcs { count: usize },
    #[error("payload length must be a multiple of 4: {length}")]
    UnalignedPayload { length: usize },
}

/// RTCP packet type identifiers.
///
/// RTCP 包类型标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RtcpPacketType {
    SenderReport = 200,
    ReceiverReport = 201,
    SourceDescription = 202,
    Bye = 203,
    App = 204,
}

impl RtcpPacketType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            200 => Some(Self::SenderReport),
            201 => Some(Self::ReceiverReport),
            202 => Some(Self::SourceDescription),
            203 => Some(Self::Bye),
            204 => Some(Self::App),
            _ => None,
        }
    }
}
