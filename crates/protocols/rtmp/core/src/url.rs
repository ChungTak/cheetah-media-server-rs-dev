use alloc::borrow::ToOwned;
use alloc::string::String;

use crate::error::Error;

/// RTMP URL components parsed from a connection string.
/// 从连接字符串解析出的 RTMP URL 组件。
///
/// # NOTE
///
/// [`core::str::FromStr`] 的实现使用了 [`RtmpUrl::parse()`]。
/// 如果需要将流名称与 URL 字符串分开指定，请使用 [`RtmpUrl::parse_with_stream_name()`]。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RtmpUrl {
    /// RTMP server hostname or IP address.
    /// RTMP 服务器的主机名或 IP 地址。
    pub host: String,

    /// RTMP server port.
    /// RTMP 服务器的端口号。
    pub port: u16,

    /// RTMP application name.
    /// RTMP 应用名。
    pub app: String,

    /// RTMP stream name.
    /// 流名称。
    pub stream_name: String,

    /// Whether TLS is used (true for `rtmps`).
    /// 是否使用 TLS 连接（rtmps 时为 true）。
    pub tls: bool,
}

impl RtmpUrl {
    /// Parses a full RTMP URL including the stream name: `rtmp[s]://host[:port]/app/stream_name`.
    /// 解析包含流名称的 RTMP URL：`rtmp[s]://host[:port]/app/stream_name`。
    ///
    /// 当路径部分包含多个 `/` 时，以最后一个 `/` 分割应用名和流名称。
    ///
    /// 当端口被省略时，使用默认端口：
    /// - rtmp: 1935
    /// - rtmps: 443
    pub fn parse(s: &str) -> Result<Self, Error> {
        let (tls, host, port, path) = Self::parse_scheme_and_host_port(s)?;

        let (app, stream_name) = path
            .rsplit_once('/')
            .ok_or_else(|| Error::invalid_input("missing app and/or stream_name in path"))?;
        if app.is_empty() {
            return Err(Error::invalid_input("app name cannot be empty"));
        }
        if stream_name.is_empty() {
            return Err(Error::invalid_input("stream name cannot be empty"));
        }

        Ok(RtmpUrl {
            host: host.to_owned(),
            port,
            app: app.to_owned(),
            stream_name: stream_name.to_owned(),
            tls,
        })
    }

    /// Parses an RTMP URL with a separate stream name: `rtmp[s]://host[:port]/app`.
    /// 单独指定流名称来解析 RTMP URL：`rtmp[s]://host[:port]/app`。
    ///
    /// 当端口被省略时，使用默认端口：
    /// - rtmp: 1935
    /// - rtmps: 443
    pub fn parse_with_stream_name(s: &str, stream_name: &str) -> Result<Self, Error> {
        let (tls, host, port, app) = Self::parse_scheme_and_host_port(s)?;

        if app.is_empty() {
            return Err(Error::invalid_input("app name cannot be empty"));
        }
        if stream_name.is_empty() {
            return Err(Error::invalid_input("stream name cannot be empty"));
        }

        Ok(RtmpUrl {
            host: host.to_owned(),
            port,
            app: app.to_owned(),
            stream_name: stream_name.to_owned(),
            tls,
        })
    }

    fn parse_scheme_and_host_port(s: &str) -> Result<(bool, &str, u16, &str), Error> {
        // scheme://
        let (scheme, rest) = s
            .split_once("://")
            .ok_or_else(|| Error::invalid_input("missing '://' in RTMP URL"))?;
        let tls = match scheme {
            "rtmp" => false,
            "rtmps" => true,
            _ => {
                return Err(Error::invalid_input(format!(
                    "invalid scheme '{scheme}', expected 'rtmp' or 'rtmps'"
                )));
            }
        };

        // host[:port]/path
        let (host_port, path) = rest
            .split_once('/')
            .ok_or_else(|| Error::invalid_input("missing '/' separator for app name"))?;

        // host, port
        let (host, port) = parse_host_port(host_port, tls)?;

        if host.is_empty() {
            return Err(Error::invalid_input("host cannot be empty"));
        }

        Ok((tls, host, port, path))
    }
}

