//! Property-based tests for RTSP RTP sequence tracking and RTCP feedback.
//!
//! These tests exercise `SeqTracker` (loss detection, reset thresholds, gap
//! counting), `RtpRewriter` (SSRC/sequence rewrite), and RTCP feedback builders
//! for NACK, PLI, and FIR.
//!
//! RTSP RTP 序列追踪与 RTCP 反馈属性测试。
//!
//! 这些测试覆盖 `SeqTracker`（丢包检测、重置阈值、间隔计数）、`RtpRewriter`
//!（SSRC/序列号重写）以及 NACK、PLI、FIR 的 RTCP 反馈构造器。

use cheetah_rtsp_core::{
    build_rtcp_fir, build_rtcp_nack, build_rtcp_pli, nack_items_from_lost_seqs, parse_rtcp_fb,
    FirEntry, NackItem, RtcpFeedback, RtcpFir, RtcpNack, RtcpPli, RtpRewriter, SeqEvent,
    SeqTracker, PSFB_FMT_FIR, PSFB_FMT_PLI, RTCP_PT_PSFB, RTCP_PT_RTPFB, RTPFB_FMT_NACK,
};
use proptest::prelude::*;

// --- SeqTracker property tests ---

proptest! {
    /// A monotonic sequence starting from any point never reports loss.
    ///
    /// 从任意起点开始的单调序列不会报告丢包。
    #[test]
    fn seq_tracker_monotonic_sequence_never_reports_loss(start in 0u16..=65000u16, count in 1u16..=500u16) {
        let mut tracker = SeqTracker::new();
        let mut seq = start;
        for i in 0..count {
            let event = tracker.update(seq);
            if i == 0 {
                prop_assert_eq!(event, SeqEvent::Initial);
            } else {
                prop_assert!(
                    matches!(event, SeqEvent::Normal | SeqEvent::Wrap),
                    "unexpected event at i={i}: {event:?}"
                );
            }
            seq = seq.wrapping_add(1);
        }
        prop_assert_eq!(tracker.total_lost(), 0);
    }

    /// Total accepted packets equals the number of update calls.
    ///
    /// 已接受包总数等于 update 调用次数。
    #[test]
    fn seq_tracker_total_packets_equals_update_count(start in 0u16..=65535u16, count in 1u32..=1000u32) {
        let mut tracker = SeqTracker::new();
        let mut seq = start;
        for _ in 0..count {
            tracker.update(seq);
            seq = seq.wrapping_add(1);
        }
        prop_assert_eq!(tracker.total_packets(), u64::from(count));
    }

    /// A jump larger than the reset threshold is reported as a stream reset.
    ///
    /// 大于重置阈值的跳变会被报告为流重置。
    #[test]
    fn seq_tracker_detects_reset_for_large_jumps(
        start in 0u16..=10000u16,
        jump in 5002u16..=60000u16
    ) {
        let mut tracker = SeqTracker::with_threshold(5000);
        tracker.update(start);
        tracker.update(start.wrapping_add(1));
        let event = tracker.update(start.wrapping_add(jump));
        prop_assert_eq!(event, SeqEvent::Reset);
    }

    /// A small gap reports the correct lost count.
    ///
    /// 小间隔报告正确的丢包数。
    #[test]
    fn seq_tracker_gap_reports_correct_lost_count(
        start in 0u16..=60000u16,
        gap in 2u16..=100u16
    ) {
        let mut tracker = SeqTracker::new();
        tracker.update(start);
        let event = tracker.update(start.wrapping_add(gap));
        match event {
            SeqEvent::Gap { lost } => prop_assert_eq!(lost, gap - 1),
            SeqEvent::Reset => {} // large gap may trigger reset
            _ => prop_assert!(false, "expected Gap or Reset, got {event:?}"),
        }
    }
}

// --- RtpRewriter property tests ---

/// Build a minimal 12-byte RTP header with the given sequence, timestamp, and SSRC.
///
/// 用给定序列号、时间戳和 SSRC 构造最小 12 字节 RTP 头。
fn make_rtp(seq: u16, ts: u32, ssrc: u32) -> Vec<u8> {
    let mut pkt = vec![0u8; 16];
    pkt[0] = 0x80;
    pkt[1] = 96;
    pkt[2] = (seq >> 8) as u8;
    pkt[3] = seq as u8;
    pkt[4..8].copy_from_slice(&ts.to_be_bytes());
    pkt[8..12].copy_from_slice(&ssrc.to_be_bytes());
    pkt[12..16].copy_from_slice(b"data");
    pkt
}

