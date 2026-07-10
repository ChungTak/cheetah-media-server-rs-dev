use std::fmt;

/// `SdpError` enumeration.
/// `SdpError` 枚举.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SdpError {
    /// `MissingRequiredField` variant.
    /// `MissingRequiredField` 变体.
    #[error("missing required SDP field `{field}`")]
    MissingRequiredField { field: &'static str },
    /// `InvalidOrigin` variant.
    /// `InvalidOrigin` 变体.
    #[error("invalid SDP origin field: {0}")]
    InvalidOrigin(String),
    /// `InvalidConnection` variant.
    /// `InvalidConnection` 变体.
    #[error("invalid SDP connection field: {0}")]
    InvalidConnection(String),
    /// `InvalidBandwidth` variant.
    /// `InvalidBandwidth` 变体.
    #[error("invalid SDP bandwidth field: {0}")]
    InvalidBandwidth(String),
    /// `InvalidTiming` variant.
    /// `InvalidTiming` 变体.
    #[error("invalid SDP timing field: {0}")]
    InvalidTiming(String),
    /// `InvalidMedia` variant.
    /// `InvalidMedia` 变体.
    #[error("invalid SDP media field: {0}")]
    InvalidMedia(String),
    /// `InvalidAttribute` variant.
    /// `InvalidAttribute` 变体.
    #[error("invalid SDP attribute `{attribute}`: {value}")]
    InvalidAttribute {
        attribute: &'static str,
        value: String,
    },
    /// `InvalidNumber` variant.
    /// `InvalidNumber` 变体.
    #[error("invalid numeric value for `{field}`: {value}")]
    InvalidNumber { field: &'static str, value: String },
}

/// SDP 会话描述（RFC 8866）。
#[derive(Debug, Clone, PartialEq)]
pub struct Sdp {
    /// `version` field of type `u8`.
    /// `version` 字段，类型为 `u8`.
    pub version: u8,
    /// `origin` field of type `SdpOrigin`.
    /// `origin` 字段，类型为 `SdpOrigin`.
    pub origin: SdpOrigin,
    /// `session_name` field of type `String`.
    /// `session_name` 字段，类型为 `String`.
    pub session_name: String,
    /// `session_info` field.
    /// `session_info` 字段.
    pub session_info: Option<String>,
    /// `uri` field.
    /// `uri` 字段.
    pub uri: Option<String>,
    /// `email` field.
    /// `email` 字段.
    pub email: Option<String>,
    /// `phone` field.
    /// `phone` 字段.
    pub phone: Option<String>,
    /// `connection` field.
    /// `connection` 字段.
    pub connection: Option<SdpConnection>,
    /// `bandwidth` field.
    /// `bandwidth` 字段.
    pub bandwidth: Vec<SdpBandwidth>,
    /// `timing` field of type `SdpTiming`.
    /// `timing` 字段，类型为 `SdpTiming`.
    pub timing: SdpTiming,
    /// `attributes` field.
    /// `attributes` 字段.
    pub attributes: Vec<SdpAttribute>,
    /// `media` field.
    /// `media` 字段.
    pub media: Vec<SdpMedia>,
}

/// `o=` origin 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpOrigin {
    /// `username` field of type `String`.
    /// `username` 字段，类型为 `String`.
    pub username: String,
    /// `session_id` field of type `String`.
    /// `session_id` 字段，类型为 `String`.
    pub session_id: String,
    /// `session_version` field of type `String`.
    /// `session_version` 字段，类型为 `String`.
    pub session_version: String,
    /// `net_type` field of type `String`.
    /// `net_type` 字段，类型为 `String`.
    pub net_type: String,
    /// `addr_type` field of type `String`.
    /// `addr_type` 字段，类型为 `String`.
    pub addr_type: String,
    /// `address` field of type `String`.
    /// `address` 字段，类型为 `String`.
    pub address: String,
}

/// `c=` connection 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpConnection {
    /// `net_type` field of type `String`.
    /// `net_type` 字段，类型为 `String`.
    pub net_type: String,
    /// `addr_type` field of type `String`.
    /// `addr_type` 字段，类型为 `String`.
    pub addr_type: String,
    /// `address` field of type `String`.
    /// `address` 字段，类型为 `String`.
    pub address: String,
}

