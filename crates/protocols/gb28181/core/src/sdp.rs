use crate::error::Gb28181CoreError;

/// `GbSdp` data structure.
/// `GbSdp` 数据结构.
#[derive(Debug, Clone, Default)]
pub struct GbSdp {
    /// `ip` field of type `String`.
    /// `ip` 字段，类型为 `String`.
    pub ip: String,
    /// `video_port` field.
    /// `video_port` 字段.
    pub video_port: Option<u16>,
    /// `audio_port` field.
    /// `audio_port` 字段.
    pub audio_port: Option<u16>,
    /// `ssrc` field.
    /// `ssrc` 字段.
    pub ssrc: Option<u32>,
    /// `sendrecv_mode` field of type `String`.
    /// `sendrecv_mode` 字段，类型为 `String`.
    pub sendrecv_mode: String, // recvonly, sendonly, sendrecv
}

impl GbSdp {
    /// `parse` function.
    /// `parse` 函数.
    pub fn parse(text: &str) -> Result<Self, Gb28181CoreError> {
        let mut sdp = GbSdp::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((field, value)) = line.split_once('=') {
                match field {
                    "c" => {
                        // c=IN IP4 192.168.1.100
                        let parts: Vec<&str> = value.split_whitespace().collect();
                        if parts.len() >= 3 {
                            sdp.ip = parts[2].to_string();
                        }
                    }
                    "m" => {
                        // m=video 10000 RTP/AVP 96
                        let parts: Vec<&str> = value.split_whitespace().collect();
                        if parts.len() >= 2 {
                            let port = parts[1].parse::<u16>().map_err(|e| {
                                Gb28181CoreError::SdpError(format!("invalid port: {e}"))
                            })?;
                            if parts[0] == "video" {
                                sdp.video_port = Some(port);
                            } else if parts[0] == "audio" {
                                sdp.audio_port = Some(port);
                            }
                        }
                    }
                    "a" => {
                        if let Some((attr_name, attr_val)) = value.split_once(':') {
                            if attr_name == "y" {
                                // a=y:0123456789 (ssrc)
                                sdp.ssrc = attr_val.trim().parse::<u32>().ok();
                            }
                        } else if let Some((attr_name, attr_val)) = value.split_once('=') {
                            if attr_name == "y" {
                                // a=y=0123456789 (ssrc)
                                sdp.ssrc = attr_val.trim().parse::<u32>().ok();
                            }
                        } else {
                            match value {
                                "recvonly" | "sendonly" | "sendrecv" => {
                                    sdp.sendrecv_mode = value.to_string();
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(sdp)
    }

    /// Converts to `string` representation.
    /// Converts 为 `string` 表示.
    pub fn to_string(
        session_id: &str,
        ip: &str,
        port: u16,
        ssrc: u32,
        is_video: bool,
        mode: &str,
    ) -> String {
        let media_type = if is_video { "video" } else { "audio" };
        let payload_type = if is_video { "96" } else { "8" }; // 96 for PS, 8 for PCMA G711
        let rtpmap = if is_video {
            "rtpmap:96 PS/90000"
        } else {
            "rtpmap:8 PCMA/8000"
        };

        // GB28181 SDP convention uses `a=y:<ssrc>` with a colon separator. Some
        // legacy peers also accept `=`; we emit the standard form.
        format!(
            "v=0\r\n\
             o=- {session_id} 1 IN IP4 {ip}\r\n\
             s=Play\r\n\
             c=IN IP4 {ip}\r\n\
             t=0 0\r\n\
             m={media_type} {port} RTP/AVP {payload_type}\r\n\
             a={rtpmap}\r\n\
             a={mode}\r\n\
             a=y:{ssrc:010}\r\n"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gb_sdp() {
        let sdp_text = "v=0\r\n\
                        o=- 34020000002000000001 0 IN IP4 192.168.1.100\r\n\
                        s=Play\r\n\
                        c=IN IP4 192.168.1.100\r\n\
                        t=0 0\r\n\
                        m=video 30000 RTP/AVP 96\r\n\
                        a=rtpmap:96 PS/90000\r\n\
                        a=recvonly\r\n\
                        a=y=1234567890\r\n";

        let parsed = GbSdp::parse(sdp_text).unwrap();
        assert_eq!(parsed.ip, "192.168.1.100");
        assert_eq!(parsed.video_port, Some(30000));
        assert_eq!(parsed.ssrc, Some(1234567890));
        assert_eq!(parsed.sendrecv_mode, "recvonly");
    }
}
