use std::num::ParseIntError;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtspTransportError {
    #[error("empty transport header")]
    EmptyHeader,
    #[error("invalid transport protocol")]
    InvalidProtocol,
    #[error("invalid transport header value")]
    InvalidHeaderValue,
    #[error("invalid {parameter} value: {value}")]
    InvalidParameter {
        parameter: &'static str,
        value: String,
    },
    #[error("cannot infer second value for {parameter} from {value}")]
    MissingPairedValue {
        parameter: &'static str,
        value: String,
    },
}

/// RTSP Transport 头语义（RFC 2326 Section 12.39）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RtspTransport {
    /// 传输协议（例如 `RTP/AVP`、`RTP/AVP/TCP`）。
    pub protocol: String,
    /// 是否单播；`false` 表示 multicast。
    pub unicast: bool,
    /// interleaved 通道对。
    pub interleaved: Option<(u8, u8)>,
    /// 客户端 RTP/RTCP 端口对。
    pub client_port: Option<(u16, u16)>,
    /// 服务端 RTP/RTCP 端口对。
    pub server_port: Option<(u16, u16)>,
    /// SSRC（十六进制）。
    pub ssrc: Option<u32>,
    /// mode（例如 PLAY、RECORD）。
    pub mode: Option<String>,
    /// destination 地址。
    pub destination: Option<String>,
    /// source 地址。
    pub source: Option<String>,
    /// multicast TTL。
    pub ttl: Option<u8>,
    /// multicast layers。
    pub layers: Option<u32>,
    /// multicast port 对。
    pub port: Option<(u16, u16)>,
    /// append 标志。
    pub append: bool,
}

impl RtspTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rtp_avp_tcp_interleaved(rtp_channel: u8, rtcp_channel: u8) -> Self {
        Self {
            protocol: "RTP/AVP/TCP".to_string(),
            unicast: true,
            interleaved: Some((rtp_channel, rtcp_channel)),
            ..Self::default()
        }
    }

    pub fn rtp_avp_udp(client_rtp_port: u16, client_rtcp_port: u16) -> Self {
        Self {
            protocol: "RTP/AVP".to_string(),
            unicast: true,
            client_port: Some((client_rtp_port, client_rtcp_port)),
            ..Self::default()
        }
    }

    pub fn parse(header_value: &str) -> Result<Self, RtspTransportError> {
        let header_value = header_value.trim();
        if header_value.is_empty() {
            return Err(RtspTransportError::EmptyHeader);
        }
        if contains_invalid_header_char(header_value) {
            return Err(RtspTransportError::InvalidHeaderValue);
        }

        let mut transport = Self::default();
        let mut parts = header_value.split(';').map(str::trim);
        let Some(protocol) = parts.next() else {
            return Err(RtspTransportError::EmptyHeader);
        };
        if !is_valid_transport_protocol(protocol) {
            return Err(RtspTransportError::InvalidProtocol);
        }
        transport.protocol = protocol.to_string();

        for part in parts {
            if part.is_empty() {
                continue;
            }

            if part.eq_ignore_ascii_case("unicast") {
                transport.unicast = true;
                continue;
            }
            if part.eq_ignore_ascii_case("multicast") {
                transport.unicast = false;
                continue;
            }
            if part.eq_ignore_ascii_case("append") {
                transport.append = true;
                continue;
            }
            if part.eq_ignore_ascii_case("destination") {
                transport.destination = Some(String::new());
                continue;
            }

            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            if key.is_empty()
                || contains_invalid_header_char(key)
                || contains_invalid_header_char(value)
            {
                return Err(RtspTransportError::InvalidHeaderValue);
            }

            if key.eq_ignore_ascii_case("interleaved") {
                transport.interleaved = Some(parse_u8_pair("interleaved", value)?);
            } else if key.eq_ignore_ascii_case("client_port") {
                transport.client_port = Some(parse_u16_pair("client_port", value)?);
            } else if key.eq_ignore_ascii_case("server_port") {
                transport.server_port = Some(parse_u16_pair("server_port", value)?);
            } else if key.eq_ignore_ascii_case("port") {
                transport.port = Some(parse_u16_pair("port", value)?);
            } else if key.eq_ignore_ascii_case("ssrc") {
                transport.ssrc = Some(parse_ssrc(value)?);
            } else if key.eq_ignore_ascii_case("mode") {
                transport.mode = Some(value.trim_matches('"').to_string());
            } else if key.eq_ignore_ascii_case("destination") {
                transport.destination = Some(value.to_string());
            } else if key.eq_ignore_ascii_case("source") {
                transport.source = Some(value.to_string());
            } else if key.eq_ignore_ascii_case("ttl") {
                transport.ttl = Some(parse_u8("ttl", value)?);
            } else if key.eq_ignore_ascii_case("layers") {
                transport.layers = Some(parse_u32("layers", value)?);
            }
        }

        Ok(transport)
    }

    pub fn parse_multiple(header_value: &str) -> Result<Vec<Self>, RtspTransportError> {
        let header_value = header_value.trim();
        if header_value.is_empty() {
            return Err(RtspTransportError::EmptyHeader);
        }
        if contains_invalid_header_char(header_value) {
            return Err(RtspTransportError::InvalidHeaderValue);
        }

        let mut transports = Vec::new();
        for part in header_value.split(',').map(str::trim) {
            if part.is_empty() {
                continue;
            }
            transports.push(Self::parse(part)?);
        }

        if transports.is_empty() {
            return Err(RtspTransportError::EmptyHeader);
        }
        Ok(transports)
    }

    pub fn to_header(&self) -> String {
        let mut parts = vec![self.protocol.clone()];
        if self.unicast {
            parts.push("unicast".to_string());
        } else {
            parts.push("multicast".to_string());
        }

        if let Some(destination) = &self.destination {
            parts.push(format!("destination={destination}"));
        }
        if let Some(source) = &self.source {
            parts.push(format!("source={source}"));
        }
        if let Some((a, b)) = self.interleaved {
            parts.push(format!("interleaved={a}-{b}"));
        }
        if self.append {
            parts.push("append".to_string());
        }
        if let Some(ttl) = self.ttl {
            parts.push(format!("ttl={ttl}"));
        }
        if let Some(layers) = self.layers {
            parts.push(format!("layers={layers}"));
        }
        if let Some((a, b)) = self.port {
            parts.push(format!("port={a}-{b}"));
        }
        if let Some((a, b)) = self.client_port {
            parts.push(format!("client_port={a}-{b}"));
        }
        if let Some((a, b)) = self.server_port {
            parts.push(format!("server_port={a}-{b}"));
        }
        if let Some(ssrc) = self.ssrc {
            parts.push(format!("ssrc={ssrc:08X}"));
        }
        if let Some(mode) = &self.mode {
            parts.push(format!("mode=\"{mode}\""));
        }
        parts.join(";")
    }
}