/// `b=` bandwidth 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpBandwidth {
    /// `bwtype` field of type `String`.
    /// `bwtype` 字段，类型为 `String`.
    pub bwtype: String,
    /// `bandwidth` field of type `u64`.
    /// `bandwidth` 字段，类型为 `u64`.
    pub bandwidth: u64,
}

/// `t=` timing 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpTiming {
    /// `start` field of type `u64`.
    /// `start` 字段，类型为 `u64`.
    pub start: u64,
    /// `stop` field of type `u64`.
    /// `stop` 字段，类型为 `u64`.
    pub stop: u64,
}

/// `m=` media 描述。
#[derive(Debug, Clone, PartialEq)]
pub struct SdpMedia {
    /// `media_type` field of type `String`.
    /// `media_type` 字段，类型为 `String`.
    pub media_type: String,
    /// `port` field of type `u16`.
    /// `port` 字段，类型为 `u16`.
    pub port: u16,
    /// `num_ports` field.
    /// `num_ports` 字段.
    pub num_ports: Option<u16>,
    /// `protocol` field of type `String`.
    /// `protocol` 字段，类型为 `String`.
    pub protocol: String,
    /// `formats` field.
    /// `formats` 字段.
    pub formats: Vec<String>,
    /// `title` field.
    /// `title` 字段.
    pub title: Option<String>,
    /// `connection` field.
    /// `connection` 字段.
    pub connection: Option<SdpConnection>,
    /// `bandwidth` field.
    /// `bandwidth` 字段.
    pub bandwidth: Vec<SdpBandwidth>,
    /// `attributes` field.
    /// `attributes` 字段.
    pub attributes: Vec<SdpAttribute>,
}

/// `a=` attribute 字段。
#[derive(Debug, Clone, PartialEq)]
pub enum SdpAttribute {
    /// `Rtpmap` variant.
    /// `Rtpmap` 变体.
    Rtpmap {
        payload_type: u8,
        encoding: String,
        clock_rate: u32,
        encoding_params: Option<String>,
    },
    /// `Fmtp` variant.
    /// `Fmtp` 变体.
    Fmtp {
        payload_type: u8,
        parameters: String,
    },
    /// `Control` variant.
    /// `Control` 变体.
    Control(String),
    /// `Range` variant.
    /// `Range` 变体.
    Range(String),
    /// `Recvonly` variant.
    /// `Recvonly` 变体.
    Recvonly,
    /// `Sendrecv` variant.
    /// `Sendrecv` 变体.
    Sendrecv,
    /// `Sendonly` variant.
    /// `Sendonly` 变体.
    Sendonly,
    /// `Inactive` variant.
    /// `Inactive` 变体.
    Inactive,
    /// `Framerate` variant.
    /// `Framerate` 变体.
    Framerate(f64),
    /// `Tool` variant.
    /// `Tool` 变体.
    Tool(String),
    /// `Type` variant.
    /// `Type` 变体.
    Type(String),
    /// `Charset` variant.
    /// `Charset` 变体.
    Charset(String),
    /// `Sdplang` variant.
    /// `Sdplang` 变体.
    Sdplang(String),
    /// `Lang` variant.
    /// `Lang` 变体.
    Lang(String),
    /// `Custom` variant.
    /// `Custom` 变体.
    Custom { name: String, value: Option<String> },
}

impl Sdp {
    /// `parse` function.
    /// `parse` 函数.
    pub fn parse(text: &str) -> Result<Self, SdpError> {
        let mut version = None;
        let mut origin = None;
        let mut session_name = None;
        let mut session_info = None;
        let mut uri = None;
        let mut email = None;
        let mut phone = None;
        let mut connection = None;
        let mut bandwidth = Vec::new();
        let mut timing = None;
        let mut attributes = Vec::new();
        let mut media = Vec::new();
        let mut current_media: Option<SdpMedia> = None;

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }

            let bytes = line.as_bytes();
            if bytes.len() < 2 || bytes[1] != b'=' || !bytes[0].is_ascii_alphabetic() {
                continue;
            }

