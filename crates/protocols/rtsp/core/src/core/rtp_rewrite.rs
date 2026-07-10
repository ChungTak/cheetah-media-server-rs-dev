//! RTP header rewriting for Direct Proxy mode.
//!
//! In Direct Proxy mode, RTP packets are forwarded without decode/re-encode.
//! Only the RTP header fields (SSRC, seq, optionally PT) are rewritten.

use bytes::{Bytes, BytesMut};

/// State for rewriting RTP headers in Direct Proxy mode.
#[derive(Debug, Clone)]
pub struct RtpRewriter {
    /// SSRC to write into forwarded packets.
    target_ssrc: u32,
    /// Independent sequence counter for the target stream.
    next_seq: u16,
    /// Optional payload type override (None = keep original).
    target_pt: Option<u8>,
}

impl RtpRewriter {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(target_ssrc: u32, initial_seq: u16) -> Self {
        Self {
            target_ssrc,
            next_seq: initial_seq,
            target_pt: None,
        }
    }

    /// Returns a copy with `payload_type` set.
    /// 返回 一个 copy 带有 `payload_type` 设置.
    pub fn with_payload_type(mut self, pt: u8) -> Self {
        self.target_pt = Some(pt);
        self
    }

    /// Rewrite an RTP packet's header fields for forwarding.
    ///
    /// Returns the rewritten packet, or None if the input is too short.
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

    /// `current_seq` function.
    /// `current_seq` 函数.
    pub fn current_seq(&self) -> u16 {
        self.next_seq
    }

    /// `target_ssrc` function.
    /// `target_ssrc` 函数.
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
