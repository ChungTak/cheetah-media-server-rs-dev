use bytes::{BufMut, Bytes, BytesMut};

use super::{
    limits::{find_header_end, parse_content_length, split_header_line},
    method::RtspMethod,
    RtspCoreError, RtspMessageLimits,
};

/// Header for `RTSP`.
/// `RTSP` 的头。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspHeader {
    pub name: String,
    pub value: String,
}

/// Message used by `RTSP Request`.
/// `RTSP Request` 使用的消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspRequestMessage {
    pub method: String,
    pub uri: String,
    pub version: String,
    pub headers: Vec<RtspHeader>,
    pub body: Bytes,
}

/// Message used by `RTSP Response`.
/// `RTSP Response` 使用的消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspResponseMessage {
    pub version: String,
    pub status_code: u16,
    pub reason_phrase: String,
    pub headers: Vec<RtspHeader>,
    pub body: Bytes,
}

/// Request for `RTSP`.
/// `RTSP` 的请求。
#[derive(Debug, Clone)]
pub struct RtspRequest {
    pub method: RtspMethod,
    pub uri: String,
    pub version: String,
    pub headers: Vec<RtspHeader>,
    pub body: Bytes,
    pub cseq: Option<u32>,
    pub session: Option<String>,
}

/// `RtspRequestDecoder` data structure.
/// `RtspRequestDecoder` 数据结构。
#[derive(Debug, Clone)]
pub struct RtspRequestDecoder {
    buffer: BytesMut,
    limits: RtspMessageLimits,
}

/// `RtspResponseDecoder` data structure.
/// `RtspResponseDecoder` 数据结构。
#[derive(Debug, Clone)]
pub struct RtspResponseDecoder {
    buffer: BytesMut,
    limits: RtspMessageLimits,
}

fn header_value<'a>(headers: &'a [RtspHeader], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

impl RtspRequestMessage {
    /// `header_value` function of `RtspRequestMessage`.
    /// `RtspRequestMessage` 的 `header_value` 函数。
    pub fn header_value(&self, name: &str) -> Option<&str> {
        header_value(&self.headers, name)
    }
}

impl RtspResponseMessage {
    /// `header_value` function of `RtspResponseMessage`.
    /// `RtspResponseMessage` 的 `header_value` 函数。
    pub fn header_value(&self, name: &str) -> Option<&str> {
        header_value(&self.headers, name)
    }
}

impl RtspRequest {
    /// `header_value` function of `RtspRequest`.
    /// `RtspRequest` 的 `header_value` 函数。
    pub fn header_value(&self, name: &str) -> Option<&str> {
        header_value(&self.headers, name)
    }
}

impl Default for RtspRequestDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for RtspResponseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl RtspRequestDecoder {
    /// Creates a new `RtspRequestDecoder` instance.
    /// 创建新的 `RtspRequestDecoder` 实例。
    pub fn new() -> Self {
        Self::with_limits(RtspMessageLimits::default())
    }

    /// Returns a copy with `limits` set.
    /// 返回将 `limits` 设置后的副本。
    pub fn with_limits(limits: RtspMessageLimits) -> Self {
        Self {
            buffer: BytesMut::new(),
            limits,
        }
    }

    /// `feed` function of `RtspRequestDecoder`.
    /// `RtspRequestDecoder` 的 `feed` 函数。
    pub fn feed(&mut self, data: &[u8]) -> Result<(), RtspCoreError> {
        self.limits
            .validate_buffer_growth(self.buffer.len(), data.len())?;
        self.buffer.extend_from_slice(data);
        Ok(())
    }

    /// Decodes the value from the input buffer.
    /// 从输入缓冲区解码值。
    pub fn decode(&mut self) -> Result<Option<RtspRequestMessage>, RtspCoreError> {
        match try_parse_message(&mut self.buffer, &self.limits)? {
            ParsedMessage::Incomplete => Ok(None),
            ParsedMessage::Request(request) => Ok(Some(request)),
            ParsedMessage::Response(_) => Err(RtspCoreError::UnexpectedRtspResponse),
        }
    }
}