            let field_type = bytes[0];
            let value = &line[2..];
            match field_type {
                b'v' => {
                    version = Some(parse_u8("version", value)?);
                }
                b'o' => {
                    origin = Some(parse_origin(value)?);
                }
                b's' => {
                    session_name = Some(value.to_string());
                }
                b'i' => {
                    if let Some(ref mut media) = current_media {
                        media.title = Some(value.to_string());
                    } else {
                        session_info = Some(value.to_string());
                    }
                }
                b'u' => {
                    uri = Some(value.to_string());
                }
                b'e' => {
                    email = Some(value.to_string());
                }
                b'p' => {
                    phone = Some(value.to_string());
                }
                b'c' => {
                    let parsed = parse_connection(value)?;
                    if let Some(ref mut media) = current_media {
                        media.connection = Some(parsed);
                    } else {
                        connection = Some(parsed);
                    }
                }
                b'b' => {
                    let parsed = parse_bandwidth(value)?;
                    if let Some(ref mut media) = current_media {
                        media.bandwidth.push(parsed);
                    } else {
                        bandwidth.push(parsed);
                    }
                }
                b't' => {
                    timing = Some(parse_timing(value)?);
                }
                b'a' => {
                    let parsed = parse_attribute(value)?;
                    if let Some(ref mut media) = current_media {
                        media.attributes.push(parsed);
                    } else {
                        attributes.push(parsed);
                    }
                }
                b'm' => {
                    if let Some(item) = current_media.take() {
                        media.push(item);
                    }
                    current_media = Some(parse_media(value)?);
                }
                _ => {}
            }
        }

        if let Some(item) = current_media.take() {
            media.push(item);
        }

        Ok(Self {
            version: version.unwrap_or(0),
            origin: origin.ok_or(SdpError::MissingRequiredField { field: "o" })?,
            session_name: session_name.ok_or(SdpError::MissingRequiredField { field: "s" })?,
            session_info,
            uri,
            email,
            phone,
            connection,
            bandwidth,
            timing: timing.ok_or(SdpError::MissingRequiredField { field: "t" })?,
            attributes,
            media,
        })
    }

    /// `builder` function.
    /// `builder` 函数.
    pub fn builder() -> SdpBuilder {
        SdpBuilder::new()
    }
}

impl fmt::Display for Sdp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v={}\r\n", self.version)?;
        write!(
            f,
            "o={} {} {} {} {} {}\r\n",
            self.origin.username,
            self.origin.session_id,
            self.origin.session_version,
            self.origin.net_type,
            self.origin.addr_type,
            self.origin.address
        )?;
        write!(f, "s={}\r\n", self.session_name)?;

        if let Some(info) = &self.session_info {
            write!(f, "i={}\r\n", info)?;
        }
        if let Some(uri) = &self.uri {
            write!(f, "u={}\r\n", uri)?;
        }
        if let Some(email) = &self.email {
            write!(f, "e={}\r\n", email)?;
        }
        if let Some(phone) = &self.phone {
            write!(f, "p={}\r\n", phone)?;
        }
        if let Some(connection) = &self.connection {
            write!(
                f,
                "c={} {} {}\r\n",
                connection.net_type, connection.addr_type, connection.address
            )?;
        }
        for bw in &self.bandwidth {
            write!(f, "b={}:{}\r\n", bw.bwtype, bw.bandwidth)?;
        }
        write!(f, "t={} {}\r\n", self.timing.start, self.timing.stop)?;

        for attr in &self.attributes {
            write!(f, "a=")?;
            fmt_attribute(f, attr)?;
            write!(f, "\r\n")?;
        }

        for media in &self.media {
            if let Some(num_ports) = media.num_ports {
                write!(
                    f,
                    "m={} {}/{} {}",
                    media.media_type, media.port, num_ports, media.protocol
                )?;
            } else {
                write!(
                    f,
                    "m={} {} {}",
                    media.media_type, media.port, media.protocol
                )?;
            }

            for format in &media.formats {
                write!(f, " {}", format)?;
            }
            write!(f, "\r\n")?;

            if let Some(title) = &media.title {
                write!(f, "i={}\r\n", title)?;
            }
            if let Some(connection) = &media.connection {
                write!(
                    f,
                    "c={} {} {}\r\n",
                    connection.net_type, connection.addr_type, connection.address
                )?;
            }
            for bw in &media.bandwidth {
                write!(f, "b={}:{}\r\n", bw.bwtype, bw.bandwidth)?;
            }
            for attr in &media.attributes {
                write!(f, "a=")?;
                fmt_attribute(f, attr)?;
                write!(f, "\r\n")?;
            }
        }

        Ok(())
    }
}

