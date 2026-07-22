use super::{sign_extend_24, RtcpEncodeError, RtcpPacketType, RtcpParseError};
use bytes::{Buf, BufMut, Bytes, BytesMut};

/// One report block from an SR or RR packet.
///
/// SR/RR 包中的一个报告块。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcpReportBlock {
    pub ssrc: u32,
    pub fraction_lost: u8,
    pub cumulative_lost: i32,
    pub highest_seq: u32,
    pub jitter: u32,
    pub last_sr: u32,
    pub delay_since_last_sr: u32,
}

impl RtcpReportBlock {
    /// Parse one 24-byte report block.
    pub fn parse(buf: &mut Bytes) -> Result<Self, RtcpParseError> {
        if buf.len() < 24 {
            return Err(RtcpParseError::TooShort);
        }
        let mut tmp = buf.split_to(24);
        let ssrc = tmp.get_u32();
        let fl_and_cl = tmp.get_u32();
        let fraction_lost = (fl_and_cl >> 24) as u8;
        let cumulative_lost = sign_extend_24(fl_and_cl & 0x00ff_ffff);
        let highest_seq = tmp.get_u32();
        let jitter = tmp.get_u32();
        let last_sr = tmp.get_u32();
        let delay_since_last_sr = tmp.get_u32();
        Ok(Self {
            ssrc,
            fraction_lost,
            cumulative_lost,
            highest_seq,
            jitter,
            last_sr,
            delay_since_last_sr,
        })
    }

    /// Encode this report block into 24 bytes.
    pub fn encode(&self, out: &mut BytesMut) {
        out.put_u32(self.ssrc);
        let cl = self.cumulative_lost as u32 & 0x00ff_ffff;
        out.put_u32((u32::from(self.fraction_lost) << 24) | cl);
        out.put_u32(self.highest_seq);
        out.put_u32(self.jitter);
        out.put_u32(self.last_sr);
        out.put_u32(self.delay_since_last_sr);
    }
}

/// RTCP Sender Report (PT=200).
///
/// RTCP 发送者报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpSenderReport {
    pub ssrc: u32,
    pub ntp_timestamp: u64,
    pub rtp_timestamp: u32,
    pub packets_sent: u32,
    pub octets_sent: u32,
    pub report_blocks: Vec<RtcpReportBlock>,
}

impl RtcpSenderReport {
    pub fn encode(&self) -> Result<Bytes, RtcpEncodeError> {
        let block_count = self.report_blocks.len();
        if block_count > 31 {
            return Err(RtcpEncodeError::TooManyReportBlocks { count: block_count });
        }
        let rc = block_count as u8;
        let total_len = 4 + 24 + block_count * 24;
        let length = ((total_len / 4) - 1) as u16;
        let mut out = BytesMut::with_capacity(total_len);
        out.put_u8(0x80 | rc);
        out.put_u8(RtcpPacketType::SenderReport as u8);
        out.put_u16(length);
        out.put_u32(self.ssrc);
        out.put_u64(self.ntp_timestamp);
        out.put_u32(self.rtp_timestamp);
        out.put_u32(self.packets_sent);
        out.put_u32(self.octets_sent);
        for block in &self.report_blocks {
            block.encode(&mut out);
        }
        Ok(out.freeze())
    }

    pub fn parse(buf: &mut Bytes, count: u8) -> Result<Self, RtcpParseError> {
        if buf.len() < 24 {
            return Err(RtcpParseError::Truncated {
                pt: RtcpPacketType::SenderReport as u8,
            });
        }
        let mut header = buf.split_to(24);
        let ssrc = header.get_u32();
        let ntp_timestamp = header.get_u64();
        let rtp_timestamp = header.get_u32();
        let packets_sent = header.get_u32();
        let octets_sent = header.get_u32();
        let mut report_blocks = Vec::with_capacity(count as usize);
        for _ in 0..count {
            report_blocks.push(RtcpReportBlock::parse(buf)?);
        }
        Ok(Self {
            ssrc,
            ntp_timestamp,
            rtp_timestamp,
            packets_sent,
            octets_sent,
            report_blocks,
        })
    }
}

/// RTCP Receiver Report (PT=201).
///
/// RTCP 接收者报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpReceiverReport {
    pub ssrc: u32,
    pub report_blocks: Vec<RtcpReportBlock>,
}

impl RtcpReceiverReport {
    pub fn encode(&self) -> Result<Bytes, RtcpEncodeError> {
        let block_count = self.report_blocks.len();
        if block_count > 31 {
            return Err(RtcpEncodeError::TooManyReportBlocks { count: block_count });
        }
        let rc = block_count as u8;
        let total_len = 4 + 4 + block_count * 24;
        let length = ((total_len / 4) - 1) as u16;
        let mut out = BytesMut::with_capacity(total_len);
        out.put_u8(0x80 | rc);
        out.put_u8(RtcpPacketType::ReceiverReport as u8);
        out.put_u16(length);
        out.put_u32(self.ssrc);
        for block in &self.report_blocks {
            block.encode(&mut out);
        }
        Ok(out.freeze())
    }

    pub fn parse(buf: &mut Bytes, count: u8) -> Result<Self, RtcpParseError> {
        if buf.len() < 4 {
            return Err(RtcpParseError::Truncated {
                pt: RtcpPacketType::ReceiverReport as u8,
            });
        }
        let ssrc = buf.get_u32();
        let mut report_blocks = Vec::with_capacity(count as usize);
        for _ in 0..count {
            report_blocks.push(RtcpReportBlock::parse(buf)?);
        }
        Ok(Self {
            ssrc,
            report_blocks,
        })
    }
}