proptest! {
    /// The rewriter emits a monotonically increasing sequence number.
    ///
    /// 重写器发出单调递增的序列号。
    #[test]
    fn rtp_rewriter_seq_is_monotonic(
        target_ssrc in 1u32..=u32::MAX,
        initial_seq in 0u16..=65535u16,
        count in 1u16..=500u16
    ) {
        let mut rewriter = RtpRewriter::new(target_ssrc, initial_seq);
        let mut expected_seq = initial_seq;
        for i in 0..count {
            let pkt = make_rtp(i * 7, i as u32 * 160, 0x12345678);
            let out = rewriter.rewrite(&pkt).unwrap();
            let out_seq = u16::from_be_bytes([out[2], out[3]]);
            prop_assert_eq!(out_seq, expected_seq);
            expected_seq = expected_seq.wrapping_add(1);
        }
    }

    /// The rewriter preserves timestamp and payload while rewriting the SSRC.
    ///
    /// 重写器保留时间戳与 payload，同时重写 SSRC。
    #[test]
    fn rtp_rewriter_preserves_timestamp_and_payload(
        ts in 0u32..=u32::MAX,
        ssrc in 0u32..=u32::MAX
    ) {
        let mut rewriter = RtpRewriter::new(0xAAAA, 0);
        let pkt = make_rtp(999, ts, ssrc);
        let out = rewriter.rewrite(&pkt).unwrap();
        // Timestamp preserved
        prop_assert_eq!(&out[4..8], &ts.to_be_bytes());
        // Payload preserved
        prop_assert_eq!(&out[12..16], b"data");
        // SSRC rewritten
        prop_assert_eq!(&out[8..12], &0xAAAAu32.to_be_bytes());
    }

    /// Packets shorter than the fixed 12-byte RTP header are rejected.
    ///
    /// 短于 12 字节固定 RTP 头的包被拒绝。
    #[test]
    fn rtp_rewriter_rejects_short_packets(len in 0usize..12usize) {
        let mut rewriter = RtpRewriter::new(1, 0);
        let pkt = vec![0u8; len];
        prop_assert!(rewriter.rewrite(&pkt).is_none());
    }
}

// --- RTCP-FB property tests ---

proptest! {
    /// NACK build/parse round-trip preserves the original items.
    ///
    /// NACK 构造/解析往返保留原始条目。
    #[test]
    fn nack_roundtrip(
        sender_ssrc in 0u32..=u32::MAX,
        media_ssrc in 0u32..=u32::MAX,
        pid in 0u16..=u16::MAX,
        blp in 0u16..=u16::MAX
    ) {
        let nack = RtcpNack {
            sender_ssrc,
            media_ssrc,
            nack_items: vec![NackItem { pid, blp }],
        };
        let encoded = build_rtcp_nack(&nack);
        let parsed = parse_rtcp_fb(RTCP_PT_RTPFB, RTPFB_FMT_NACK, &encoded[4..]);
        prop_assert_eq!(parsed, Some(RtcpFeedback::Nack(nack)));
    }

    /// PLI build/parse round-trip preserves the sender/media SSRC pair.
    ///
    /// PLI 构造/解析往返保留 sender/media SSRC 对。
    #[test]
    fn pli_roundtrip(
        sender_ssrc in 0u32..=u32::MAX,
        media_ssrc in 0u32..=u32::MAX
    ) {
        let pli = RtcpPli { sender_ssrc, media_ssrc };
        let encoded = build_rtcp_pli(&pli);
        let parsed = parse_rtcp_fb(RTCP_PT_PSFB, PSFB_FMT_PLI, &encoded[4..]);
        prop_assert_eq!(parsed, Some(RtcpFeedback::Pli(pli)));
    }

    /// FIR build/parse round-trip preserves the FCI entry list.
    ///
    /// FIR 构造/解析往返保留 FCI 条目列表。
    #[test]
    fn fir_roundtrip(
        sender_ssrc in 0u32..=u32::MAX,
        media_ssrc in 0u32..=u32::MAX,
        fci_ssrc in 0u32..=u32::MAX,
        seq_nr in 0u8..=u8::MAX
    ) {
        let fir = RtcpFir {
            sender_ssrc,
            media_ssrc,
            fci: vec![FirEntry { ssrc: fci_ssrc, seq_nr }],
        };
        let encoded = build_rtcp_fir(&fir);
        let parsed = parse_rtcp_fb(RTCP_PT_PSFB, PSFB_FMT_FIR, &encoded[4..]);
        prop_assert_eq!(parsed, Some(RtcpFeedback::Fir(fir)));
    }

    /// `nack_items_from_lost_seqs` covers every unique input sequence number.
    ///
    /// `nack_items_from_lost_seqs` 覆盖每个唯一输入序列号。
    #[test]
    fn nack_items_from_lost_seqs_covers_all_inputs(
        lost_seqs in proptest::collection::vec(0u16..=u16::MAX, 0..50)
    ) {
        let items = nack_items_from_lost_seqs(&lost_seqs);
        // Every unique input seq should appear in some item's lost_seqs()
        let mut deduped = lost_seqs.clone();
        deduped.sort_unstable();
        deduped.dedup();
        let recovered: Vec<u16> = items.iter().flat_map(|item| item.lost_seqs()).collect();
        for seq in &deduped {
            prop_assert!(
                recovered.contains(seq),
                "seq {seq} not found in NACK items"
            );
        }
    }
}
