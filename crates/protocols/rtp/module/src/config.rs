//! RTP module configuration.
//!
//! RTP 模块配置。

use cheetah_rtp_core::RtpPayloadMode;
use cheetah_sdk::media_api::rtp_session::GbMediaCompatibilityProfile;
use serde::{Deserialize, Serialize};

/// Configuration for the RTP module.
///
/// RTP 模块配置。
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
    /// Driver tick interval in milliseconds.
    ///
    /// 驱动 tick 间隔（毫秒）。
    #[serde(default = "default_tick_interval_ms")]
    pub tick_interval_ms: u64,
    /// Interval between RTCP sender/receiver reports in milliseconds.
    ///
    /// RTCP Sender/Receiver Report 生成间隔（毫秒）。
    #[serde(default = "default_rtcp_report_interval_ms")]
    pub rtcp_report_interval_ms: u64,
    #[serde(
        default = "default_payload_mode",
        serialize_with = "serialize_payload_mode",
        deserialize_with = "deserialize_payload_mode"
    )]
    pub default_payload: RtpPayloadMode,
    #[serde(default = "default_true")]
    pub allow_unaligned_payload: bool,
    /// Video RTP MTU in bytes. Default 1400 to leave room for IP/UDP headers.
    ///
    /// 视频 RTP MTU（字节）。默认 1400，为 IP/UDP 头留出空间。
    #[serde(default = "default_video_mtu")]
    pub video_mtu: usize,
    /// Audio RTP MTU in bytes. Defaults to a smaller value to keep audio packets small.
    ///
    /// 音频 RTP MTU（字节）。默认较小，以保持音频包较小。
    #[serde(default = "default_audio_mtu")]
    pub audio_mtu: usize,
    /// Maximum RTP send rate in KB/s applied at the egress layer. 0 disables the cap.
    ///
    /// 出站层 RTP 最大发送速率（KB/s）。0 表示不限制。
    #[serde(default)]
    pub max_rtp_kb: u32,
    /// G711 RTP packet duration in milliseconds. ZLM defaults to 100ms for GB28181 interop.
    ///
    /// G711 RTP 包时长（毫秒）。ZLM 为 GB28181 互操作默认 100ms。
    #[serde(default = "default_g711_packet_duration_ms")]
    pub g711_packet_duration_ms: u32,
    /// Audio codecs enabled for talk (duplex voice) sessions.
    /// PCMA/PCMU are enabled by default; AAC must be explicitly added after capability
    /// negotiation.
    ///
    /// 对讲允许使用的音频 codec。默认启用 PCMA/PCMU；AAC 必须在能力协商后显式添加。
    #[serde(default = "default_enabled_talk_codecs")]
    pub enabled_talk_codecs: Vec<String>,
    /// Per-talkback subscriber queue capacity. A smaller queue keeps latency low and lets the
    /// drop policy shed audio frames when the downstream device is slow.
    ///
    /// 对讲订阅队列容量。较小的队列可以降低延迟，并在下游设备慢时让丢弃策略丢弃音频帧。
    #[serde(default = "default_talkback_queue_capacity")]
    pub talkback_queue_capacity: usize,
    /// Maximum acceptable latency for a talkback audio frame in milliseconds. Frames older than
    /// this are dropped unless they are non-droppable (e.g. key/config frames).
    ///
    /// 对讲音频帧最大可接受延迟（毫秒）。超过此值的帧将被丢弃，除非是不可丢弃帧（如关键帧/参数集帧）。
    #[serde(default = "default_talkback_max_latency_ms")]
    pub talkback_max_latency_ms: u32,
    /// UDP socket receive buffer (`SO_RCVBUF`). 0 keeps the OS default.
    ///
    /// UDP 套接字接收缓冲区（SO_RCVBUF）。0 保持 OS 默认。
    #[serde(default = "default_udp_recv_buffer")]
    pub udp_recv_buffer: usize,
    /// Bounded ingress frame buffer size used while waiting for publish auth (ZLM
    /// `RtpProcess` behaviour). 0 disables the cache and starts publishing immediately.
    ///
    /// 等待发布授权时使用的有界入站帧缓存（ZLM `RtpProcess` 行为）。0 表示禁用缓存并立即发布。
    #[serde(default = "default_publish_frame_cache")]
    pub publish_frame_cache_frames: usize,
    /// Persist raw RTP payload to disk for debugging purposes (ABL `nSaveGB28181Rtp` /
    /// `save_gb28181_rtp`). Path defaults to OS temp dir / `cheetah_rtp/{session}.rtp` when
    /// enabled. Disabled by default in production.
    ///
    /// 将原始 RTP 负载落盘用于调试（ABL `nSaveGB28181Rtp`）。生产环境默认禁用。
    #[serde(default)]
    pub save_debug_payload: bool,
    /// Default TCP framing applied when reading inbound TCP RTP traffic (`auto`, `two_byte`,
    /// `interleaved_4byte`). Defaults to `auto`.
    ///
    /// 读取入站 TCP RTP 时使用的默认分帧模式（auto/two_byte/interleaved_4byte）。默认 `auto`。
    #[serde(default = "default_tcp_header_type")]
    pub tcp_header_type: String,
    /// Initial guess for the maximum RTP packet size; the driver may grow this up to
    /// `max_rtp_len_cap` as it observes larger I-frames.
    ///
    /// RTP 包最大尺寸的初始猜测值；驱动在观察到更大的 I 帧时可增长到 `max_rtp_len_cap`。
    #[serde(default = "default_max_rtp_len_initial")]
    pub max_rtp_len_initial: usize,
    /// Hard upper bound for the dynamic `nMaxRtpLength` learner.
    ///
    /// 动态 `nMaxRtpLength` 学习器的硬上限。
    #[serde(default = "default_max_rtp_len_cap")]
    pub max_rtp_len_cap: usize,
    #[serde(default)]
    pub pull_jobs: Vec<RtpClientJobConfig>,
    /// Maximum number of concurrent RTP sessions.
    ///
    /// 最大并发 RTP 会话数。
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
    /// Profiles enabled for new sessions. An empty list disables all profiles.
    ///
    /// 新会话允许的兼容 profile 列表。空列表表示禁用所有 profile。
    #[serde(default = "default_enabled_profiles")]
    pub enabled_profiles: Vec<GbMediaCompatibilityProfile>,
}

