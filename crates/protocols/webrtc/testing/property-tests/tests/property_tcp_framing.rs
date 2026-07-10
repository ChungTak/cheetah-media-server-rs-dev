//! Property tests for the WebRTC-over-TCP RFC 4571 framing decoder.
//!
//! Phase 05 promises:
//!
//! * The decoder never panics on arbitrary byte inputs (length prefix,
//!   zero-length frames, truncated frames, oversized advertised lengths).
//! * Encode/decode round-trip preserves payload bytes verbatim.
//! * Splitting a long encoded byte stream at any boundary still yields the
//!   same set of decoded frames as feeding the entire buffer at once.
//!
//! These properties match how a real TCP socket delivers bytes to the driver:
//! kernel buffer boundaries fragment the stream at arbitrary offsets, so the
//! decoder must be robust to partial reads.
//!
//! WebRTC-over-TCP RFC 4571 成帧解码器属性测试。
//!
//! 阶段 05 承诺：
//! * 解码器对任意字节输入不 panic（长度前缀、零长度帧、截断帧、超大声明长度）。
//! * 编码/解码往返保持 payload 字节逐字一致。
//! * 在任意边界切分长编码字节流仍产生与一次性输入整缓冲相同的解码帧集合。
//!
//! 这些属性与真实 TCP socket 向 driver 交付字节的方式一致：内核缓冲区边界会
//! 在任意偏移处分片流，因此解码器必须能处理部分读取。

use cheetah_webrtc_driver_tokio::{tcp_encode_frame, Tcp4571Decoder};
use proptest::prelude::*;

/// Generate an arbitrary payload byte vector.
///
/// 生成任意 payload 字节向量。
fn arb_payload() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), 0..256)
}

/// Generate a list of arbitrary payloads.
///
/// 生成任意 payload 列表。
fn arb_payloads() -> impl Strategy<Value = Vec<Vec<u8>>> {
    proptest::collection::vec(arb_payload(), 0..16)
}

proptest! {
    /// Feeding arbitrary bytes never panics. The decoder either surfaces complete
    /// frames, returns `Ok(None)` to wait for more bytes, or rejects the stream
    /// with a structured error.
    ///
    /// 输入任意字节不 panic。解码器要么输出完整帧，要么返回 `Ok(None)` 等待更多
    /// 字节，要么以结构化错误拒绝流。
    #[test]
    fn decoder_does_not_panic(input in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let mut dec = Tcp4571Decoder::new();
        dec.extend(&input);
        // Drain at most one frame to avoid long runs on synthetic inputs that
        // happen to advertise 65535-byte frames; we only need non-panic.
        let _ = dec.next_frame();
    }

    /// Encode followed by decode yields the original payload bytes.
    ///
    /// 编码后解码得到原始 payload 字节。
    #[test]
    fn encode_then_decode_roundtrips(payload in arb_payload()) {
        let framed = tcp_encode_frame(&payload).expect("payload below 64KiB encodes");
        let mut dec = Tcp4571Decoder::new();
        dec.extend(&framed);
        let decoded = dec.next_frame().expect("decode result").expect("frame present");
        prop_assert_eq!(decoded.as_ref(), payload.as_slice());
        // No bytes left over.
        prop_assert!(dec.next_frame().expect("trailing decode").is_none());
    }

    /// Concatenated encoded frames decode back to the same payload list,
    /// regardless of how the byte stream is fragmented across `extend` calls.
    ///
    /// 连接后的编码帧解码回相同 payload 列表，与 `extend` 调用之间的字节流分片无关。
    #[test]
    fn concatenated_frames_decode_independently_of_fragmentation(
        payloads in arb_payloads(),
        chunk_size in 1usize..32,
    ) {
        // Build the concatenated wire bytes.
        let mut wire = Vec::new();
        for payload in &payloads {
            let framed = tcp_encode_frame(payload).expect("encode");
            wire.extend_from_slice(&framed);
        }

        // Feed in `chunk_size` slices to mimic fragmented kernel reads.
        let mut dec = Tcp4571Decoder::new();
        let mut decoded: Vec<Vec<u8>> = Vec::new();
        for chunk in wire.chunks(chunk_size) {
            dec.extend(chunk);
            while let Ok(Some(frame)) = dec.next_frame() {
                decoded.push(frame.to_vec());
            }
        }

        // Decoded list must match the original payload list exactly.
        prop_assert_eq!(decoded.len(), payloads.len());
        for (got, want) in decoded.iter().zip(payloads.iter()) {
            prop_assert_eq!(got.as_slice(), want.as_slice());
        }
    }

    /// A single `extend` of the entire buffer yields the same decoded list as
    /// fragmented `extend` calls. This is the dual of the previous property.
    ///
    /// 单次 `extend` 输入整缓冲与分片 `extend` 产生相同解码列表。这是前一条属性的
    /// 对偶。
    #[test]
    fn full_extend_matches_fragmented_extend(
        payloads in arb_payloads(),
    ) {
        let mut wire = Vec::new();
        for p in &payloads {
            wire.extend_from_slice(&tcp_encode_frame(p).expect("encode"));
        }

        let mut full = Tcp4571Decoder::new();
        full.extend(&wire);
        let mut full_out: Vec<Vec<u8>> = Vec::new();
        while let Ok(Some(frame)) = full.next_frame() {
            full_out.push(frame.to_vec());
        }

        let mut split = Tcp4571Decoder::new();
        for byte in &wire {
            split.extend(std::slice::from_ref(byte));
            while let Ok(Some(frame)) = split.next_frame() {
                // We do not collect here — drain whenever frames become ready
                // so the comparison below runs against the steady-state output.
                let _ = frame;
            }
        }

        // Build a second decoded list by fragmented input.
        let mut split2 = Tcp4571Decoder::new();
        let mut split_out: Vec<Vec<u8>> = Vec::new();
        for byte in &wire {
            split2.extend(std::slice::from_ref(byte));
            while let Ok(Some(frame)) = split2.next_frame() {
                split_out.push(frame.to_vec());
            }
        }

        prop_assert_eq!(full_out, split_out);
    }
}
