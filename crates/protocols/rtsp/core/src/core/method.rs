use std::fmt;
use std::str::FromStr;

/// `RtspMethod` enumeration.
/// `RtspMethod` 枚举.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RtspMethod {
    /// `Get` variant.
    /// `Get` 变体.
    Get,
    /// `Post` variant.
    /// `Post` 变体.
    Post,
    /// `Options` variant.
    /// `Options` 变体.
    Options,
    /// `Describe` variant.
    /// `Describe` 变体.
    Describe,
    /// `Announce` variant.
    /// `Announce` 变体.
    Announce,
    /// `Setup` variant.
    /// `Setup` 变体.
    Setup,
    /// `Play` variant.
    /// `Play` 变体.
    Play,
    /// `Pause` variant.
    /// `Pause` 变体.
    Pause,
    /// `Teardown` variant.
    /// `Teardown` 变体.
    Teardown,
    /// `GetParameter` variant.
    /// `GetParameter` 变体.
    GetParameter,
    /// `SetParameter` variant.
    /// `SetParameter` 变体.
    SetParameter,
    /// `Redirect` variant.
    /// `Redirect` 变体.
    Redirect,
    /// `Record` variant.
    /// `Record` 变体.
    Record,
    /// `Extension` variant.
    /// `Extension` 变体.
    Extension(String),
}

impl RtspMethod {
    /// `as_str` function.
    /// `as_str` 函数.
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

    /// `parse` function.
    /// `parse` 函数.
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
