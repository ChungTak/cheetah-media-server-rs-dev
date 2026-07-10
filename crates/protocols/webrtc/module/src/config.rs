//! WebRTC module configuration.
//!
//! Maps onto the `modules.webrtc` section of `config.yaml`. The schema
//! follows the plan document:
//! `dev-docs/plans-27-webrtc-sms/webrtc-str0m-architecture.md` §"配置草案".

use std::net::IpAddr;

use cheetah_webrtc_core::{
    WebRtcCodecProfile, WebRtcCoreConfig, WebRtcCoreLimits, WebRtcIceTransportPolicy,
};
use cheetah_webrtc_driver_tokio::{UdpPortRange, WebRtcDriverConfig};
use serde::{Deserialize, Serialize};

use crate::codec_policy::AudioOutputStrategy;
use crate::compat::{parse_ome_transport_mode, OmeTransportMode};

/// Configuration for `Web Rtc ICE Server`.
/// `Web Rtc ICE Server` 的配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebRtcIceServerConfig {
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub credential: Option<String>,
}

/// Configuration for `Web Rtc Module`.
/// `Web Rtc Module` 的配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebRtcModuleConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_listen_udp")]
    pub listen_udp: String,
    #[serde(default)]
    pub listen_tcp: Option<String>,
    /// Minimum UDP port for the driver listener. When both
    /// `udp_port_min` and `udp_port_max` are set, the driver binds
    /// within `[udp_port_min, udp_port_max]` instead of using the
    /// port from `listen_udp`. Useful for firewall/NAT deployments
    /// that restrict outbound UDP to a known range.
    #[serde(default)]
    pub udp_port_min: Option<u16>,
    /// Maximum UDP port for the driver listener. See `udp_port_min`.
    #[serde(default)]
    pub udp_port_max: Option<u16>,
    #[serde(default)]
    pub public_ips: Vec<String>,
    #[serde(default)]
    pub candidate_hostname: Option<String>,
    #[serde(default)]
    pub ice_lite: bool,
    /// ICE candidate gathering policy: `all`, `relay-only`, or `p2p-only`.
    /// Default: `all`.
    #[serde(default = "default_ice_transport_policy")]
    pub ice_transport_policy: String,
    /// OME-compatible default `transport` value used when an OME URL
    /// omits the query parameter. Accepted values:
    /// `udp`, `tcp`, `relay`, `udptcp`, `all`. Default: `udptcp`.
    #[serde(default = "default_ome_default_transport")]
    pub ome_default_transport: String,
    /// OME `TcpRelayForce` compatibility switch. When enabled, OME
    /// URL sessions are forced to relay-only candidate output even if
    /// the request asks for another transport.
    #[serde(default)]
    pub ome_tcp_relay_force: bool,
    /// ICE servers advertised to OME-compatible clients when the
    /// request transport requires relay candidates (`relay`/`all`) or
    /// `ome_tcp_relay_force` is enabled.
    #[serde(default)]
    pub ome_ice_servers: Vec<WebRtcIceServerConfig>,
    /// Optional OME-compatible custom WebSocket signaling listener.
    /// When set, the module accepts `request_offer`/`answer`/`candidate`
    /// JSON frames on this socket in addition to the HTTP WHIP/WHEP
    /// endpoints.
    #[serde(default)]
    pub ome_ws_listen: Option<String>,
    /// Maximum accepted OME WebSocket signaling connections.
    #[serde(default = "default_ome_ws_max_connections")]
    pub ome_ws_max_connections: usize,
    /// OME WebSocket handshake timeout in milliseconds.
    #[serde(default = "default_ome_ws_handshake_timeout_ms")]
    pub ome_ws_handshake_timeout_ms: u64,
    #[serde(default = "default_true")]
    pub enable_udp: bool,
    #[serde(default)]
    pub enable_tcp: bool,
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
    #[serde(default = "default_shard_count")]
    pub shard_count: usize,
    #[serde(default = "default_read_buffer")]
    pub read_buffer_size: usize,
    #[serde(default = "default_write_queue")]
    pub write_queue_capacity: usize,
    #[serde(default = "default_event_queue")]
    pub event_queue_capacity: usize,
    #[serde(default = "default_session_idle_timeout_ms")]
    pub session_idle_timeout_ms: u64,
    /// Idle timeout (ms) for an accepted TCP connection. The driver
    /// closes the connection if no bytes arrive for this long. `0`
    /// disables the timeout.
    #[serde(default = "default_tcp_idle_timeout_ms")]
    pub tcp_idle_timeout_ms: u64,
    #[serde(default = "default_handshake_timeout_ms")]
    pub handshake_timeout_ms: u64,
    #[serde(default = "default_migration_route_ttl_ms")]
    pub migration_route_ttl_ms: u64,
    #[serde(default)]
    pub codec_profile: CodecProfileWire,
    #[serde(default = "default_prefer_video_codec")]
    pub prefer_video_codec: String,
    #[serde(default = "default_prefer_audio_codec")]
    pub prefer_audio_codec: String,
    /// Audio output strategy for WebRTC playback. Controls how the module
    /// handles audio codec mismatches between source streams and client
    /// capabilities.
    ///
    /// - `auto` (default): G711 passes through when client supports it;
    ///   AAC/MP3 transcode to Opus for Browser profile.
    /// - `transcode_to_opus`: Always output Opus regardless of source.
    /// - `passthrough`: Pass through source codec unchanged.
    #[serde(default)]
    pub audio_output_strategy: AudioOutputStrategy,
    #[serde(default = "default_true")]
    pub enable_simulcast: bool,
    /// Simulcast layer selection policy when multiple RIDs are
    /// negotiated. Accepted values: `highest`, `lowest`, `rid:<name>`.
    /// `highest` is the SMS default.
    #[serde(default = "default_simulcast_policy")]
    pub simulcast_default_policy: String,
    #[serde(default = "default_true")]
    pub enable_bwe: bool,
    #[serde(default = "default_bwe_initial")]
    pub bwe_initial_bitrate_kbps: u64,
    /// Lower BWE estimate (in kilobits per second) below which the
    /// `Adaptive` simulcast policy elects the lowest available layer.
    /// Mirrors the ZLM / SMS convention. `0` disables the lower bound
    /// (the policy then never elects "low").
    #[serde(default = "default_bwe_low_threshold")]
    pub bwe_low_threshold_kbps: u64,
    /// Upper BWE estimate (in kilobits per second) above which the
    /// `Adaptive` simulcast policy elects the highest available
    /// layer. `0` disables the upper bound.
    #[serde(default = "default_bwe_high_threshold")]
    pub bwe_high_threshold_kbps: u64,
    /// OME `RtcpBasedTimestamp` compatibility switch for WebRTC ingest.
    /// Default `false` normalizes each inbound RTP track to a zero-based
    /// fast-start timeline. `true` preserves the raw RTP timestamp ticks
    /// so future RTCP-SR wall-clock alignment can use the sender epoch.
    #[serde(default)]
    pub rtcp_based_timestamp: bool,
    /// OME-style Auto ABR switch. When enabled (default), publish
    /// simulcast layer election is updated from BWE/REMB feedback.
    /// When disabled, the configured static simulcast policy is kept.
    #[serde(default = "default_true")]
    pub webrtc_auto_abr: bool,
    /// Playback jitter buffer target in milliseconds. `0` keeps the
    /// current low-latency pass-through behaviour.
    #[serde(default)]
    pub play_jitter_buffer_ms: u64,
    /// Desired playout-delay hint lower bound in milliseconds.
    /// `0` means "no explicit minimum".
    #[serde(default)]
    pub playout_delay_min_ms: u16,
    /// Desired playout-delay hint upper bound in milliseconds.
    /// `0` means "no explicit maximum".
    #[serde(default)]
    pub playout_delay_max_ms: u16,
    /// OME-style periodic FIR interval in milliseconds.
    /// `0` disables periodic keyframe requests.
    #[serde(default)]
    pub fir_interval_ms: u64,
    /// Whether RED/ULPFEC payloads should remain advertised in local
    /// SDP. Default `false` for conservative compatibility.
    #[serde(default)]
    pub enable_red_ulpfec: bool,
    #[serde(default = "default_rtx_cache_packets")]
    pub rtx_cache_packets: usize,
    #[serde(default = "default_rtx_cache_age_ms")]
    pub rtx_cache_age_ms: u64,
    #[serde(default = "default_video_reorder")]
    pub video_reorder_packets: usize,
    #[serde(default = "default_audio_reorder")]
    pub audio_reorder_packets: usize,
    #[serde(default = "default_bootstrap_frames")]
    pub bootstrap_frame_count: usize,
    #[serde(default = "default_bootstrap_max_age_ms")]
    pub bootstrap_max_age_ms: u64,
    #[serde(default = "default_wait_stream_ms")]
    pub wait_stream_timeout_ms: u64,
    /// Maximum DataChannel message size in bytes the core will accept
    /// from the boundary `WebRtcCoreCommand::SendDataChannel`. Larger
    /// payloads are dropped with a diagnostic. Mirrors the ZLM
    /// `data_channel_message_max` knob.
    #[serde(default = "default_datachannel_max_message_bytes")]
    pub datachannel_max_message_bytes: usize,
    /// When true, the echo answer SDP rewrites `a=msid:` lines to a
    /// unique per-session stream id, preventing Chrome from silently
    /// discarding remote tracks whose `msid` matches the local track.
    /// Aligns with ZLM `WebRtcEchoTest` behaviour. Default: true.
    #[serde(default = "default_true")]
    pub echo_rewrite_msid: bool,
    /// When true, H264 B-frames are filtered out on the WebRTC play
    /// egress path. This avoids decode glitches on clients that do not
    /// support B-frame reordering (most WebRTC browsers). The actual
    /// filtering is performed by `cheetah-codec`; this flag controls
    /// whether the module enables the filter. Default: false (pass
    /// through all frames; codec-side implementation pending).
    #[serde(default)]
    pub h264_bframe_filter: bool,
    /// When true, the OPTIONS preflight response includes the
    /// `Access-Control-Allow-Private-Network: true` header. This is
    /// required for browsers that enforce the Private Network Access
    /// spec (Chrome 104+) when a public page fetches a local/private
    /// network resource. Aligns with ABL's `ResponseOPTIONS` behaviour.
    /// Default: false.
    #[serde(default)]
    pub enable_private_network_access: bool,
    /// Diagnostic message included in the WHEP 404 response when the
    /// play slow-start wait window expires without the stream coming
    /// online. When empty, a default message is used.
    #[serde(default)]
    pub play_timeout_diagnostic: Option<String>,
    /// Minimum play duration (in milliseconds) before a disconnect
    /// triggers a business-level `play_disconnect` event on the SDK
    /// event bus. Connections shorter than this threshold are
    /// considered "short connections" — they still record a metric
    /// counter but do NOT emit the business event. Reference: ABL
    /// uses 8 seconds. Default: 8000 ms.
    #[serde(default = "default_play_disconnect_min_duration_ms")]
    pub play_disconnect_min_duration_ms: u64,
    /// Public base URL for WebRTC WHEP play URLs exposed in the
    /// session list / control plane. When set, the session list uses
    /// this as the URL prefix for WHEP endpoints (e.g.
    /// `http://cdn.example.com:8080/api/v1/rtc`). When absent, the
    /// URL is derived from the request `Host` header at query time.
    /// ABL 2025-12-26/29 `getOutList` exposes a similar URL.
    #[serde(default)]
    pub public_webrtc_base_url: Option<String>,
    #[serde(default)]
    pub server_label: Option<String>,
}

