use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtspRtpInfoError {
    #[error("empty rtp-info header")]
    EmptyHeader,
    #[error("empty rtp-info stream entry")]
    EmptyStreamEntry,
    #[error("missing url in rtp-info stream")]
    MissingUrl,
    #[error("invalid {parameter} value: {value}")]
    InvalidParameter {
        parameter: &'static str,
        value: String,
    },
}

/// RTSP RTP-Info 头语义（RFC 2326 Section 12.33）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RtspRtpInfo {
    pub streams: Vec<RtspRtpInfoStream>,
}

impl RtspRtpInfo {
    pub fn new() -> Self {
        Self {
            streams: Vec::new(),
        }
    }

    pub fn parse(header_value: &str) -> Result<Self, RtspRtpInfoError> {
        let value = header_value.trim();
        if value.is_empty() {
            return Err(RtspRtpInfoError::EmptyHeader);
        }

        let mut streams = Vec::new();
        for stream_value in split_rtp_info_streams(value) {
            let stream_value = stream_value.trim();
            if stream_value.is_empty() {
                return Err(RtspRtpInfoError::EmptyStreamEntry);
            }
            streams.push(RtspRtpInfoStream::parse(stream_value)?);
        }
        if streams.is_empty() {
            return Err(RtspRtpInfoError::EmptyHeader);
        }
        Ok(Self { streams })
    }

    pub fn add_stream(&mut self, stream: RtspRtpInfoStream) {
        self.streams.push(stream);
    }

    pub fn find_by_url(&self, url: &str) -> Option<&RtspRtpInfoStream> {
        self.streams.iter().find(|stream| stream.url == url)
    }
}

impl fmt::Display for RtspRtpInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.streams.iter().map(ToString::to_string).collect();
        write!(f, "{}", parts.join(","))
    }
}

/// RTP-Info 中单个流条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspRtpInfoStream {
    pub url: String,
    pub seq: Option<u16>,
    pub rtptime: Option<u32>,
}

impl RtspRtpInfoStream {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            seq: None,
            rtptime: None,
        }
    }

    pub fn with_seq(mut self, seq: u16) -> Self {
        self.seq = Some(seq);
        self
    }

    pub fn with_rtptime(mut self, rtptime: u32) -> Self {
        self.rtptime = Some(rtptime);
        self
    }

    fn parse(value: &str) -> Result<Self, RtspRtpInfoError> {
        let mut url = None;
        let mut seq = None;
        let mut rtptime = None;

        for part in value.split(';').map(str::trim) {
            if part.is_empty() {
                continue;
            }
            let Some((key, raw_value)) = part.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let field_value = raw_value.trim();

            if key.eq_ignore_ascii_case("url") {
                if field_value.is_empty() {
                    return Err(RtspRtpInfoError::MissingUrl);
                }
                url = Some(field_value.to_string());
                continue;
            }
            if key.eq_ignore_ascii_case("seq") {
                let parsed =
                    field_value
                        .parse()
                        .map_err(|_| RtspRtpInfoError::InvalidParameter {
                            parameter: "seq",
                            value: field_value.to_string(),
                        })?;
                seq = Some(parsed);
                continue;
            }
            if key.eq_ignore_ascii_case("rtptime") {
                let parsed =
                    field_value
                        .parse()
                        .map_err(|_| RtspRtpInfoError::InvalidParameter {
                            parameter: "rtptime",
                            value: field_value.to_string(),
                        })?;
                rtptime = Some(parsed);
            }
        }

        let url = url.ok_or(RtspRtpInfoError::MissingUrl)?;
        Ok(Self { url, seq, rtptime })
    }
}

impl fmt::Display for RtspRtpInfoStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "url={}", self.url)?;
        if let Some(seq) = self.seq {
            write!(f, ";seq={seq}")?;
        }
        if let Some(rtptime) = self.rtptime {
            write!(f, ";rtptime={rtptime}")?;
        }
        Ok(())
    }
}

