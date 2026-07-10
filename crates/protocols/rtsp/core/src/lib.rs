//! `cheetah-rtsp-core` is the Sans-I/O protocol state machine for RTSP.
//!
//! It owns RTSP message parsing, digest authentication, SDP negotiation,
//! interleaved RTP/RTCP framing, and RTCP feedback construction. No Tokio,
//! socket, or wall-clock access is present here; the driver consumes
//! `CoreInput` and produces `CoreOutput`.
//!
//! `cheetah-rtsp-core` 是 RTSP 的 Sans-I/O 协议状态机。
//!
//! 它负责 RTSP 消息解析、摘要认证、SDP 协商、交错 RTP/RTCP 帧封装以及
//! RTCP 反馈报文构造。此处不依赖 Tokio、socket 或墙上时间；驱动层消费
//! `CoreInput` 并产出 `CoreOutput`。

pub mod core;

pub use core::{
    build_rtcp_fir, build_rtcp_nack, build_rtcp_pli, build_rtcp_xr_receiver_reference_time,
    compute_digest_response, default_clock_rate, encode_interleaved_frame, encode_rtsp_request,
    encode_rtsp_response, nack_items_from_lost_seqs, normalize_range_now, normalize_transport,
    parse_authorization_header, parse_interleaved_frame, parse_redirect_location, parse_rtcp_fb,
    parse_rtcp_xr, public_header_value, resolve_control_url, strip_sdp_suffix, supported_methods,
    verify_digest_response, ClockRange, CoreInput, CoreOutput, DlrrSubBlock, FirEntry, NackItem,
    NptRange, NptTime, RtcpApp, RtcpBye, RtcpError, RtcpFeedback, RtcpFir, RtcpNack, RtcpPacket,
    RtcpPli, RtcpReceiverReport, RtcpReportBlock, RtcpSdes, RtcpSdesChunk, RtcpSdesItem,
    RtcpSenderReport, RtcpXr, RtpError, RtpExtension, RtpHeader, RtpPacket, RtpRewriter,
    RtspAuthorization, RtspAuthorizationError, RtspCommand, RtspConnectionLimits,
    RtspConnectionState, RtspCore, RtspCoreError, RtspDigestAlgorithm, RtspDigestAuthorization,
    RtspDigestChallenge, RtspEvent, RtspHeader, RtspInterleavedEncodeError,
    RtspInterleavedFrameHeader, RtspMessageLimits, RtspMethod, RtspRange, RtspRangeError,
    RtspRequest, RtspRequestDecoder, RtspRequestMessage, RtspResponseDecoder, RtspResponseMessage,
    RtspRtpInfo, RtspRtpInfoError, RtspRtpInfoStream, RtspSession, RtspSessionError, RtspTransport,
    RtspTransportError, Sdp, SdpAttribute, SdpBandwidth, SdpBuilder, SdpConnection, SdpError,
    SdpMedia, SdpMediaBuilder, SdpOrigin, SdpTiming, SeqEvent, SeqTracker, SmpteRange, SmpteTime,
    SmpteType, VoipMetricsBlock, XrBlock, PSFB_FMT_FIR, PSFB_FMT_PLI, RTCP_PT_PSFB, RTCP_PT_RTPFB,
    RTCP_PT_XR, RTPFB_FMT_NACK,
};