/// Simulcast layer selection policy.
///
/// Mirrors the values described in the plan document. Module passes the
/// policy to the bridge so that ingress media frames from layers that
/// are not selected get dropped before reaching the engine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SimulcastPolicy {
    #[default]
    Highest,
    Lowest,
    Rid(String),
    /// Adaptive layer selection driven by upstream BWE / loss
    /// feedback. The bridge maintains a per-session estimate and
    /// picks the highest layer whose bitrate floor fits under the
    /// estimate. Layer ranking comes from RID lexicographic order
    /// (matching the ZLM `q < h < f` convention) until BWE arrives;
    /// once BWE lands, the elected layer is whichever RID has
    /// `floor(bitrate)` that best matches the estimate without
    /// exceeding it.
    Adaptive,
    /// Multi-stream mode: each simulcast RID is published as a
    /// separate sub-stream in the engine. The sub-stream key is
    /// derived from the base stream key by appending `@rid:<name>`,
    /// e.g. `live/cam@rid:h`. Each sub-stream gets its own
    /// `PublishLease` and can be independently subscribed by
    /// downstream protocols.
    MultiStream,
}

impl SimulcastPolicy {
    /// Parses the input into a structured value, returning an error if malformed.
    /// 将输入解析为结构化值，格式错误时返回错误。
    pub fn parse(input: &str) -> Self {
        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("highest") || trimmed.is_empty() {
            return Self::Highest;
        }
        if trimmed.eq_ignore_ascii_case("lowest") {
            return Self::Lowest;
        }
        if trimmed.eq_ignore_ascii_case("adaptive") {
            return Self::Adaptive;
        }
        if trimmed.eq_ignore_ascii_case("multi-stream")
            || trimmed.eq_ignore_ascii_case("multistream")
        {
            return Self::MultiStream;
        }
        if let Some(rid) = trimmed.strip_prefix("rid:") {
            return Self::Rid(rid.to_string());
        }
        // Unknown policies fall back to the safe default to avoid
        // silently routing nothing.
        Self::Highest
    }
}

