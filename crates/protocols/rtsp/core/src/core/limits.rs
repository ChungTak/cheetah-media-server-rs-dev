use super::RtspCoreError;

/// `RtspMessageLimits` data structure.
/// `RtspMessageLimits` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspMessageLimits {
    /// `max_buffer_size` field of type `usize`.
    /// `max_buffer_size` 字段，类型为 `usize`.
    pub max_buffer_size: usize,
    /// `max_headers_count` field of type `usize`.
    /// `max_headers_count` 字段，类型为 `usize`.
    pub max_headers_count: usize,
    /// `max_header_line_size` field of type `usize`.
    /// `max_header_line_size` 字段，类型为 `usize`.
    pub max_header_line_size: usize,
    /// `max_body_size` field of type `usize`.
    /// `max_body_size` 字段，类型为 `usize`.
    pub max_body_size: usize,
    /// `max_interleaved_frame_size` field of type `usize`.
    /// `max_interleaved_frame_size` 字段，类型为 `usize`.
    pub max_interleaved_frame_size: usize,
    /// `validate_version` field of type `bool`.
    /// `validate_version` 字段，类型为 `bool`.
    pub validate_version: bool,
}

impl Default for RtspMessageLimits {
    fn default() -> Self {
        Self {
            max_buffer_size: 1024 * 1024,
            max_headers_count: 64,
            max_header_line_size: 8 * 1024,
            max_body_size: 512 * 1024,
            max_interleaved_frame_size: 64 * 1024,
            validate_version: true,
        }
    }
}

impl RtspMessageLimits {
    /// `validate_buffer_growth` function.
    /// `validate_buffer_growth` 函数.
    pub(crate) fn validate_buffer_growth(
        &self,
        current_len: usize,
        incoming_len: usize,
    ) -> Result<(), RtspCoreError> {
        let Some(total_len) = current_len.checked_add(incoming_len) else {
            return Err(RtspCoreError::BufferSizeLimitExceeded {
                max: self.max_buffer_size,
                actual: usize::MAX,
            });
        };
        if total_len > self.max_buffer_size {
            return Err(RtspCoreError::BufferSizeLimitExceeded {
                max: self.max_buffer_size,
                actual: total_len,
            });
        }
        Ok(())
    }
}

/// Parses `content_length` from input.
/// 解析 `content_length` 来自 输入.
pub(crate) fn parse_content_length(headers: &[u8]) -> Result<usize, RtspCoreError> {
    let text = std::str::from_utf8(headers).map_err(|_| RtspCoreError::InvalidUtf8)?;
    for line in text.split("\r\n") {
        let Some((name, value)) = split_header_line(line) else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            return value
                .parse::<usize>()
                .map_err(|_| RtspCoreError::InvalidContentLength);
        }
    }
    Ok(0)
}

/// `split_header_line` function.
/// `split_header_line` 函数.
pub(crate) fn split_header_line(line: &str) -> Option<(&str, &str)> {
    let index = line.find(':')?;
    let name = line[..index].trim();
    let value = line[index + 1..].trim();
    Some((name, value))
}

/// `find_header_end` function.
/// `find_header_end` 函数.
pub(crate) fn find_header_end(raw: &[u8]) -> Option<usize> {
    if raw.len() < 4 {
        return None;
    }
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}