fn fmt_attribute(f: &mut fmt::Formatter<'_>, attr: &SdpAttribute) -> fmt::Result {
    match attr {
        SdpAttribute::Rtpmap {
            payload_type,
            encoding,
            clock_rate,
            encoding_params,
        } => {
            write!(f, "rtpmap:{} {}/{}", payload_type, encoding, clock_rate)?;
            if let Some(params) = encoding_params {
                write!(f, "/{}", params)?;
            }
        }
        SdpAttribute::Fmtp {
            payload_type,
            parameters,
        } => {
            write!(f, "fmtp:{} {}", payload_type, parameters)?;
        }
        SdpAttribute::Control(url) => {
            write!(f, "control:{}", url)?;
        }
        SdpAttribute::Range(range) => {
            write!(f, "range:{}", range)?;
        }
        SdpAttribute::Recvonly => {
            write!(f, "recvonly")?;
        }
        SdpAttribute::Sendrecv => {
            write!(f, "sendrecv")?;
        }
        SdpAttribute::Sendonly => {
            write!(f, "sendonly")?;
        }
        SdpAttribute::Inactive => {
            write!(f, "inactive")?;
        }
        SdpAttribute::Framerate(fps) => {
            write!(f, "framerate:{}", fps)?;
        }
        SdpAttribute::Tool(name) => {
            write!(f, "tool:{}", name)?;
        }
        SdpAttribute::Type(media_type) => {
            write!(f, "type:{}", media_type)?;
        }
        SdpAttribute::Charset(charset) => {
            write!(f, "charset:{}", charset)?;
        }
        SdpAttribute::Sdplang(lang) => {
            write!(f, "sdplang:{}", lang)?;
        }
        SdpAttribute::Lang(lang) => {
            write!(f, "lang:{}", lang)?;
        }
        SdpAttribute::Custom { name, value } => {
            write!(f, "{}", name)?;
            if let Some(value) = value {
                write!(f, ":{}", value)?;
            }
        }
    }
    Ok(())
}

/// SDP 构造器。
#[derive(Debug, Clone)]
pub struct SdpBuilder {
    /// `version` field of type `u8`.
    /// `version` 字段，类型为 `u8`.
    version: u8,
    /// `origin` field.
    /// `origin` 字段.
    origin: Option<SdpOrigin>,
    /// `session_name` field.
    /// `session_name` 字段.
    session_name: Option<String>,
    /// `session_info` field.
    /// `session_info` 字段.
    session_info: Option<String>,
    /// `uri` field.
    /// `uri` 字段.
    uri: Option<String>,
    /// `email` field.
    /// `email` 字段.
    email: Option<String>,
    /// `phone` field.
    /// `phone` 字段.
    phone: Option<String>,
    /// `connection` field.
    /// `connection` 字段.
    connection: Option<SdpConnection>,
    /// `bandwidth` field.
    /// `bandwidth` 字段.
    bandwidth: Vec<SdpBandwidth>,
    /// `timing` field.
    /// `timing` 字段.
    timing: Option<SdpTiming>,
    /// `attributes` field.
    /// `attributes` 字段.
    attributes: Vec<SdpAttribute>,
    /// `media` field.
    /// `media` 字段.
    media: Vec<SdpMedia>,
}

impl Default for SdpBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SdpBuilder {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new() -> Self {
        Self {
            version: 0,
            origin: None,
            session_name: None,
            session_info: None,
            uri: None,
            email: None,
            phone: None,
            connection: None,
            bandwidth: Vec::new(),
            timing: None,
            attributes: Vec::new(),
            media: Vec::new(),
        }
    }

    /// `version` function.
    /// `version` 函数.
    pub fn version(mut self, version: u8) -> Self {
        self.version = version;
        self
    }

    /// `origin` function.
    /// `origin` 函数.
    pub fn origin(mut self, origin: SdpOrigin) -> Self {
        self.origin = Some(origin);
        self
    }

    /// `origin_simple` function.
    /// `origin_simple` 函数.
    pub fn origin_simple(mut self, session_id: &str, address: &str) -> Self {
        self.origin = Some(SdpOrigin {
            username: "-".to_string(),
            session_id: session_id.to_string(),
            session_version: "1".to_string(),
            net_type: "IN".to_string(),
            addr_type: "IP4".to_string(),
            address: address.to_string(),
        });
        self
    }

    /// `session_name` function.
    /// `session_name` 函数.
    pub fn session_name(mut self, name: &str) -> Self {
        self.session_name = Some(name.to_string());
        self
    }

    /// `session_info` function.
    /// `session_info` 函数.
    pub fn session_info(mut self, info: &str) -> Self {
        self.session_info = Some(info.to_string());
        self
    }

