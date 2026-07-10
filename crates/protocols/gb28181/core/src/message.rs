use crate::error::Gb28181CoreError;
use std::fmt;

/// `StartLine` enumeration.
/// `StartLine` 枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartLine {
    Request {
        method: String,
        uri: String,
        version: String,
    },
    Response {
        version: String,
        status: u16,
        reason: String,
    },
}

/// Message used by `SIP`.
/// `SIP` 使用的消息。
#[derive(Debug, Clone)]
pub struct SipMessage {
    pub start_line: StartLine,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl SipMessage {
    /// Parse a SIP message with ABL-style lenient handling:
    ///
    /// - Accept `\r\n`, `\n`, or `\r` line terminators (and any mix between header lines).
    /// - Trim leading whitespace and skip leading blank lines.
    /// - Accept duplicate header names (kept in insertion order so callers can rebuild verbatim).
    /// - Stop header parsing on the first blank line and capture the remainder as the body,
    ///   truncating to `Content-Length` if specified.
    pub fn parse(raw: &str) -> Result<Self, Gb28181CoreError> {
        // Find the boundary between headers and body. We accept `\r\n\r\n`, `\n\n`, or `\r\r`,
        // plus the common mixed cases observed from real devices.
        let body_start = find_header_body_split(raw);
        let header_part = if let Some((boundary, sep_len)) = body_start {
            &raw[..boundary + sep_len]
        } else {
            raw
        };
        let body_text = if let Some((boundary, sep_len)) = body_start {
            &raw[boundary + sep_len..]
        } else {
            ""
        };

        // Tokenise headers using lenient line splitting.
        let mut lines = split_sip_lines(header_part);
        let first_line = loop {
            match lines.next() {
                Some(l) if l.trim().is_empty() => continue,
                Some(l) => break l,
                None => {
                    return Err(Gb28181CoreError::SipSyntax("empty SIP message".to_string()));
                }
            }
        };

        let start_line = Self::parse_start_line(first_line.trim())?;

        let mut headers = Vec::new();
        let mut content_length = 0usize;

        for line in lines {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            // Skip any extra noise lines that lack a colon (e.g. trailing CRLFs).
            let Some((name, val)) = line.split_once(':') else {
                continue;
            };
            let name_trimmed = name.trim().to_string();
            let val_trimmed = val.trim().to_string();
            if name_trimmed.eq_ignore_ascii_case("content-length") {
                content_length = val_trimmed.parse::<usize>().unwrap_or(0);
            }
            headers.push((name_trimmed, val_trimmed));
        }

        let mut body = body_text.as_bytes().to_vec();
        if body.len() > content_length {
            body.truncate(content_length);
        }

        Ok(Self {
            start_line,
            headers,
            body,
        })
    }

    fn parse_start_line(line: &str) -> Result<StartLine, Gb28181CoreError> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            return Err(Gb28181CoreError::SipSyntax(format!(
                "invalid SIP start line: {line}"
            )));
        }