fn contains_invalid_header_char(value: &str) -> bool {
    value
        .bytes()
        .any(|byte| (byte < 0x20 && byte != b'\t') || byte == 0x7f)
}

fn is_valid_transport_protocol(protocol: &str) -> bool {
    !protocol.is_empty()
        && protocol.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b'+')
        })
}

fn parse_u8_pair(parameter: &'static str, value: &str) -> Result<(u8, u8), RtspTransportError> {
    if let Some((a, b)) = value.split_once('-') {
        return Ok((
            parse_u8(parameter, a.trim())?,
            parse_u8(parameter, b.trim())?,
        ));
    }
    let first = parse_u8(parameter, value)?;
    let second = first
        .checked_add(1)
        .ok_or_else(|| RtspTransportError::MissingPairedValue {
            parameter,
            value: value.to_string(),
        })?;
    Ok((first, second))
}

fn parse_u16_pair(parameter: &'static str, value: &str) -> Result<(u16, u16), RtspTransportError> {
    if let Some((a, b)) = value.split_once('-') {
        return Ok((
            parse_u16(parameter, a.trim())?,
            parse_u16(parameter, b.trim())?,
        ));
    }
    let first = parse_u16(parameter, value)?;
    let second = first
        .checked_add(1)
        .ok_or_else(|| RtspTransportError::MissingPairedValue {
            parameter,
            value: value.to_string(),
        })?;
    Ok((first, second))
}

fn parse_ssrc(value: &str) -> Result<u32, RtspTransportError> {
    let value = value.trim();
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    u32::from_str_radix(trimmed, 16).map_err(|_| RtspTransportError::InvalidParameter {
        parameter: "ssrc",
        value: value.to_string(),
    })
}

fn parse_u8(parameter: &'static str, value: &str) -> Result<u8, RtspTransportError> {
    parse_int(parameter, value, str::parse::<u8>)
}

fn parse_u16(parameter: &'static str, value: &str) -> Result<u16, RtspTransportError> {
    parse_int(parameter, value, str::parse::<u16>)
}

fn parse_u32(parameter: &'static str, value: &str) -> Result<u32, RtspTransportError> {
    parse_int(parameter, value, str::parse::<u32>)
}