impl core::fmt::Display for RtmpUrl {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let scheme = if self.tls { "rtmps" } else { "rtmp" };
        write!(
            f,
            "{}://{}:{}/{}/{}",
            scheme, self.host, self.port, self.app, self.stream_name
        )
    }
}

impl core::str::FromStr for RtmpUrl {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

fn parse_host_port(host_port: &str, tls: bool) -> Result<(&str, u16), Error> {
    // 检查是否包含端口号
    // IPv6 地址的情况下，端口在 ] 后面
    let (host, port_str) = if host_port.starts_with('[') {
        // IPv6 地址的情况: [::1]:1935
        if let Some(bracket_end) = host_port.find(']') {
            let host = &host_port[..=bracket_end];
            let remainder = &host_port[bracket_end + 1..];

            if remainder.is_empty() {
                // 无端口号
                (host, None)
            } else if let Some(remainder) = remainder.strip_prefix(':') {
                // 有端口号
                (host, Some(remainder))
            } else {
                return Err(Error::invalid_input(
                    "invalid format after IPv6 address, expected ':' before port",
                ));
            }
        } else {
            return Err(Error::invalid_input(
                "invalid IPv6 address format, missing ']'",
            ));
        }
    } else {
        // IPv4 或主机名（带方括号的 IPv6 已在上面的分支中处理）
        let colon_count = host_port.chars().filter(|&c| c == ':').count();
        if colon_count > 1 {
            return Err(Error::invalid_input(
                "bare IPv6 address must be enclosed in brackets (e.g. '[::1]')",
            ));
        }
        if let Some(colon_pos) = host_port.rfind(':') {
            let potential_port = &host_port[colon_pos + 1..];
            if potential_port.chars().all(|c| c.is_ascii_digit()) {
                (&host_port[..colon_pos], Some(potential_port))
            } else {
                return Err(Error::invalid_input("invalid host:port format"));
            }
        } else {
            (host_port, None)
        }
    };

    // 解析端口号
    let port = match port_str {
        Some(port_s) => port_s
            .parse::<u16>()
            .map_err(|e| Error::invalid_input(format!("invalid port number '{port_s}': {e}")))?,
        None => {
            if tls {
                443
            } else {
                1935
            }
        }
    };

    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::string::ToString;
    use core::str::FromStr;

    #[test]
    fn test_basic_rtmp_url() {
        let url = RtmpUrl::from_str("rtmp://example.com:1935/live/stream").unwrap();
        assert_eq!(url.host, "example.com");
        assert_eq!(url.port, 1935);
        assert_eq!(url.app, "live");
        assert_eq!(url.stream_name, "stream");
        assert!(!url.tls);
    }

    #[test]
    fn test_rtmps_url() {
        let url = RtmpUrl::from_str("rtmps://example.com:443/live/stream").unwrap();
        assert_eq!(url.host, "example.com");
        assert_eq!(url.port, 443);
        assert_eq!(url.app, "live");
        assert_eq!(url.stream_name, "stream");
        assert!(url.tls);
    }

    #[test]
    fn test_default_port_rtmp() {
        let url = RtmpUrl::from_str("rtmp://example.com/live/stream").unwrap();
        assert_eq!(url.port, 1935);
    }

    #[test]
    fn test_default_port_rtmps() {
        let url = RtmpUrl::from_str("rtmps://example.com/live/stream").unwrap();
        assert_eq!(url.port, 443);
    }

    #[test]
    fn test_nested_app_path() {
        let url = RtmpUrl::from_str("rtmp://example.com/app/path/stream").unwrap();
        assert_eq!(url.app, "app/path");
        assert_eq!(url.stream_name, "stream");
    }

    #[test]
    fn test_display() {
        let url = RtmpUrl {
            host: "example.com".to_owned(),
            port: 1935,
            app: "live".to_owned(),
            stream_name: "stream".to_owned(),
            tls: false,
        };
        assert_eq!(url.to_string(), "rtmp://example.com:1935/live/stream");
    }

    #[test]
    fn test_display_rtmps() {
        let url = RtmpUrl {
            host: "example.com".to_owned(),
            port: 443,
            app: "live".to_owned(),
            stream_name: "stream".to_owned(),
            tls: true,
        };
        assert_eq!(url.to_string(), "rtmps://example.com:443/live/stream");
    }

    #[test]
    fn test_round_trip() {
        let original = "rtmp://example.com:1935/live/stream";
        let url = RtmpUrl::from_str(original).unwrap();
        assert_eq!(url.to_string(), original);
    }

    #[test]
    fn test_invalid_scheme() {
        let result = RtmpUrl::from_str("http://example.com/live/stream");
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_scheme_separator() {
        let result = RtmpUrl::from_str("rtmp example.com/live/stream");
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_path_separator() {
        let result = RtmpUrl::from_str("rtmp://example.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_host() {
        let result = RtmpUrl::from_str("rtmp://:1935/live/stream");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_app() {
        let result = RtmpUrl::from_str("rtmp://example.com//stream");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_stream_name() {
        let result = RtmpUrl::from_str("rtmp://example.com/live/");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_port() {
        let result = RtmpUrl::from_str("rtmp://example.com:invalid/live/stream");
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_stream_name() {
        let result = RtmpUrl::from_str("rtmp://example.com/live");
        assert!(result.is_err());
    }

    #[test]
    fn test_ipv4_address() {
        let url = RtmpUrl::from_str("rtmp://192.168.1.1:1935/live/stream").unwrap();
        assert_eq!(url.host, "192.168.1.1");
    }

    #[test]
    fn test_clone_and_equality() {
        let url1 = RtmpUrl::from_str("rtmp://example.com/live/stream").unwrap();
        let url2 = url1.clone();
        assert_eq!(url1, url2);
    }

    // IPv6 支持测试用例
    #[test]
    fn test_ipv6_address_with_port() {
        let url = RtmpUrl::from_str("rtmp://[::1]:1935/live/stream").unwrap();
        assert_eq!(url.host, "[::1]");
        assert_eq!(url.port, 1935);
        assert_eq!(url.app, "live");
        assert_eq!(url.stream_name, "stream");
    }

    #[test]
    fn test_ipv6_address_full() {
        let url = RtmpUrl::from_str("rtmp://[2001:db8::1]:1935/live/stream").unwrap();
        assert_eq!(url.host, "[2001:db8::1]");
        assert_eq!(url.port, 1935);
    }

    #[test]
    fn test_ipv6_address_default_port_rtmp() {
        let url = RtmpUrl::from_str("rtmp://[::1]/live/stream").unwrap();
        assert_eq!(url.host, "[::1]");
        assert_eq!(url.port, 1935);
    }

    #[test]
    fn test_ipv6_address_default_port_rtmps() {
        let url = RtmpUrl::from_str("rtmps://[::1]/live/stream").unwrap();
        assert_eq!(url.host, "[::1]");
        assert_eq!(url.port, 443);
    }

    #[test]
    fn test_ipv6_display() {
        let url = RtmpUrl {
            host: "[::1]".to_owned(),
            port: 1935,
            app: "live".to_owned(),
            stream_name: "stream".to_owned(),
            tls: false,
        };
        assert_eq!(url.to_string(), "rtmp://[::1]:1935/live/stream");
    }

    #[test]
    fn test_ipv6_display_rtmps() {
        let url = RtmpUrl {
            host: "[2001:db8::1]".to_owned(),
            port: 443,
            app: "app".to_owned(),
            stream_name: "stream".to_owned(),
            tls: true,
        };
        assert_eq!(url.to_string(), "rtmps://[2001:db8::1]:443/app/stream");
    }

    #[test]
    fn test_ipv6_round_trip() {
        let original = "rtmp://[::1]:1935/live/stream";
        let url = RtmpUrl::from_str(original).unwrap();
        assert_eq!(url.to_string(), original);
    }

    #[test]
    fn test_invalid_ipv6_address_in_brackets_still_parsed() {
        // RtmpUrl 不会验证 IPv6 地址本身的有效性，所以会成功
        let result = RtmpUrl::from_str("rtmp://[::gggg]:1935/live/stream");
        assert!(result.is_ok());
    }

    #[test]
    fn test_bare_ipv6_address_rejected() {
        // 不带方括号的 IPv6 地址会明确报错
        let result = RtmpUrl::from_str("rtmp://::1/live/stream");
        assert!(result.is_err());
    }

    #[test]
    fn test_ipv6_missing_closing_bracket() {
        let result = RtmpUrl::from_str("rtmp://[::1:1935/live/stream");
        assert!(result.is_err());
    }
}