    /// `uri` function.
    /// `uri` 函数.
    pub fn uri(mut self, uri: &str) -> Self {
        self.uri = Some(uri.to_string());
        self
    }

    /// `email` function.
    /// `email` 函数.
    pub fn email(mut self, email: &str) -> Self {
        self.email = Some(email.to_string());
        self
    }

    /// `phone` function.
    /// `phone` 函数.
    pub fn phone(mut self, phone: &str) -> Self {
        self.phone = Some(phone.to_string());
        self
    }

    /// `connection` function.
    /// `connection` 函数.
    pub fn connection(mut self, connection: SdpConnection) -> Self {
        self.connection = Some(connection);
        self
    }

    /// `connection_simple` function.
    /// `connection_simple` 函数.
    pub fn connection_simple(mut self, address: &str) -> Self {
        self.connection = Some(SdpConnection {
            net_type: "IN".to_string(),
            addr_type: "IP4".to_string(),
            address: address.to_string(),
        });
        self
    }

    /// `bandwidth` function.
    /// `bandwidth` 函数.
    pub fn bandwidth(mut self, bwtype: &str, bandwidth: u64) -> Self {
        self.bandwidth.push(SdpBandwidth {
            bwtype: bwtype.to_string(),
            bandwidth,
        });
        self
    }

    /// `timing` function.
    /// `timing` 函数.
    pub fn timing(mut self, start: u64, stop: u64) -> Self {
        self.timing = Some(SdpTiming { start, stop });
        self
    }

    /// `attribute` function.
    /// `attribute` 函数.
    pub fn attribute(mut self, attr: SdpAttribute) -> Self {
        self.attributes.push(attr);
        self
    }

    /// `control` function.
    /// `control` 函数.
    pub fn control(self, url: &str) -> Self {
        self.attribute(SdpAttribute::Control(url.to_string()))
    }

    /// `range` function.
    /// `range` 函数.
    pub fn range(self, range: &str) -> Self {
        self.attribute(SdpAttribute::Range(range.to_string()))
    }

    /// `add_media` function.
    /// `add_media` 函数.
    pub fn add_media(mut self, media: SdpMedia) -> Self {
        self.media.push(media);
        self
    }

    /// `build` function.
    /// `build` 函数.
    pub fn build(self) -> Result<Sdp, SdpError> {
        Ok(Sdp {
            version: self.version,
            origin: self
                .origin
                .ok_or(SdpError::MissingRequiredField { field: "o" })?,
            session_name: self
                .session_name
                .ok_or(SdpError::MissingRequiredField { field: "s" })?,
            session_info: self.session_info,
            uri: self.uri,
            email: self.email,
            phone: self.phone,
            connection: self.connection,
            bandwidth: self.bandwidth,
            timing: self
                .timing
                .ok_or(SdpError::MissingRequiredField { field: "t" })?,
            attributes: self.attributes,
            media: self.media,
        })
    }
}

/// SDP 媒体描述构造器。
#[derive(Debug, Clone)]
pub struct SdpMediaBuilder {
    /// `media_type` field of type `String`.
    /// `media_type` 字段，类型为 `String`.
    media_type: String,
    /// `port` field of type `u16`.
    /// `port` 字段，类型为 `u16`.
    port: u16,
    /// `num_ports` field.
    /// `num_ports` 字段.
    num_ports: Option<u16>,
    /// `protocol` field of type `String`.
    /// `protocol` 字段，类型为 `String`.
    protocol: String,
    /// `formats` field.
    /// `formats` 字段.
    formats: Vec<String>,
    /// `title` field.
    /// `title` 字段.
    title: Option<String>,
    /// `connection` field.
    /// `connection` 字段.
    connection: Option<SdpConnection>,
    /// `bandwidth` field.
    /// `bandwidth` 字段.
    bandwidth: Vec<SdpBandwidth>,
    /// `attributes` field.
    /// `attributes` 字段.
    attributes: Vec<SdpAttribute>,
}

impl SdpMediaBuilder {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(media_type: &str, port: u16, protocol: &str) -> Self {
        Self {
            media_type: media_type.to_string(),
            port,
            num_ports: None,
            protocol: protocol.to_string(),
            formats: Vec::new(),
            title: None,
            connection: None,
            bandwidth: Vec::new(),
            attributes: Vec::new(),
        }
    }

