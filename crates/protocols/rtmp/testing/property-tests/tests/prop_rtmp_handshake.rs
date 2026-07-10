//! Property-based tests for the RTMP handshake state machines.
//!
//! RTMP handshake is a three-step exchange: C0/S0 (1 byte version = 3),
//! C1/S1 (1536 bytes), and C2/S2 (1536 bytes echo). These tests verify the
//! complete exchange, partial-data handling, invalid version rejection, and
//! echo-byte verification for C2/S2.
//!
//! RTMP 握手状态机的属性测试。
//!
//! RTMP 握手是三步交换：C0/S0（1 字节版本号 = 3）、C1/S1（1536 字节）、C2/S2（1536 字节回显）。
//! 这些测试校验完整交换、部分数据处理、无效版本拒绝以及 C2/S2 回显字节校验。

use cheetah_rtmp_core::{RtmpClientHandshake, RtmpServerHandshake};
use proptest::prelude::*;

// RTMP specification constants
const RTMP_VERSION: u8 = 3;
const HANDSHAKE_PACKET_SIZE: usize = 1536;

// =============================================================================
// Happy-path tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Verify the complete client-server handshake exchange succeeds.
    ///
    /// 校验完整的客户端-服务端握手交换成功。
    #[test]
    fn complete_handshake_roundtrip(_dummy in Just(())) {
        let mut client = RtmpClientHandshake::new();
        let c0_c1 = client.send_buf().to_vec();
        client.advance_send_buf(c0_c1.len());

        prop_assert_eq!(c0_c1.len(), 1 + HANDSHAKE_PACKET_SIZE,
            "C0 + C1 should be 1 + 1536 = 1537 bytes");
        prop_assert_eq!(c0_c1[0], RTMP_VERSION, "C0 should be RTMP version 3");

        let mut server = RtmpServerHandshake::new();
        server.feed_recv_buf(&c0_c1).expect("server should accept C0 + C1");

        let s0_s1_s2 = server.send_buf().to_vec();
        server.advance_send_buf(s0_s1_s2.len());

        prop_assert_eq!(s0_s1_s2.len(), 1 + HANDSHAKE_PACKET_SIZE * 2,
            "S0 + S1 + S2 should be 1 + 1536 + 1536 = 3073 bytes");
        prop_assert_eq!(s0_s1_s2[0], RTMP_VERSION, "S0 should be RTMP version 3");

        client.feed_recv_buf(&s0_s1_s2).expect("client should accept S0 + S1 + S2");

        let c2 = client.send_buf().to_vec();
        client.advance_send_buf(c2.len());

        prop_assert_eq!(c2.len(), HANDSHAKE_PACKET_SIZE,
            "C2 should be 1536 bytes");

        server.feed_recv_buf(&c2).expect("server should accept C2");

        prop_assert!(client.is_recv_complete(), "client should have completed receiving");
        prop_assert!(client.is_send_complete(), "client should have completed sending");
        prop_assert!(server.is_recv_complete(), "server should have completed receiving");
        prop_assert!(server.is_send_complete(), "server should have completed sending");
    }
}

// =============================================================================
// Invalid version tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Verify that the server rejects any RTMP version other than 3.
    ///
    /// 校验服务端拒绝除 3 以外的任何 RTMP 版本。
    #[test]
    fn invalid_version_rejected_by_server(
        version in (0u8..=255u8).prop_filter("not valid version", |&v| v != RTMP_VERSION)
    ) {
        let mut server = RtmpServerHandshake::new();

        let result = server.feed_recv_buf(&[version]);

        prop_assert!(result.is_err(), "server should reject invalid RTMP version {}", version);
    }

    /// Verify that the client rejects any RTMP version other than 3 in S0.
    ///
    /// 校验客户端拒绝 S0 中除 3 以外的任何 RTMP 版本。
    #[test]
    fn invalid_version_rejected_by_client(
        version in (0u8..=255u8).prop_filter("not valid version", |&v| v != RTMP_VERSION)
    ) {
        let mut client = RtmpClientHandshake::new();

        let result = client.feed_recv_buf(&[version]);

        prop_assert!(result.is_err(), "client should reject invalid RTMP version {}", version);
    }
}

