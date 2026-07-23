//! Typed request DTOs for the GB28181 REST media aliases.
//!
//! These types only normalize the media fields used by `GbHttpService`; they do not
//! carry or parse SIP/SDP/XML signaling fields.

use cheetah_sdk::media_api::rtp_session::RtpPayloadBinding;
use cheetah_sdk::SdkError;
use serde::Deserialize;

fn default_app() -> String {
    "live".to_string()
}

fn default_localhost() -> String {
    "127.0.0.1".to_string()
}

fn default_talk_port() -> u16 {
    30000
}

fn default_pcma_pt() -> u8 {
    8
}

fn default_pcma_codec() -> String {
    "PCMA".to_string()
}

fn default_pcma_clock_rate() -> u32 {
    8000
}

/// Common fields shared by GB28181 REST media requests.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GbBaseRequest {
    #[serde(
        default = "default_app",
        alias = "appName",
        alias = "recv_app",
        alias = "recvApp",
        alias = "send_app",
        alias = "sendApp"
    )]
    pub app: String,
    #[serde(
        alias = "streamName",
        alias = "recv_stream",
        alias = "recvStream",
        alias = "recvStreamId",
        alias = "send_stream",
        alias = "sendStream",
        alias = "sendStreamId",
        alias = "send_stream_id"
    )]
    pub stream: String,
}

impl GbBaseRequest {
    pub fn validate(&self) -> Result<(), SdkError> {
        if self.app.is_empty() || self.app.len() > MAX_NAME_LEN {
            return Err(SdkError::InvalidArgument(format!(
                "app length {} out of bounds",
                self.app.len()
            )));
        }
        if self.stream.is_empty() || self.stream.len() > MAX_NAME_LEN {
            return Err(SdkError::InvalidArgument(format!(
                "stream length {} out of bounds",
                self.stream.len()
            )));
        }
        Ok(())
    }
}

fn default_ssrc_from_stream(stream: &str) -> u32 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut s = DefaultHasher::new();
    stream.hash(&mut s);
    (s.finish() % 1_000_000_000) as u32
}

const MAX_NAME_LEN: usize = 256;
const MAX_U32: u64 = u32::MAX as u64;
const MAX_U16: u64 = u16::MAX as u64;
const MAX_CLOCK_RATE: u32 = 192_000;
const MAX_RTP_PT: u8 = 127;
const MAX_CHANNELS: u8 = 2;

/// Request body for `/recv/create`.
#[derive(Debug, Clone, Deserialize)]
pub struct GbRecvRequest {
    #[serde(flatten)]
    pub base: GbBaseRequest,
    #[serde(default)]
    pub ssrc: Option<u64>,
    #[serde(default)]
    pub port: Option<u64>,
}

impl GbRecvRequest {
    pub fn validate(&self) -> Result<(), SdkError> {
        self.base.validate()?;
        if let Some(ssrc) = self.ssrc {
            if ssrc > MAX_U32 {
                return Err(SdkError::InvalidArgument(format!(
                    "ssrc {ssrc} exceeds u32 range"
                )));
            }
        }
        if let Some(port) = self.port {
            if port == 0 || port > MAX_U16 {
                return Err(SdkError::InvalidArgument(format!(
                    "port {port} out of range"
                )));
            }
        }
        Ok(())
    }

    pub fn ssrc(&self) -> u32 {
        self.ssrc
            .map(|v| v as u32)
            .unwrap_or_else(|| default_ssrc_from_stream(&self.base.stream))
    }

    pub fn port(&self, default_media_port: u16) -> u16 {
        self.port.map(|v| v as u16).unwrap_or(default_media_port)
    }
}

/// Request body for `/recv/stop` and `/send/stop`.
#[derive(Debug, Clone, Deserialize)]
pub struct GbStopRequest {
    #[serde(flatten)]
    pub base: GbBaseRequest,
}

impl GbStopRequest {
    pub fn validate(&self) -> Result<(), SdkError> {
        self.base.validate()
    }
}

/// Request body for `/send/create`.
#[derive(Debug, Clone, Deserialize)]
pub struct GbSendRequest {
    #[serde(flatten)]
    pub base: GbBaseRequest,
    pub ip: String,
    pub port: u64,
    pub ssrc: u64,
}

impl GbSendRequest {
    pub fn validate(&self) -> Result<(), SdkError> {
        self.base.validate()?;
        if self.ip.is_empty() || self.ip.len() > MAX_NAME_LEN {
            return Err(SdkError::InvalidArgument("invalid ip".to_string()));
        }
        if self.port == 0 || self.port > MAX_U16 {
            return Err(SdkError::InvalidArgument(format!(
                "port {} out of range",
                self.port
            )));
        }
        if self.ssrc > MAX_U32 {
            return Err(SdkError::InvalidArgument(format!(
                "ssrc {} exceeds u32 range",
                self.ssrc
            )));
        }
        Ok(())
    }
}

/// Request body for `/talk/start`.
#[derive(Debug, Clone, Deserialize)]
pub struct GbTalkRequest {
    #[serde(flatten)]
    pub base: GbBaseRequest,
    #[serde(default)]
    pub ssrc: Option<u64>,
    #[serde(default = "default_localhost")]
    pub ip: String,
    #[serde(default = "default_talk_port")]
    pub port: u16,
    #[serde(default, alias = "localPort")]
    pub local_port: Option<u64>,
    #[serde(default = "default_pcma_pt")]
    pub pt: u8,
    #[serde(default = "default_pcma_codec")]
    pub codec: String,
    #[serde(default = "default_pcma_clock_rate", alias = "clockRate")]
    pub clock_rate: u32,
    #[serde(default)]
    pub channels: Option<u8>,
}