impl RtspResponseDecoder {
    /// Creates a new `RtspResponseDecoder` instance.
    /// 创建新的 `RtspResponseDecoder` 实例。
    pub fn new() -> Self {
        Self::with_limits(RtspMessageLimits::default())
    }

    /// Returns a copy with `limits` set.
    /// 返回将 `limits` 设置后的副本。
    pub fn with_limits(limits: RtspMessageLimits) -> Self {
        Self {
            buffer: BytesMut::new(),
            limits,
        }
    }

    /// `feed` function of `RtspResponseDecoder`.
    /// `RtspResponseDecoder` 的 `feed` 函数。
    pub fn feed(&mut self, data: &[u8]) -> Result<(), RtspCoreError> {
        self.limits
            .validate_buffer_growth(self.buffer.len(), data.len())?;
        self.buffer.extend_from_slice(data);
        Ok(())
    }

    /// Decodes the value from the input buffer.
    /// 从输入缓冲区解码值。
    pub fn decode(&mut self) -> Result<Option<RtspResponseMessage>, RtspCoreError> {
        match try_parse_message(&mut self.buffer, &self.limits)? {
            ParsedMessage::Incomplete => Ok(None),
            ParsedMessage::Request(_) => Err(RtspCoreError::UnexpectedRtspRequest),
            ParsedMessage::Response(response) => Ok(Some(response)),
        }
    }
}

pub(crate) enum ParsedMessage {
    Incomplete,
    Request(RtspRequestMessage),
    Response(RtspResponseMessage),
}

pub(crate) fn try_parse_request(
    buffer: &mut BytesMut,
    limits: &RtspMessageLimits,
) -> Result<ParsedMessage, RtspCoreError> {
    try_parse_message(buffer, limits)
}

pub(crate) fn try_parse_message(
    buffer: &mut BytesMut,
    limits: &RtspMessageLimits,
) -> Result<ParsedMessage, RtspCoreError> {
    let Some(header_end) = find_header_end(buffer.as_ref()) else {
        return Ok(ParsedMessage::Incomplete);
    };

    let header_len = header_end + 4;
    let content_length = parse_content_length(&buffer.as_ref()[..header_len])?;
    if content_length > limits.max_body_size {
        return Err(RtspCoreError::BodySizeLimitExceeded {
            max: limits.max_body_size,
            actual: content_length,
        });
    }
    let Some(total_len) = header_len.checked_add(content_length) else {
        return Err(RtspCoreError::BufferSizeLimitExceeded {
            max: limits.max_buffer_size,
            actual: usize::MAX,
        });
    };
    if buffer.len() < total_len {
        return Ok(ParsedMessage::Incomplete);
    }

    let message = buffer.split_to(total_len).freeze();
    match parse_rtsp_message(message, limits)? {
        ParsedMessageBody::Request(request) => Ok(ParsedMessage::Request(request)),
        ParsedMessageBody::Response(response) => Ok(ParsedMessage::Response(response)),
    }
}

/// Encodes `RTSP request` into the output buffer.
/// 将 `RTSP request` 编码到输出缓冲区。
pub fn encode_rtsp_request(request: &RtspRequestMessage) -> Result<Bytes, RtspCoreError> {
    validate_start_line_field(&request.method, "method")?;
    validate_start_line_field(&request.uri, "uri")?;
    validate_start_line_field(&request.version, "version")?;

    let mut buf = BytesMut::new();
    buf.put_slice(format!("{} {} {}\r\n", request.method, request.uri, request.version).as_bytes());

    let mut has_content_length = false;
    for header in &request.headers {
        validate_header_name(&header.name)?;
        validate_header_value(&header.value)?;
        if header.name.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        buf.put_slice(format!("{}: {}\r\n", header.name, header.value).as_bytes());
    }

    if !request.body.is_empty() && !has_content_length {
        buf.put_slice(format!("Content-Length: {}\r\n", request.body.len()).as_bytes());
    }
    buf.put_slice(b"\r\n");
    buf.put_slice(&request.body);
    Ok(buf.freeze())
}

