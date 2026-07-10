use bytes::Bytes;
use cheetah_rtsp_core::{
    RtcpBye, RtcpPacket, RtcpReceiverReport, RtcpReportBlock, RtcpSdes, RtcpSdesChunk,
    RtcpSdesItem, RtcpSenderReport,
};

/// `ParsedRtcpSenderReport` data structure.
/// `ParsedRtcpSenderReport` 数据结构.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedRtcpSenderReport {
    /// `sender_ssrc` field of type `u32`.
    /// `sender_ssrc` 字段，类型为 `u32`.
    pub sender_ssrc: u32,
    /// `lsr` field of type `u32`.
    /// `lsr` 字段，类型为 `u32`.
    pub lsr: u32,
}

/// Parses `rtcp_sender_report` from input.
/// 解析 `rtcp_sender_report` 来自 输入.
pub fn parse_rtcp_sender_report(
    payload: &[u8],
) -> Result<Option<ParsedRtcpSenderReport>, cheetah_rtsp_core::RtcpError> {
    let packets = RtcpPacket::parse(payload)?;
    for packet in packets {
        if let RtcpPacket::SenderReport(sr) = packet {
            let ntp_secs = (sr.ntp_timestamp >> 32) as u32;
            let ntp_frac = sr.ntp_timestamp as u32;
            let lsr = ((ntp_secs & 0xffff) << 16) | ((ntp_frac >> 16) & 0xffff);
            return Ok(Some(ParsedRtcpSenderReport {
                sender_ssrc: sr.ssrc,
                lsr,
            }));
        }
    }
    Ok(None)
}

/// `RtcpReceiverReportBlock` data structure.
/// `RtcpReceiverReportBlock` 数据结构.
pub struct RtcpReceiverReportBlock {
    /// `sender_ssrc` field of type `u32`.
    /// `sender_ssrc` 字段，类型为 `u32`.
    pub sender_ssrc: u32,
    /// `fraction_lost` field of type `u8`.
    /// `fraction_lost` 字段，类型为 `u8`.
    pub fraction_lost: u8,
    /// `cumulative_lost` field of type `u32`.
    /// `cumulative_lost` 字段，类型为 `u32`.
    pub cumulative_lost: u32,
    /// `extended_highest_seq` field of type `u32`.
    /// `extended_highest_seq` 字段，类型为 `u32`.
    pub extended_highest_seq: u32,
    /// `jitter` field of type `u32`.
    /// `jitter` 字段，类型为 `u32`.
    pub jitter: u32,
    /// `lsr` field of type `u32`.
    /// `lsr` 字段，类型为 `u32`.
    pub lsr: u32,
    /// `dlsr` field of type `u32`.
    /// `dlsr` 字段，类型为 `u32`.
    pub dlsr: u32,
}

/// Builds `rtcp_sender_report` output.
/// 构建 `rtcp_sender_report` 输出.
pub fn build_rtcp_sender_report(
    ssrc: u32,
    rtp_timestamp: u32,
    packet_count: u32,
    octet_count: u32,
    unix_micros: u64,
) -> Result<Bytes, cheetah_rtsp_core::RtcpError> {
    let (ntp_secs, ntp_frac) = unix_micros_to_ntp(unix_micros);
    let packet = RtcpPacket::SenderReport(RtcpSenderReport {
        ssrc,
        ntp_timestamp: ((u64::from(ntp_secs)) << 32) | u64::from(ntp_frac),
        rtp_timestamp,
        packet_count,
        octet_count,
        reports: Vec::new(),
    });
    RtcpPacket::build(&[packet]).map(Bytes::from)
}

/// Builds `rtcp_sdes_cname` output.
/// 构建 `rtcp_sdes_cname` 输出.
pub fn build_rtcp_sdes_cname(
    ssrc: u32,
    cname: &str,
) -> Result<Bytes, cheetah_rtsp_core::RtcpError> {
    let packet = RtcpPacket::SourceDescription(RtcpSdes {
        chunks: vec![RtcpSdesChunk {
            ssrc,
            items: vec![RtcpSdesItem::Cname(cname.to_string())],
        }],
    });
    RtcpPacket::build(&[packet]).map(Bytes::from)
}

/// Builds `rtcp_receiver_report` output.
/// 构建 `rtcp_receiver_report` 输出.
pub fn build_rtcp_receiver_report(
    receiver_ssrc: u32,
    block: RtcpReceiverReportBlock,
) -> Result<Bytes, cheetah_rtsp_core::RtcpError> {
    let packet = RtcpPacket::ReceiverReport(RtcpReceiverReport {
        ssrc: receiver_ssrc,
        reports: vec![RtcpReportBlock {
            ssrc: block.sender_ssrc,
            fraction_lost: block.fraction_lost,
            cumulative_lost: block.cumulative_lost.min(0x00ff_ffff),
            highest_seq: block.extended_highest_seq,
            jitter: block.jitter,
            last_sr: block.lsr,
            delay_since_sr: block.dlsr,
        }],
    });
    RtcpPacket::build(&[packet]).map(Bytes::from)
}

/// Builds `rtcp_bye` output.
/// 构建 `rtcp_bye` 输出.
pub fn build_rtcp_bye(
    ssrc: u32,
    reason: Option<&str>,
) -> Result<Bytes, cheetah_rtsp_core::RtcpError> {
    let packet = RtcpPacket::Bye(RtcpBye {
        ssrcs: vec![ssrc],
        reason: reason.map(ToOwned::to_owned),
    });
    RtcpPacket::build(&[packet]).map(Bytes::from)
}

/// Build a minimal RTCP Receiver Report with no report blocks.
/// Used by the pull client as a keep-alive signal to sources that require RTCP activity.
pub fn build_rtcp_empty_rr(receiver_ssrc: u32) -> Result<Bytes, cheetah_rtsp_core::RtcpError> {
    let packet = RtcpPacket::ReceiverReport(RtcpReceiverReport {
        ssrc: receiver_ssrc,
        reports: vec![],
    });
    RtcpPacket::build(&[packet]).map(Bytes::from)
}

fn unix_micros_to_ntp(unix_micros: u64) -> (u32, u32) {
    const NTP_UNIX_OFFSET_SECS: u64 = 2_208_988_800;
    let unix_secs = unix_micros / 1_000_000;
    let unix_frac_micros = unix_micros % 1_000_000;
    let ntp_secs = unix_secs.saturating_add(NTP_UNIX_OFFSET_SECS);
    let ntp_frac = ((unix_frac_micros as u128) << 32) / 1_000_000u128;
    (
        ntp_secs.min(u64::from(u32::MAX)) as u32,
        ntp_frac.min(u128::from(u32::MAX)) as u32,
    )
}
