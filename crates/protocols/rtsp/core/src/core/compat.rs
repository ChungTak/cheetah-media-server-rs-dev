//! Compatibility layer for non-standard RTSP implementations.
//!
//! All quirk handling is centralized here with explicit naming. Each function is
//! independently toggleable via configuration so vendor-specific behavior is not
//! scattered through the protocol state machine.
//!
//! 非标准 RTSP 实现的兼容层。
//!
//! 所有厂商特殊处理都集中在这里，并通过显式命名管理。每个函数可独立通过配置开启，
//! 避免厂商特定行为分散在协议状态机中。

/// Strip the `.sdp` suffix from RTSP URLs (EasyDarwin compatibility).
///
/// Some clients append `.sdp` to the stream URL; this normalizer removes it so
/// the stream key lookup is stable.
///
/// 从 RTSP URL 中移除 `.sdp` 后缀（EasyDarwin 兼容性）。
///
/// 某些客户端会在流 URL 后追加 `.sdp`；该归一化器将其移除，使流键查找稳定。
pub fn strip_sdp_suffix(url: &str) -> &str {
    url.strip_suffix(".sdp")
        .or_else(|| url.strip_suffix(".SDP"))
        .unwrap_or(url)
}

/// Resolve a `control` URL from an SDP `a=control:` attribute against a base URL.
///
/// Handles absolute URLs, relative paths, slash-prefixed paths, and the aggregate
/// wildcard `*`, so the SETUP/PLAY URLs formed from SDP are unambiguous.
///
/// 将 SDP `a=control:` 属性中的 `control` URL 与 base URL 解析为完整 URL。
///
/// 处理绝对 URL、相对路径、以斜杠开头的路径以及聚合通配符 `*`，使由 SDP 形成的
/// SETUP/PLAY URL 无歧义。
pub fn resolve_control_url(base_url: &str, control: &str) -> String {
    let control = control.trim();
    if control == "*" || control.is_empty() {
        return base_url.to_string();
    }
    if control.starts_with("rtsp://") || control.starts_with("rtsps://") {
        return control.to_string();
    }
    let base = base_url.trim_end_matches('/');
    let relative = control.trim_start_matches('/');
    format!("{base}/{relative}")
}

/// Default clock rate for known codecs when SDP omits it.
///
/// This fallback table is used for SDP `rtpmap` parsing and for clients that do
/// not explicitly signal the clock rate; unknown codecs default to the 90 kHz
/// video convention.
///
/// 当 SDP 未声明时，已知编解码器的默认时钟频率。
///
/// 该回退表用于 SDP `rtpmap` 解析以及未显式声明时钟频率的客户端；未知编解码器默认
/// 采用 90 kHz 视频约定。
pub fn default_clock_rate(codec_name: &str) -> u32 {
    let upper = codec_name.to_ascii_uppercase();
    match upper.as_str() {
        "H264" | "H265" | "VP8" | "VP9" | "AV1" | "MP2T" | "JPEG" | "MP4V-ES" | "MPA" => 90000,
        "MPEG4-GENERIC" | "MP4A-LATM" => 44100,
        "OPUS" => 48000,
        "PCMA" | "PCMU" | "G711A" | "G711U" | "G722" | "G728" | "G729" | "TELEPHONE-EVENT" => 8000,
        "L16" => 44100,
        _ => 90000,
    }
}

/// Normalize `npt=now-` to `npt=0.000-` for live streams.
///
/// Many clients request the live edge with `now-`; the server treats this as an
/// open-ended range starting from zero.
///
/// 将直播流的 `npt=now-` 归一化为 `npt=0.000-`。
///
/// 许多客户端使用 `now-` 请求直播边缘；服务器将其视为从 0 开始的开放范围。
pub fn normalize_range_now(range: &str) -> &str {
    let trimmed = range.trim();
    if trimmed.eq_ignore_ascii_case("npt=now-") || trimmed.eq_ignore_ascii_case("npt = now-") {
        "npt=0.000-"
    } else {
        range
    }
}

/// Parse a `Location` header from a REDIRECT response or request.
///
/// Validates that the redirect target uses an `rtsp://` or `rtsps://` scheme.
///
/// 解析 REDIRECT 响应或请求中的 `Location` 头。
///
/// 校验重定向目标使用 `rtsp://` 或 `rtsps://` 协议。
pub fn parse_redirect_location(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
        .map(|(_, value)| value.as_str())
        .filter(|url| url.starts_with("rtsp://") || url.starts_with("rtsps://"))
}

