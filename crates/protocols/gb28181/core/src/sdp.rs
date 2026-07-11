use crate::error::Gb28181CoreError;

/// Parsed GB28181 SDP payload: media address, ports, SSRC, and direction.
///
/// GB28181 SDP 解析结果：媒体地址、端口、SSRC 与方向。
#[derive(Debug, Clone, Default)]
pub struct GbSdp {
    pub ip: String,
    pub video_port: Option<u16>,
    pub audio_port: Option<u16>,
    pub ssrc: Option<u32>,
    pub sendrecv_mode: String, // recvonly, sendonly, sendrecv
}

impl GbSdp {
    /// Parse a minimal GB28181 SDP, extracting the `c=`, `m=`, and `a=` fields that matter for
    /// establishing an RTP/PS or RTP/PCMA stream.
    ///
    /// 解析最小化 GB28181 SDP，提取建立 RTP/PS 或 RTP/PCMA 流所需的 `c=`、`m=` 与 `a=` 字段。
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

    /// Build a GB28181 SDP body for an outgoing `INVITE`.
    ///
    /// The generated SDP advertises `m=` for PS video (`RTP/AVP 96`) or PCMA audio
    /// (`RTP/AVP 8`), depending on `is_video`, and includes the `a=y:<ssrc>` SSRC line
    /// required by the standard for RTP stream correlation.
    ///
    /// 为 outgoing `INVITE` 构造 GB28181 SDP 体。
    ///
    /// 根据 `is_video` 在 SDP 中声明 PS 视频（`RTP/AVP 96`）或 PCMA 音频
    /// （`RTP/AVP 8`）的 `m=` 行，并包含标准要求的 `a=y:<ssrc>` SSRC 行用于
    /// RTP 流关联。
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
