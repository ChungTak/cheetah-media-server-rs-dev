use std::fmt;
use std::str::FromStr;

/// RTSP request method (RFC 2326 §10).
///
/// Standard methods are matched case-sensitively; any unknown method is
/// preserved as an extension variant so the stack can still forward or log it.
///
/// RTSP 请求方法（RFC 2326 §10）。
///
/// 标准方法按大小写敏感匹配；未知方法作为扩展变体保留，以便协议栈转发或记录。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RtspMethod {
    Get,
    Post,
    Options,
    Describe,
    Announce,
    Setup,
    Play,
    Pause,
    Teardown,
    GetParameter,
    SetParameter,
    Redirect,
    Record,
    Extension(String),
}

impl RtspMethod {
    /// Return the canonical wire representation of the method.
    ///
    /// `Extension` values are returned as-is, preserving the original case.
    ///
    /// 返回该方法的标准线表示。
    ///
    /// `Extension` 值按原样返回，保留原始大小写。
    pub fn as_str(&self) -> &str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Options => "OPTIONS",
            Self::Describe => "DESCRIBE",
            Self::Announce => "ANNOUNCE",
            Self::Setup => "SETUP",
            Self::Play => "PLAY",
            Self::Pause => "PAUSE",
            Self::Teardown => "TEARDOWN",
            Self::GetParameter => "GET_PARAMETER",
            Self::SetParameter => "SET_PARAMETER",
            Self::Redirect => "REDIRECT",
            Self::Record => "RECORD",
            Self::Extension(value) => value.as_str(),
        }
    }

    /// Parse a method string from an RTSP request line.
    ///
    /// Unknown methods are stored as `Extension` instead of rejected, which keeps
    /// the parser tolerant of custom methods while still routing known ones.
    ///
    /// 从 RTSP 请求行解析方法字符串。
    ///
    /// 未知方法存为 `Extension` 而非拒绝，使解析器对自定义方法保持容忍，同时仍能路由已知方法。
    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "GET" => Self::Get,
            "POST" => Self::Post,
            "OPTIONS" => Self::Options,
            "DESCRIBE" => Self::Describe,
            "ANNOUNCE" => Self::Announce,
            "SETUP" => Self::Setup,
            "PLAY" => Self::Play,
            "PAUSE" => Self::Pause,
            "TEARDOWN" => Self::Teardown,
            "GET_PARAMETER" => Self::GetParameter,
            "SET_PARAMETER" => Self::SetParameter,
            "REDIRECT" => Self::Redirect,
            "RECORD" => Self::Record,
            _ => Self::Extension(value.to_string()),
        }
    }
}

impl fmt::Display for RtspMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for RtspMethod {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(value))
    }
}

#[cfg(test)]
mod tests {
    use super::RtspMethod;
    use std::str::FromStr;

    #[test]
    fn standard_methods_are_case_sensitive() {
        let upper = RtspMethod::from_str("PLAY").expect("infallible parse");
        assert_eq!(upper, RtspMethod::Play);

        let lower = RtspMethod::from_str("play").expect("infallible parse");
        assert_eq!(lower, RtspMethod::Extension("play".to_string()));
    }

    #[test]
    fn parses_get_and_post_as_well_known_methods() {
        let get = RtspMethod::from_str("GET").expect("infallible parse");
        assert_eq!(get, RtspMethod::Get);

        let post = RtspMethod::from_str("POST").expect("infallible parse");
        assert_eq!(post, RtspMethod::Post);
    }
}
