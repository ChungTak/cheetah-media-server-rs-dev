use cheetah_gb28181_core::{GbSdp, SipMessage, StartLine};
use proptest::prelude::*;

/// 生成有效的方法或标头名称字符串。
fn valid_identifier() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Za-z0-9\\-]+").expect("regex")
}

/// 生成有效的值。
fn valid_value() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Za-z0-9 \\-/:=]+").expect("regex")
}

/// 生成有效的 URI（不含空格）。
fn valid_uri() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Za-z0-9\\-/:=.]+").expect("regex")
}

/// 生成有效 IP 地址。
fn valid_ip() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("127.0.0.1".to_string()),
        Just("192.168.1.1".to_string()),
        Just("10.0.0.2".to_string()),
        Just("8.8.8.8".to_string()),
    ]
}

/// 生成有效 SSRC。
fn valid_ssrc() -> impl Strategy<Value = u32> {
    any::<u32>()
}

/// 生成合法的 UTF-8 文本 Body（模拟 SDP/XML）。
fn valid_text_body() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Za-z0-9 \\-/:=\r\n]{0,256}").expect("regex")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// 测试 SIP 消息序列化和反序列化的一致性与稳定性
    #[test]
    fn test_sip_message_roundtrip(
        method in valid_identifier(),
        uri in valid_uri(),
        version in prop_oneof![Just("SIP/2.0".to_string())],
        headers in prop::collection::vec((valid_identifier(), valid_value()), 0..10),
        body_str in valid_text_body(),
    ) {
        let body = body_str.into_bytes();

        // 构建 StartLine::Request
        let start_line = StartLine::Request {
            method: method.clone(),
            uri: uri.clone(),
            version: version.clone(),
        };

        // 排除可能破坏 content-length 计算的 headers
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

        // 序列化
        let msg_str = msg.to_string();

        // 反序列化
        let parsed = SipMessage::parse(&msg_str).expect("SIP parse should succeed");

        // 校验 StartLine
        if let StartLine::Request { method: parsed_method, uri: parsed_uri, version: parsed_ver } = parsed.start_line {
            prop_assert_eq!(parsed_method, method);
            prop_assert_eq!(parsed_uri, uri);
            prop_assert_eq!(parsed_ver, version);
        } else {
            prop_assert!(false, "StartLine must be Request");
        }

        // 校验 Body
        prop_assert_eq!(parsed.body, body);
    }

    /// 测试 GbSdp 生成、序列化、反序列化在各种属性变化下的鲁棒性
    #[test]
    fn test_gb_sdp_roundtrip(
        session_id in valid_identifier(),
        ip in valid_ip(),
        port in 1..65535_u16,
        ssrc in valid_ssrc(),
        is_video in any::<bool>(),
        mode in prop_oneof![Just("recvonly".to_string()), Just("sendonly".to_string()), Just("sendrecv".to_string())],
    ) {
        // 使用 GbSdp::to_string 构造 SDP 文本
        let sdp_text = GbSdp::to_string(&session_id, &ip, port, ssrc, is_video, &mode);

        // 使用 GbSdp::parse 进行解析
        let parsed = GbSdp::parse(&sdp_text).expect("GbSdp parse should succeed");

        // 验证基本属性是否均恢复
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
