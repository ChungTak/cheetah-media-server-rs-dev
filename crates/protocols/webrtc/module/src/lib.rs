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

/// `bootstrap` module.
/// `bootstrap` 模块.
pub mod bootstrap;
/// `bridge` module.
/// `bridge` 模块.
pub mod bridge;
/// `codec_policy` module.
/// `codec_policy` 模块.
pub mod codec_policy;
/// `compat` module.
/// `compat` 模块.
pub mod compat;
/// `config` module.
/// `config` 模块.
pub mod config;
/// `http` module.
/// `http` 模块.
pub mod http;
/// `jobs` module.
/// `jobs` 模块.
pub mod jobs;
/// `metrics` module.
/// `metrics` 模块.
pub mod metrics;
/// `module` module.
/// `模块` 模块.
pub mod module;
/// `ome_signaling` module.
/// `ome_signaling` 模块.
pub mod ome_signaling;
/// `ome_ws` module.
/// `ome_ws` 模块.
pub mod ome_ws;
/// `p2p` module.
/// `p2p` 模块.
pub mod p2p;
/// `p2p_jobs` module.
/// `p2p_jobs` 模块.
pub mod p2p_jobs;
/// `play_disconnect` module.
/// `play_disconnect` 模块.
pub mod play_disconnect;
/// `session` module.
/// `session` 模块.
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