fn split_rtp_info_streams(value: &str) -> Vec<&str> {
    let mut streams = Vec::new();
    let mut start = 0;
    let mut in_url_value = false;

    for (index, ch) in value.char_indices() {
        match ch {
            '=' if !in_url_value => {
                let key = value[start..index].trim();
                if key.eq_ignore_ascii_case("url") {
                    in_url_value = true;
                }
            }
            ';' if in_url_value => {
                in_url_value = false;
            }
            ',' if in_url_value => {
                let remaining = value[index + ch.len_utf8()..].trim_start();
                if remaining.len() >= 4 && remaining[..4].eq_ignore_ascii_case("url=") {
                    streams.push(&value[start..index]);
                    start = index + 1;
                    in_url_value = false;
                }
            }
            ',' if !in_url_value => {
                streams.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < value.len() {
        streams.push(&value[start..]);
    }
    streams
}

#[cfg(test)]
mod tests {
    use super::{RtspRtpInfo, RtspRtpInfoError, RtspRtpInfoStream};

    #[test]
    fn test_parse_single_stream() {
        let info = RtspRtpInfo::parse("url=rtsp://example.com/audio;seq=1234;rtptime=12345678")
            .expect("parse single stream");
        assert_eq!(info.streams.len(), 1);
        assert_eq!(info.streams[0].url, "rtsp://example.com/audio");
        assert_eq!(info.streams[0].seq, Some(1234));
        assert_eq!(info.streams[0].rtptime, Some(12345678));
    }

    #[test]
    fn test_parse_multiple_streams() {
        let info = RtspRtpInfo::parse(
            "url=rtsp://example.com/audio;seq=100;rtptime=1000,url=rtsp://example.com/video;seq=200;rtptime=2000",
        )
        .expect("parse multiple streams");
        assert_eq!(info.streams.len(), 2);

        assert_eq!(info.streams[0].url, "rtsp://example.com/audio");
        assert_eq!(info.streams[0].seq, Some(100));
        assert_eq!(info.streams[0].rtptime, Some(1000));

        assert_eq!(info.streams[1].url, "rtsp://example.com/video");
        assert_eq!(info.streams[1].seq, Some(200));
        assert_eq!(info.streams[1].rtptime, Some(2000));
    }

    #[test]
    fn test_parse_without_optional() {
        let info = RtspRtpInfo::parse("url=rtsp://example.com/stream")
            .expect("parse stream without optional fields");
        assert_eq!(info.streams.len(), 1);
        assert_eq!(info.streams[0].url, "rtsp://example.com/stream");
        assert!(info.streams[0].seq.is_none());
        assert!(info.streams[0].rtptime.is_none());
    }

    #[test]
    fn test_display() {
        let mut info = RtspRtpInfo::new();
        info.add_stream(
            RtspRtpInfoStream::new("rtsp://example.com/audio")
                .with_seq(100)
                .with_rtptime(1000),
        );
        info.add_stream(
            RtspRtpInfoStream::new("rtsp://example.com/video")
                .with_seq(200)
                .with_rtptime(2000),
        );

        let header_value = info.to_string();
        assert!(header_value.contains("url=rtsp://example.com/audio"));
        assert!(header_value.contains("seq=100"));
        assert!(header_value.contains("rtptime=1000"));
        assert!(header_value.contains("url=rtsp://example.com/video"));
        assert!(header_value.contains("seq=200"));
        assert!(header_value.contains("rtptime=2000"));
    }

    #[test]
    fn test_find_by_url() {
        let info = RtspRtpInfo::parse(
            "url=rtsp://example.com/audio;seq=100,url=rtsp://example.com/video;seq=200",
        )
        .expect("parse streams");

        let audio = info.find_by_url("rtsp://example.com/audio");
        assert!(audio.is_some());
        assert_eq!(audio.and_then(|stream| stream.seq), Some(100));

        let video = info.find_by_url("rtsp://example.com/video");
        assert!(video.is_some());
        assert_eq!(video.and_then(|stream| stream.seq), Some(200));

        let unknown = info.find_by_url("rtsp://example.com/unknown");
        assert!(unknown.is_none());
    }

    #[test]
    fn parse_invalid_seq_reports_error() {
        let err = RtspRtpInfo::parse("url=rtsp://example.com/audio;seq=bad")
            .expect_err("invalid seq should fail");
        assert_eq!(
            err,
            RtspRtpInfoError::InvalidParameter {
                parameter: "seq",
                value: "bad".to_string(),
            }
        );
    }

    #[test]
    fn parse_missing_url_reports_error() {
        let err = RtspRtpInfo::parse("seq=100;rtptime=2000").expect_err("missing url should fail");
        assert_eq!(err, RtspRtpInfoError::MissingUrl);
    }
}
