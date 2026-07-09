// 来源: vendor-ref/rtsp-rs/pbt/tests/pbt_limits.rs

use bytes::Bytes;
use cheetah_rtsp_core::{
    encode_rtsp_request, encode_rtsp_response, CoreInput, RtspConnectionLimits, RtspCore,
    RtspCoreError, RtspHeader, RtspMessageLimits, RtspRequestDecoder, RtspRequestMessage,
    RtspResponseDecoder, RtspResponseMessage,
};
use proptest::prelude::*;

/// 创建带小限制的 RTSP 请求解码器，用于覆盖限制命中路径。
fn request_decoder_with_small_limits() -> RtspRequestDecoder {
    RtspRequestDecoder::with_limits(RtspMessageLimits {
        max_buffer_size: 1024,
        max_headers_count: 5,
        max_header_line_size: 128,
        max_body_size: 256,
        ..RtspMessageLimits::default()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// 验证喂入数据超过 buffer 限制时会立即返回错误。
    #[test]
    fn test_buffer_size_limit_exceeded(
        data in prop::collection::vec(any::<u8>(), 2000..3000)
    ) {
        let mut decoder = request_decoder_with_small_limits();
        let result = decoder.feed(&data);
        prop_assert!(result.is_err());
    }

    /// 验证喂入数据处于 buffer 限制内时不会返回错误。
    #[test]
    fn test_buffer_size_within_limit(
        data in prop::collection::vec(any::<u8>(), 0..500)
    ) {
        let mut decoder = request_decoder_with_small_limits();
        let result = decoder.feed(&data);
        prop_assert!(result.is_ok());
    }

    /// 验证请求头数量超过限制时会返回错误。
    #[test]
    fn test_header_count_limit_exceeded(
        header_count in 10usize..20usize
    ) {
        let mut decoder = request_decoder_with_small_limits();

        let mut request = "OPTIONS rtsp://example.com RTSP/1.0\r\n".to_string();
        for index in 0..header_count {
            request.push_str(&format!("Header{}: value{}\r\n", index, index));
        }
        request.push_str("\r\n");

        decoder.feed(request.as_bytes()).expect("feed request");
        let result = decoder.decode().expect_err("header count should exceed limit");
        let is_header_count_limit_error = match result {
            RtspCoreError::HeaderCountLimitExceeded { max, actual } => max == 5 && actual > max,
            _ => false,
        };
        prop_assert!(is_header_count_limit_error);
    }

    /// 验证请求头数量处于限制内时可以成功解析。
    #[test]
    fn test_header_count_within_limit(
        header_count in 1usize..=5usize
    ) {
        let mut decoder = request_decoder_with_small_limits();

        let mut request = "OPTIONS rtsp://example.com RTSP/1.0\r\n".to_string();
        for index in 0..header_count {
            request.push_str(&format!("H{}: v{}\r\n", index, index));
        }
        request.push_str("\r\n");

        decoder.feed(request.as_bytes()).expect("feed request");
        let result = decoder.decode().expect("decode request within limit");
        prop_assert!(result.is_some());
    }

    /// 验证请求 body 大小超过限制时会返回错误。
    #[test]
    fn test_body_size_limit_exceeded(
        body_size in 500usize..1000usize
    ) {
        let mut decoder = request_decoder_with_small_limits();

        let request = format!(
            "POST rtsp://example.com RTSP/1.0\r\nContent-Length: {}\r\n\r\n",
            body_size
        );

        decoder.feed(request.as_bytes()).expect("feed request");
        let result = decoder.decode().expect_err("body size should exceed limit");
        let is_body_size_limit_error = match result {
            RtspCoreError::BodySizeLimitExceeded { max, actual } => max == 256 && actual == body_size,
            _ => false,
        };
        prop_assert!(is_body_size_limit_error);
    }

    /// 验证请求 body 大小处于限制内时可以成功解析。
    #[test]
    fn test_body_size_within_limit(
        body_size in 1usize..100usize
    ) {
        let mut decoder = request_decoder_with_small_limits();

        let body = vec![b'x'; body_size];
        let request = format!(
            "POST rtsp://example.com RTSP/1.0\r\nContent-Length: {}\r\n\r\n",
            body_size
        );

        decoder.feed(request.as_bytes()).expect("feed request");
        decoder.feed(&body).expect("feed body");
        let result = decoder.decode().expect("decode request");
        let request = result.expect("request should be complete");
        prop_assert_eq!(request.body.len(), body_size);
    }

    /// 验证请求头单行长度超过限制时会返回错误。
    #[test]
    fn test_header_line_size_limit_exceeded(
        value_len in 200usize..500usize
    ) {
        let mut decoder = request_decoder_with_small_limits();

        let long_value = "x".repeat(value_len);
        let request = format!(
            "OPTIONS rtsp://example.com RTSP/1.0\r\nLongHeader: {}\r\n\r\n",
            long_value
        );

        decoder.feed(request.as_bytes()).expect("feed request");
        let result = decoder
            .decode()
            .expect_err("header line size should exceed limit");
        let is_header_line_limit_error = match result {
            RtspCoreError::HeaderLineSizeLimitExceeded { max, actual } => {
                max == 128 && actual > max
            }
            _ => false,
        };
        prop_assert!(is_header_line_limit_error);
    }

    /// 验证请求在限制范围内 encode/decode 可以稳定 roundtrip。
    #[test]
    fn test_request_roundtrip_with_limits(
        method in "[A-Z]{3,10}",
        uri in "rtsp://[a-z]+/[a-z]+",
        header_count in 0usize..3usize,
        body_size in 0usize..100usize
    ) {
        let mut headers = Vec::new();
        for index in 0..header_count {
            headers.push(RtspHeader {
                name: format!("H{}", index),
                value: format!("v{}", index),
            });
        }
        let request = RtspRequestMessage {
            method: method.clone(),
            uri: uri.clone(),
            version: "RTSP/1.0".to_string(),
            headers,
            body: Bytes::from(vec![b'x'; body_size]),
        };
        let encoded = encode_rtsp_request(&request).expect("encode request");

        let mut decoder = RtspRequestDecoder::new();
        decoder.feed(&encoded).expect("feed request");
        let decoded = decoder.decode().expect("decode request");
        prop_assert!(decoded.is_some());
    }

    /// 验证响应在限制范围内 encode/decode 可以稳定 roundtrip。
    #[test]
    fn test_response_roundtrip_with_limits(
        status_code in prop::sample::select(vec![200u16, 301, 400, 404, 500]),
        header_count in 0usize..3usize,
        body_size in 0usize..100usize
    ) {
        let mut headers = Vec::new();
        for index in 0..header_count {
            headers.push(RtspHeader {
                name: format!("H{}", index),
                value: format!("v{}", index),
            });
        }
        let response = RtspResponseMessage {
            version: "RTSP/1.0".to_string(),
            status_code,
            reason_phrase: "OK".to_string(),
            headers,
            body: Bytes::from(vec![b'x'; body_size]),
        };
        let encoded = encode_rtsp_response(&response).expect("encode response");

        let mut decoder = RtspResponseDecoder::new();
        decoder.feed(&encoded).expect("feed response");
        let decoded = decoder
            .decode()
            .expect("decode response")
            .expect("response should decode");
        prop_assert_eq!(decoded.version, "RTSP/1.0");
        prop_assert_eq!(decoded.status_code, status_code);
        prop_assert_eq!(decoded.reason_phrase, "OK");
        prop_assert_eq!(decoded.body.len(), body_size);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// 验证 interleaved 帧长度超过连接限制时会立即返回错误。
    #[test]
    fn test_interleaved_frame_size_limit_exceeded(
        frame_size in 2000u16..10000u16
    ) {
        let mut core = RtspCore::with_connection_limits(RtspConnectionLimits {
            max_interleaved_frame_size: 1000,
            ..RtspConnectionLimits::default()
        });
        let mut frame = vec![b'$', 0];
        frame.extend_from_slice(&frame_size.to_be_bytes());

        let result = core.handle_input(CoreInput::Bytes(Bytes::from(frame)));
        let is_expected_error = matches!(
            result,
            Err(RtspCoreError::InterleavedFrameSizeLimitExceeded {
                max: 1000,
                actual,
            }) if actual == frame_size as usize
        );
        prop_assert!(is_expected_error);
    }

    /// 验证 interleaved 帧长度处于限制内时可被 core 正常接收。
    #[test]
    fn test_interleaved_frame_within_limit(
        frame_size in 12u16..1000u16
    ) {
        let mut core = RtspCore::with_connection_limits(RtspConnectionLimits {
            max_interleaved_frame_size: 64 * 1024,
            ..RtspConnectionLimits::default()
        });

        let mut rtp_data = vec![
            0x80, 0x60,
            0x00, 0x01,
            0x00, 0x00, 0x00, 0x00,
            0x12, 0x34, 0x56, 0x78,
        ];
        while rtp_data.len() < frame_size as usize {
            rtp_data.push(0);
        }
        rtp_data.truncate(frame_size as usize);

        let mut frame = vec![b'$', 0];
        frame.extend_from_slice(&(rtp_data.len() as u16).to_be_bytes());
        frame.extend_from_slice(&rtp_data);

        let result = core.handle_input(CoreInput::Bytes(Bytes::from(frame)));
        prop_assert!(result.is_ok());
        prop_assert!(!result.expect("frame should parse").is_empty());
    }
}
