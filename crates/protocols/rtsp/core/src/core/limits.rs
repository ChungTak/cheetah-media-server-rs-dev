use super::RtspCoreError;

/// Limits enforced by the Sans-I/O RTSP message parser.
///
/// These bounds prevent unbounded buffer growth, oversized headers, and
/// malformed bodies from consuming arbitrary memory during parsing.
///
/// Sans-I/O RTSP 消息解析器强制执行的限制。
///
/// 这些边界防止无限缓冲区增长、超大头部和畸形体在解析时消耗任意内存。
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
    /// Default limits tuned for a typical streaming server.
    ///
    /// 1 MiB buffer, 64 headers, 8 KiB per header line, 512 KiB body, and 64 KiB
    /// interleaved frame are enough for normal RTSP/SDP usage while rejecting
    /// obvious abuse.
    ///
    /// 为典型流媒体服务器调整后的默认限制。
    ///
    /// 1 MiB 缓冲区、64 个头部、每行 8 KiB 头部、512 KiB 体、64 KiB 交错帧足以满足
    /// 正常 RTSP/SDP 使用，同时拒绝明显滥用。
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
    /// Validate that appending `incoming_len` bytes to `current_len` does not exceed `max_buffer_size`.
    ///
    /// Uses `checked_add` to avoid usize overflow and reports a friendly limit
    /// error instead of panicking.
    ///
    /// 校验将 `incoming_len` 字节追加到 `current_len` 后不超过 `max_buffer_size`。
    ///
    /// 使用 `checked_add` 避免 usize 溢出，并返回友好的限制错误而非 panic。
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

/// Scan a raw header block for a `Content-Length` value.
///
/// Splits each header line into `name:value` and returns the first case-
/// insensitive `content-length` parse. If no such header is found, the body
/// length is assumed to be zero.
///
/// 在原始头部块中扫描 `Content-Length` 值。
///
/// 将每行头部分割为 `name:value`，返回第一个大小写不敏感的 `content-length` 解析值。
/// 若未找到该头部，则假定体部长度为 0。
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

/// Split a single header line into `(name, value)` at the first `:`.
///
/// Both sides are trimmed so later parsing is case-insensitive and whitespace-
/// tolerant.
///
/// 在第一个 `:` 处将单行头部分割为 `(name, value)`。
///
/// 两侧都会去空白，使后续解析对大小写和空白字符具有容忍性。
pub(crate) fn split_header_line(line: &str) -> Option<(&str, &str)> {
    let index = line.find(':')?;
    let name = line[..index].trim();
    let value = line[index + 1..].trim();
    Some((name, value))
}

/// Locate the end of the HTTP-style header block (`\r\n\r\n`).
///
/// Returns the byte offset of the first `\r` in the terminator, so callers can
/// add four to obtain the total header length.
///
/// 定位 HTTP 风格头部块结束位置（`\r\n\r\n`）。
///
/// 返回终结符中第一个 `\r` 的字节偏移，调用者可加 4 得到头部总长度。
pub(crate) fn find_header_end(raw: &[u8]) -> Option<usize> {
    if raw.len() < 4 {
        return None;
    }
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}
