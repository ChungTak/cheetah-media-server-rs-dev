//! RTMP Handshake 的 Property-Based Testing
//!
//! RTMP 规范 Section 5.2: Handshake
//!
//! 握手序列:
//! - C0/S0: 1 byte (RTMP version, 始终为 3)
//! - C1/S1: 1536 bytes (timestamp + version + random data)
//! - C2/S2: 1536 bytes (C1/S1 的回显)
//!
//! 按照规范，测试以下内容:
//! - 正常的握手序列
//! - 对无效版本的错误处理
//! - 对部分数据的容错

use cheetah_rtmp_core::{RtmpClientHandshake, RtmpServerHandshake};
use proptest::prelude::*;

// RTMP 规范常量
const RTMP_VERSION: u8 = 3;
const HANDSHAKE_PACKET_SIZE: usize = 1536;

// =============================================================================
// 正常系测试
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// 正常的客户端-服务器握手完成测试
    #[test]
    fn complete_handshake_roundtrip(_dummy in Just(())) {
        // 客户端: 发送 C0 + C1
        let mut client = RtmpClientHandshake::new();
        let c0_c1 = client.send_buf().to_vec();
        client.advance_send_buf(c0_c1.len());

        // 验证: C0 + C1 的长度
        prop_assert_eq!(c0_c1.len(), 1 + HANDSHAKE_PACKET_SIZE,
            "C0 + C1 should be 1 + 1536 = 1537 bytes");

        // 验证: C0 是 RTMP version 3
        prop_assert_eq!(c0_c1[0], RTMP_VERSION, "C0 should be RTMP version 3");

        // 服务器: 接收 C0 + C1
        let mut server = RtmpServerHandshake::new();
        server.feed_recv_buf(&c0_c1).expect("server should accept C0 + C1");

        // 服务器: 发送 S0 + S1 + S2
        let s0_s1_s2 = server.send_buf().to_vec();
        server.advance_send_buf(s0_s1_s2.len());

        // 验证: S0 + S1 + S2 的长度
        prop_assert_eq!(s0_s1_s2.len(), 1 + HANDSHAKE_PACKET_SIZE * 2,
            "S0 + S1 + S2 should be 1 + 1536 + 1536 = 3073 bytes");

        // 验证: S0 是 RTMP version 3
        prop_assert_eq!(s0_s1_s2[0], RTMP_VERSION, "S0 should be RTMP version 3");

        // 客户端: 接收 S0 + S1 + S2
        client.feed_recv_buf(&s0_s1_s2).expect("client should accept S0 + S1 + S2");

        // 客户端: 发送 C2
        let c2 = client.send_buf().to_vec();
        client.advance_send_buf(c2.len());

        // 验证: C2 的长度
        prop_assert_eq!(c2.len(), HANDSHAKE_PACKET_SIZE,
            "C2 should be 1536 bytes");

        // 服务器: 接收 C2
        server.feed_recv_buf(&c2).expect("server should accept C2");

        // 验证: 双方握手完成
        prop_assert!(client.is_recv_complete(), "client should have completed receiving");
        prop_assert!(client.is_send_complete(), "client should have completed sending");
        prop_assert!(server.is_recv_complete(), "server should have completed receiving");
        prop_assert!(server.is_send_complete(), "server should have completed sending");
    }
}

// =============================================================================
// 异常系测试 (无效版本)
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// 对无效 RTMP 版本的服务器错误处理
    #[test]
    fn invalid_version_rejected_by_server(
        version in (0u8..=255u8).prop_filter("not valid version", |&v| v != RTMP_VERSION)
    ) {
        let mut server = RtmpServerHandshake::new();

        // 发送无效版本的 C0
        let result = server.feed_recv_buf(&[version]);

        prop_assert!(result.is_err(), "server should reject invalid RTMP version {}", version);
    }

    /// 对无效 RTMP 版本的客户端错误处理
    #[test]
    fn invalid_version_rejected_by_client(
        version in (0u8..=255u8).prop_filter("not valid version", |&v| v != RTMP_VERSION)
    ) {
        let mut client = RtmpClientHandshake::new();

        // 发送无效版本的 S0
        let result = client.feed_recv_buf(&[version]);

        prop_assert!(result.is_err(), "client should reject invalid RTMP version {}", version);
    }
}

