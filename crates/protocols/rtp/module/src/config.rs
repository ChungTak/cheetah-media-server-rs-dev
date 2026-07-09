//! RTP module configuration.

use cheetah_rtp_core::RtpPayloadMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RtpModuleConfig {
    pub enabled: bool,
    pub listen_udp: Option<String>,
    pub listen_tcp: Option<String>,
    pub rtcp_listen_udp: Option<String>,
    #[serde(default = "default_write_queue_capacity")]
    pub write_queue_capacity: usize,
    #[serde(default = "default_read_buffer_size")]
    pub read_buffer_size: usize,
    #[serde(default = "default_max_reassembly_bytes")]
    pub max_reassembly_bytes: usize,
    #[serde(default = "default_max_tracks")]
    pub max_tracks: usize,
    #[serde(default = "default_idle_timeout_ms")]
    pub idle_timeout_ms: u64,
    #[serde(
        default = "default_payload_mode",
        serialize_with = "serialize_payload_mode",
        deserialize_with = "deserialize_payload_mode"
    )]
    pub default_payload: RtpPayloadMode,
    #[serde(default = "default_true")]
    pub allow_unaligned_payload: bool,
    /// Video RTP MTU in bytes. Default 1400 to leave room for IP/UDP headers.
    #[serde(default = "default_video_mtu")]
    pub video_mtu: usize,
    /// Audio RTP MTU in bytes. Defaults to a smaller value to keep audio packets small.
    #[serde(default = "default_audio_mtu")]
    pub audio_mtu: usize,
    /// Maximum RTP send rate in KB/s applied at the egress layer. 0 disables the cap.
    #[serde(default)]
    pub max_rtp_kb: u32,
    /// G711 RTP packet duration in milliseconds. ZLM defaults to 100ms for GB28181 interop.
    #[serde(default = "default_g711_packet_duration_ms")]
    pub g711_packet_duration_ms: u32,
    /// UDP socket receive buffer (`SO_RCVBUF`). 0 keeps the OS default.
    #[serde(default = "default_udp_recv_buffer")]
    pub udp_recv_buffer: usize,
    /// Bounded ingress frame buffer size used while waiting for publish auth (ZLM
    /// `RtpProcess` behaviour). 0 disables the cache and starts publishing immediately.
    #[serde(default = "default_publish_frame_cache")]
    pub publish_frame_cache_frames: usize,
    /// Persist raw RTP payload to disk for debugging purposes (ABL `nSaveGB28181Rtp` /
    /// `save_gb28181_rtp`). Path defaults to OS temp dir / `cheetah_rtp/{session}.rtp` when
    /// enabled. Disabled by default in production.
    #[serde(default)]
    pub save_debug_payload: bool,
    /// Default TCP framing applied when reading inbound TCP RTP traffic (`auto`, `two_byte`,
    /// `interleaved_4byte`). Defaults to `auto`.
    #[serde(default = "default_tcp_header_type")]
    pub tcp_header_type: String,
    /// Initial guess for the maximum RTP packet size; the driver may grow this up to
    /// `max_rtp_len_cap` as it observes larger I-frames.
    #[serde(default = "default_max_rtp_len_initial")]
    pub max_rtp_len_initial: usize,
    /// Hard upper bound for the dynamic `nMaxRtpLength` learner.
    #[serde(default = "default_max_rtp_len_cap")]
    pub max_rtp_len_cap: usize,
    #[serde(default)]
    pub pull_jobs: Vec<RtpClientJobConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RtpClientJobConfig {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub destination: String,
    pub ssrc: u32,
    #[serde(
        default = "default_payload_mode",
        serialize_with = "serialize_payload_mode",
        deserialize_with = "deserialize_payload_mode"
    )]
    pub payload_mode: RtpPayloadMode,
    #[serde(default = "default_retry_backoff_ms")]
    pub retry_backoff_ms: u64,
    #[serde(default = "default_max_retry_backoff_ms")]
    pub max_retry_backoff_ms: u64,
}

impl Default for RtpModuleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen_udp: Some("0.0.0.0:20000".to_string()),
            listen_tcp: Some("0.0.0.0:20000".to_string()),
            rtcp_listen_udp: Some("0.0.0.0:20001".to_string()),
            write_queue_capacity: 256,
            read_buffer_size: 65536,
            max_reassembly_bytes: 4 * 1024 * 1024,
            max_tracks: 32,
            idle_timeout_ms: 15000,
            default_payload: RtpPayloadMode::Ps,
            allow_unaligned_payload: true,
            video_mtu: default_video_mtu(),
            audio_mtu: default_audio_mtu(),
            max_rtp_kb: 0,
            g711_packet_duration_ms: default_g711_packet_duration_ms(),
            udp_recv_buffer: default_udp_recv_buffer(),
            publish_frame_cache_frames: default_publish_frame_cache(),
            save_debug_payload: false,
            tcp_header_type: default_tcp_header_type(),
            max_rtp_len_initial: default_max_rtp_len_initial(),
            max_rtp_len_cap: default_max_rtp_len_cap(),
            pull_jobs: Vec::new(),
        }
    }
}

