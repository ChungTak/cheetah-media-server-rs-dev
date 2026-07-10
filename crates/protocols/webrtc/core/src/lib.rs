//! `cheetah-webrtc-core` is the Sans-I/O WebRTC protocol surface for the project.
//!
//! It wraps [`str0m::Rtc`] sessions and exposes deterministic, runtime-neutral
//! input/output types so that the driver layer can drive WebRTC sessions
//! without leaking `tokio`, sockets, or system clock dependencies into the
//! state machine.
//!
//! `cheetah-webrtc-core` 是本项目的无 I/O（Sans-I/O）WebRTC 协议表面。
//!
//! 它包装 [`str0m::Rtc`] 会话，并暴露确定性的、运行时无关的输入/输出类型，
//! 使驱动层能够驱动 WebRTC 会话，而无需将 `tokio`、socket 或系统时钟依赖
//! 泄漏到状态机内部。
//!
//! Boundary invariants enforced by this crate:
//!
//! - No call to [`std::time::Instant::now`] from any state-machine method.
//! - No async fn, no spawned tasks, no internal channels.
//! - Time is provided externally as `u64` microseconds anchored at the
//!   `start_micros` value supplied to [`WebRtcCore::new`].
//! - Network packets are pure data; the driver layer is responsible for I/O.
//!
//! 本 crate 强制执行的边界不变式：
//!
//! - 任何状态机方法内部均不调用 [`std::time::Instant::now`]。
//! - 无 async fn、无 spawned 任务、无内部通道。
//! - 时间由外部以 `u64` 微秒形式提供，并锚定 [`WebRtcCore::new`] 时传入的
//!   `start_micros` 值。
//! - 网络包为纯数据；驱动层负责 I/O。
//!
//! Phase 01 scope: SDP offer/answer plumbing, ICE candidate ingestion, timer
//! and network packet pumping for one or more sessions, and a small bridge
//! between [`str0m::Event`] and [`event::WebRtcCoreEvent`] for downstream
//! phases. Media write paths, RTX/NACK/TWCC policy and DataChannel writes are
//! sketched as commands but only implement the safe subset the rest of the
//! pipeline currently consumes.
//!
//! 阶段 01 范围：SDP offer/answer 管线、ICE candidate 注入、单个或多个会话的
//! 定时器与网络包轮询，以及 [`str0m::Event`] 与 [`event::WebRtcCoreEvent`] 之间的
//! 小型桥接，供后续阶段使用。媒体写路径、RTX/NACK/TWCC 策略与 DataChannel 写入
//! 已以命令形式勾勒，但只实现当前管线其余部分安全消费的最小子集。

/// Configuration knobs for the core: limits, codec profile, ICE/BWE/RTX settings.
///
/// 核心配置项：限制、编解码器配置、ICE/BWE/RTX 设置。
pub mod config;

/// Error and diagnostic types surfaced by the core.
///
/// 核心暴露的错误与诊断类型。
pub mod error;

/// Events emitted by the core for the driver and module layers.
///
/// 核心向驱动层与模块层发出的事件。
pub mod event;

/// Input model and commands fed into the state machine.
///
/// 输入模型与喂入状态机的命令。
pub mod input;

/// Offer/answer payload type negotiation helpers.
///
/// Offer/answer 负载类型协商辅助。
pub mod offer_payload;

/// Output items and timers produced by the core.
///
/// 核心产生的输出项与定时器。
pub mod output;

/// SDP compatibility preprocessing for real-world browser/vendor stacks.
///
/// 面向真实浏览器/厂商实现的 SDP 兼容性预处理。
pub mod sdp_compat;

/// Multi-session `str0m` wrapper and the WebRTC SDP state machine.
///
/// 多会话 `str0m` 包装器与 WebRTC SDP 状态机。
pub mod session;

/// Stats records exposed at the boundary.
///
/// 边界处暴露的统计记录。
pub mod stats;

/// Stable boundary types for sessions, tracks, directions, and roles.
///
/// 会话、track、方向与角色的稳定边界类型。
pub mod types;

pub use config::{WebRtcCoreConfig, WebRtcCoreLimits, WebRtcIceTransportPolicy};
pub use error::{WebRtcCoreDiagnostic, WebRtcCoreError};
pub use event::{
    WebRtcCodecKind, WebRtcCoreEvent, WebRtcDataChannelEvent, WebRtcFrameMeta, WebRtcIceState,
    WebRtcMediaDirection, WebRtcMediaEvent, WebRtcMediaKind, WebRtcMediaTrack, WebRtcRtcpFeedback,
    WebRtcSessionLifecycle, WebRtcSimulcastLayerObservation, WebRtcSimulcastRidSource,
};
pub use input::{
    WebRtcCloseReason, WebRtcCoreCommand, WebRtcCoreInput, WebRtcDataChannelOut,
    WebRtcNetworkInput, WebRtcOfferDirection, WebRtcOfferSpec, WebRtcRequestKeyframeKind,
    WebRtcSendFrame,
};
pub use offer_payload::{extract_offer_payloads, OfferCodec, OfferPayloads, PayloadNotFound};
pub use output::{WebRtcCoreOutput, WebRtcLocalDescriptionKind, WebRtcPacketOut, WebRtcTimer};
pub use sdp_compat::{
    extract_rtp_extension_mappings, inject_rid_from_ssrc_group_sim, preprocess_remote_sdp,
    RtpExtensionMapping, RtpExtensionType, SdpCompatReport,
};
pub use session::WebRtcCore;
pub use stats::{WebRtcBweStats, WebRtcSessionStats};
pub use types::{
    DataChannelId, MidLabel, WebRtcCodecProfile, WebRtcDirection, WebRtcIceRole, WebRtcSessionId,
    WebRtcSessionRole, WebRtcSessionState,
};