// =============================================================================
// 部分数据测试
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// 对部分 C1 数据的服务器容错测试
    /// 规范: 等待数据到齐，不报错
    #[test]
    fn server_handles_partial_c1(partial_size in 1usize..HANDSHAKE_PACKET_SIZE) {
        let mut server = RtmpServerHandshake::new();

        // 发送正确的 C0
        server.feed_recv_buf(&[RTMP_VERSION]).expect("C0 should be accepted");

        // 发送部分的 C1
        let partial_c1 = vec![0u8; partial_size];
        server.feed_recv_buf(&partial_c1).expect("partial C1 should be buffered");

        // 验证: 尚未完成
        prop_assert!(!server.is_recv_complete(),
            "server should not complete with partial C1 ({} bytes)", partial_size);
    }

    /// 对部分 S1+S2 数据的客户端容错测试
    #[test]
    fn client_handles_partial_s1_s2(partial_size in 1usize..(HANDSHAKE_PACKET_SIZE * 2)) {
        let mut client = RtmpClientHandshake::new();

        // 发送正确的 S0
        client.feed_recv_buf(&[RTMP_VERSION]).expect("S0 should be accepted");

        // 发送部分的 S1+S2
        let partial_data = vec![0u8; partial_size];
        client.feed_recv_buf(&partial_data).expect("partial S1+S2 should be buffered");

        // 验证: 尚未完成
        prop_assert!(!client.is_recv_complete(),
            "client should not complete with partial S1+S2 ({} bytes)", partial_size);
    }

    /// 空数据容错测试
    #[test]
    fn empty_data_handled(_dummy in Just(())) {
        let mut server = RtmpServerHandshake::new();
        let mut client = RtmpClientHandshake::new();

        // 发送空数据不会报错
        server.feed_recv_buf(&[]).expect("server should handle empty data");
        client.feed_recv_buf(&[]).expect("client should handle empty data");

        // 验证: 尚未完成
        prop_assert!(!server.is_recv_complete());
        prop_assert!(!client.is_recv_complete());
    }
}

// =============================================================================
// C2/S2 验证测试
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// 对无效 C2 (与 S1 不一致) 的服务器错误处理
    #[test]
    fn invalid_c2_rejected_by_server(
        modification_index in 0usize..HANDSHAKE_PACKET_SIZE,
        modification_value in 1u8..=255u8
    ) {
        // 执行正常握手到中途
        let mut client = RtmpClientHandshake::new();
        let c0_c1 = client.send_buf().to_vec();
        client.advance_send_buf(c0_c1.len());

        let mut server = RtmpServerHandshake::new();
        server.feed_recv_buf(&c0_c1).expect("C0 + C1 should be accepted");

        let s0_s1_s2 = server.send_buf().to_vec();
        server.advance_send_buf(s0_s1_s2.len());

        // 获取 S1 (S0 之后、S2 之前)
        let s1 = &s0_s1_s2[1..1 + HANDSHAKE_PACKET_SIZE];

        // 创建篡改了 S1 的 C2 (规范: C2 应为 S1 的回显)
        let mut invalid_c2 = s1.to_vec();
        invalid_c2[modification_index] = invalid_c2[modification_index].wrapping_add(modification_value);

        // 发送无效的 C2
        let result = server.feed_recv_buf(&invalid_c2);

        prop_assert!(result.is_err(),
            "server should reject C2 that doesn't match S1 (modified at index {})",
            modification_index);
    }

    /// 对无效 S2 (与 C1 不一致) 的客户端错误处理
    #[test]
    fn invalid_s2_rejected_by_client(
        modification_index in 0usize..HANDSHAKE_PACKET_SIZE,
        modification_value in 1u8..=255u8
    ) {
        let mut client = RtmpClientHandshake::new();
        let c0_c1 = client.send_buf().to_vec();

        // 获取 C1 (C0 之后)
        let c1 = &c0_c1[1..];

        // 创建正确的 S0 + S1
        let mut s0_s1_s2 = vec![RTMP_VERSION]; // S0
        s0_s1_s2.extend_from_slice(&[0u8; HANDSHAKE_PACKET_SIZE]); // S1

        // 创建篡改了 C1 的 S2 (规范: S2 应为 C1 的回显)
        let mut invalid_s2 = c1.to_vec();
        invalid_s2[modification_index] = invalid_s2[modification_index].wrapping_add(modification_value);
        s0_s1_s2.extend_from_slice(&invalid_s2);

        // 发送无效的 S0 + S1 + S2
        let result = client.feed_recv_buf(&s0_s1_s2);

        prop_assert!(result.is_err(),
            "client should reject S2 that doesn't match C1 (modified at index {})",
            modification_index);
    }
}

// =============================================================================
// 字节流分割测试
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// 分多次发送数据的情况测试
    #[test]
    fn chunked_handshake(chunk_size in 1usize..100usize) {
        let mut client = RtmpClientHandshake::new();
        let c0_c1 = client.send_buf().to_vec();
        client.advance_send_buf(c0_c1.len());

        let mut server = RtmpServerHandshake::new();

        // 将 C0 + C1 分成小块发送
        for chunk in c0_c1.chunks(chunk_size) {
            server.feed_recv_buf(chunk).expect("chunk should be accepted");
        }

        let s0_s1_s2 = server.send_buf().to_vec();
        server.advance_send_buf(s0_s1_s2.len());

        // 将 S0 + S1 + S2 分成小块发送
        for chunk in s0_s1_s2.chunks(chunk_size) {
            client.feed_recv_buf(chunk).expect("chunk should be accepted");
        }

        let c2 = client.send_buf().to_vec();
        client.advance_send_buf(c2.len());

        // 将 C2 分成小块发送
        for chunk in c2.chunks(chunk_size) {
            server.feed_recv_buf(chunk).expect("chunk should be accepted");
        }

        // 验证: 双方握手完成
        prop_assert!(client.is_recv_complete() && client.is_send_complete(),
            "client handshake should be complete");
        prop_assert!(server.is_recv_complete() && server.is_send_complete(),
            "server handshake should be complete");
    }
}
