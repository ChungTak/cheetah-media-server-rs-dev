use super::RtspCoreError;

/// `RtspMessageLimits` data structure.
/// `RtspMessageLimits` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspMessageLimits {
    pub max_buffer_size: usize,
    pub max_headers_count: usize,
    pub max_header_line_size: usize,
    pub max_body_size: usize,
    pub max_interleaved_frame_size: usize,
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

pub(crate) fn split_header_line(line: &str) -> Option<(&str, &str)> {
    let index = line.find(':')?;
    let name = line[..index].trim();
    let value = line[index + 1..].trim();
    Some((name, value))
}

pub(crate) fn find_header_end(raw: &[u8]) -> Option<usize> {
    if raw.len() < 4 {
        return None;
    }
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}