/// `CodecProfileWire` enumeration.
/// `CodecProfileWire` 枚举。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodecProfileWire {
    #[default]
    Browser,
    Device,
    Passthrough,
}

impl From<CodecProfileWire> for WebRtcCodecProfile {
    fn from(value: CodecProfileWire) -> Self {
        match value {
            CodecProfileWire::Browser => WebRtcCodecProfile::Browser,
            CodecProfileWire::Device => WebRtcCodecProfile::Device,
            CodecProfileWire::Passthrough => WebRtcCodecProfile::Passthrough,
        }
    }
}

impl Default for WebRtcModuleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen_udp: default_listen_udp(),
            listen_tcp: None,
            udp_port_min: None,
            udp_port_max: None,
            public_ips: Vec::new(),
            candidate_hostname: None,
            ice_lite: false,
            ice_transport_policy: default_ice_transport_policy(),
            ome_default_transport: default_ome_default_transport(),
            ome_tcp_relay_force: false,
            ome_ice_servers: Vec::new(),
            ome_ws_listen: None,
            ome_ws_max_connections: default_ome_ws_max_connections(),
            ome_ws_handshake_timeout_ms: default_ome_ws_handshake_timeout_ms(),
            enable_udp: true,
            enable_tcp: false,
            max_sessions: default_max_sessions(),
            shard_count: default_shard_count(),
            read_buffer_size: default_read_buffer(),
            write_queue_capacity: default_write_queue(),
            event_queue_capacity: default_event_queue(),
            session_idle_timeout_ms: default_session_idle_timeout_ms(),
            tcp_idle_timeout_ms: default_tcp_idle_timeout_ms(),
            handshake_timeout_ms: default_handshake_timeout_ms(),
            migration_route_ttl_ms: default_migration_route_ttl_ms(),
            codec_profile: CodecProfileWire::default(),
            prefer_video_codec: default_prefer_video_codec(),
            prefer_audio_codec: default_prefer_audio_codec(),
            audio_output_strategy: AudioOutputStrategy::default(),
            enable_simulcast: true,
            simulcast_default_policy: default_simulcast_policy(),
            enable_bwe: true,
            bwe_initial_bitrate_kbps: default_bwe_initial(),
            bwe_low_threshold_kbps: default_bwe_low_threshold(),
            bwe_high_threshold_kbps: default_bwe_high_threshold(),
            rtcp_based_timestamp: false,
            webrtc_auto_abr: true,
            play_jitter_buffer_ms: 0,
            playout_delay_min_ms: 0,
            playout_delay_max_ms: 0,
            fir_interval_ms: 0,
            enable_red_ulpfec: false,
            rtx_cache_packets: default_rtx_cache_packets(),
            rtx_cache_age_ms: default_rtx_cache_age_ms(),
            video_reorder_packets: default_video_reorder(),
            audio_reorder_packets: default_audio_reorder(),
            bootstrap_frame_count: default_bootstrap_frames(),
            bootstrap_max_age_ms: default_bootstrap_max_age_ms(),
            wait_stream_timeout_ms: default_wait_stream_ms(),
            datachannel_max_message_bytes: default_datachannel_max_message_bytes(),
            echo_rewrite_msid: true,
            h264_bframe_filter: false,
            enable_private_network_access: false,
            play_timeout_diagnostic: None,
            play_disconnect_min_duration_ms: default_play_disconnect_min_duration_ms(),
            public_webrtc_base_url: None,
            server_label: None,
        }
    }
}

