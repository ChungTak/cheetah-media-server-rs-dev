//! Property-based tests for GB28181 SIP message and SDP handling.
//!
//! These tests verify `SipMessage` serialization/parse stability and the
//! GB28181-specific `GbSdp` round-trip, which picks either a video or audio
//! media line based on `is_video`.
//!
//! GB28181 SIP 消息与 SDP 处理属性测试。
//!
//! 这些测试验证 `SipMessage` 序列化/解析的稳定性以及 GB28181 专用 `GbSdp` 的
//! 往返；`GbSdp` 根据 `is_video` 选择视频或音频媒体行。

use cheetah_gb28181_core::{GbSdp, SipMessage, StartLine};
use proptest::prelude::*;

/// Generate a valid method or header name token.
///
/// 生成有效的方法或头名称 token。
fn valid_identifier() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Za-z0-9\\-]+").expect("regex")
}

/// Generate a valid header value.
///
/// 生成有效头值。
fn valid_value() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Za-z0-9 \\-/:=]+").expect("regex")
}

/// Generate a valid URI without spaces.
///
/// 生成不含空格的有效 URI。
fn valid_uri() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Za-z0-9\\-/:=.]+").expect("regex")
}

/// Generate a valid unicast IP address.
///
/// 生成有效单播 IP 地址。
fn valid_ip() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("127.0.0.1".to_string()),
        Just("192.168.1.1".to_string()),
        Just("10.0.0.2".to_string()),
        Just("8.8.8.8".to_string()),
    ]
}

/// Generate an arbitrary SSRC.
///
/// 生成任意 SSRC。
fn valid_ssrc() -> impl Strategy<Value = u32> {
    any::<u32>()
}

/// Generate a UTF-8 text body (simulating SDP/XML).
///
/// 生成 UTF-8 文本 body（模拟 SDP/XML）。
fn valid_text_body() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Za-z0-9 \\-/:=\r\n]{0,256}").expect("regex")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// SIP message serialization and parse are stable and round-trip.
    ///
    /// SIP 消息序列化与解析稳定且可往返。
    #[test]
    fn test_sip_message_roundtrip(
        method in valid_identifier(),
        uri in valid_uri(),
        version in prop_oneof![Just("SIP/2.0".to_string())],
        headers in prop::collection::vec((valid_identifier(), valid_value()), 0..10),
        body_str in valid_text_body(),
    ) {
        let body = body_str.into_bytes();

        // Build a request StartLine.
        let start_line = StartLine::Request {
            method: method.clone(),
            uri: uri.clone(),
            version: version.clone(),
        };

        // Avoid any user-supplied Content-Length header; add the exact one.
        let mut final_headers = Vec::new();
        for (k, v) in headers {
            if !k.eq_ignore_ascii_case("content-length") {
                final_headers.push((k, v));
            }
        }
        final_headers.push(("Content-Length".to_string(), body.len().to_string()));

        let msg = SipMessage {
            start_line,
            headers: final_headers,
            body: body.clone(),
        };

        let msg_str = msg.to_string();
        let parsed = SipMessage::parse(&msg_str).expect("SIP parse should succeed");

        if let StartLine::Request {
            method: parsed_method,
            uri: parsed_uri,
            version: parsed_ver,
        } = parsed.start_line
        {
            prop_assert_eq!(parsed_method, method);
            prop_assert_eq!(parsed_uri, uri);
            prop_assert_eq!(parsed_ver, version);
        } else {
            prop_assert!(false, "StartLine must be Request");
        }

        prop_assert_eq!(parsed.body, body);
    }

    /// `GbSdp` build/parse round-trip is robust over parameter variations.
    ///
    /// `GbSdp` 构造/解析往返在参数变化下保持鲁棒。
    #[test]
    fn test_gb_sdp_roundtrip(
        session_id in valid_identifier(),
        ip in valid_ip(),
        port in 1..65535_u16,
        ssrc in valid_ssrc(),
        is_video in any::<bool>(),
        mode in prop_oneof![
            Just("recvonly".to_string()),
            Just("sendonly".to_string()),
            Just("sendrecv".to_string()),
        ],
    ) {
        let sdp_text = GbSdp::to_string(&session_id, &ip, port, ssrc, is_video, &mode);
        let parsed = GbSdp::parse(&sdp_text).expect("GbSdp parse should succeed");

        prop_assert_eq!(parsed.ip, ip);
        prop_assert_eq!(parsed.sendrecv_mode, mode);
        prop_assert_eq!(parsed.ssrc, Some(ssrc));

        if is_video {
            prop_assert_eq!(parsed.video_port, Some(port));
            prop_assert_eq!(parsed.audio_port, None);
        } else {
            prop_assert_eq!(parsed.audio_port, Some(port));
            prop_assert_eq!(parsed.video_port, None);
        }
    }
}
