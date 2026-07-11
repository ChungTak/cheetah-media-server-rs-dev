//! RTP header rewriting for Direct Proxy mode.
//!
//! In Direct Proxy mode, RTP packets are forwarded without decode/re-encode.
//! Only the RTP header fields (SSRC, sequence number, and optionally payload
//! type) are rewritten to match the target stream.
//!
//! 直接代理模式下的 RTP 头部重写。
//!
//! 在直接代理模式下，RTP 包不解码/重编码直接转发。仅重写 RTP 头部字段
//!（SSRC、序列号、可选的 payload type）以匹配目标流。

use bytes::{Bytes, BytesMut};

/// State for rewriting RTP headers in Direct Proxy mode.
///
/// Maintains an independent target sequence counter and optional payload type
/// override. The payload bytes are not inspected or copied more than necessary.
///
/// 直接代理模式下重写 RTP 头部的状态。
///
/// 维护独立的目标序列计数器与可选的 payload type 覆盖。负载字节不会被检查或
/// 不必要地拷贝。
pub struct RtpRewriter {
    target_ssrc: u32,
    next_seq: u16,
    target_pt: Option<u8>,
}

impl RtpRewriter {
    /// Create a rewriter with a fixed target SSRC and initial sequence number.
    ///
    /// `initial_seq` is the sequence number that will be written into the next
    /// forwarded packet; it is incremented after each successful rewrite.
    ///
    /// 创建以固定目标 SSRC 和初始序列号开头的重写器。
    ///
    /// `initial_seq` 是下一次成功转发时写入的序列号；每次重写成功后自增。
    pub fn new(target_ssrc: u32, initial_seq: u16) -> Self {
        Self {
            target_ssrc,
            next_seq: initial_seq,
            target_pt: None,
        }
    }

    /// Override the payload type in the forwarded RTP header.
    ///
    /// The marker bit is preserved and the lower 7 bits are replaced. If no
    /// override is set, the original payload type is left unchanged.
    ///
    /// 覆盖转发 RTP 头部的 payload type。
    ///
    /// 保留 marker 位并替换低 7 位。若未设置覆盖，则保留原始 payload type。
    pub fn with_payload_type(mut self, pt: u8) -> Self {
        self.target_pt = Some(pt);
        self
    }

    /// Rewrite an RTP packet's header fields for forwarding.
    ///
    /// The header is modified in-place: sequence number (bytes 2-3), SSRC
    /// (bytes 8-11), and payload type if configured. Returns `None` if the input
    /// is shorter than the 12-byte fixed RTP header.
    ///
    /// 重写 RTP 包头部字段以进行转发。
    ///
    /// 原地修改头部：序列号（字节 2-3）、SSRC（字节 8-11），以及配置后的 payload type。
    /// 若输入短于 12 字节固定 RTP 头则返回 `None`。
    pub fn rewrite(&mut self, packet: &[u8]) -> Option<Bytes> {
        if packet.len() < 12 {
            return None;
        }
        let mut out = BytesMut::from(packet);
        // Rewrite PT if configured (byte 1, lower 7 bits, preserving marker bit)
        if let Some(pt) = self.target_pt {
            out[1] = (out[1] & 0x80) | (pt & 0x7f);
        }
        // Rewrite sequence number (bytes 2-3)
        let seq = self.next_seq;
        out[2] = (seq >> 8) as u8;
        out[3] = seq as u8;
        self.next_seq = self.next_seq.wrapping_add(1);
        // Rewrite SSRC (bytes 8-11)
        out[8..12].copy_from_slice(&self.target_ssrc.to_be_bytes());
        Some(out.freeze())
    }

    /// Current sequence counter value (the value that will be used next).
    ///
    /// 当前序列计数器值（下一次将使用的值）。
    pub fn current_seq(&self) -> u16 {
        self.next_seq
    }

    /// Target SSRC configured for this rewriter.
    ///
    /// 该重写器配置的目标 SSRC。
    pub fn target_ssrc(&self) -> u32 {
        self.target_ssrc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rtp_packet(pt: u8, seq: u16, ts: u32, ssrc: u32, payload: &[u8]) -> Vec<u8> {
        let mut pkt = vec![0u8; 12 + payload.len()];
        pkt[0] = 0x80; // V=2, no padding, no extension, CC=0
        pkt[1] = pt;
        pkt[2] = (seq >> 8) as u8;
        pkt[3] = seq as u8;
        pkt[4..8].copy_from_slice(&ts.to_be_bytes());
        pkt[8..12].copy_from_slice(&ssrc.to_be_bytes());
        pkt[12..].copy_from_slice(payload);
        pkt
    }

    #[test]
    fn rewrite_replaces_ssrc_and_seq() {
        let mut rewriter = RtpRewriter::new(0xAABBCCDD, 1000);
        let pkt = make_rtp_packet(96, 500, 12345, 0x11223344, b"hello");
        let out = rewriter.rewrite(&pkt).unwrap();
        // Check SSRC
        assert_eq!(&out[8..12], &0xAABBCCDDu32.to_be_bytes());
        // Check seq
        assert_eq!(u16::from_be_bytes([out[2], out[3]]), 1000);
        // Check PT unchanged
        assert_eq!(out[1] & 0x7f, 96);
        // Check timestamp unchanged
        assert_eq!(&out[4..8], &12345u32.to_be_bytes());
        // Check payload unchanged
        assert_eq!(&out[12..], b"hello");
    }

    #[test]
    fn rewrite_increments_seq() {
        let mut rewriter = RtpRewriter::new(0x1, 65534);
        let pkt = make_rtp_packet(96, 0, 0, 0, b"");
        rewriter.rewrite(&pkt).unwrap();
        assert_eq!(rewriter.current_seq(), 65535);
        rewriter.rewrite(&pkt).unwrap();
        assert_eq!(rewriter.current_seq(), 0); // wraps
    }

    #[test]
    fn rewrite_with_pt_override() {
        let mut rewriter = RtpRewriter::new(0x1, 0).with_payload_type(111);
        let pkt = make_rtp_packet(96, 0, 0, 0, b"data");
        let out = rewriter.rewrite(&pkt).unwrap();
        assert_eq!(out[1] & 0x7f, 111);
    }

    #[test]
    fn rewrite_preserves_marker_bit() {
        let mut rewriter = RtpRewriter::new(0x1, 0).with_payload_type(111);
        let mut pkt = make_rtp_packet(96, 0, 0, 0, b"");
        pkt[1] |= 0x80; // set marker bit
        let out = rewriter.rewrite(&pkt).unwrap();
        assert_eq!(out[1], 0x80 | 111); // marker preserved, PT changed
    }

    #[test]
    fn rewrite_returns_none_for_short_packet() {
        let mut rewriter = RtpRewriter::new(0x1, 0);
        assert!(rewriter.rewrite(&[0; 11]).is_none());
    }
}