    /// `video` function.
    /// `video` 函数.
    pub fn video(port: u16) -> Self {
        Self::new("video", port, "RTP/AVP")
    }

    /// `audio` function.
    /// `audio` 函数.
    pub fn audio(port: u16) -> Self {
        Self::new("audio", port, "RTP/AVP")
    }

    /// `num_ports` function.
    /// `num_ports` 函数.
    pub fn num_ports(mut self, num_ports: u16) -> Self {
        self.num_ports = Some(num_ports);
        self
    }

    /// `format` function.
    /// `format` 函数.
    pub fn format(mut self, format: &str) -> Self {
        self.formats.push(format.to_string());
        self
    }

    /// `title` function.
    /// `title` 函数.
    pub fn title(mut self, title: &str) -> Self {
        self.title = Some(title.to_string());
        self
    }

    /// `connection` function.
    /// `connection` 函数.
    pub fn connection(mut self, connection: SdpConnection) -> Self {
        self.connection = Some(connection);
        self
    }

    /// `bandwidth` function.
    /// `bandwidth` 函数.
    pub fn bandwidth(mut self, bwtype: &str, bandwidth: u64) -> Self {
        self.bandwidth.push(SdpBandwidth {
            bwtype: bwtype.to_string(),
            bandwidth,
        });
        self
    }

    /// `attribute` function.
    /// `attribute` 函数.
    pub fn attribute(mut self, attr: SdpAttribute) -> Self {
        self.attributes.push(attr);
        self
    }

    /// `rtpmap` function.
    /// `rtpmap` 函数.
    pub fn rtpmap(self, payload_type: u8, encoding: &str, clock_rate: u32) -> Self {
        self.attribute(SdpAttribute::Rtpmap {
            payload_type,
            encoding: encoding.to_string(),
            clock_rate,
            encoding_params: None,
        })
    }

    /// `fmtp` function.
    /// `fmtp` 函数.
    pub fn fmtp(self, payload_type: u8, parameters: &str) -> Self {
        self.attribute(SdpAttribute::Fmtp {
            payload_type,
            parameters: parameters.to_string(),
        })
    }

    /// `control` function.
    /// `control` 函数.
    pub fn control(self, url: &str) -> Self {
        self.attribute(SdpAttribute::Control(url.to_string()))
    }

    /// `range` function.
    /// `range` 函数.
    pub fn range(self, range: &str) -> Self {
        self.attribute(SdpAttribute::Range(range.to_string()))
    }

    /// `build` function.
    /// `build` 函数.
    pub fn build(self) -> SdpMedia {
        SdpMedia {
            media_type: self.media_type,
            port: self.port,
            num_ports: self.num_ports,
            protocol: self.protocol,
            formats: self.formats,
            title: self.title,
            connection: self.connection,
            bandwidth: self.bandwidth,
            attributes: self.attributes,
        }
    }
}

fn parse_origin(value: &str) -> Result<SdpOrigin, SdpError> {
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() != 6 {
        return Err(SdpError::InvalidOrigin(value.to_string()));
    }

    Ok(SdpOrigin {
        username: parts[0].to_string(),
        session_id: parts[1].to_string(),
        session_version: parts[2].to_string(),
        net_type: parts[3].to_string(),
        addr_type: parts[4].to_string(),
        address: parts[5].to_string(),
    })
}

fn parse_connection(value: &str) -> Result<SdpConnection, SdpError> {
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(SdpError::InvalidConnection(value.to_string()));
    }

    Ok(SdpConnection {
        net_type: parts[0].to_string(),
        addr_type: parts[1].to_string(),
        address: parts[2].to_string(),
    })
}

fn parse_bandwidth(value: &str) -> Result<SdpBandwidth, SdpError> {
    let Some((bwtype, bandwidth)) = value.split_once(':') else {
        return Err(SdpError::InvalidBandwidth(value.to_string()));
    };
    let bandwidth = parse_u64("bandwidth", bandwidth)?;
    Ok(SdpBandwidth {
        bwtype: bwtype.to_string(),
        bandwidth,
    })
}

fn parse_timing(value: &str) -> Result<SdpTiming, SdpError> {
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() != 2 {
        return Err(SdpError::InvalidTiming(value.to_string()));
    }
    let start = parse_u64("timing.start", parts[0])?;
    let stop = parse_u64("timing.stop", parts[1])?;
    Ok(SdpTiming { start, stop })
}