        if parts[0].starts_with("SIP/") {
            // Response: SIP/2.0 200 OK
            let version = parts[0].to_string();
            let status = parts[1]
                .parse::<u16>()
                .map_err(|e| Gb28181CoreError::SipSyntax(format!("invalid status code: {e}")))?;
            let reason = parts[2..].join(" ");
            Ok(StartLine::Response {
                version,
                status,
                reason,
            })
        } else {
            // Request: REGISTER sip:12345@192.168.1.1 SIP/2.0
            let method = parts[0].to_string();
            let uri = parts[1].to_string();
            let version = parts[2].to_string();
            Ok(StartLine::Request {
                method,
                uri,
                version,
            })
        }
    }

    /// Returns the `header` value.
    /// 返回 `header` 的值。
    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(h_name, _)| h_name.eq_ignore_ascii_case(name))
            .map(|(_, val)| val.as_str())
    }

    /// Returns every value observed for a given header name, in insertion order.
    /// ABL devices occasionally repeat headers (notably `Via`); callers that need to inspect
    /// every entry can reach for this instead of `get_header`.
    pub fn get_headers_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers
            .iter()
            .filter(move |(h_name, _)| h_name.eq_ignore_ascii_case(name))
            .map(|(_, val)| val.as_str())
    }

    /// Sets the `header` value.
    /// 设置 `header` 的值。
    pub fn set_header(&mut self, name: &str, value: &str) {
        if let Some(pos) = self
            .headers
            .iter()
            .position(|(h_name, _)| h_name.eq_ignore_ascii_case(name))
        {
            self.headers[pos].1 = value.to_string();
        } else {
            self.headers.push((name.to_string(), value.to_string()));
        }
    }

    /// Serialize the message into a wire-format byte buffer. Unlike `Display`, this preserves
    /// non-UTF-8 body bytes verbatim so that any caller transmitting the result over the
    /// network sees byte counts that match the `Content-Length` header.
    pub fn to_bytes(&self) -> Vec<u8> {
        let start_line_len = match &self.start_line {
            StartLine::Request {
                method,
                uri,
                version,
            } => method.len() + 1 + uri.len() + 1 + version.len() + 2,
            StartLine::Response {
                version,
                status,
                reason,
            } => version.len() + 1 + status.to_string().len() + 1 + reason.len() + 2,
        };
        let headers_len: usize = self
            .headers
            .iter()
            .map(|(name, val)| name.len() + 2 + val.len() + 2)
            .sum();
        let mut out = Vec::with_capacity(start_line_len + headers_len + 2 + self.body.len());
        match &self.start_line {
            StartLine::Request {
                method,
                uri,
                version,
            } => {
                out.extend_from_slice(method.as_bytes());
                out.push(b' ');
                out.extend_from_slice(uri.as_bytes());
                out.push(b' ');
                out.extend_from_slice(version.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            StartLine::Response {
                version,
                status,
                reason,
            } => {
                out.extend_from_slice(version.as_bytes());
                out.push(b' ');
                out.extend_from_slice(status.to_string().as_bytes());
                out.push(b' ');
                out.extend_from_slice(reason.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
        }
        for (name, val) in &self.headers {
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(val.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

/// Find the first occurrence of any blank-line separator that ends the SIP header section.
/// Returns `(byte_index_of_first_terminator_byte, separator_length)`.
///
/// We walk every recognised pattern and pick the *earliest* match (longest pattern wins on
/// ties). A naive sequential `find` per pattern would mis-report the boundary when both
/// `\r\n\r\n` and `\n\n` are present in the same buffer but at different positions — an
/// observed quirk on devices that mix line terminators between header lines and the
/// header/body separator.
fn find_header_body_split(raw: &str) -> Option<(usize, usize)> {
    let candidates: [&str; 5] = ["\r\n\r\n", "\n\r\n", "\r\n\n", "\n\n", "\r\r"];
    let mut best: Option<(usize, usize)> = None;
    for pat in candidates {
        if let Some(idx) = raw.find(pat) {
            best = match best {
                None => Some((idx, pat.len())),
                Some((cur_idx, cur_len))
                    if idx < cur_idx || (idx == cur_idx && pat.len() > cur_len) =>
                {
                    Some((idx, pat.len()))
                }
                Some(_) => best,
            };
        }
    }
    best
}

/// Split a SIP header section into individual lines, accepting `\r\n`, `\n`, or `\r` as line
/// terminators (and any mix between header lines).
fn split_sip_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut start = 0usize;
    let bytes = text.as_bytes();
    std::iter::from_fn(move || {
        if start >= bytes.len() {
            return None;
        }
        let mut i = start;
        while i < bytes.len() {
            match bytes[i] {
                b'\r' => {
                    let line = &text[start..i];
                    let mut next = i + 1;
                    if next < bytes.len() && bytes[next] == b'\n' {
                        next += 1;
                    }
                    start = next;
                    return Some(line);
                }
                b'\n' => {
                    let line = &text[start..i];
                    start = i + 1;
                    return Some(line);
                }
                _ => i += 1,
            }
        }
        let line = &text[start..];
        start = bytes.len();
        Some(line)
    })
}

impl fmt::Display for SipMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.start_line {
            StartLine::Request {
                method,
                uri,
                version,
            } => {
                write!(f, "{method} {uri} {version}\r\n")?;
            }
            StartLine::Response {
                version,
                status,
                reason,
            } => {
                write!(f, "{version} {status} {reason}\r\n")?;
            }
        }

        for (name, val) in &self.headers {
            write!(f, "{name}: {val}\r\n")?;
        }
        write!(f, "\r\n")?;

        if !self.body.is_empty() {
            if let Ok(body_str) = std::str::from_utf8(&self.body) {
                write!(f, "{body_str}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sip_request() {
        let req_str = "REGISTER sip:34020000002000000001@3402000000 SIP/2.0\r\n\
                       Via: SIP/2.0/UDP 192.168.1.100:5060;branch=z9hG4bK12345\r\n\
                       From: <sip:34020000001320000001@3402000000>;tag=abc\r\n\
                       To: <sip:34020000002000000001@3402000000>\r\n\
                       Call-ID: call-9999\r\n\
                       CSeq: 1 REGISTER\r\n\
                       Content-Length: 0\r\n\
                       \r\n";

        let msg = SipMessage::parse(req_str).unwrap();
        assert_eq!(msg.get_header("Call-ID"), Some("call-9999"));
        assert_eq!(msg.get_header("cseq"), Some("1 REGISTER"));

        if let StartLine::Request {
            method,
            uri,
            version,
        } = &msg.start_line
        {
            assert_eq!(method, "REGISTER");
            assert_eq!(uri, "sip:34020000002000000001@3402000000");
            assert_eq!(version, "SIP/2.0");
        } else {
            panic!("Expected Request start line");
        }
    }

    #[test]
    fn test_parse_sip_response_with_body() {
        let res_str = "SIP/2.0 200 OK\r\n\
                       Content-Type: application/sdp\r\n\
                       Content-Length: 18\r\n\
                       \r\n\
                       v=0\r\no=123 456 IP4";

        let msg = SipMessage::parse(res_str).unwrap();
        if let StartLine::Response {
            version,
            status,
            reason,
        } = &msg.start_line
        {
            assert_eq!(version, "SIP/2.0");
            assert_eq!(*status, 200);
            assert_eq!(reason, "OK");
        } else {
            panic!("Expected Response start line");
        }

        assert_eq!(msg.get_header("Content-Type"), Some("application/sdp"));
        assert_eq!(msg.body, b"v=0\r\no=123 456 IP4");
    }

    #[test]
    fn test_parse_sip_lenient_lf_only_terminators() {
        // Some embedded devices terminate header lines with bare `\n`.
        let req_str = "REGISTER sip:34020000002000000001@3402000000 SIP/2.0\n\
                       Via: SIP/2.0/UDP 192.168.1.100:5060;branch=z9hG4bK12345\n\
                       From: <sip:34020000001320000001@3402000000>;tag=abc\n\
                       To: <sip:34020000002000000001@3402000000>\n\
                       Call-ID: call-9999\n\
                       CSeq: 1 REGISTER\n\
                       Content-Length: 0\n\
                       \n";

        let msg = SipMessage::parse(req_str).unwrap();
        assert_eq!(msg.get_header("Call-ID"), Some("call-9999"));
        assert_eq!(msg.get_header("CSeq"), Some("1 REGISTER"));
        if let StartLine::Request { method, .. } = &msg.start_line {
            assert_eq!(method, "REGISTER");
        } else {
            panic!("Expected Request");
        }
    }

    #[test]
    fn test_parse_sip_lenient_duplicate_headers() {
        // Real devices may emit two `Via` headers when relayed through a proxy.
        let req_str = "REGISTER sip:x@host SIP/2.0\r\n\
                       Via: SIP/2.0/UDP 192.168.1.100:5060;branch=z9hG4bK1\r\n\
                       Via: SIP/2.0/UDP 10.0.0.1:5060;branch=z9hG4bK2\r\n\
                       Call-ID: dup\r\n\
                       Content-Length: 0\r\n\
                       \r\n";

        let msg = SipMessage::parse(req_str).unwrap();
        let vias: Vec<&str> = msg.get_headers_all("Via").collect();
        assert_eq!(vias.len(), 2);
        assert!(vias[0].contains("192.168.1.100"));
        assert!(vias[1].contains("10.0.0.1"));
    }

    #[test]
    fn test_parse_sip_picks_earliest_blank_line_separator() {
        // Headers terminated with bare `\n`, blank line uses `\n\n`; body contains
        // `\r\n\r\n` (e.g., a binary attachment whose bytes happen to spell out the
        // CRLF separator). The previous implementation searched for `\r\n\r\n` first
        // and returned that match, swallowing all of the headers as the body. The fix
        // walks every separator pattern and picks the earliest match.
        let req_str = "REGISTER sip:x@host SIP/2.0\n\
                       Call-ID: mixed-sep\n\
                       Content-Length: 12\n\
                       \n\
                       AAA\r\n\r\nEND";
        let msg = SipMessage::parse(req_str).unwrap();
        assert_eq!(msg.get_header("Call-ID"), Some("mixed-sep"));
        assert_eq!(msg.body, b"AAA\r\n\r\nEND");
    }

    #[test]
    fn test_to_bytes_preserves_non_utf8_body() {
        // Build a message whose body intentionally contains non-UTF-8 bytes and verify
        // that the byte-level serializer round-trips them. The Display impl can't do
        // this because `fmt::Formatter` only handles `&str`.
        let mut msg = SipMessage {
            start_line: StartLine::Request {
                method: "MESSAGE".to_string(),
                uri: "sip:x@host".to_string(),
                version: "SIP/2.0".to_string(),
            },
            headers: Vec::new(),
            body: vec![0xFF, 0xFE, 0xFD, 0xFC],
        };
        msg.set_header("Content-Length", "4");
        let wire = msg.to_bytes();
        assert!(wire.ends_with(&[0xFF, 0xFE, 0xFD, 0xFC]));
        // Header section must end with the `\r\n\r\n` blank-line separator.
        assert!(wire.windows(4).any(|w| w == [b'\r', b'\n', b'\r', b'\n']));
    }
}