// =============================================================================
// Partial data tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Verify that the server buffers a partial C1 without erroring.
    ///
    /// 校验服务端缓冲部分 C1 而不报错。
    #[test]
    fn server_handles_partial_c1(partial_size in 1usize..HANDSHAKE_PACKET_SIZE) {
        let mut server = RtmpServerHandshake::new();

        server.feed_recv_buf(&[RTMP_VERSION]).expect("C0 should be accepted");

        let partial_c1 = vec![0u8; partial_size];
        server.feed_recv_buf(&partial_c1).expect("partial C1 should be buffered");

        prop_assert!(!server.is_recv_complete(),
            "server should not complete with partial C1 ({} bytes)", partial_size);
    }

    /// Verify that the client buffers a partial S1+S2 without erroring.
    ///
    /// 校验客户端缓冲部分 S1+S2 而不报错。
    #[test]
    fn client_handles_partial_s1_s2(partial_size in 1usize..(HANDSHAKE_PACKET_SIZE * 2)) {
        let mut client = RtmpClientHandshake::new();

        client.feed_recv_buf(&[RTMP_VERSION]).expect("S0 should be accepted");

        let partial_data = vec![0u8; partial_size];
        client.feed_recv_buf(&partial_data).expect("partial S1+S2 should be buffered");

        prop_assert!(!client.is_recv_complete(),
            "client should not complete with partial S1+S2 ({} bytes)", partial_size);
    }

    /// Verify that empty input is accepted and leaves the handshake incomplete.
    ///
    /// 校验空输入被接受，握手保持未完成。
    #[test]
    fn empty_data_handled(_dummy in Just(())) {
        let mut server = RtmpServerHandshake::new();
        let mut client = RtmpClientHandshake::new();

        server.feed_recv_buf(&[]).expect("server should handle empty data");
        client.feed_recv_buf(&[]).expect("client should handle empty data");

        prop_assert!(!server.is_recv_complete());
        prop_assert!(!client.is_recv_complete());
    }
}

// =============================================================================
// C2/S2 echo verification tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// Verify that the server rejects a C2 that does not echo S1.
    ///
    /// 校验服务端拒绝未回显 S1 的 C2。
    #[test]
    fn invalid_c2_rejected_by_server(
        modification_index in 0usize..HANDSHAKE_PACKET_SIZE,
        modification_value in 1u8..=255u8
    ) {
        let mut client = RtmpClientHandshake::new();
        let c0_c1 = client.send_buf().to_vec();
        client.advance_send_buf(c0_c1.len());

        let mut server = RtmpServerHandshake::new();
        server.feed_recv_buf(&c0_c1).expect("C0 + C1 should be accepted");

        let s0_s1_s2 = server.send_buf().to_vec();
        server.advance_send_buf(s0_s1_s2.len());

        let s1 = &s0_s1_s2[1..1 + HANDSHAKE_PACKET_SIZE];

        let mut invalid_c2 = s1.to_vec();
        invalid_c2[modification_index] = invalid_c2[modification_index].wrapping_add(modification_value);

        let result = server.feed_recv_buf(&invalid_c2);

        prop_assert!(result.is_err(),
            "server should reject C2 that doesn't match S1 (modified at index {})",
            modification_index);
    }

    /// Verify that the client rejects an S2 that does not echo C1.
    ///
    /// 校验客户端拒绝未回显 C1 的 S2。
    #[test]
    fn invalid_s2_rejected_by_client(
        modification_index in 0usize..HANDSHAKE_PACKET_SIZE,
        modification_value in 1u8..=255u8
    ) {
        let mut client = RtmpClientHandshake::new();
        let c0_c1 = client.send_buf().to_vec();

        let c1 = &c0_c1[1..];

        let mut s0_s1_s2 = vec![RTMP_VERSION];
        s0_s1_s2.extend_from_slice(&[0u8; HANDSHAKE_PACKET_SIZE]);

        let mut invalid_s2 = c1.to_vec();
        invalid_s2[modification_index] = invalid_s2[modification_index].wrapping_add(modification_value);
        s0_s1_s2.extend_from_slice(&invalid_s2);

        let result = client.feed_recv_buf(&s0_s1_s2);

        prop_assert!(result.is_err(),
            "client should reject S2 that doesn't match C1 (modified at index {})",
            modification_index);
    }
}

// =============================================================================
// Segmented delivery tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// Verify that the handshake completes when data is delivered in small chunks.
    ///
    /// 校验数据以小块分段交付时握手仍能完成。
    #[test]
    fn chunked_handshake(chunk_size in 1usize..100usize) {
        let mut client = RtmpClientHandshake::new();
        let c0_c1 = client.send_buf().to_vec();
        client.advance_send_buf(c0_c1.len());

        let mut server = RtmpServerHandshake::new();

        for chunk in c0_c1.chunks(chunk_size) {
            server.feed_recv_buf(chunk).expect("chunk should be accepted");
        }

        let s0_s1_s2 = server.send_buf().to_vec();
        server.advance_send_buf(s0_s1_s2.len());

        for chunk in s0_s1_s2.chunks(chunk_size) {
            client.feed_recv_buf(chunk).expect("chunk should be accepted");
        }

        let c2 = client.send_buf().to_vec();
        client.advance_send_buf(c2.len());

        for chunk in c2.chunks(chunk_size) {
            server.feed_recv_buf(chunk).expect("chunk should be accepted");
        }

        prop_assert!(client.is_recv_complete() && client.is_send_complete(),
            "client handshake should be complete");
        prop_assert!(server.is_recv_complete() && server.is_send_complete(),
            "server handshake should be complete");
    }
}