/// Encodes `RTSP response` into the output buffer.
/// 将 `RTSP response` 编码到输出缓冲区。
pub fn encode_rtsp_response(response: &RtspResponseMessage) -> Result<Bytes, RtspCoreError> {
    validate_start_line_field(&response.version, "version")?;
    validate_reason_phrase(&response.reason_phrase)?;

    let mut buf = BytesMut::new();
    buf.put_slice(
        format!(
            "{} {} {}\r\n",
            response.version, response.status_code, response.reason_phrase
        )
        .as_bytes(),
    );

    let mut has_content_length = false;
    for header in &response.headers {
        validate_header_name(&header.name)?;
        validate_header_value(&header.value)?;
        if header.name.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        buf.put_slice(format!("{}: {}\r\n", header.name, header.value).as_bytes());
    }

    if !has_content_length {
        buf.put_slice(format!("Content-Length: {}\r\n", response.body.len()).as_bytes());
    }
    buf.put_slice(b"\r\n");
    buf.put_slice(&response.body);
    Ok(buf.freeze())
}

pub(crate) fn encode_rtsp_response_parts(
    cseq: Option<u32>,
    status_code: u16,
    reason: &str,
    headers: Vec<(String, String)>,
    body: Bytes,
) -> Result<Bytes, RtspCoreError> {
    let mut message_headers = Vec::with_capacity(headers.len() + 1);
    let mut has_cseq = false;
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("cseq") {
            has_cseq = true;
        }
        message_headers.push(RtspHeader { name, value });
    }

    if !has_cseq {
        if let Some(cseq) = cseq {
            message_headers.push(RtspHeader {
                name: "CSeq".to_string(),
                value: cseq.to_string(),
            });
        }
    }

    encode_rtsp_response(&RtspResponseMessage {
        version: "RTSP/1.0".to_string(),
        status_code,
        reason_phrase: reason.to_string(),
        headers: message_headers,
        body,
    })
}

pub(crate) fn into_rtsp_request(request: RtspRequestMessage) -> RtspRequest {
    let method = RtspMethod::parse(&request.method);
    let cseq = request
        .header_value("cseq")
        .and_then(|value| value.parse::<u32>().ok());
    let session = request.header_value("session").map(str::to_string);

    RtspRequest {
        method,
        uri: request.uri,
        version: request.version,
        headers: request.headers,
        body: request.body,
        cseq,
        session,
    }
}

enum ParsedMessageBody {
    Request(RtspRequestMessage),
    Response(RtspResponseMessage),
}

fn parse_rtsp_message(
    message: Bytes,
    limits: &RtspMessageLimits,
) -> Result<ParsedMessageBody, RtspCoreError> {
    let Some(header_end) = find_header_end(&message) else {
        return Err(RtspCoreError::InvalidStartLine);
    };

    let headers_bytes = &message[..header_end];
    let body = message.slice(header_end + 4..);
    let headers_str = std::str::from_utf8(headers_bytes).map_err(|_| RtspCoreError::InvalidUtf8)?;
    let mut lines = headers_str.split("\r\n");
    let start_line = lines.next().ok_or(RtspCoreError::InvalidStartLine)?;

    let parsed_headers = parse_headers(lines, limits)?;
    if start_line.starts_with("RTSP/") {
        let (version, status_code, reason_phrase) = parse_response_start_line(start_line)?;
        return Ok(ParsedMessageBody::Response(RtspResponseMessage {
            version: version.to_string(),
            status_code,
            reason_phrase: reason_phrase.to_string(),
            headers: parsed_headers,
            body,
        }));
    }

    let (method, uri, version) = parse_request_start_line(start_line)?;
    Ok(ParsedMessageBody::Request(RtspRequestMessage {
        method: method.to_string(),
        uri: uri.to_string(),
        version: version.to_string(),
        headers: parsed_headers,
        body,
    }))
}

fn parse_headers<'a>(
    lines: impl Iterator<Item = &'a str>,
    limits: &RtspMessageLimits,
) -> Result<Vec<RtspHeader>, RtspCoreError> {
    let mut parsed_headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if line.len() > limits.max_header_line_size {
            return Err(RtspCoreError::HeaderLineSizeLimitExceeded {
                max: limits.max_header_line_size,
                actual: line.len(),
            });
        }
        if parsed_headers.len() == limits.max_headers_count {
            return Err(RtspCoreError::HeaderCountLimitExceeded {
                max: limits.max_headers_count,
                actual: parsed_headers.len() + 1,
            });
        }
        let Some((name, value)) = split_header_line(line) else {
            return Err(RtspCoreError::InvalidHeaderLine);
        };
        parsed_headers.push(RtspHeader {
            name: name.to_string(),
            value: value.to_string(),
        });
    }
    Ok(parsed_headers)
}

