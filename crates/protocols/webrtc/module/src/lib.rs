//! `cheetah-webrtc-module` exposes WebRTC publish/play/WHIP/WHEP APIs to
//! the engine.
//!
//! Phase 03 (this crate's first real implementation) covers:
//!
//! * SDK module factory and lifecycle (`init`, `start`, `stop`, `apply_config`).
//! * SMS-style `/api/v1/rtc/play` and `/api/v1/rtc/publish` HTTP routes.
//! * WHIP/WHEP routes returning `201 Created`, `Content-Type: application/sdp`,
//!   and a `Location` header pointing to `/api/v1/rtc/session/{id}`.
//! * Session DELETE / GET endpoints for lifecycle management.
//! * Driver bridge that translates HTTP requests into
//!   [`cheetah_webrtc_driver_tokio::WebRtcDriverCommand`] values.
//!
//! Phase 04+ wire engine publishers/subscribers, GOP bootstrap, simulcast
//! and RTX policy on top of the lifecycle plumbing landed here.

/// Module for `bootstrap`.
/// `bootstrap` 相关模块。
pub mod bootstrap;
/// Module for `bridge`.
/// `bridge` 相关模块。
pub mod bridge;
/// Module for `codec_policy`.
/// `codec_policy` 相关模块。
pub mod codec_policy;
/// Module for `compat`.
/// `compat` 相关模块。
pub mod compat;
/// Module for `config`.
/// `config` 相关模块。
pub mod config;
/// Module for `http`.
/// `http` 相关模块。
pub mod http;
/// Module for `jobs`.
/// `jobs` 相关模块。
pub mod jobs;
/// Module for `metrics`.
/// `metrics` 相关模块。
pub mod metrics;
/// Module for `module`.
/// `module` 相关模块。
pub mod module;
/// Module for `ome_signaling`.
/// `ome_signaling` 相关模块。
pub mod ome_signaling;
/// Module for `ome_ws`.
/// `ome_ws` 相关模块。
pub mod ome_ws;
/// Module for `p2p`.
/// `p2p` 相关模块。
pub mod p2p;
/// Module for `p2p_jobs`.
/// `p2p_jobs` 相关模块。
pub mod p2p_jobs;
/// Module for `play_disconnect`.
/// `play_disconnect` 相关模块。
pub mod play_disconnect;
/// Module for `session`.
/// `session` 相关模块。
pub mod session;

/// Re-export of the WHIP/WHEP HTTP client, which now lives in the
/// driver layer (`cheetah-webrtc-driver-tokio`) since it owns TCP/TLS
/// I/O. Kept at `crate::http_client` so callers and fuzz harnesses that
/// reference `cheetah_webrtc_module::http_client` keep resolving.
pub use cheetah_webrtc_driver_tokio::http_client;

pub use bridge::WebRtcPlayBootstrapStats;
pub use compat::{
    extract_trickle_candidates, extract_trickle_ice_restart_creds, parse_zlm_rtc_url, ZlmRtcScheme,
    ZlmRtcUrl, ZlmRtcUrlError,
};
pub use config::{SimulcastPolicy, WebRtcModuleConfig};
pub use metrics::{WebRtcModuleMetrics, WebRtcModuleMetricsSnapshot, WebRtcSessionStatsDelta};
pub use module::{WebRtcModule, WebRtcModuleFactory};
pub use play_disconnect::{
    evaluate_play_disconnect, NetworkType, PlayDisconnectOutcome, PlayDisconnectReason,
    WebRtcPlayDisconnectEvent,
};
pub use session::WebRtcSessionTelemetry;