impl RtpModuleConfig {
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).unwrap_or_default()
    }

    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut errors = Vec::new();

        if let Some(ref addr) = self.listen_udp {
            if addr.parse::<std::net::SocketAddr>().is_err() {
                errors.push(format!("invalid listen_udp: {addr}"));
            }
        }

        if let Some(ref addr) = self.listen_tcp {
            if addr.parse::<std::net::SocketAddr>().is_err() {
                errors.push(format!("invalid listen_tcp: {addr}"));
            }
        }

        if let Some(ref addr) = self.rtcp_listen_udp {
            if addr.parse::<std::net::SocketAddr>().is_err() {
                errors.push(format!("invalid rtcp_listen_udp: {addr}"));
            }
        }

        if self.write_queue_capacity < 1 {
            errors.push("write_queue_capacity must be >= 1".to_string());
        }

        if self.max_tracks < 1 {
            errors.push("max_tracks must be >= 1".to_string());
        }

        match self.tcp_header_type.to_lowercase().as_str() {
            "auto" | "two_byte" | "twobyte" | "interleaved_4byte" | "interleaved"
            | "interleaved4byte" => {}
            other => errors.push(format!(
                "invalid tcp_header_type '{other}'; expected one of auto/two_byte/interleaved_4byte"
            )),
        }

        if self.max_rtp_len_initial > self.max_rtp_len_cap {
            errors.push(format!(
                "max_rtp_len_initial ({}) > max_rtp_len_cap ({})",
                self.max_rtp_len_initial, self.max_rtp_len_cap
            ));
        }

        for job in &self.pull_jobs {
            if job.destination.parse::<std::net::SocketAddr>().is_err() {
                errors.push(format!(
                    "pull job '{}': invalid destination: {}",
                    job.name, job.destination
                ));
            }
            if job.retry_backoff_ms > job.max_retry_backoff_ms {
                errors.push(format!(
                    "pull job '{}': retry_backoff_ms ({}) > max_retry_backoff_ms ({})",
                    job.name, job.retry_backoff_ms, job.max_retry_backoff_ms
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

fn default_write_queue_capacity() -> usize {
    256
}
fn default_read_buffer_size() -> usize {
    65536
}
fn default_max_reassembly_bytes() -> usize {
    4 * 1024 * 1024
}
fn default_max_tracks() -> usize {
    32
}
fn default_idle_timeout_ms() -> u64 {
    15000
}
fn default_payload_mode() -> RtpPayloadMode {
    RtpPayloadMode::Ps
}
fn default_true() -> bool {
    true
}
fn default_retry_backoff_ms() -> u64 {
    500
}
fn default_max_retry_backoff_ms() -> u64 {
    5000
}

fn default_video_mtu() -> usize {
    1400
}

fn default_audio_mtu() -> usize {
    600
}

fn default_g711_packet_duration_ms() -> u32 {
    100
}

fn default_udp_recv_buffer() -> usize {
    4 * 1024 * 1024
}

fn default_publish_frame_cache() -> usize {
    // 10 seconds of frames at ~30fps for a single video track. Bounded to avoid memory blowups.
    300
}

fn default_tcp_header_type() -> String {
    "auto".to_string()
}

fn default_max_rtp_len_initial() -> usize {
    2048
}

fn default_max_rtp_len_cap() -> usize {
    65536
}

fn serialize_payload_mode<S>(mode: &RtpPayloadMode, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let s = match mode {
        RtpPayloadMode::Ps => "ps",
        RtpPayloadMode::Ts => "ts",
        RtpPayloadMode::Es => "es",
        RtpPayloadMode::Ehome => "ehome",
        RtpPayloadMode::Xhb => "xhb",
        RtpPayloadMode::Jtt1078 => "jtt1078",
        RtpPayloadMode::RawAudio => "raw_audio",
        RtpPayloadMode::RawVideo => "raw_video",
        RtpPayloadMode::Unknown => "unknown",
    };
    serializer.serialize_str(s)
}

fn deserialize_payload_mode<'de, D>(deserializer: D) -> Result<RtpPayloadMode, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    match s.to_lowercase().as_str() {
        "ps" | "1" => Ok(RtpPayloadMode::Ps),
        "ts" | "2" => Ok(RtpPayloadMode::Ts),
        "es" | "3" => Ok(RtpPayloadMode::Es),
        "ehome" | "4" => Ok(RtpPayloadMode::Ehome),
        "xhb" | "hk" => Ok(RtpPayloadMode::Xhb),
        "jtt1078" | "1078" => Ok(RtpPayloadMode::Jtt1078),
        "raw_audio" | "audio" => Ok(RtpPayloadMode::RawAudio),
        "raw_video" | "video" => Ok(RtpPayloadMode::RawVideo),
        _ => Ok(RtpPayloadMode::Unknown),
    }
}