/// Normalize a parsed `Transport` header for compatibility.
///
/// Defaults to unicast when neither unicast nor multicast is explicitly set, and
/// canonicalizes the protocol string case (`rtp/avp` -> `RTP/AVP`, etc.).
///
/// 对解析后的 `Transport` 头进行兼容性归一化。
///
/// 当未显式声明单播或多播时默认单播，并将协议字符串大小写规范化（`rtp/avp` -> `RTP/AVP` 等）。
pub fn normalize_transport(transport: &mut super::transport::RtspTransport) {
    // Default to unicast when not explicitly specified and protocol is RTP/AVP-based
    let proto_upper = transport.protocol.to_ascii_uppercase();
    if proto_upper.starts_with("RTP/AVP") && !proto_upper.contains("MULTICAST") {
        // If no explicit multicast destination/ttl, assume unicast
        if transport.destination.is_none() && transport.ttl.is_none() {
            transport.unicast = true;
        }
    }
    // Normalize protocol case
    match proto_upper.as_str() {
        "RTP/AVP" => transport.protocol = "RTP/AVP".to_string(),
        "RTP/AVP/TCP" => transport.protocol = "RTP/AVP/TCP".to_string(),
        "RTP/AVP/UDP" => transport.protocol = "RTP/AVP".to_string(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_sdp_suffix_removes_extension() {
        assert_eq!(
            strip_sdp_suffix("rtsp://host/live/test.sdp"),
            "rtsp://host/live/test"
        );
        assert_eq!(
            strip_sdp_suffix("rtsp://host/live/test.SDP"),
            "rtsp://host/live/test"
        );
        assert_eq!(
            strip_sdp_suffix("rtsp://host/live/test"),
            "rtsp://host/live/test"
        );
    }

    #[test]
    fn resolve_control_url_absolute() {
        let base = "rtsp://192.168.1.1/live/test";
        assert_eq!(
            resolve_control_url(base, "rtsp://192.168.1.1/live/test/trackID=0"),
            "rtsp://192.168.1.1/live/test/trackID=0"
        );
    }

    #[test]
    fn resolve_control_url_relative() {
        let base = "rtsp://192.168.1.1/live/test";
        assert_eq!(
            resolve_control_url(base, "trackID=0"),
            "rtsp://192.168.1.1/live/test/trackID=0"
        );
        assert_eq!(
            resolve_control_url(base, "/trackID=0"),
            "rtsp://192.168.1.1/live/test/trackID=0"
        );
    }

    #[test]
    fn resolve_control_url_aggregate() {
        let base = "rtsp://192.168.1.1/live/test";
        assert_eq!(resolve_control_url(base, "*"), base);
        assert_eq!(resolve_control_url(base, ""), base);
    }

    #[test]
    fn resolve_control_url_rtsps() {
        let base = "rtsps://host/path";
        assert_eq!(
            resolve_control_url(base, "rtsps://host/path/track1"),
            "rtsps://host/path/track1"
        );
    }

    #[test]
    fn default_clock_rate_known_codecs() {
        assert_eq!(default_clock_rate("H264"), 90000);
        assert_eq!(default_clock_rate("h265"), 90000);
        assert_eq!(default_clock_rate("PCMA"), 8000);
        assert_eq!(default_clock_rate("opus"), 48000);
        assert_eq!(default_clock_rate("MPEG4-GENERIC"), 44100);
        assert_eq!(default_clock_rate("unknown-codec"), 90000);
    }

    #[test]
    fn normalize_range_now_converts() {
        assert_eq!(normalize_range_now("npt=now-"), "npt=0.000-");
        assert_eq!(normalize_range_now("npt=0.000-"), "npt=0.000-");
        assert_eq!(normalize_range_now("npt=10.5-20.0"), "npt=10.5-20.0");
    }

    #[test]
    fn normalize_transport_defaults_unicast_and_normalizes_case() {
        use super::super::transport::RtspTransport;

        // Lowercase protocol → normalized
        let mut t = RtspTransport::parse("rtp/avp/tcp;interleaved=0-1").unwrap();
        normalize_transport(&mut t);
        assert!(t.unicast);
        assert_eq!(t.protocol, "RTP/AVP/TCP");

        // RTP/AVP/UDP → RTP/AVP
        let mut t2 = RtspTransport::parse("RTP/AVP/UDP;client_port=5000-5001").unwrap();
        normalize_transport(&mut t2);
        assert_eq!(t2.protocol, "RTP/AVP");
        assert!(t2.unicast);
    }

    #[test]
    fn parse_redirect_location_extracts_rtsp_url() {
        let headers = vec![
            ("CSeq".to_string(), "5".to_string()),
            (
                "Location".to_string(),
                "rtsp://backup.example.com/live/test".to_string(),
            ),
        ];
        assert_eq!(
            parse_redirect_location(&headers),
            Some("rtsp://backup.example.com/live/test")
        );

        let no_location = vec![("CSeq".to_string(), "5".to_string())];
        assert_eq!(parse_redirect_location(&no_location), None);

        let invalid_scheme = vec![(
            "Location".to_string(),
            "http://example.com/test".to_string(),
        )];
        assert_eq!(parse_redirect_location(&invalid_scheme), None);
    }
}
