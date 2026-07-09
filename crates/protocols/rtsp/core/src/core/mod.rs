mod auth;
mod compat;
mod connection;
mod interleaved;
mod limits;
mod message;
mod method;
mod range;
mod rtcp;
mod rtcp_fb;
mod rtcp_xr;
mod rtp;
mod rtp_info;
mod rtp_rewrite;
mod sdp;
mod seq_tracker;
mod transport;

use bytes::{Bytes, BytesMut};
use interleaved::{encode_frame, try_parse_frame};
use message::{encode_rtsp_response_parts, into_rtsp_request, ParsedMessage};

pub use auth::{
    compute_digest_response, parse_authorization_header, verify_digest_response, RtspAuthorization,
    RtspAuthorizationError, RtspDigestAlgorithm, RtspDigestAuthorization, RtspDigestChallenge,
};
pub use compat::{
    default_clock_rate, normalize_range_now, normalize_transport, parse_redirect_location,
    resolve_control_url, strip_sdp_suffix,
};
pub use connection::{
    encode_interleaved_frame, parse_interleaved_frame, public_header_value, supported_methods,
    RtspConnectionLimits, RtspConnectionState, RtspInterleavedEncodeError,
    RtspInterleavedFrameHeader, RtspSession, RtspSessionError,
};
pub use limits::RtspMessageLimits;
pub use message::{
    encode_rtsp_request, encode_rtsp_response, RtspHeader, RtspRequest, RtspRequestDecoder,
    RtspRequestMessage, RtspResponseDecoder, RtspResponseMessage,
};
pub use method::RtspMethod;
pub use range::{
    ClockRange, NptRange, NptTime, RtspRange, RtspRangeError, SmpteRange, SmpteTime, SmpteType,
};
pub use rtcp::{
    RtcpApp, RtcpBye, RtcpError, RtcpPacket, RtcpReceiverReport, RtcpReportBlock, RtcpSdes,
    RtcpSdesChunk, RtcpSdesItem, RtcpSenderReport,
};
pub use rtcp_fb::{
    build_rtcp_fir, build_rtcp_nack, build_rtcp_pli, nack_items_from_lost_seqs, parse_rtcp_fb,
    FirEntry, NackItem, RtcpFeedback, RtcpFir, RtcpNack, RtcpPli, PSFB_FMT_FIR, PSFB_FMT_PLI,
    RTCP_PT_PSFB, RTCP_PT_RTPFB, RTPFB_FMT_NACK,
};
pub use rtcp_xr::{
    build_rtcp_xr_receiver_reference_time, parse_rtcp_xr, DlrrSubBlock, RtcpXr, VoipMetricsBlock,
    XrBlock, RTCP_PT_XR,
};
pub use rtp::{RtpError, RtpExtension, RtpHeader, RtpPacket};
pub use rtp_info::{RtspRtpInfo, RtspRtpInfoError, RtspRtpInfoStream};
pub use rtp_rewrite::RtpRewriter;
pub use sdp::{
    Sdp, SdpAttribute, SdpBandwidth, SdpBuilder, SdpConnection, SdpError, SdpMedia,
    SdpMediaBuilder, SdpOrigin, SdpTiming,
};
pub use seq_tracker::{SeqEvent, SeqTracker};
pub use transport::{RtspTransport, RtspTransportError};

#[derive(Debug, Clone)]
pub enum CoreInput {
    Bytes(Bytes),
    Command(RtspCommand),
    PeerClosed,
}

#[derive(Debug, Clone)]
pub enum CoreOutput {
    Write(Bytes),
    Event(RtspEvent),
    Close,
}

#[derive(Debug, Clone)]
pub enum RtspEvent {
    Request(RtspRequest),
    InterleavedFrame { channel: u8, payload: Bytes },
    PeerClosed,
}

#[derive(Debug, Clone)]
pub enum RtspCommand {
    SendResponse {
        cseq: Option<u32>,
        status_code: u16,
        reason: String,
        headers: Vec<(String, String)>,
        body: Bytes,
    },
    SendInterleaved {
        channel: u8,
        payload: Bytes,
    },
    Close,
}

