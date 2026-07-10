//! Property-based tests for RTSP request/response message parsing.
//!
//! These tests verify that `encode_rtsp_request` and `encode_rtsp_response` are
//! inverses of `RtspRequestDecoder` and `RtspResponseDecoder` for both empty and
//! non-empty bodies, and that the encoder correctly auto-derives `Content-Length`.
//!
//! RTSP 请求/响应消息解析属性测试。
//!
//! 这些测试验证 `encode_rtsp_request`/`encode_rtsp_response` 与
//! `RtspRequestDecoder`/`RtspResponseDecoder` 在空 body 与非空 body 下互逆，
//! 并且编码器自动推导出 `Content-Length`。

use bytes::Bytes;
use cheetah_rtsp_core::{
    encode_rtsp_request, encode_rtsp_response, RtspHeader, RtspRequestDecoder, RtspRequestMessage,
    RtspResponseDecoder, RtspResponseMessage,
};
use proptest::prelude::*;

/// Generate a valid RTSP token (no control characters or separators).
///
/// 生成有效 RTSP token（不含控制字符与分隔符）。
fn valid_token() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Za-z][A-Za-z0-9_-]{0,30}")
        .expect("valid token regex")
        .prop_filter("non-empty", |s| !s.is_empty())
}

/// Generate a valid RTSP URI.
///
/// 生成有效 RTSP URI。
fn valid_uri() -> impl Strategy<Value = String> {
    prop::string::string_regex("rtsp://[a-z0-9.-]+/[A-Za-z0-9/_.-]*")
        .expect("valid uri regex")
        .prop_filter("non-empty", |s| !s.is_empty())
}

/// Generate a valid protocol version string.
///
/// 生成有效协议版本字符串。
fn valid_version() -> impl Strategy<Value = String> {
    prop_oneof![Just("RTSP/1.0".to_string()), Just("RTSP/2.0".to_string()),]
}

/// Generate a valid header field name.
///
/// 生成有效头字段名。
fn valid_header_name() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Za-z][A-Za-z0-9-]{0,20}")
        .expect("valid header name regex")
        .prop_filter("non-empty", |s| !s.is_empty())
}

/// Generate a valid header field value (no CRLF).
///
/// 生成有效头字段值（不含 CRLF）。
fn valid_header_value() -> impl Strategy<Value = String> {
    prop::string::string_regex("[A-Za-z0-9 ,;:=/_.-]{0,100}").expect("valid header value regex")
}

/// Generate a status code that may carry a body (excludes 1xx/204/304).
///
/// 生成可带 body 的状态码（排除 1xx/204/304）。
fn valid_status_code_with_body() -> impl Strategy<Value = u16> {
    prop_oneof![
        Just(200u16),
        Just(201u16),
        Just(301u16),
        Just(400u16),
        Just(401u16),
        Just(404u16),
        Just(500u16),
    ]
}

/// Generate a reason phrase.
///
/// 生成 reason phrase。
fn valid_reason_phrase() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("OK".to_string()),
        Just("Not Found".to_string()),
        Just("Bad Request".to_string()),
        Just("Internal Server Error".to_string()),
    ]
}

