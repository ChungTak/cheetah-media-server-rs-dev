use std::fmt;

/// Errors that can occur while parsing or building SDP session descriptions.
///
/// SDP 会话描述解析或构造错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SdpError {
    #[error("missing required SDP field `{field}`")]
    MissingRequiredField { field: &'static str },
    #[error("invalid SDP origin field: {0}")]
    InvalidOrigin(String),
    #[error("invalid SDP connection field: {0}")]
    InvalidConnection(String),
    #[error("invalid SDP bandwidth field: {0}")]
    InvalidBandwidth(String),
    #[error("invalid SDP timing field: {0}")]
    InvalidTiming(String),
    #[error("invalid SDP media field: {0}")]
    InvalidMedia(String),
    #[error("invalid SDP attribute `{attribute}`: {value}")]
    InvalidAttribute {
        attribute: &'static str,
        value: String,
    },
    #[error("invalid numeric value for `{field}`: {value}")]
    InvalidNumber { field: &'static str, value: String },
}

/// SDP session description (RFC 8866).
///
/// SDP 会话描述（RFC 8866）。
#[derive(Debug, Clone, PartialEq)]
pub struct Sdp {
    pub version: u8,
    pub origin: SdpOrigin,
    pub session_name: String,
    pub session_info: Option<String>,
    pub uri: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub connection: Option<SdpConnection>,
    pub bandwidth: Vec<SdpBandwidth>,
    pub timing: SdpTiming,
    pub attributes: Vec<SdpAttribute>,
    pub media: Vec<SdpMedia>,
}

/// SDP `o=` origin field.
///
/// SDP `o=` origin 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpOrigin {
    pub username: String,
    pub session_id: String,
    pub session_version: String,
    pub net_type: String,
    pub addr_type: String,
    pub address: String,
}

/// SDP `c=` connection field.
///
/// SDP `c=` connection 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpConnection {
    pub net_type: String,
    pub addr_type: String,
    pub address: String,
}

/// SDP `b=` bandwidth field.
///
/// SDP `b=` bandwidth 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpBandwidth {
    pub bwtype: String,
    pub bandwidth: u64,
}

/// SDP `t=` timing field.
///
/// SDP `t=` timing 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpTiming {
    pub start: u64,
    pub stop: u64,
}

/// SDP `m=` media description.
///
/// SDP `m=` 媒体描述。
#[derive(Debug, Clone, PartialEq)]
pub struct SdpMedia {
    pub media_type: String,
    pub port: u16,
    pub num_ports: Option<u16>,
    pub protocol: String,
    pub formats: Vec<String>,
    pub title: Option<String>,
    pub connection: Option<SdpConnection>,
    pub bandwidth: Vec<SdpBandwidth>,
    pub attributes: Vec<SdpAttribute>,
}

/// SDP `a=` attribute field.
///
/// SDP `a=` 属性字段。
#[derive(Debug, Clone, PartialEq)]
pub enum SdpAttribute {
    Rtpmap {
        payload_type: u8,
        encoding: String,
        clock_rate: u32,
        encoding_params: Option<String>,
    },
    Fmtp {
        payload_type: u8,
        parameters: String,
    },
    Control(String),
    Range(String),
    Recvonly,
    Sendrecv,
    Sendonly,
    Inactive,
    Framerate(f64),
    Tool(String),
    Type(String),
    Charset(String),
    Sdplang(String),
    Lang(String),
    Custom {
        name: String,
        value: Option<String>,
    },
}

impl Sdp {
    /// Parse an SDP session description from a text document.
    ///
    /// Walks the line-oriented `key=value` format, dispatching by SDP field type and
    /// keeping track of whether the current line belongs to session or media scope.
    ///
    /// 从文本文档解析 SDP 会话描述。
    ///
    /// 遍历行格式的 `key=value`，按 SDP 字段类型分派，并跟踪当前行属于会话还是媒体作用域。
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

    /// Return a new `SdpBuilder` to construct an SDP description fluently.
    ///
    /// 返回新的 `SdpBuilder`，用于流畅构造 SDP 描述。
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

/// Format an `SdpAttribute` as an `a=` line value.
///
/// 将 `SdpAttribute` 格式化为 `a=` 行值。
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

/// Builder for constructing `Sdp` values step by step.
///
/// 用于逐步构造 `Sdp` 的构建器。
#[derive(Debug, Clone)]
pub struct SdpBuilder {
    version: u8,
    origin: Option<SdpOrigin>,
    session_name: Option<String>,
    session_info: Option<String>,
    uri: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    connection: Option<SdpConnection>,
    bandwidth: Vec<SdpBandwidth>,
    timing: Option<SdpTiming>,
    attributes: Vec<SdpAttribute>,
    media: Vec<SdpMedia>,
}

impl Default for SdpBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SdpBuilder {
    /// Create a new builder with empty/default values.
    ///
    /// 以空/默认值创建新的构建器。
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