#[derive(Debug, thiserror::Error)]
pub enum RtspCoreError {
    #[error("invalid utf-8 in rtsp header")]
    InvalidUtf8,
    #[error("invalid rtsp start line")]
    InvalidStartLine,
    #[error("invalid content-length")]
    InvalidContentLength,
    #[error("invalid header line")]
    InvalidHeaderLine,
    #[error("invalid message field: {0}")]
    InvalidMessageField(&'static str),
    #[error("unexpected rtsp response while decoding request")]
    UnexpectedRtspResponse,
    #[error("unexpected rtsp request while decoding response")]
    UnexpectedRtspRequest,
    #[error("interleaved payload too large: {0} bytes")]
    InterleavedPayloadTooLarge(usize),
    #[error("interleaved frame size limit exceeded: {actual} > {max}")]
    InterleavedFrameSizeLimitExceeded { max: usize, actual: usize },
    #[error("rtsp buffer size limit exceeded: {actual} > {max}")]
    BufferSizeLimitExceeded { max: usize, actual: usize },
    #[error("rtsp header count limit exceeded: {actual} > {max}")]
    HeaderCountLimitExceeded { max: usize, actual: usize },
    #[error("rtsp header line size limit exceeded: {actual} > {max}")]
    HeaderLineSizeLimitExceeded { max: usize, actual: usize },
    #[error("rtsp body size limit exceeded: {actual} > {max}")]
    BodySizeLimitExceeded { max: usize, actual: usize },
}

pub struct RtspCore {
    buffer: BytesMut,
    closed: bool,
    limits: RtspMessageLimits,
}

impl Default for RtspCore {
    fn default() -> Self {
        Self::new()
    }
}

impl RtspCore {
    pub fn new() -> Self {
        Self::with_limits(RtspMessageLimits::default())
    }

    pub fn with_limits(limits: RtspMessageLimits) -> Self {
        Self {
            buffer: BytesMut::new(),
            closed: false,
            limits,
        }
    }

    pub fn with_connection_limits(limits: RtspConnectionLimits) -> Self {
        Self::with_limits(limits.into())
    }