impl GbTalkRequest {
    pub fn validate(&self) -> Result<(), SdkError> {
        self.base.validate()?;
        if self.ip.is_empty() || self.ip.len() > MAX_NAME_LEN {
            return Err(SdkError::InvalidArgument("invalid ip".to_string()));
        }
        if self.port == 0 {
            return Err(SdkError::InvalidArgument(format!(
                "port {} out of range",
                self.port
            )));
        }
        if let Some(local_port) = self.local_port {
            if local_port == 0 || local_port > MAX_U16 {
                return Err(SdkError::InvalidArgument(format!(
                    "local_port {local_port} out of range"
                )));
            }
        }
        if let Some(ssrc) = self.ssrc {
            if ssrc > MAX_U32 {
                return Err(SdkError::InvalidArgument(format!(
                    "ssrc {ssrc} exceeds u32 range"
                )));
            }
        }
        if self.pt > MAX_RTP_PT {
            return Err(SdkError::InvalidArgument(format!(
                "payload type {} exceeds 127",
                self.pt
            )));
        }
        if self.codec.is_empty() || self.codec.len() > MAX_NAME_LEN {
            return Err(SdkError::InvalidArgument(format!(
                "codec length {} out of bounds",
                self.codec.len()
            )));
        }
        if self.clock_rate == 0 || self.clock_rate > MAX_CLOCK_RATE {
            return Err(SdkError::InvalidArgument(format!(
                "clock_rate {} out of range",
                self.clock_rate
            )));
        }
        if let Some(channels) = self.channels {
            if channels == 0 || channels > MAX_CHANNELS {
                return Err(SdkError::InvalidArgument(format!(
                    "channels {channels} out of range"
                )));
            }
        }
        Ok(())
    }

    pub fn payload_binding(&self) -> RtpPayloadBinding {
        RtpPayloadBinding {
            payload_type: self.pt,
            codec: self.codec.clone(),
            clock_rate: self.clock_rate,
            channels: self.channels,
        }
    }

    pub fn ssrc(&self) -> u32 {
        self.ssrc.unwrap_or(0) as u32
    }

    pub fn local_port(&self, default_media_port: u16) -> u16 {
        self.local_port
            .map(|v| v as u16)
            .unwrap_or(default_media_port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_request_accepts_aliases() {
        let json = serde_json::json!({
            "appName": "gb",
            "recvStream": "cam-1"
        });
        let req: GbBaseRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.app, "gb");
        assert_eq!(req.stream, "cam-1");
    }

    #[test]
    fn base_request_defaults_app_to_live() {
        let json = serde_json::json!({"stream": "cam-1"});
        let req: GbBaseRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.app, "live");
        assert_eq!(req.stream, "cam-1");
    }

    #[test]
    fn recv_request_optional_ssrc_and_port() {
        let json = serde_json::json!({"stream": "cam-1", "ssrc": 123, "port": 40000});
        let req: GbRecvRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.base.stream, "cam-1");
        assert_eq!(req.ssrc(), 123);
        assert_eq!(req.port(30000), 40000);
    }

    #[test]
    fn recv_request_defaults_are_consistent() {
        let json = serde_json::json!({"stream": "cam-1"});
        let req: GbRecvRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.port(30000), 30000);
        // Default SSRC is deterministic from stream name.
        assert_eq!(req.ssrc(), req.ssrc());
    }

    #[test]
    fn send_request_requires_fields() {
        let json =
            serde_json::json!({"stream": "cam-1", "ip": "10.0.0.1", "port": 10000, "ssrc": 42});
        let req: GbSendRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.base.app, "live");
        assert_eq!(req.base.stream, "cam-1");
        assert_eq!(req.ip, "10.0.0.1");
        assert_eq!(req.port, 10000);
        assert_eq!(req.ssrc, 42);
    }

    #[test]
    fn talk_request_defaults_to_pcma() {
        let json = serde_json::json!({"stream": "cam-1"});
        let req: GbTalkRequest = serde_json::from_value(json).unwrap();
        let binding = req.payload_binding();
        assert_eq!(binding.payload_type, 8);
        assert_eq!(binding.codec, "PCMA");
        assert_eq!(binding.clock_rate, 8000);
        assert_eq!(req.ip, "127.0.0.1");
        assert_eq!(req.port, 30000);
    }

    #[test]
    fn talk_request_accepts_camel_case_aliases() {
        let json = serde_json::json!({
            "stream": "cam-1",
            "localPort": 40000,
            "clockRate": 16000,
            "pt": 0,
            "codec": "PCMU"
        });
        let req: GbTalkRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.local_port(30000), 40000);
        assert_eq!(req.clock_rate, 16000);
        assert_eq!(req.payload_binding().payload_type, 0);
        assert_eq!(req.payload_binding().codec, "PCMU");
    }

    #[test]
    fn validation_rejects_out_of_range_fields() {
        let json = serde_json::json!({"stream": "cam-1", "ssrc": u64::MAX, "port": 70000});
        let req: GbRecvRequest = serde_json::from_value(json).unwrap();
        assert!(req.validate().is_err());

        let json = serde_json::json!({
            "stream": "cam-1",
            "ip": "10.0.0.1",
            "port": 0,
            "ssrc": u64::MAX
        });
        let req: GbSendRequest = serde_json::from_value(json).unwrap();
        assert!(req.validate().is_err());

        let json = serde_json::json!({
            "stream": "cam-1",
            "pt": 200,
            "clockRate": 0,
            "channels": 3
        });
        let req: GbTalkRequest = serde_json::from_value(json).unwrap();
        assert!(req.validate().is_err());
    }
}