    /// Set the SDP version (`v=`).
    ///
    /// 设置 SDP 版本（`v=`）。
    pub fn version(mut self, version: u8) -> Self {
        self.version = version;
        self
    }

    /// Set the origin field (`o=`).
    ///
    /// 设置 origin 字段（`o=`）。
    pub fn origin(mut self, origin: SdpOrigin) -> Self {
        self.origin = Some(origin);
        self
    }

    /// Set a simple origin with default user, version, and `IN IP4` network type.
    ///
    /// 使用默认用户名、版本和 `IN IP4` 网络类型设置简化 origin。
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

    /// Set the session name (`s=`).
    ///
    /// 设置会话名（`s=`）。
    pub fn session_name(mut self, name: &str) -> Self {
        self.session_name = Some(name.to_string());
        self
    }

    /// Set the optional session information (`i=`).
    ///
    /// 设置可选会话信息（`i=`）。
    pub fn session_info(mut self, info: &str) -> Self {
        self.session_info = Some(info.to_string());
        self
    }

    /// Set the optional URI (`u=`).
    ///
    /// 设置可选 URI（`u=`）。
    pub fn uri(mut self, uri: &str) -> Self {
        self.uri = Some(uri.to_string());
        self
    }

    /// Set the optional contact email (`e=`).
    ///
    /// 设置可选联系邮箱（`e=`）。
    pub fn email(mut self, email: &str) -> Self {
        self.email = Some(email.to_string());
        self
    }

    /// Set the optional contact phone (`p=`).
    ///
    /// 设置可选联系电话（`p=`）。
    pub fn phone(mut self, phone: &str) -> Self {
        self.phone = Some(phone.to_string());
        self
    }

    /// Set the connection field (`c=`).
    ///
    /// 设置 connection 字段（`c=`）。
    pub fn connection(mut self, connection: SdpConnection) -> Self {
        self.connection = Some(connection);
        self
    }

    /// Set a simple `IN IP4` connection address.
    ///
    /// 设置简化的 `IN IP4` 连接地址。
    pub fn connection_simple(mut self, address: &str) -> Self {
        self.connection = Some(SdpConnection {
            net_type: "IN".to_string(),
            addr_type: "IP4".to_string(),
            address: address.to_string(),
        });
        self
    }

    /// Add a bandwidth field (`b=`).
    ///
    /// 添加 bandwidth 字段（`b=`）。
    pub fn bandwidth(mut self, bwtype: &str, bandwidth: u64) -> Self {
        self.bandwidth.push(SdpBandwidth {
            bwtype: bwtype.to_string(),
            bandwidth,
        });
        self
    }

    /// Set the timing field (`t=`).
    ///
    /// 设置 timing 字段（`t=`）。
    pub fn timing(mut self, start: u64, stop: u64) -> Self {
        self.timing = Some(SdpTiming { start, stop });
        self
    }

    /// Add a generic attribute (`a=`).
    ///
    /// 添加通用属性（`a=`）。
    pub fn attribute(mut self, attr: SdpAttribute) -> Self {
        self.attributes.push(attr);
        self
    }

    /// Add a `control` attribute.
    ///
    /// 添加 `control` 属性。
    pub fn control(self, url: &str) -> Self {
        self.attribute(SdpAttribute::Control(url.to_string()))
    }

    /// Add a `range` attribute.
    ///
    /// 添加 `range` 属性。
    pub fn range(self, range: &str) -> Self {
        self.attribute(SdpAttribute::Range(range.to_string()))
    }

    /// Add a media description to the session.
    ///
    /// 向会话添加媒体描述。
    pub fn add_media(mut self, media: SdpMedia) -> Self {
        self.media.push(media);
        self
    }

    /// Build the `Sdp` value, requiring origin, session name, and timing.
    ///
    /// 构建 `Sdp` 值，要求 origin、session name 和 timing 已设置。
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

/// Builder for constructing `SdpMedia` values step by step.
///
/// 用于逐步构造 `SdpMedia` 的构建器。
#[derive(Debug, Clone)]
pub struct SdpMediaBuilder {
    media_type: String,
    port: u16,
    num_ports: Option<u16>,
    protocol: String,
    formats: Vec<String>,
    title: Option<String>,
    connection: Option<SdpConnection>,
    bandwidth: Vec<SdpBandwidth>,
    attributes: Vec<SdpAttribute>,
}

impl SdpMediaBuilder {
    /// Create a media builder for the given type, port, and protocol.
    ///
    /// 为给定类型、端口和协议创建媒体构建器。
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