fn parse_attribute(value: &str) -> Result<SdpAttribute, SdpError> {
    if let Some((name, attr_value)) = value.split_once(':') {
        return match name {
            "rtpmap" => parse_rtpmap_attribute(value, attr_value),
            "fmtp" => parse_fmtp_attribute(value, attr_value),
            "control" => Ok(SdpAttribute::Control(attr_value.to_string())),
            "range" => Ok(SdpAttribute::Range(attr_value.to_string())),
            "framerate" => Ok(SdpAttribute::Framerate(parse_f64(
                "attribute.framerate",
                attr_value,
            )?)),
            "tool" => Ok(SdpAttribute::Tool(attr_value.to_string())),
            "type" => Ok(SdpAttribute::Type(attr_value.to_string())),
            "charset" => Ok(SdpAttribute::Charset(attr_value.to_string())),
            "sdplang" => Ok(SdpAttribute::Sdplang(attr_value.to_string())),
            "lang" => Ok(SdpAttribute::Lang(attr_value.to_string())),
            _ => Ok(SdpAttribute::Custom {
                name: name.to_string(),
                value: Some(attr_value.to_string()),
            }),
        };
    }

    Ok(match value {
        "recvonly" => SdpAttribute::Recvonly,
        "sendrecv" => SdpAttribute::Sendrecv,
        "sendonly" => SdpAttribute::Sendonly,
        "inactive" => SdpAttribute::Inactive,
        _ => SdpAttribute::Custom {
            name: value.to_string(),
            value: None,
        },
    })
}

fn parse_rtpmap_attribute(full_value: &str, attr_value: &str) -> Result<SdpAttribute, SdpError> {
    let parts: Vec<&str> = attr_value.splitn(2, ' ').collect();
    if parts.len() != 2 {
        return Err(SdpError::InvalidAttribute {
            attribute: "rtpmap",
            value: full_value.to_string(),
        });
    }

    let payload_type = parse_u8("attribute.rtpmap.payload_type", parts[0])?;
    let encoding_parts: Vec<&str> = parts[1].split('/').collect();
    if encoding_parts.len() < 2 {
        return Err(SdpError::InvalidAttribute {
            attribute: "rtpmap",
            value: full_value.to_string(),
        });
    }
    let clock_rate = parse_u32("attribute.rtpmap.clock_rate", encoding_parts[1])?;

    Ok(SdpAttribute::Rtpmap {
        payload_type,
        encoding: encoding_parts[0].to_string(),
        clock_rate,
        encoding_params: encoding_parts.get(2).map(|v| v.to_string()),
    })
}

fn parse_fmtp_attribute(full_value: &str, attr_value: &str) -> Result<SdpAttribute, SdpError> {
    let parts: Vec<&str> = attr_value.splitn(2, ' ').collect();
    if parts.len() != 2 {
        return Err(SdpError::InvalidAttribute {
            attribute: "fmtp",
            value: full_value.to_string(),
        });
    }
    let payload_type = parse_u8("attribute.fmtp.payload_type", parts[0])?;
    Ok(SdpAttribute::Fmtp {
        payload_type,
        parameters: parts[1].to_string(),
    })
}

fn parse_media(value: &str) -> Result<SdpMedia, SdpError> {
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() < 4 {
        return Err(SdpError::InvalidMedia(value.to_string()));
    }

    let (port, num_ports) = if let Some((port, num_ports)) = parts[1].split_once('/') {
        (
            parse_u16("media.port", port)?,
            Some(parse_u16("media.num_ports", num_ports)?),
        )
    } else {
        (parse_u16("media.port", parts[1])?, None)
    };

    Ok(SdpMedia {
        media_type: parts[0].to_string(),
        port,
        num_ports,
        protocol: parts[2].to_string(),
        formats: parts[3..].iter().map(ToString::to_string).collect(),
        title: None,
        connection: None,
        bandwidth: Vec::new(),
        attributes: Vec::new(),
    })
}

fn parse_u8(field: &'static str, value: &str) -> Result<u8, SdpError> {
    value.parse::<u8>().map_err(|_| SdpError::InvalidNumber {
        field,
        value: value.to_string(),
    })
}

fn parse_u16(field: &'static str, value: &str) -> Result<u16, SdpError> {
    value.parse::<u16>().map_err(|_| SdpError::InvalidNumber {
        field,
        value: value.to_string(),
    })
}