fn parse_int<T>(
    parameter: &'static str,
    value: &str,
    parser: impl FnOnce(&str) -> Result<T, ParseIntError>,
) -> Result<T, RtspTransportError> {
    parser(value.trim()).map_err(|_| RtspTransportError::InvalidParameter {
        parameter,
        value: value.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{RtspTransport, RtspTransportError};

    #[test]
    fn constructors_match_vendor_semantics() {
        let tcp = RtspTransport::rtp_avp_tcp_interleaved(2, 3);
        assert_eq!(tcp.protocol, "RTP/AVP/TCP");
        assert!(tcp.unicast);
        assert_eq!(tcp.interleaved, Some((2, 3)));
        assert_eq!(tcp.client_port, None);

        let udp = RtspTransport::rtp_avp_udp(5000, 5001);
        assert_eq!(udp.protocol, "RTP/AVP");
        assert!(udp.unicast);
        assert_eq!(udp.client_port, Some((5000, 5001)));
        assert_eq!(udp.interleaved, None);
    }

    #[test]
    fn transport_roundtrip_preserves_known_fields() {
        let transport = RtspTransport {
            protocol: "RTP/AVP/TCP".to_string(),
            unicast: true,
            interleaved: Some((0, 1)),
            client_port: Some((5000, 5001)),
            server_port: Some((6000, 6001)),
            ssrc: Some(0x1234_ABCD),
            mode: Some("PLAY".to_string()),
            destination: Some("239.0.0.1".to_string()),
            source: Some("10.0.0.1".to_string()),
            ttl: Some(16),
            layers: Some(2),
            port: Some((7000, 7001)),
            append: true,
        };

        let header = transport.to_header();
        let parsed = RtspTransport::parse(&header).expect("parse transport header");
        assert_eq!(parsed, transport);
    }

    #[test]
    fn parse_rejects_invalid_ttl() {
        let err = RtspTransport::parse("RTP/AVP;ttl=abc").expect_err("invalid ttl must fail");
        assert!(matches!(
            err,
            RtspTransportError::InvalidParameter {
                parameter: "ttl",
                value
            } if value == "abc"
        ));
    }

    #[test]
    fn parse_rejects_interleaved_without_pair_on_boundary() {
        let err = RtspTransport::parse("RTP/AVP/TCP;interleaved=255")
            .expect_err("interleaved boundary must fail");
        assert!(matches!(
            err,
            RtspTransportError::MissingPairedValue {
                parameter: "interleaved",
                value
            } if value == "255"
        ));
    }

    #[test]
    fn parse_multiple_roundtrip() {
        let header = "RTP/AVP/TCP;unicast;interleaved=0-1, RTP/AVP;multicast;client_port=5000-5001";
        let parsed = RtspTransport::parse_multiple(header).expect("parse multiple transport");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].protocol, "RTP/AVP/TCP");
        assert_eq!(parsed[1].protocol, "RTP/AVP");
    }

    #[test]
    fn parse_infers_paired_values_for_single_ports_and_channels() {
        let parsed = RtspTransport::parse(
            "RTP/AVP/TCP;unicast;interleaved=2;client_port=5000;server_port=6000;port=7000",
        )
        .expect("parse transport header");
        assert_eq!(parsed.interleaved, Some((2, 3)));
        assert_eq!(parsed.client_port, Some((5000, 5001)));
        assert_eq!(parsed.server_port, Some((6000, 6001)));
        assert_eq!(parsed.port, Some((7000, 7001)));
    }

    #[test]
    fn parse_rejects_invalid_protocol_characters() {
        let err = RtspTransport::parse("RTP AVP;unicast").expect_err("invalid protocol");
        assert!(matches!(err, RtspTransportError::InvalidProtocol));
    }

    #[test]
    fn parse_rejects_control_character_in_header_value() {
        let err = RtspTransport::parse("RTP/AVP;source=10.0.0.1\r\nX-Test: 1")
            .expect_err("header injection must fail");
        assert!(matches!(err, RtspTransportError::InvalidHeaderValue));
    }

    #[test]
    fn parse_multiple_reports_invalid_entry_error() {
        let err =
            RtspTransport::parse_multiple("RTP/AVP/TCP;unicast;interleaved=0-1, RTP/AVP;ttl=bad")
                .expect_err("invalid entry in transport list must fail");
        assert!(matches!(
            err,
            RtspTransportError::InvalidParameter {
                parameter: "ttl",
                value
            } if value == "bad"
        ));
    }
}