    /// Convenience constructor for an RTP/AVP video media builder.
    ///
    /// 创建 RTP/AVP 视频媒体构建器的便捷构造器。
    pub fn video(port: u16) -> Self {
        Self::new("video", port, "RTP/AVP")
    }

    /// Convenience constructor for an RTP/AVP audio media builder.
    ///
    /// 创建 RTP/AVP 音频媒体构建器的便捷构造器。
    pub fn audio(port: u16) -> Self {
        Self::new("audio", port, "RTP/AVP")
    }

    /// Set the optional number of ports for the media line.
    ///
    /// 设置媒体行可选端口数。
    pub fn num_ports(mut self, num_ports: u16) -> Self {
        self.num_ports = Some(num_ports);
        self
    }

    /// Add a format/payload type to the media line.
    ///
    /// 向媒体行添加格式/负载类型。
    pub fn format(mut self, format: &str) -> Self {
        self.formats.push(format.to_string());
        self
    }

    /// Set the optional media title (`i=`).
    ///
    /// 设置可选媒体标题（`i=`）。
    pub fn title(mut self, title: &str) -> Self {
        self.title = Some(title.to_string());
        self
    }

    /// Set the connection field (`c=`).
    ///
    /// 设置 connection 字段（`c=`）。
    pub fn connection(mut self, connection: SdpConnection) -> Self {
        self.connection = Some(connection);
        self
    }

    /// Add a bandwidth field (`b=`).
    ///
    /// 添加 bandwidth 字段（`b=`）。
    pub fn bandwidth(mut self, bwtype: &str, bandwidth: u64) -> Self {
        self.bandwidth.push(SdpBandwidth {
            bwtype: bwtype.to_string(),
            bandwidth,
        });
        self
    }

    /// Add a generic attribute (`a=`).
    ///
    /// 添加通用属性（`a=`）。
    pub fn attribute(mut self, attr: SdpAttribute) -> Self {
        self.attributes.push(attr);
        self
    }

    /// Add an `rtpmap` attribute for the given payload type.
    ///
    /// 为给定负载类型添加 `rtpmap` 属性。
    pub fn rtpmap(self, payload_type: u8, encoding: &str, clock_rate: u32) -> Self {
        self.attribute(SdpAttribute::Rtpmap {
            payload_type,
            encoding: encoding.to_string(),
            clock_rate,
            encoding_params: None,
        })
    }

    /// Add an `fmtp` attribute for the given payload type.
    ///
    /// 为给定负载类型添加 `fmtp` 属性。
    pub fn fmtp(self, payload_type: u8, parameters: &str) -> Self {
        self.attribute(SdpAttribute::Fmtp {
            payload_type,
            parameters: parameters.to_string(),
        })
    }

    /// Add a `control` attribute.
    ///
    /// 添加 `control` 属性。
    pub fn control(self, url: &str) -> Self {
        self.attribute(SdpAttribute::Control(url.to_string()))
    }

    /// Add a `range` attribute.
    ///
    /// 添加 `range` 属性。
    pub fn range(self, range: &str) -> Self {
        self.attribute(SdpAttribute::Range(range.to_string()))
    }

    /// Build the `SdpMedia` value.
    ///
    /// 构建 `SdpMedia` 值。
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

/// Parse the `o=` origin field from its six whitespace-separated tokens.
///
/// 从六个空白分隔的 token 解析 `o=` origin 字段。
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

/// Parse the `c=` connection field from its three whitespace-separated tokens.
///
/// 从三个空白分隔的 token 解析 `c=` connection 字段。
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

/// Parse the `b=` bandwidth field as `bwtype:bandwidth`.
///
/// 以 `bwtype:bandwidth` 格式解析 `b=` bandwidth 字段。
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

/// Parse the `t=` timing field from its two whitespace-separated time values.
///
/// 从两个空白分隔的时间值解析 `t=` timing 字段。
fn parse_timing(value: &str) -> Result<SdpTiming, SdpError> {
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() != 2 {
        return Err(SdpError::InvalidTiming(value.to_string()));
    }
    let start = parse_u64("timing.start", parts[0])?;
    let stop = parse_u64("timing.stop", parts[1])?;
    Ok(SdpTiming { start, stop })
}

/// Parse an `a=` attribute, dispatching by attribute name and optional value.
///
/// 解析 `a=` 属性，按属性名和可选值分派。
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

/// Parse an `rtpmap:` attribute as `payload_type encoding/clock_rate[/params]`.
///
/// 以 `payload_type encoding/clock_rate[/params]` 格式解析 `rtpmap:` 属性。
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

/// Parse an `fmtp:` attribute as `payload_type parameters`.
///
/// 以 `payload_type parameters` 格式解析 `fmtp:` 属性。
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

/// Parse an `m=` media line, including optional `port/num_ports` syntax.
///
/// 解析 `m=` 媒体行，包括可选的 `port/num_ports` 语法。
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