fn parse_u32(field: &'static str, value: &str) -> Result<u32, SdpError> {
    value.parse::<u32>().map_err(|_| SdpError::InvalidNumber {
        field,
        value: value.to_string(),
    })
}

fn parse_u64(field: &'static str, value: &str) -> Result<u64, SdpError> {
    value.parse::<u64>().map_err(|_| SdpError::InvalidNumber {
        field,
        value: value.to_string(),
    })
}

fn parse_f64(field: &'static str, value: &str) -> Result<f64, SdpError> {
    value.parse::<f64>().map_err(|_| SdpError::InvalidNumber {
        field,
        value: value.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{Sdp, SdpBuilder, SdpError, SdpMediaBuilder};

    #[test]
    fn test_parse_sdp() {
        let sdp_text = r#"v=0
o=- 2890844526 2890842807 IN IP4 192.168.1.1
s=Example
t=0 0
m=video 49170 RTP/AVP 96
a=rtpmap:96 H264/90000
a=control:trackID=1
m=audio 49180 RTP/AVP 97
a=rtpmap:97 MPEG4-GENERIC/44100/2
a=control:trackID=2
"#;

        let sdp = Sdp::parse(sdp_text).expect("parse sdp");
        assert_eq!(sdp.version, 0);
        assert_eq!(sdp.session_name, "Example");
        assert_eq!(sdp.media.len(), 2);
        assert_eq!(sdp.media[0].media_type, "video");
        assert_eq!(sdp.media[1].media_type, "audio");
    }

    #[test]
    fn parse_requires_origin() {
        let text = "v=0\r\ns=Missing Origin\r\nt=0 0\r\n";
        let err = Sdp::parse(text).expect_err("missing origin should fail");
        assert_eq!(err, SdpError::MissingRequiredField { field: "o" });
    }

    #[test]
    fn parse_rejects_invalid_rtpmap() {
        let text = "v=0\r\n\
            o=- 1 1 IN IP4 127.0.0.1\r\n\
            s=InvalidRtpmap\r\n\
            t=0 0\r\n\
            m=video 0 RTP/AVP 96\r\n\
            a=rtpmap:96\r\n";
        let err = Sdp::parse(text).expect_err("invalid rtpmap should fail");
        assert_eq!(
            err,
            SdpError::InvalidAttribute {
                attribute: "rtpmap",
                value: "rtpmap:96".to_string(),
            }
        );
    }

    #[test]
    fn test_build_sdp() {
        let sdp = Sdp::builder()
            .origin_simple("1234567890", "127.0.0.1")
            .session_name("Test Session")
            .timing(0, 0)
            .control("*")
            .add_media(
                SdpMediaBuilder::video(0)
                    .format("96")
                    .rtpmap(96, "H264", 90000)
                    .control("trackID=1")
                    .build(),
            )
            .build()
            .expect("build sdp");

        let text = sdp.to_string();
        assert!(text.contains("v=0"));
        assert!(text.contains("s=Test Session"));
        assert!(text.contains("m=video 0 RTP/AVP 96"));
    }

    #[test]
    fn builder_requires_required_fields() {
        let missing_origin = SdpBuilder::new()
            .session_name("no origin")
            .timing(0, 0)
            .build()
            .expect_err("build without origin should fail");
        assert_eq!(
            missing_origin,
            SdpError::MissingRequiredField { field: "o" }
        );

        let missing_session_name = SdpBuilder::new()
            .origin_simple("1", "127.0.0.1")
            .timing(0, 0)
            .build()
            .expect_err("build without session name should fail");
        assert_eq!(
            missing_session_name,
            SdpError::MissingRequiredField { field: "s" }
        );

        let missing_timing = SdpBuilder::new()
            .origin_simple("1", "127.0.0.1")
            .session_name("missing timing")
            .build()
            .expect_err("build without timing should fail");
        assert_eq!(
            missing_timing,
            SdpError::MissingRequiredField { field: "t" }
        );
    }

    #[test]
    fn parse_rejects_invalid_media_num_ports() {
        let text = "v=0\r\n\
            o=- 1 1 IN IP4 127.0.0.1\r\n\
            s=InvalidMediaNumPorts\r\n\
            t=0 0\r\n\
            m=video 49170/x RTP/AVP 96\r\n";
        let err = Sdp::parse(text).expect_err("invalid media num_ports should fail");
        assert_eq!(
            err,
            SdpError::InvalidNumber {
                field: "media.num_ports",
                value: "x".to_string(),
            }
        );
    }
}