    pub fn handle_input(&mut self, input: CoreInput) -> Result<Vec<CoreOutput>, RtspCoreError> {
        if self.closed {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        match input {
            CoreInput::Bytes(bytes) => {
                self.limits
                    .validate_buffer_growth(self.buffer.len(), bytes.len())?;
                self.buffer.extend_from_slice(&bytes);
                self.drain_messages(&mut out)?;
            }
            CoreInput::Command(command) => {
                self.apply_command(command, &mut out)?;
            }
            CoreInput::PeerClosed => {
                out.push(CoreOutput::Event(RtspEvent::PeerClosed));
                self.closed = true;
            }
        }

        Ok(out)
    }

    fn drain_messages(&mut self, out: &mut Vec<CoreOutput>) -> Result<(), RtspCoreError> {
        loop {
            if self.buffer.is_empty() {
                break;
            }

            if self.buffer[0] == b'$' {
                let Some(frame) =
                    try_parse_frame(&mut self.buffer, self.limits.max_interleaved_frame_size)?
                else {
                    break;
                };
                out.push(CoreOutput::Event(RtspEvent::InterleavedFrame {
                    channel: frame.channel,
                    payload: frame.payload,
                }));
                continue;
            }

            match message::try_parse_request(&mut self.buffer, &self.limits)? {
                ParsedMessage::Incomplete => break,
                ParsedMessage::Request(req) => out.push(CoreOutput::Event(RtspEvent::Request(
                    into_rtsp_request(req),
                ))),
                ParsedMessage::Response(_) => continue,
            }
        }

        Ok(())
    }

    fn apply_command(
        &mut self,
        command: RtspCommand,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtspCoreError> {
        match command {
            RtspCommand::SendResponse {
                cseq,
                status_code,
                reason,
                headers,
                body,
            } => {
                out.push(CoreOutput::Write(encode_rtsp_response_parts(
                    cseq,
                    status_code,
                    &reason,
                    headers,
                    body,
                )?));
            }
            RtspCommand::SendInterleaved { channel, payload } => {
                out.push(CoreOutput::Write(encode_frame(channel, payload)?));
            }
            RtspCommand::Close => {
                self.closed = true;
                out.push(CoreOutput::Close);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/capture.rs"]
mod capture_tests;

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::{
        CoreInput, CoreOutput, RtspCommand, RtspConnectionLimits, RtspCore, RtspCoreError,
        RtspEvent, RtspMessageLimits, RtspMethod,
    };

    #[test]
    fn parses_rtsp_request_and_body() {
        let mut core = RtspCore::new();
        let input = Bytes::from_static(
            b"ANNOUNCE rtsp://127.0.0.1/live/test RTSP/1.0\r\nCSeq: 3\r\nContent-Length: 4\r\n\r\ntest",
        );
        let outputs = core
            .handle_input(CoreInput::Bytes(input))
            .expect("parse request");
        assert_eq!(outputs.len(), 1);
        match &outputs[0] {
            CoreOutput::Event(RtspEvent::Request(req)) => {
                assert_eq!(req.method, RtspMethod::Announce);
                assert_eq!(req.cseq, Some(3));
                assert_eq!(req.body, Bytes::from_static(b"test"));
            }
            _ => panic!("unexpected output"),
        }
    }

    #[test]
    fn parses_pause_method() {
        let mut core = RtspCore::new();
        let input =
            Bytes::from_static(b"PAUSE rtsp://127.0.0.1/live/test RTSP/1.0\r\nCSeq: 9\r\n\r\n");
        let outputs = core
            .handle_input(CoreInput::Bytes(input))
            .expect("parse pause request");
        assert_eq!(outputs.len(), 1);
        match &outputs[0] {
            CoreOutput::Event(RtspEvent::Request(req)) => {
                assert_eq!(req.method, RtspMethod::Pause);
                assert_eq!(req.cseq, Some(9));
            }
            _ => panic!("unexpected output"),
        }
    }

    #[test]
    fn parses_interleaved_frame() {
        let mut core = RtspCore::new();
        let input = Bytes::from_static(b"$\x00\x00\x04ABCD");
        let outputs = core
            .handle_input(CoreInput::Bytes(input))
            .expect("parse interleaved");
        assert_eq!(outputs.len(), 1);
        match &outputs[0] {
            CoreOutput::Event(RtspEvent::InterleavedFrame { channel, payload }) => {
                assert_eq!(*channel, 0);
                assert_eq!(payload, &Bytes::from_static(b"ABCD"));
            }
            _ => panic!("unexpected output"),
        }
    }

    #[test]
    fn encodes_response() {
        let mut core = RtspCore::new();
        let outputs = core
            .handle_input(CoreInput::Command(RtspCommand::SendResponse {
                cseq: Some(5),
                status_code: 200,
                reason: "OK".to_string(),
                headers: vec![("Session".to_string(), "sess1".to_string())],
                body: Bytes::from_static(b"hello"),
            }))
            .expect("encode response");

        assert_eq!(outputs.len(), 1);
        match &outputs[0] {
            CoreOutput::Write(data) => {
                let text = std::str::from_utf8(data).expect("utf8");
                assert!(text.contains("RTSP/1.0 200 OK"));
                assert!(text.contains("CSeq: 5"));
                assert!(text.contains("Content-Length: 5"));
            }
            _ => panic!("unexpected output"),
        }
    }

    #[test]
    fn rejects_oversized_interleaved_payload() {
        let mut core = RtspCore::new();
        let err = core
            .handle_input(CoreInput::Command(RtspCommand::SendInterleaved {
                channel: 1,
                payload: Bytes::from(vec![0u8; u16::MAX as usize + 1]),
            }))
            .expect_err("oversized interleaved payload should fail");
        assert!(
            matches!(err, RtspCoreError::InterleavedPayloadTooLarge(size) if size == u16::MAX as usize + 1)
        );
    }

    #[test]
    fn rejects_oversized_inbound_interleaved_frame() {
        let mut core = RtspCore::with_connection_limits(RtspConnectionLimits {
            max_interleaved_frame_size: 8,
            ..RtspConnectionLimits::default()
        });
        let err = core
            .handle_input(CoreInput::Bytes(Bytes::from_static(b"$\x00\x00\x09")))
            .expect_err("oversized interleaved frame should fail");
        assert!(matches!(
            err,
            RtspCoreError::InterleavedFrameSizeLimitExceeded { max: 8, actual: 9 }
        ));
    }

    #[test]
    fn rejects_oversized_buffer_input() {
        let mut core = RtspCore::with_limits(RtspMessageLimits {
            max_buffer_size: 8,
            ..RtspMessageLimits::default()
        });
        let err = core
            .handle_input(CoreInput::Bytes(Bytes::from_static(b"0123456789")))
            .expect_err("oversized buffer should fail");
        assert!(matches!(
            err,
            RtspCoreError::BufferSizeLimitExceeded { max: 8, actual: 10 }
        ));
    }
}
