//! Configuration values consumed by [`crate::WebRtcCore`].
//!
//! All fields here are pure data; the core never reads environment variables
//! or files. Driver and module layers populate the config from their
//! respective configuration models and pass it in.
//!
//! 本模块包含 [`crate::WebRtcCore`] 消费的配置值。
//!
//! 所有字段均为纯数据；核心不会读取环境变量或文件。驱动层与模块层
//! 从各自的配置模型填充并传入。

use serde::{Deserialize, Serialize};

use crate::types::WebRtcCodecProfile;

/// ICE transport policy filter applied at candidate gathering time.
///
/// Mirrors the W3C `RTCIceTransportPolicy` enum. The driver layer is
/// responsible for applying the filter when it adds local candidates;
/// the core stores the policy so observability surfaces report it.
///
/// ICE 传输策略过滤器，在收集 candidate 时应用。
///
/// 与 W3C `RTCIceTransportPolicy` 枚举对齐。驱动层在添加本地 candidate 时
/// 负责应用过滤；核心仅保存策略以便可观测性表面上报。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WebRtcIceTransportPolicy {
    /// All candidate types are gathered (default).
    ///
    /// 收集所有类型的 candidate（默认）。
    #[default]
    All,
    /// Only relay (TURN) candidates are gathered.
    ///
    /// 仅收集 relay（TURN）candidate。
    RelayOnly,
    /// Only host + reflexive candidates are gathered (no TURN).
    ///
    /// 仅收集 host + reflexive candidate（无 TURN）。
    P2pOnly,
}

/// Bounds on the WebRTC core to keep all caches and queues finite.
///
/// These limits are checked synchronously inside the core to avoid relying on
/// driver-layer backpressure alone. Exceeding a limit produces a structured
/// error or a diagnostic rather than unbounded growth.
///
/// WebRTC 核心的边界，使所有缓存与队列保持有限。
///
/// 这些限制在核心内部同步检查，避免单独依赖驱动层背压。超出限制会
/// 产生结构化错误或诊断，而非无限制增长。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebRtcCoreLimits {
    /// Maximum number of concurrent sessions the core will accept.
    ///
    /// 核心将接受的最大并发会话数。
    pub max_sessions: usize,
    /// Maximum number of pending output items per session that we buffer
    /// internally between `pump_output` calls.
    ///
    /// 在 `pump_output` 调用之间，我们内部为每个会话缓冲的最大待输出项数。
    pub max_pending_outputs_per_session: usize,
    /// Maximum allowed remote SDP size in bytes.
    ///
    /// Rejects requests larger than this with [`crate::WebRtcCoreError::SdpTooLarge`]
    /// instead of leaving the bound to upstream HTTP layers.
    ///
    /// 允许的最大远端 SDP 字节数。
    ///
    /// 超过该值会通过 [`crate::WebRtcCoreError::SdpTooLarge`] 拒绝，
    /// 而不是让上游 HTTP 层负责限制。
    pub max_remote_sdp_bytes: usize,
    /// Maximum number of remote ICE candidates accepted per session.
    ///
    /// 每个会话最多接受的远端 ICE candidate 数。
    pub max_remote_candidates_per_session: usize,
    /// Maximum DataChannel message size in bytes the core will accept
    /// from the boundary `WebRtcCoreCommand::SendDataChannel`. Larger
    /// payloads emit a [`crate::WebRtcCoreDiagnosticKind::PendingOutputDropped`]
    /// diagnostic and are silently dropped instead of overflowing
    /// `str0m`'s SCTP buffer or crashing the peer.
    ///
    /// ZLM clamps to 256 KiB by default; we follow suit.
    ///
    /// 核心从边界 `WebRtcCoreCommand::SendDataChannel` 接受的最大 DataChannel
    /// 消息字节数。过大的负载会发出
    /// [`crate::WebRtcCoreDiagnosticKind::PendingOutputDropped`] 诊断并被静默丢弃，
    /// 避免溢出 `str0m` 的 SCTP 缓冲区或导致对端崩溃。
    ///
    /// ZLM 默认限制为 256 KiB；本处遵循同样限制。
    pub max_data_channel_message_bytes: usize,
}

impl Default for WebRtcCoreLimits {
    fn default() -> Self {
        Self {
            max_sessions: 4096,
            max_pending_outputs_per_session: 4096,
            max_remote_sdp_bytes: 64 * 1024,
            max_remote_candidates_per_session: 256,
            max_data_channel_message_bytes: 256 * 1024,
        }
    }
}