/// Configuration for a pull/egress RTP client job.
///
/// 拉流/出站 RTP 客户端任务配置。
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
            tick_interval_ms: default_tick_interval_ms(),
            rtcp_report_interval_ms: default_rtcp_report_interval_ms(),
            default_payload: RtpPayloadMode::Ps,
            allow_unaligned_payload: true,
            video_mtu: default_video_mtu(),
            audio_mtu: default_audio_mtu(),
            max_rtp_kb: 0,
            g711_packet_duration_ms: default_g711_packet_duration_ms(),
            enabled_talk_codecs: default_enabled_talk_codecs(),
            talkback_queue_capacity: default_talkback_queue_capacity(),
            talkback_max_latency_ms: default_talkback_max_latency_ms(),
            udp_recv_buffer: default_udp_recv_buffer(),
            publish_frame_cache_frames: default_publish_frame_cache(),
            save_debug_payload: false,
            tcp_header_type: default_tcp_header_type(),
            max_rtp_len_initial: default_max_rtp_len_initial(),
            max_rtp_len_cap: default_max_rtp_len_cap(),
            pull_jobs: Vec::new(),
            max_sessions: default_max_sessions(),
            enabled_profiles: default_enabled_profiles(),
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

    /// Validate the RTP module config and return any errors as a single string.
    ///
    /// 校验 RTP 模块配置，并将错误合并为一个字符串返回。
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

        if self.max_sessions < 1 {
            errors.push("max_sessions must be >= 1".to_string());
        }

        if self.tick_interval_ms < 1 {
            errors.push("tick_interval_ms must be >= 1".to_string());
        }

        if self.rtcp_report_interval_ms < 1 {
            errors.push("rtcp_report_interval_ms must be >= 1".to_string());
        }

        if self.talkback_queue_capacity < 1 {
            errors.push("talkback_queue_capacity must be >= 1".to_string());
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

/// Default write queue capacity for RTP sockets.
///
/// RTP 套接字默认写队列容量。
fn default_write_queue_capacity() -> usize {
    256
}
/// Default TCP/UDP read buffer size.
///
/// 默认 TCP/UDP 读缓冲区大小。
fn default_read_buffer_size() -> usize {
    65536
}
/// Default maximum reassembly bytes for the RTP depacketizer.
///
/// RTP 拆包重组缓冲区默认最大值。
fn default_max_reassembly_bytes() -> usize {
    4 * 1024 * 1024
}
/// Default maximum number of tracks to publish.
///
/// 默认最大发布轨道数。
fn default_max_tracks() -> usize {
    32
}
/// Default session idle timeout in milliseconds.
///
/// 默认会话空闲超时（毫秒）。
fn default_idle_timeout_ms() -> u64 {
    15000
}
/// Default driver tick interval in milliseconds.
///
/// 默认驱动 tick 间隔（毫秒）。
fn default_tick_interval_ms() -> u64 {
    100
}
/// Default RTCP sender/receiver report interval in milliseconds.
///
/// 默认 RTCP Sender/Receiver Report 间隔（毫秒）。
fn default_rtcp_report_interval_ms() -> u64 {
    5000
}
/// Default RTP payload mode.
///
/// 默认 RTP 负载模式。
fn default_payload_mode() -> RtpPayloadMode {
    RtpPayloadMode::Ps
}
/// Default `true` for serde `#[serde(default)]`.
///
/// 用于 serde `#[serde(default)]` 的默认 `true`。
fn default_true() -> bool {
    true
}
/// Default retry backoff for pull jobs.
///
/// 拉流任务默认重试退避（毫秒）。
fn default_retry_backoff_ms() -> u64 {
    500
}
/// Default maximum retry backoff for pull jobs.
///
/// 拉流任务默认最大重试退避（毫秒）。
fn default_max_retry_backoff_ms() -> u64 {
    5000
}

/// Default video RTP MTU.
///
/// 默认视频 RTP MTU。
fn default_video_mtu() -> usize {
    1400
}

/// Default audio RTP MTU.
///
/// 默认音频 RTP MTU。
fn default_audio_mtu() -> usize {
    600
}

/// Default G711 packet duration in milliseconds.
///
/// 默认 G711 包时长（毫秒）。
fn default_g711_packet_duration_ms() -> u32 {
    100
}

/// Default UDP socket receive buffer size.
///
/// 默认 UDP 套接字接收缓冲区大小。
fn default_udp_recv_buffer() -> usize {
    4 * 1024 * 1024
}

/// Default publish frame cache size.
///
/// 默认发布帧缓存大小。
fn default_publish_frame_cache() -> usize {
    // 10 seconds of frames at ~30fps for a single video track. Bounded to avoid memory blowups.
    // 以单路视频约 30fps 缓存 10 秒帧，设置上限以避免内存爆增。
    300
}

/// Default TCP framing type string.
///
/// 默认 TCP 分帧类型字符串。
fn default_tcp_header_type() -> String {
    "auto".to_string()
}

/// Default initial guess for the maximum RTP packet size.
///
/// 默认 RTP 包最大尺寸初始猜测值。
fn default_max_rtp_len_initial() -> usize {
    2048
}

/// Default hard upper bound for `nMaxRtpLength`.
///
/// 默认 `nMaxRtpLength` 硬上限。
fn default_max_rtp_len_cap() -> usize {
    65536
}

/// Serialize `RtpPayloadMode` as a string alias.
///
/// 将 `RtpPayloadMode` 序列化为字符串别名。
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

/// Deserialize `RtpPayloadMode` from a string or numeric alias.
///
/// 从字符串或数字别名反序列化 `RtpPayloadMode`。
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

fn default_max_sessions() -> usize {
    10_000
}

fn default_enabled_talk_codecs() -> Vec<String> {
    vec!["PCMA".to_string(), "PCMU".to_string()]
}

fn default_talkback_queue_capacity() -> usize {
    32
}

fn default_talkback_max_latency_ms() -> u32 {
    500
}

fn default_enabled_profiles() -> Vec<GbMediaCompatibilityProfile> {
    vec![
        GbMediaCompatibilityProfile::Strict,
        GbMediaCompatibilityProfile::GbCommon,
        GbMediaCompatibilityProfile::Zlm,
        GbMediaCompatibilityProfile::Sms,
        GbMediaCompatibilityProfile::Abl,
        GbMediaCompatibilityProfile::HikvisionEhome,
        GbMediaCompatibilityProfile::Jtt1078,
    ]
}