impl WebRtcModuleConfig {
    /// `default_json` function of `WebRtcModuleConfig`.
    /// `WebRtcModuleConfig` 的 `default_json` 函数。
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).expect("default WebRtcModuleConfig serialises")
    }

    /// Creates `value` from input.
    /// 从输入创建 `value`。
    pub fn from_value(value: serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|err| err.to_string())
    }

    /// Validates the input and returns an error if invalid.
    /// 验证输入，无效时返回错误。
    pub fn validate(&self) -> Result<(), String> {
        if self.listen_udp.is_empty() {
            return Err("listen_udp must not be empty".into());
        }
        let _: std::net::SocketAddr = self
            .listen_udp
            .parse()
            .map_err(|e| format!("invalid listen_udp {}: {e}", self.listen_udp))?;
        if let Some(tcp) = &self.listen_tcp {
            if !tcp.is_empty() {
                let _: std::net::SocketAddr = tcp
                    .parse()
                    .map_err(|e| format!("invalid listen_tcp {tcp}: {e}"))?;
            }
        }
        // Validate UDP port range if either bound is specified.
        match (self.udp_port_min, self.udp_port_max) {
            (Some(min), Some(max)) => {
                let range = UdpPortRange { min, max };
                range.validate()?;
            }
            (Some(_), None) => {
                return Err("udp_port_min is set but udp_port_max is missing".into());
            }
            (None, Some(_)) => {
                return Err("udp_port_max is set but udp_port_min is missing".into());
            }
            (None, None) => {}
        }
        for ip in &self.public_ips {
            let _: IpAddr = ip
                .parse()
                .map_err(|e| format!("invalid public ip {ip}: {e}"))?;
        }
        if self.max_sessions == 0 {
            return Err("max_sessions must be > 0".into());
        }
        if self.read_buffer_size == 0 {
            return Err("read_buffer_size must be > 0".into());
        }
        if self.write_queue_capacity == 0 {
            return Err("write_queue_capacity must be > 0".into());
        }
        if self.event_queue_capacity == 0 {
            return Err("event_queue_capacity must be > 0".into());
        }
        if self.datachannel_max_message_bytes == 0 {
            return Err("datachannel_max_message_bytes must be > 0".into());
        }
        if self.bwe_low_threshold_kbps != 0
            && self.bwe_high_threshold_kbps != 0
            && self.bwe_low_threshold_kbps >= self.bwe_high_threshold_kbps
        {
            return Err(format!(
                "bwe_low_threshold_kbps ({}) must be < bwe_high_threshold_kbps ({})",
                self.bwe_low_threshold_kbps, self.bwe_high_threshold_kbps
            ));
        }
        if self.playout_delay_max_ms != 0 && self.playout_delay_min_ms > self.playout_delay_max_ms {
            return Err(format!(
                "playout_delay_min_ms ({}) must be <= playout_delay_max_ms ({}) when max is non-zero",
                self.playout_delay_min_ms, self.playout_delay_max_ms
            ));
        }
        // Reject obviously malformed simulcast policies; unknown
        // values fall through to `Highest` at runtime, but we still
        // report the typo in config validation so operators see it.
        let policy = self.simulcast_default_policy.trim();
        let is_valid = policy.is_empty()
            || policy.eq_ignore_ascii_case("highest")
            || policy.eq_ignore_ascii_case("lowest")
            || policy.eq_ignore_ascii_case("adaptive")
            || policy.eq_ignore_ascii_case("multi-stream")
            || policy.eq_ignore_ascii_case("multistream")
            || policy
                .strip_prefix("rid:")
                .is_some_and(|rid| !rid.trim().is_empty());
        if !is_valid {
            return Err(format!(
                "simulcast_default_policy must be one of: highest, lowest, adaptive, rid:<name> (got {policy:?})"
            ));
        }
        // Validate ice_transport_policy
        parse_ice_transport_policy(&self.ice_transport_policy)?;
        self.ome_default_transport_mode()
            .map_err(|err| format!("invalid ome_default_transport: {err}"))?;
        validate_ome_ice_servers(&self.ome_ice_servers)?;
        if let Some(listen) = &self.ome_ws_listen {
            let trimmed = listen.trim();
            if trimmed.is_empty() {
                return Err("ome_ws_listen must not be empty when set".into());
            }
            let _: std::net::SocketAddr = trimmed
                .parse()
                .map_err(|e| format!("invalid ome_ws_listen {trimmed}: {e}"))?;
            if self.ome_ws_max_connections == 0 {
                return Err("ome_ws_max_connections must be > 0 when ome_ws_listen is set".into());
            }
            if self.ome_ws_handshake_timeout_ms == 0 {
                return Err(
                    "ome_ws_handshake_timeout_ms must be > 0 when ome_ws_listen is set".into(),
                );
            }
        }
        // Validate public_webrtc_base_url: when set, it must start with
        // an explicit http:// or https:// scheme. We do NOT infer the
        // scheme from port parity (ABL behaviour we intentionally avoid).
        if let Some(ref base) = self.public_webrtc_base_url {
            let trimmed = base.trim();
            if trimmed.is_empty() {
                return Err(
                    "public_webrtc_base_url must not be empty when set; remove the key or provide a valid URL".into()
                );
            }
            if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
                return Err(format!(
                    "public_webrtc_base_url must start with http:// or https:// (got {trimmed:?}); \
                     the scheme is never inferred from port number"
                ));
            }
        }
        Ok(())
    }

    /// `simulcast_policy` function of `WebRtcModuleConfig`.
    /// `WebRtcModuleConfig` 的 `simulcast_policy` 函数。
    pub fn simulcast_policy(&self) -> SimulcastPolicy {
        SimulcastPolicy::parse(&self.simulcast_default_policy)
    }

    /// Returns the configured audio output strategy.
    pub fn audio_strategy(&self) -> AudioOutputStrategy {
        self.audio_output_strategy
    }

    /// `ome_default_transport_mode` function of `WebRtcModuleConfig`.
    /// `WebRtcModuleConfig` 的 `ome_default_transport_mode` 函数。
    pub fn ome_default_transport_mode(&self) -> Result<OmeTransportMode, String> {
        parse_ome_transport_mode(&self.ome_default_transport).map_err(|err| err.to_string())
    }

    /// Effective playout target delay in milliseconds used by the
    /// play subscriber's smoothing path.
    pub fn effective_playout_delay_ms(&self) -> u64 {
        let mut delay = self
            .play_jitter_buffer_ms
            .max(self.playout_delay_min_ms as u64);
        if self.playout_delay_max_ms != 0 {
            delay = delay.min(self.playout_delay_max_ms as u64);
        }
        delay
    }

    /// Convert the configured BWE thresholds into a tuple suitable
    /// for the bridge's adaptive simulcast logic. `(low_bps, high_bps)`
    /// where `0` on either side means "no bound".
    pub fn bwe_thresholds_bps(&self) -> (u64, u64) {
        (
            self.bwe_low_threshold_kbps.saturating_mul(1_000),
            self.bwe_high_threshold_kbps.saturating_mul(1_000),
        )
    }

    /// Converts to `driver config` representation.
    /// 转换为 `driver config` 表示。
    pub fn to_driver_config(&self) -> Result<WebRtcDriverConfig, String> {
        self.validate()?;
        let listen_udp = self
            .listen_udp
            .parse()
            .map_err(|e| format!("invalid listen_udp: {e}"))?;
        let listen_tcp = self
            .listen_tcp
            .as_deref()
            .filter(|s| !s.is_empty() && self.enable_tcp)
            .map(|s| s.parse::<std::net::SocketAddr>())
            .transpose()
            .map_err(|e| format!("invalid listen_tcp: {e}"))?;
        let public_ips = self
            .public_ips
            .iter()
            .map(|s| {
                s.parse::<IpAddr>()
                    .map_err(|e| format!("invalid public_ip {s}: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let core = WebRtcCoreConfig {
            ice_lite: self.ice_lite,
            ice_transport_policy: parse_ice_transport_policy(&self.ice_transport_policy)?,
            codec_profile: self.codec_profile.into(),
            enable_bwe: self.enable_bwe,
            bwe_initial_bitrate_bps: Some(self.bwe_initial_bitrate_kbps.saturating_mul(1_000)),
            enable_simulcast: self.enable_simulcast,
            rtx_cache_packets: self.rtx_cache_packets,
            rtx_cache_age_ms: self.rtx_cache_age_ms,
            rtx_ratio_cap: Some(0.15),
            video_reorder_packets: self.video_reorder_packets,
            audio_reorder_packets: self.audio_reorder_packets,
            enable_rtp_mode: false,
            limits: WebRtcCoreLimits {
                max_sessions: self.max_sessions,
                max_data_channel_message_bytes: self.datachannel_max_message_bytes,
                ..Default::default()
            },
        };

        Ok(WebRtcDriverConfig {
            listen_udp,
            udp_port_range: match (self.udp_port_min, self.udp_port_max) {
                (Some(min), Some(max)) => Some(UdpPortRange { min, max }),
                _ => None,
            },
            listen_tcp,
            public_ips,
            candidate_hostname: self.candidate_hostname.clone(),
            max_sessions: self.max_sessions,
            read_buffer_size: self.read_buffer_size,
            tcp_read_chunk_size: 16_384,
            tcp_frame_max_bytes: 65_535,
            tcp_idle_timeout_ms: self.tcp_idle_timeout_ms,
            write_queue_capacity: self.write_queue_capacity,
            event_queue_capacity: self.event_queue_capacity,
            command_queue_capacity: 256,
            session_idle_timeout_ms: self.session_idle_timeout_ms,
            handshake_timeout_ms: self.handshake_timeout_ms,
            migration_route_ttl_ms: self.migration_route_ttl_ms,
            // Phase 02 follow-up: thread the shard count through to
            // the driver. Default `shard_count = 0` selects auto.
            driver_shards: self.shard_count,
            shard_command_capacity: 256,
            // Conservative defaults; the directory is bounded by
            // `max_sessions * 2` so a peer that migrates once doesn't
            // overflow it. Stale cap follows the same shape.
            route_directory_capacity: self.max_sessions.saturating_mul(2).max(1024),
            route_directory_stale_capacity: self.max_sessions.max(512),
            // Phase 02 follow-up R9: leave shard auto-restart off
            // by default — operators opt in via the driver config.
            // The module config doesn't expose these knobs directly
            // because the auto-eviction policy is operationally
            // sensitive and best chosen at the driver level.
            shard_restart_on_panic: false,
            shard_max_restart_count: 3,
            shard_restart_backoff_ms: 250,
            shard_max_restart_backoff_ms: 30_000,
            core,
        })
    }
}

fn default_enabled() -> bool {
    true
}

fn default_true() -> bool {
    true
}

fn default_listen_udp() -> String {
    "0.0.0.0:8000".to_string()
}

fn default_max_sessions() -> usize {
    4096
}

fn default_shard_count() -> usize {
    0
}

fn default_read_buffer() -> usize {
    65_536
}

fn default_write_queue() -> usize {
    512
}

fn default_event_queue() -> usize {
    1024
}

fn default_session_idle_timeout_ms() -> u64 {
    30_000
}

fn default_tcp_idle_timeout_ms() -> u64 {
    30_000
}

fn default_handshake_timeout_ms() -> u64 {
    10_000
}

fn default_migration_route_ttl_ms() -> u64 {
    30_000
}

fn default_prefer_video_codec() -> String {
    "h264".into()
}

fn default_prefer_audio_codec() -> String {
    "opus".into()
}

fn default_bwe_initial() -> u64 {
    1_200
}

fn default_bwe_low_threshold() -> u64 {
    // 600 kbps mirrors the SMS / ZLM "low quality" threshold.
    600
}

fn default_bwe_high_threshold() -> u64 {
    // 1800 kbps mirrors the SMS / ZLM "high quality" threshold.
    1_800
}

fn default_rtx_cache_packets() -> usize {
    1024
}

fn default_rtx_cache_age_ms() -> u64 {
    3_000
}

fn default_video_reorder() -> usize {
    30
}

fn default_audio_reorder() -> usize {
    10
}

fn default_bootstrap_frames() -> usize {
    150
}

fn default_bootstrap_max_age_ms() -> u64 {
    5_000
}

fn default_wait_stream_ms() -> u64 {
    3_000
}

fn default_datachannel_max_message_bytes() -> usize {
    256 * 1024
}

fn default_simulcast_policy() -> String {
    "highest".into()
}

fn default_play_disconnect_min_duration_ms() -> u64 {
    8_000
}

fn default_ice_transport_policy() -> String {
    "all".into()
}

fn default_ome_default_transport() -> String {
    "udptcp".into()
}

fn default_ome_ws_max_connections() -> usize {
    1024
}

fn default_ome_ws_handshake_timeout_ms() -> u64 {
    10_000
}

fn validate_ome_ice_servers(servers: &[WebRtcIceServerConfig]) -> Result<(), String> {
    for (server_idx, server) in servers.iter().enumerate() {
        if server.urls.is_empty() {
            return Err(format!(
                "ome_ice_servers[{server_idx}].urls must contain at least one stun/turn URL"
            ));
        }
        for (url_idx, url) in server.urls.iter().enumerate() {
            let trimmed = url.trim();
            if trimmed.is_empty() {
                return Err(format!(
                    "ome_ice_servers[{server_idx}].urls[{url_idx}] must not be empty"
                ));
            }
            if trimmed != url {
                return Err(format!(
                    "ome_ice_servers[{server_idx}].urls[{url_idx}] must not contain leading or trailing whitespace"
                ));
            }
            let lower = trimmed.to_ascii_lowercase();
            if !lower.starts_with("stun:")
                && !lower.starts_with("stuns:")
                && !lower.starts_with("turn:")
                && !lower.starts_with("turns:")
            {
                return Err(format!(
                    "ome_ice_servers[{server_idx}].urls[{url_idx}] must start with stun:, stuns:, turn:, or turns:"
                ));
            }
        }
    }
    Ok(())
}

/// Parse an `ice_transport_policy` string into `WebRtcIceTransportPolicy`.
/// Accepts `all`, `relay-only`, `relay_only`, `relayonly`, `p2p-only`,
/// `p2p_only`, `p2ponly` (case-insensitive).
fn parse_ice_transport_policy(input: &str) -> Result<WebRtcIceTransportPolicy, String> {
    let s = input.trim().to_ascii_lowercase();
    match s.as_str() {
        "" | "all" => Ok(WebRtcIceTransportPolicy::All),
        "relay-only" | "relay_only" | "relayonly" => Ok(WebRtcIceTransportPolicy::RelayOnly),
        "p2p-only" | "p2p_only" | "p2ponly" => Ok(WebRtcIceTransportPolicy::P2pOnly),
        other => Err(format!(
            "invalid ice_transport_policy {other:?}; expected one of: all, relay-only, p2p-only"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_validates() {
        let cfg = WebRtcModuleConfig::default();
        cfg.validate().expect("default config must validate");
    }

    #[test]
    fn ome_default_transport_defaults_to_udptcp() {
        let cfg = WebRtcModuleConfig::default();
        assert_eq!(
            cfg.ome_default_transport_mode().unwrap(),
            OmeTransportMode::UdpTcp
        );
        assert!(!cfg.ome_tcp_relay_force);
    }

    #[test]
    fn ome_default_transport_parses_known_values() {
        let cfg = WebRtcModuleConfig {
            ome_default_transport: "relay".into(),
            ..Default::default()
        };
        assert_eq!(
            cfg.ome_default_transport_mode().unwrap(),
            OmeTransportMode::Relay
        );
    }

    #[test]
    fn validate_rejects_invalid_ome_default_transport() {
        let cfg = WebRtcModuleConfig {
            ome_default_transport: "sideways".into(),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("ome_default_transport"));
    }

    #[test]
    fn validate_rejects_empty_ome_ice_server_urls() {
        let cfg = WebRtcModuleConfig {
            ome_ice_servers: vec![WebRtcIceServerConfig {
                urls: Vec::new(),
                username: Some("ome".into()),
                credential: Some("airen".into()),
            }],
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("ome_ice_servers"));
    }

    #[test]
    fn validate_rejects_invalid_ome_ice_server_scheme() {
        let cfg = WebRtcModuleConfig {
            ome_ice_servers: vec![WebRtcIceServerConfig {
                urls: vec!["http://relay.example.com".into()],
                username: None,
                credential: None,
            }],
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("stun") && err.contains("turn"));
    }

    #[test]
    fn validate_accepts_case_insensitive_ome_ice_server_scheme() {
        let cfg = WebRtcModuleConfig {
            ome_ice_servers: vec![WebRtcIceServerConfig {
                urls: vec![
                    "STUN:stun.example.com:3478".into(),
                    "TurnS:relay.example.com:5349?transport=tcp".into(),
                ],
                username: Some("ome".into()),
                credential: Some("airen".into()),
            }],
            ..Default::default()
        };
        cfg.validate()
            .expect("ICE server URI schemes are case-insensitive");
    }

    #[test]
    fn validate_rejects_ome_ice_server_url_with_surrounding_whitespace() {
        let cfg = WebRtcModuleConfig {
            ome_ice_servers: vec![WebRtcIceServerConfig {
                urls: vec![" turn:relay.example.com:3478 ".into()],
                username: None,
                credential: None,
            }],
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("leading or trailing whitespace"));
    }

    #[test]
    fn ome_ws_listen_defaults_to_disabled() {
        let cfg = WebRtcModuleConfig::default();
        assert!(cfg.ome_ws_listen.is_none());
        assert_eq!(cfg.ome_ws_max_connections, 1024);
        assert_eq!(cfg.ome_ws_handshake_timeout_ms, 10_000);
    }

    #[test]
    fn validate_rejects_invalid_ome_ws_listen() {
        let cfg = WebRtcModuleConfig {
            ome_ws_listen: Some("not an addr".into()),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("ome_ws_listen"));
    }

    #[test]
    fn validate_rejects_zero_ome_ws_limits_when_enabled() {
        let cfg = WebRtcModuleConfig {
            ome_ws_listen: Some("127.0.0.1:18080".into()),
            ome_ws_max_connections: 0,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("ome_ws_max_connections"));

        let cfg = WebRtcModuleConfig {
            ome_ws_listen: Some("127.0.0.1:18080".into()),
            ome_ws_handshake_timeout_ms: 0,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("ome_ws_handshake_timeout_ms"));
    }

    #[test]
    fn validate_accepts_ome_ws_listen_when_enabled() {
        let cfg = WebRtcModuleConfig {
            ome_ws_listen: Some("127.0.0.1:18080".into()),
            ome_ws_max_connections: 32,
            ome_ws_handshake_timeout_ms: 5000,
            ..Default::default()
        };
        cfg.validate().expect("valid OME WS config");
    }

    #[test]
    fn rejects_invalid_listen_udp() {
        let cfg = WebRtcModuleConfig {
            listen_udp: "not an addr".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_zero_read_buffer_size() {
        let cfg = WebRtcModuleConfig {
            read_buffer_size: 0,
            ..Default::default()
        };
        let err = cfg.validate().expect_err("zero buffer size must fail");
        assert!(err.contains("read_buffer_size"));
    }

    #[test]
    fn rejects_zero_datachannel_max_message_bytes() {
        // Phase 05 follow-up: a zero cap is meaningless because every
        // payload would be rejected. Surface it in config validation.
        let cfg = WebRtcModuleConfig {
            datachannel_max_message_bytes: 0,
            ..Default::default()
        };
        let err = cfg
            .validate()
            .expect_err("zero datachannel_max_message_bytes must fail");
        assert!(err.contains("datachannel_max_message_bytes"));
    }

    #[test]
    fn rtcp_based_timestamp_defaults_to_fast_start() {
        let cfg = WebRtcModuleConfig::default();
        assert!(!cfg.rtcp_based_timestamp);
    }

    #[test]
    fn rtcp_based_timestamp_deserializes_from_json() {
        let cfg: WebRtcModuleConfig = serde_json::from_value(serde_json::json!({
            "listen_udp": "127.0.0.1:18000",
            "rtcp_based_timestamp": true
        }))
        .unwrap();
        assert!(cfg.rtcp_based_timestamp);
        cfg.validate().expect("rtcp_based_timestamp config");
    }

    #[test]
    fn auto_abr_defaults_to_enabled() {
        let cfg = WebRtcModuleConfig::default();
        assert!(cfg.webrtc_auto_abr);
    }

    #[test]
    fn playout_delay_validation_rejects_inverted_range() {
        let cfg = WebRtcModuleConfig {
            playout_delay_min_ms: 300,
            playout_delay_max_ms: 100,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("playout_delay_min_ms"));
    }

    #[test]
    fn effective_playout_delay_uses_jitter_and_bounds() {
        let cfg = WebRtcModuleConfig {
            play_jitter_buffer_ms: 120,
            playout_delay_min_ms: 80,
            playout_delay_max_ms: 100,
            ..Default::default()
        };
        assert_eq!(cfg.effective_playout_delay_ms(), 100);
    }

    #[test]
    fn datachannel_max_message_bytes_propagates_to_driver_config() {
        let cfg = WebRtcModuleConfig {
            datachannel_max_message_bytes: 4_096,
            ..Default::default()
        };
        let driver = cfg.to_driver_config().expect("driver config");
        assert_eq!(driver.core.limits.max_data_channel_message_bytes, 4_096);
    }

    #[test]
    fn driver_config_round_trips() {
        let cfg = WebRtcModuleConfig {
            listen_udp: "127.0.0.1:18000".into(),
            ..Default::default()
        };
        let driver = cfg.to_driver_config().expect("driver config");
        assert_eq!(driver.listen_udp.port(), 18000);
        assert!(driver.listen_tcp.is_none());
    }

    #[test]
    fn simulcast_policy_parses_known_values() {
        assert_eq!(SimulcastPolicy::parse("highest"), SimulcastPolicy::Highest);
        assert_eq!(SimulcastPolicy::parse("Lowest"), SimulcastPolicy::Lowest);
        assert_eq!(
            SimulcastPolicy::parse("adaptive"),
            SimulcastPolicy::Adaptive
        );
        assert_eq!(
            SimulcastPolicy::parse("Adaptive"),
            SimulcastPolicy::Adaptive
        );
        assert_eq!(
            SimulcastPolicy::parse("multi-stream"),
            SimulcastPolicy::MultiStream
        );
        assert_eq!(
            SimulcastPolicy::parse("multistream"),
            SimulcastPolicy::MultiStream
        );
        assert_eq!(
            SimulcastPolicy::parse("Multi-Stream"),
            SimulcastPolicy::MultiStream
        );
        assert_eq!(
            SimulcastPolicy::parse("rid:high"),
            SimulcastPolicy::Rid("high".into())
        );
        assert_eq!(SimulcastPolicy::parse(""), SimulcastPolicy::Highest);
        // Unknown values fall back to the default rather than panic.
        assert_eq!(SimulcastPolicy::parse("nonsense"), SimulcastPolicy::Highest);
    }

    #[test]
    fn rejects_malformed_simulcast_policy_in_validate() {
        let cfg = WebRtcModuleConfig {
            simulcast_default_policy: "foobar".into(),
            ..Default::default()
        };
        let err = cfg.validate().expect_err("should reject malformed policy");
        assert!(err.contains("simulcast_default_policy"));
    }

    #[test]
    fn accepts_adaptive_simulcast_policy_in_validate() {
        let cfg = WebRtcModuleConfig {
            simulcast_default_policy: "adaptive".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.simulcast_policy(), SimulcastPolicy::Adaptive);
    }

    #[test]
    fn accepts_multi_stream_simulcast_policy_in_validate() {
        let cfg = WebRtcModuleConfig {
            simulcast_default_policy: "multi-stream".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.simulcast_policy(), SimulcastPolicy::MultiStream);

        let cfg2 = WebRtcModuleConfig {
            simulcast_default_policy: "multistream".into(),
            ..Default::default()
        };
        assert!(cfg2.validate().is_ok());
        assert_eq!(cfg2.simulcast_policy(), SimulcastPolicy::MultiStream);
    }

    #[test]
    fn parses_ice_transport_policy_known_values() {
        assert_eq!(
            super::parse_ice_transport_policy("all").unwrap(),
            WebRtcIceTransportPolicy::All
        );
        assert_eq!(
            super::parse_ice_transport_policy("").unwrap(),
            WebRtcIceTransportPolicy::All
        );
        assert_eq!(
            super::parse_ice_transport_policy("ALL").unwrap(),
            WebRtcIceTransportPolicy::All
        );
        assert_eq!(
            super::parse_ice_transport_policy("relay-only").unwrap(),
            WebRtcIceTransportPolicy::RelayOnly
        );
        assert_eq!(
            super::parse_ice_transport_policy("relay_only").unwrap(),
            WebRtcIceTransportPolicy::RelayOnly
        );
        assert_eq!(
            super::parse_ice_transport_policy("relayonly").unwrap(),
            WebRtcIceTransportPolicy::RelayOnly
        );
        assert_eq!(
            super::parse_ice_transport_policy("p2p-only").unwrap(),
            WebRtcIceTransportPolicy::P2pOnly
        );
        assert_eq!(
            super::parse_ice_transport_policy("p2p_only").unwrap(),
            WebRtcIceTransportPolicy::P2pOnly
        );
        assert_eq!(
            super::parse_ice_transport_policy("p2ponly").unwrap(),
            WebRtcIceTransportPolicy::P2pOnly
        );
    }

    #[test]
    fn rejects_unknown_ice_transport_policy() {
        let err = super::parse_ice_transport_policy("nonsense").unwrap_err();
        assert!(err.contains("ice_transport_policy"));
    }

    #[test]
    fn validate_rejects_invalid_ice_transport_policy() {
        let cfg = WebRtcModuleConfig {
            ice_transport_policy: "bogus".into(),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("ice_transport_policy"));
    }

    #[test]
    fn ice_transport_policy_propagates_to_driver_config() {
        let cfg = WebRtcModuleConfig {
            ice_transport_policy: "relay-only".into(),
            ..Default::default()
        };
        let driver_cfg = cfg.to_driver_config().unwrap();
        assert_eq!(
            driver_cfg.core.ice_transport_policy,
            WebRtcIceTransportPolicy::RelayOnly
        );
    }

    #[test]
    fn rejects_empty_rid_in_simulcast_policy() {
        // `rid:` without a layer name is meaningless; the runtime
        // would silently route nothing.
        let cfg = WebRtcModuleConfig {
            simulcast_default_policy: "rid:".into(),
            ..Default::default()
        };
        let err = cfg.validate().expect_err("empty `rid:` should be rejected");
        assert!(err.contains("simulcast_default_policy"));

        let cfg = WebRtcModuleConfig {
            simulcast_default_policy: "rid:   ".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err(), "whitespace-only rid is invalid");
    }

    #[test]
    fn public_webrtc_base_url_defaults_to_none() {
        let cfg = WebRtcModuleConfig::default();
        assert!(cfg.public_webrtc_base_url.is_none());
    }

    #[test]
    fn public_webrtc_base_url_deserializes_from_json() {
        let json = serde_json::json!({
            "listen_udp": "0.0.0.0:8000",
            "public_webrtc_base_url": "http://cdn.example.com:8080/api/v1/rtc"
        });
        let cfg: WebRtcModuleConfig = serde_json::from_value(json).unwrap();
        assert_eq!(
            cfg.public_webrtc_base_url.as_deref(),
            Some("http://cdn.example.com:8080/api/v1/rtc")
        );
    }

    #[test]
    fn public_webrtc_base_url_rejects_missing_scheme() {
        let cfg = WebRtcModuleConfig {
            public_webrtc_base_url: Some("cdn.example.com:8080/api/v1/rtc".into()),
            ..Default::default()
        };
        let err = cfg.validate().expect_err("must reject URL without scheme");
        assert!(
            err.contains("http://") || err.contains("https://"),
            "error should mention required schemes: {err}"
        );
    }

    #[test]
    fn public_webrtc_base_url_rejects_empty_string() {
        let cfg = WebRtcModuleConfig {
            public_webrtc_base_url: Some("".into()),
            ..Default::default()
        };
        let err = cfg.validate().expect_err("must reject empty base URL");
        assert!(err.contains("empty"));
    }

    #[test]
    fn public_webrtc_base_url_accepts_https_on_any_port() {
        // Proves we do NOT use port parity — HTTPS on even port is valid.
        let cfg = WebRtcModuleConfig {
            public_webrtc_base_url: Some("https://cdn.example.com:8080/api/v1/rtc".into()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn public_webrtc_base_url_accepts_http_on_any_port() {
        // Proves we do NOT use port parity — HTTP on odd port is valid.
        let cfg = WebRtcModuleConfig {
            public_webrtc_base_url: Some("http://cdn.example.com:8443/api/v1/rtc".into()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }
}