fn parse_request_start_line(start_line: &str) -> Result<(&str, &str, &str), RtspCoreError> {
    let mut parts = start_line.split_whitespace();
    let method = parts.next().ok_or(RtspCoreError::InvalidStartLine)?;
    let uri = parts.next().ok_or(RtspCoreError::InvalidStartLine)?;
    let version = parts.next().ok_or(RtspCoreError::InvalidStartLine)?;
    if parts.next().is_some() {
        return Err(RtspCoreError::InvalidStartLine);
    }
    Ok((method, uri, version))
}

fn parse_response_start_line(start_line: &str) -> Result<(&str, u16, &str), RtspCoreError> {
    let Some(first_space) = start_line.find(' ') else {
        return Err(RtspCoreError::InvalidStartLine);
    };
    let version = &start_line[..first_space];
    let rest = start_line[first_space + 1..].trim_start();

    let Some(second_space) = rest.find(' ') else {
        return Err(RtspCoreError::InvalidStartLine);
    };
    let status_code_text = &rest[..second_space];
    let reason_phrase = &rest[second_space + 1..];

    if version.is_empty() || reason_phrase.contains('\r') || reason_phrase.contains('\n') {
        return Err(RtspCoreError::InvalidStartLine);
    }

    let status_code = status_code_text
        .parse::<u16>()
        .map_err(|_| RtspCoreError::InvalidStartLine)?;

    Ok((version, status_code, reason_phrase))
}

fn validate_start_line_field(value: &str, field_name: &'static str) -> Result<(), RtspCoreError> {
    if value.is_empty() || value.contains('\r') || value.contains('\n') {
        return Err(RtspCoreError::InvalidMessageField(field_name));
    }
    Ok(())
}

fn validate_header_name(name: &str) -> Result<(), RtspCoreError> {
    if name.is_empty() || name.contains('\r') || name.contains('\n') || name.contains(':') {
        return Err(RtspCoreError::InvalidMessageField("header_name"));
    }
    Ok(())
}

fn validate_header_value(value: &str) -> Result<(), RtspCoreError> {
    if value.contains('\r') || value.contains('\n') {
        return Err(RtspCoreError::InvalidMessageField("header_value"));
    }
    Ok(())
}