/// Static configuration applied when a [`crate::WebRtcCore`] is created.
///
/// Most fields tune the `str0m` session builder: ICE-lite mode, reordering
/// windows, BWE/RTX settings, and the codec profile. The core keeps this
/// config for the lifetime of the `WebRtcCore`; per-session overrides are
/// not supported in Phase 01.
///
/// 创建 [`crate::WebRtcCore`] 时应用的静态配置。
///
/// 大多数字段用于调整 `str0m` 会话构建器：ICE-lite 模式、重排窗口、
/// BWE/RTX 设置与编解码器配置。核心在 `WebRtcCore` 生命周期内保持该配置；
/// 阶段 01 不支持按会话覆盖。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebRtcCoreConfig {
    /// Whether to run as ICE-lite. Defaults to `false` to match SMS behaviour.
    ///
    /// 是否以 ICE-lite 模式运行。默认为 `false` 以匹配 SMS 行为。
    pub ice_lite: bool,
    /// ICE candidate policy filter. Mirrors the WebRTC W3C
    /// `RTCIceTransportPolicy` semantics:
    ///
    /// * `All`: announce all candidates (host + reflexive + relay).
    /// * `RelayOnly`: only announce relay (TURN) candidates.
    /// * `P2pOnly`: only announce host + reflexive (no relay).
    ///
    /// The actual filtering is performed by the driver layer when it
    /// adds local candidates to the core; the core stores the policy
    /// for diagnostic surfacing only.
    ///
    /// ICE candidate 策略过滤器。与 WebRTC W3C `RTCIceTransportPolicy` 语义对齐：
    ///
    /// - `All`：宣布所有 candidate（host + reflexive + relay）。
    /// - `RelayOnly`：仅宣布 relay（TURN）candidate。
    /// - `P2pOnly`：仅宣布 host + reflexive candidate（无 TURN）。
    ///
    /// 实际过滤由驱动层在添加本地 candidate 时执行；核心仅保存策略用于诊断展示。
    pub ice_transport_policy: WebRtcIceTransportPolicy,
    /// Negotiation profile for codec selection.
    ///
    /// It gates which codecs are enabled in the `str0m` codec config and
    /// whether RTP-mode passthrough is allowed.
    ///
    /// 编解码器选择协商配置。
    ///
    /// 它控制 `str0m` 编解码器配置中启用哪些编解码器，以及是否允许 RTP 模式透传。
    pub codec_profile: WebRtcCodecProfile,
    /// Enable Transport Wide Congestion Control / Bandwidth Estimation.
    ///
    /// 启用传输级拥塞控制 / 带宽估计。
    pub enable_bwe: bool,
    /// Initial estimate when BWE is enabled.
    ///
    /// BWE 启用时的初始估计值（比特每秒）。
    pub bwe_initial_bitrate_bps: Option<u64>,
    /// Whether the server allows simulcast media in offers/answers.
    ///
    /// 服务器是否允许在 offer/answer 中协商 simulcast 媒体。
    pub enable_simulcast: bool,
    /// Send-side RTX cache packet count.
    ///
    /// 发送端 RTX 缓存包数。
    pub rtx_cache_packets: usize,
    /// Send-side RTX cache age in milliseconds.
    ///
    /// 发送端 RTX 缓存包的老化时间（毫秒）。
    pub rtx_cache_age_ms: u64,
    /// Optional cap on RTX retransmission ratio (0..1].
    ///
    /// 可选的 RTX 重传比例上限（0..1]。
    pub rtx_ratio_cap: Option<f32>,
    /// Reorder window for video receive streams.
    ///
    /// 视频接收流的重排窗口。
    pub video_reorder_packets: usize,
    /// Reorder window for audio receive streams.
    ///
    /// 音频接收流的重排窗口。
    pub audio_reorder_packets: usize,
    /// Whether to expose RTP-mode I/O (raw RTP bypass of `str0m`'s
    /// packetizer/depacketizer). Phase 01 keeps this off by default.
    ///
    /// 是否暴露 RTP 模式 I/O（绕过 `str0m` 的包化/解包化）。
    /// 阶段 01 默认关闭。
    pub enable_rtp_mode: bool,
    /// Hard limits, see [`WebRtcCoreLimits`].
    ///
    /// 硬性限制，参见 [`WebRtcCoreLimits`]。
    pub limits: WebRtcCoreLimits,
}

impl Default for WebRtcCoreConfig {
    fn default() -> Self {
        Self {
            ice_lite: false,
            ice_transport_policy: WebRtcIceTransportPolicy::default(),
            codec_profile: WebRtcCodecProfile::Browser,
            enable_bwe: true,
            bwe_initial_bitrate_bps: Some(1_200_000),
            enable_simulcast: true,
            rtx_cache_packets: 1024,
            rtx_cache_age_ms: 3_000,
            rtx_ratio_cap: Some(0.15),
            video_reorder_packets: 30,
            audio_reorder_packets: 10,
            enable_rtp_mode: false,
            limits: WebRtcCoreLimits::default(),
        }
    }
}