/// Generate a list of headers that excludes `Content-Length` and `Transfer-Encoding`.
///
/// 生成头列表，排除 `Content-Length` 与 `Transfer-Encoding`，避免与编码器自动逻辑冲突。
fn valid_headers() -> impl Strategy<Value = Vec<RtspHeader>> {
    prop::collection::vec((valid_header_name(), valid_header_value()), 0..5)
        .prop_filter("no Content-Length or Transfer-Encoding", |headers| {
            headers.iter().all(|(name, _)| {
                !name.eq_ignore_ascii_case("Content-Length")
                    && !name.eq_ignore_ascii_case("Transfer-Encoding")
            })
        })
        .prop_map(|headers| {
            headers
                .into_iter()
                .map(|(name, value)| RtspHeader { name, value })
                .collect()
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// RTSP request encode/decode round-trip with no body.
    ///
    /// RTSP 请求无 body 编码/解码往返。
    #[test]
    fn test_http_request_roundtrip_no_body(
        method in valid_token(),
        uri in valid_uri(),
        version in valid_version(),
        headers in valid_headers(),
    ) {
        let request = RtspRequestMessage {
            method: method.clone(),
            uri: uri.clone(),
            version: version.clone(),
            headers: headers.clone(),
            body: Bytes::new(),
        };

        let encoded = encode_rtsp_request(&request).expect("encode request");
        let mut decoder = RtspRequestDecoder::new();
        decoder.feed(&encoded).expect("feed request");
        let decoded = decoder
            .decode()
            .expect("decode request")
            .expect("expected decoded request");

        prop_assert_eq!(decoded.method, method);
        prop_assert_eq!(decoded.uri, uri);
        prop_assert_eq!(decoded.version, version);
        prop_assert_eq!(decoded.headers.len(), headers.len());
        prop_assert!(decoded.body.is_empty());
    }

    /// RTSP request encode/decode round-trip with a body.
    ///
    /// RTSP 请求带 body 编码/解码往返。
    #[test]
    fn test_http_request_roundtrip_with_body(
        method in valid_token(),
        uri in valid_uri(),
        version in valid_version(),
        headers in valid_headers(),
        body in prop::collection::vec(any::<u8>(), 1..256),
    ) {
        let request = RtspRequestMessage {
            method: method.clone(),
            uri: uri.clone(),
            version: version.clone(),
            headers: headers.clone(),
            body: Bytes::from(body.clone()),
        };

        let encoded = encode_rtsp_request(&request).expect("encode request");
        let mut decoder = RtspRequestDecoder::new();
        decoder.feed(&encoded).expect("feed request");
        let decoded = decoder
            .decode()
            .expect("decode request")
            .expect("expected decoded request");

        prop_assert_eq!(decoded.method, method);
        prop_assert_eq!(decoded.uri, uri);
        prop_assert_eq!(decoded.version, version);
        prop_assert_eq!(decoded.body.as_ref(), body.as_slice());
        // The encoder adds an automatic Content-Length header.
        prop_assert_eq!(decoded.headers.len(), headers.len() + 1);
    }

    /// RTSP response encode/decode round-trip with no body.
    ///
    /// RTSP 响应无 body 编码/解码往返。
    #[test]
    fn test_http_response_roundtrip_no_body(
        version in valid_version(),
        status_code in valid_status_code_with_body(),
        reason_phrase in valid_reason_phrase(),
        headers in valid_headers(),
    ) {
        let response = RtspResponseMessage {
            version: version.clone(),
            status_code,
            reason_phrase: reason_phrase.clone(),
            headers: headers.clone(),
            body: Bytes::new(),
        };

        let encoded = encode_rtsp_response(&response).expect("encode response");
        let mut decoder = RtspResponseDecoder::new();
        decoder.feed(&encoded).expect("feed response");
        let decoded = decoder
            .decode()
            .expect("decode response")
            .expect("expected decoded response");

        prop_assert_eq!(decoded.version, version);
        prop_assert_eq!(decoded.status_code, status_code);
        prop_assert_eq!(decoded.reason_phrase, reason_phrase);
        // The encoder adds an automatic Content-Length: 0.
        prop_assert_eq!(decoded.headers.len(), headers.len() + 1);
        prop_assert!(decoded.body.is_empty());
    }

    /// RTSP response encode/decode round-trip with a body.
    ///
    /// RTSP 响应带 body 编码/解码往返。
    #[test]
    fn test_http_response_roundtrip_with_body(
        version in valid_version(),
        status_code in valid_status_code_with_body(),
        reason_phrase in valid_reason_phrase(),
        headers in valid_headers(),
        body in prop::collection::vec(any::<u8>(), 1..256),
    ) {
        let response = RtspResponseMessage {
            version: version.clone(),
            status_code,
            reason_phrase: reason_phrase.clone(),
            headers: headers.clone(),
            body: Bytes::from(body.clone()),
        };

        let encoded = encode_rtsp_response(&response).expect("encode response");
        let mut decoder = RtspResponseDecoder::new();
        decoder.feed(&encoded).expect("feed response");
        let decoded = decoder
            .decode()
            .expect("decode response")
            .expect("expected decoded response");

        prop_assert_eq!(decoded.version, version);
        prop_assert_eq!(decoded.status_code, status_code);
        prop_assert_eq!(decoded.reason_phrase, reason_phrase);
        prop_assert_eq!(decoded.body.as_ref(), body.as_slice());
        // The encoder adds an automatic Content-Length header.
        prop_assert_eq!(decoded.headers.len(), headers.len() + 1);
    }

    /// Feeding a request one byte at a time only completes after the final byte.
    ///
    /// 逐字节喂入请求仅在最后一个字节到达后完成解析。
    #[test]
    fn test_http_request_chunked_feed(
        method in valid_token(),
        uri in valid_uri(),
    ) {
        let request = RtspRequestMessage {
            method: method.clone(),
            uri: uri.clone(),
            version: "RTSP/1.0".to_string(),
            headers: Vec::new(),
            body: Bytes::new(),
        };
        let encoded = encode_rtsp_request(&request).expect("encode request");
        let mut decoder = RtspRequestDecoder::new();

        for (index, byte) in encoded.iter().enumerate() {
            decoder.feed(&[*byte]).expect("feed chunk");
            let decode_result = decoder.decode().expect("decode chunk");
            if index + 1 < encoded.len() {
                prop_assert!(decode_result.is_none());
            } else {
                let decoded = decode_result.expect("expected completed request");
                prop_assert_eq!(&decoded.method, &method);
                prop_assert_eq!(&decoded.uri, &uri);
                prop_assert_eq!(&decoded.version, "RTSP/1.0");
                prop_assert!(decoded.body.is_empty());
            }
        }
    }
}