fn validate_reason_phrase(value: &str) -> Result<(), RtspCoreError> {
    if value.contains('\r') || value.contains('\n') {
        return Err(RtspCoreError::InvalidMessageField("reason_phrase"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        encode_rtsp_request, encode_rtsp_response, RtspHeader, RtspMessageLimits,
        RtspRequestDecoder, RtspRequestMessage, RtspResponseDecoder, RtspResponseMessage,
    };
    use bytes::Bytes;

    #[test]
    fn request_roundtrip_without_body() {
        let request = RtspRequestMessage {
            method: "OPTIONS".to_string(),
            uri: "rtsp://example.com/live/test".to_string(),
            version: "RTSP/1.0".to_string(),
            headers: vec![
                RtspHeader {
                    name: "CSeq".to_string(),
                    value: "1".to_string(),
                },
                RtspHeader {
                    name: "User-Agent".to_string(),
                    value: "cheetah".to_string(),
                },
            ],
            body: Bytes::new(),
        };

        let encoded = encode_rtsp_request(&request).expect("encode request");
        let encoded_text = std::str::from_utf8(&encoded).expect("utf8 request text");
        assert!(!encoded_text.contains("Content-Length:"));

        let mut decoder = RtspRequestDecoder::new();
        decoder.feed(&encoded).expect("feed request");
        let decoded = decoder.decode().expect("decode request");
        assert_eq!(decoded, Some(request));
    }

    #[test]
    fn request_feed_rejects_when_buffer_limit_exceeded() {
        let mut decoder = RtspRequestDecoder::with_limits(RtspMessageLimits {
            max_buffer_size: 8,
            ..RtspMessageLimits::default()
        });
        let err = decoder.feed(b"0123456789").expect_err("feed should fail");
        assert!(matches!(
            err,
            crate::core::RtspCoreError::BufferSizeLimitExceeded { max: 8, actual: 10 }
        ));
    }

    #[test]
    fn request_decode_rejects_when_header_count_limit_exceeded() {
        let mut decoder = RtspRequestDecoder::with_limits(RtspMessageLimits {
            max_headers_count: 1,
            ..RtspMessageLimits::default()
        });
        decoder
            .feed(
                b"OPTIONS rtsp://example.com/live/test RTSP/1.0\r\nCSeq: 1\r\nSession: abc\r\n\r\n",
            )
            .expect("feed request");
        let err = decoder
            .decode()
            .expect_err("header count should exceed limit");
        assert!(matches!(
            err,
            crate::core::RtspCoreError::HeaderCountLimitExceeded { max: 1, actual: 2 }
        ));
    }

    #[test]
    fn decode_rejects_rtsp_response() {
        let mut decoder = RtspRequestDecoder::new();
        decoder
            .feed(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\n")
            .expect("feed response bytes");
        let err = decoder
            .decode()
            .expect_err("response should be rejected by request decoder");
        assert!(matches!(
            err,
            crate::core::RtspCoreError::UnexpectedRtspResponse
        ));
    }

    #[test]
    fn response_roundtrip_without_body() {
        let response = RtspResponseMessage {
            version: "RTSP/1.0".to_string(),
            status_code: 200,
            reason_phrase: "OK".to_string(),
            headers: vec![RtspHeader {
                name: "CSeq".to_string(),
                value: "1".to_string(),
            }],
            body: Bytes::new(),
        };

        let encoded = encode_rtsp_response(&response).expect("encode response");
        let encoded_text = std::str::from_utf8(&encoded).expect("utf8 response text");
        assert!(encoded_text.contains("Content-Length: 0"));

        let mut decoder = RtspResponseDecoder::new();
        decoder.feed(&encoded).expect("feed response");
        let decoded = decoder.decode().expect("decode response");
        assert_eq!(
            decoded,
            Some(RtspResponseMessage {
                headers: vec![
                    RtspHeader {
                        name: "CSeq".to_string(),
                        value: "1".to_string(),
                    },
                    RtspHeader {
                        name: "Content-Length".to_string(),
                        value: "0".to_string(),
                    },
                ],
                ..response
            })
        );
    }

    #[test]
    fn decode_response_rejects_rtsp_request() {
        let mut decoder = RtspResponseDecoder::new();
        decoder
            .feed(b"OPTIONS rtsp://example.com/live/test RTSP/1.0\r\nCSeq: 1\r\n\r\n")
            .expect("feed request bytes");
        let err = decoder
            .decode()
            .expect_err("request should be rejected by response decoder");
        assert!(matches!(
            err,
            crate::core::RtspCoreError::UnexpectedRtspRequest
        ));
    }

    #[test]
    fn header_lookup_is_case_insensitive_for_request_response_and_runtime_request() {
        let request = RtspRequestMessage {
            method: "OPTIONS".to_string(),
            uri: "rtsp://example.com/live/test".to_string(),
            version: "RTSP/1.0".to_string(),
            headers: vec![RtspHeader {
                name: "CSeq".to_string(),
                value: "77".to_string(),
            }],
            body: Bytes::new(),
        };
        assert_eq!(request.header_value("cseq"), Some("77"));
        assert!(request.header_value("session").is_none());

        let response = RtspResponseMessage {
            version: "RTSP/1.0".to_string(),
            status_code: 200,
            reason_phrase: "OK".to_string(),
            headers: vec![RtspHeader {
                name: "Session".to_string(),
                value: "abc;timeout=60".to_string(),
            }],
            body: Bytes::new(),
        };
        assert_eq!(response.header_value("session"), Some("abc;timeout=60"));
        assert!(response.header_value("cseq").is_none());

        let runtime_request = super::into_rtsp_request(request);
        assert_eq!(runtime_request.header_value("CSEQ"), Some("77"));
    }
}
