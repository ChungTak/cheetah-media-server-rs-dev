//! Typed RTP session API and request/result types for GB28181/RTP media data plane.
//!
//! `RtpSessionApi` is the runtime-neutral, framework-neutral contract used by the
//! GB28181 module (and adapters) to open, update, query and stop RTP sessions.
//! It does not expose sockets, Tokio handles or raw signaling payloads.
//!
//! RtpSessionApi 是 GB28181/RTP 媒体数据面的运行时无关、框架无关契约。
//! GB28181 module（以及 adapter）用它打开、更新、查询和停止 RTP 会话。
//! 它不暴露 socket、Tokio handle 或原始信令 payload。

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::error::EffectOutcome;
use crate::fencing::ControlledResourceRef;
use crate::ids::{MediaKey, RtpSessionId};

/// Monotonic generation of an RTP session.
///
/// RTP 会话的单调 generation。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RtpSessionGeneration(pub u64);

impl Default for RtpSessionGeneration {
    fn default() -> Self {
        Self(1)
    }
}

/// A stable reference to a controlled RTP session resource.
///
/// `resource_ref` is nested rather than flattened because both this struct and
/// `ControlledResourceRef` carry a `generation` field; flattening would create a
/// duplicate JSON key and break serde round-trips.
///
/// 受控 RTP 会话资源的稳定引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtpSessionResourceRef {
    pub session_id: RtpSessionId,
    pub generation: RtpSessionGeneration,
    pub resource_ref: ControlledResourceRef,
}

/// RTP transport selection.
///
/// RTP 传输选择。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RtpTransport {
    #[default]
    Udp,
    Tcp,
}

/// TCP role for an RTP session. Only meaningful when transport is `Tcp`.
///
/// TCP 角色。仅在 transport 为 `Tcp` 时有效。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TcpRole {
    /// The local endpoint acts as the active connector.
    Active,
    /// The local endpoint listens passively for an incoming connection.
    #[default]
    Passive,
}

/// RTP framing used over a TCP transport.
///
/// TCP 传输上的 RTP 分帧方式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RtpFraming {
    /// Single RTP/RTCP datagrams (UDP or TCP emulation).
    Datagram,
    /// RFC 4571 2-byte length prefix.
    #[default]
    Rfc4571,
    /// RTSP-style `$` 4-byte interleaved framing (`$` + channel + length).
    DollarPrefixed,
    /// Try to detect framing automatically from the first bytes of the stream.
    AutoDetect,
}

/// Media container carried inside RTP.
///
/// RTP 承载的媒体容器。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MediaContainer {
    /// MPEG Program Stream (GB28181 default).
    #[default]
    Ps,
    /// MPEG Transport Stream.
    Ts,
    /// Elementary Stream (no PSM).
    ElementaryStream,
    /// Probe a bounded prefix before deciding.
    AutoDetect,
}

/// RTP session media direction.
///
/// RTP 会话媒体方向。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RtpDirection {
    /// Receive only (passive ingest or active pull).
    #[default]
    Receive,
    /// Send only (active or passive egress).
    Send,
    /// Bi-directional voice talk.
    DuplexTalk,
}

/// SSRC / source binding policy.
///
/// SSRC / 来源绑定策略。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SourceBindingPolicy {
    /// Only accept packets whose SSRC matches the negotiated value.
    #[default]
    Strict,
    /// Allow a one-time validated rebind when the first SSRC changes.
    AllowValidatedRebind,
}

/// GB28181 media compatibility profile.
///
/// See `dev-docs/plans-29-gb28181-impove/capability_matrix.md` for the matrix.
///
/// GB28181 媒体兼容 profile。能力矩阵见 `capability_matrix.md`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GbMediaCompatibilityProfile {
    /// Strict spec compliance; no SSRC fallback or auto-detection.
    Strict,
    /// Standard GB28181 plus verified device tolerance.
    #[default]
    GbCommon,
    /// ZLMediaKit-compatible wire behavior (2/4-byte framing, SSRC fallback).
    Zlm,
    /// simple-media-server-compatible media parameter normalization.
    Sms,
    /// ABLMediaServer-compatible framing/PT/JTT behavior.
    Abl,
    /// Hikvision Ehome2 framing.
    HikvisionEhome,
    /// JT/T 1078 SIM/channel/header rules.
    Jtt1078,
}

/// RTP payload binding for a track.
///
/// RTP 轨道 payload 绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtpPayloadBinding {
    /// RTP payload type.
    pub payload_type: u8,
    /// Codec name (e.g. "H264", "H265", "AAC", "PCMA", "PCMU", "G726").
    pub codec: String,
    /// Clock rate in Hz.
    pub clock_rate: u32,
    /// Number of audio channels, when applicable.
    #[serde(default)]
    pub channels: Option<u8>,
    /// Audio packet duration in milliseconds, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet_duration_ms: Option<u32>,
}

/// RTP session lifecycle state.
///
/// RTP 会话生命周期状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[non_exhaustive]
pub enum RtpSessionState {
    #[default]
    Allocating,
    Ready,
    Active,
    Draining,
    Stopped,
    Failed,
}

/// Resolved local and remote endpoint for an RTP session.
///
/// RTP 会话解析后的本地与远端端点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtpEndpoints {
    pub local: SocketAddr,
    pub remote: Option<SocketAddr>,
    #[serde(default)]
    pub rtcp_local: Option<SocketAddr>,
    #[serde(default)]
    pub rtcp_remote: Option<SocketAddr>,
}

/// Common session parameter fields shared by open requests.
///
/// 开请求共用的会话参数字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RtpSessionParams {
    pub media_key: MediaKey,
    pub direction: RtpDirection,
    pub transport: RtpTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_role: Option<TcpRole>,
    #[serde(default)]
    pub framing: RtpFraming,
    #[serde(default)]
    pub container: MediaContainer,
    #[serde(default)]
    pub profile: GbMediaCompatibilityProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssrc: Option<u32>,
    #[serde(default)]
    pub payload_bindings: Vec<RtpPayloadBinding>,
    #[serde(default)]
    pub source_binding_policy: SourceBindingPolicy,
    /// Optional remote endpoint for active connect or known passive sender.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_endpoint: Option<SocketAddr>,
    /// Optional local endpoint hint (address/port). Port 0 requests allocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_endpoint_hint: Option<SocketAddr>,
    /// Bound on source rebind attempts.
    #[serde(default)]
    pub max_rebind_attempts: u32,
    /// Cap on packet sniffing/probing bytes before giving up.
    #[serde(default)]
    pub max_probe_bytes: u64,
    /// RTP/RTCP mux flag; when true RTCP shares the RTP socket.
    #[serde(default)]
    pub rtcp_mux: bool,
}

/// Open a passive or active RTP receiver.
///
/// 打开被动或主动 RTP 接收端。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenRtpReceiver {
    #[serde(flatten)]
    pub params: RtpSessionParams,
    /// Start playback range for non-live pull (empty means live).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_range: Option<PlaybackRange>,
}

/// Open an RTP sender.
///
/// 打开 RTP 发送端。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenRtpSender {
    #[serde(flatten)]
    pub params: RtpSessionParams,
}

/// Open a bi-directional voice talk session.
///
/// 打开双向对讲会话。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenRtpTalk {
    #[serde(flatten)]
    pub params: RtpSessionParams,
    /// Back-channel payload binding for the return audio stream.
    #[serde(default)]
    pub talkback_binding: Option<RtpPayloadBinding>,
}

/// Optional playback/download time range.
///
/// 可选回放/下载时间范围。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackRange {
    /// Start time as Unix milliseconds.
    pub start_ms: i64,
    /// End time as Unix milliseconds; `None` means until live/end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<i64>,
}

/// Update an existing RTP session.
///
/// 更新已有 RTP 会话。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateRtpSession {
    pub session_ref: RtpSessionRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_bindings: Option<Vec<RtpPayloadBinding>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_binding_policy: Option<SourceBindingPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_endpoint: Option<SocketAddr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rebind_attempts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_probe_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_check: Option<bool>,
}

/// Reference to an RTP session used for get/update/stop.
///
/// 用于 get/update/stop 的 RTP 会话引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtpSessionRef {
    pub session_id: RtpSessionId,
    /// Expected generation for compare-and-swap updates.
    #[serde(default)]
    pub expected_generation: RtpSessionGeneration,
}

/// Stop an RTP session.
///
/// 停止 RTP 会话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopRtpSession {
    pub session_ref: RtpSessionRef,
    /// When true, delete any pending publisher/subscriber lease.
    #[serde(default)]
    pub release_lease: bool,
}

/// RTP session descriptor returned by open/update/get.
///
/// open/update/get 返回的 RTP 会话描述符。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RtpSessionDescriptor {
    pub session_id: RtpSessionId,
    pub generation: RtpSessionGeneration,
    pub state: RtpSessionState,
    pub direction: RtpDirection,
    pub transport: RtpTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_role: Option<TcpRole>,
    pub framing: RtpFraming,
    pub container: MediaContainer,
    pub profile: GbMediaCompatibilityProfile,
    pub endpoints: RtpEndpoints,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssrc: Option<u32>,
    pub payload_bindings: Vec<RtpPayloadBinding>,
    pub source_binding_policy: SourceBindingPolicy,
    pub resource_ref: ControlledResourceRef,
}

/// Query for listing RTP sessions.
///
/// RTP 会话列表查询。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtpSessionQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<RtpSessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_key: Option<MediaKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<RtpDirection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<RtpSessionState>,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
}

impl RtpSessionQuery {
    pub const MAX_PAGE_SIZE: u64 = 1_000;

    pub fn clamp_page_size(&mut self) {
        if self.page_size == 0 {
            self.page_size = default_page_size();
        }
        self.page_size = self.page_size.min(Self::MAX_PAGE_SIZE);
    }
}

fn default_page_size() -> u64 {
    50
}

/// Typed RTP session port.
///
/// Implementations are runtime-neutral and must not expose sockets, Tokio
/// handles, or internal file paths. The `MediaRequestContext` carries tenant,
/// deadline, idempotency key, owner epoch, media node instance epoch and other
/// mutation metadata; request structs carry only media parameters.
///
/// 类型化的 RTP 会话端口。实现必须是运行时无关的，且不得暴露 socket、Tokio handle
/// 或内部文件路径。`MediaRequestContext` 携带 tenant、deadline、幂等键、owner epoch、
/// media node instance epoch 等变更元数据；请求结构体只携带媒体参数。
#[async_trait::async_trait]
pub trait RtpSessionApi: Send + Sync {
    async fn open_receiver(
        &self,
        ctx: &crate::port::MediaRequestContext,
        request: OpenRtpReceiver,
    ) -> crate::error::Result<RtpSessionDescriptor>;

    async fn open_sender(
        &self,
        ctx: &crate::port::MediaRequestContext,
        request: OpenRtpSender,
    ) -> crate::error::Result<RtpSessionDescriptor>;

    async fn open_talk(
        &self,
        ctx: &crate::port::MediaRequestContext,
        request: OpenRtpTalk,
    ) -> crate::error::Result<RtpSessionDescriptor>;

    async fn update_session(
        &self,
        ctx: &crate::port::MediaRequestContext,
        request: UpdateRtpSession,
    ) -> crate::error::Result<RtpSessionDescriptor>;

    async fn get_session(
        &self,
        ctx: &crate::port::MediaRequestContext,
        session_ref: RtpSessionRef,
    ) -> crate::error::Result<RtpSessionDescriptor>;

    async fn stop_session(
        &self,
        ctx: &crate::port::MediaRequestContext,
        request: StopRtpSession,
    ) -> crate::error::Result<EffectOutcome>;

    async fn list_sessions(
        &self,
        ctx: &crate::port::MediaRequestContext,
        query: RtpSessionQuery,
    ) -> crate::error::Result<crate::model::Page<RtpSessionDescriptor>>;
}

/// Convenience builder for `RtpSessionParams`.
///
/// RtpSessionParams 的便捷构造器。
#[derive(Debug, Default)]
pub struct RtpSessionParamsBuilder {
    params: RtpSessionParams,
}

impl RtpSessionParamsBuilder {
    pub fn new(media_key: MediaKey, direction: RtpDirection) -> Self {
        Self {
            params: RtpSessionParams {
                media_key,
                direction,
                ..RtpSessionParams::default()
            },
        }
    }

    pub fn transport(mut self, transport: RtpTransport) -> Self {
        self.params.transport = transport;
        self
    }

    pub fn tcp_role(mut self, tcp_role: TcpRole) -> Self {
        self.params.tcp_role = Some(tcp_role);
        self
    }

    pub fn framing(mut self, framing: RtpFraming) -> Self {
        self.params.framing = framing;
        self
    }

    pub fn container(mut self, container: MediaContainer) -> Self {
        self.params.container = container;
        self
    }

    pub fn profile(mut self, profile: GbMediaCompatibilityProfile) -> Self {
        self.params.profile = profile;
        self
    }

    pub fn ssrc(mut self, ssrc: u32) -> Self {
        self.params.ssrc = Some(ssrc);
        self
    }

    pub fn payload_binding(mut self, binding: RtpPayloadBinding) -> Self {
        self.params.payload_bindings.push(binding);
        self
    }

    pub fn source_binding_policy(mut self, policy: SourceBindingPolicy) -> Self {
        self.params.source_binding_policy = policy;
        self
    }

    pub fn remote_endpoint(mut self, endpoint: SocketAddr) -> Self {
        self.params.remote_endpoint = Some(endpoint);
        self
    }

    pub fn local_endpoint_hint(mut self, endpoint: SocketAddr) -> Self {
        self.params.local_endpoint_hint = Some(endpoint);
        self
    }

    pub fn max_rebind_attempts(mut self, max: u32) -> Self {
        self.params.max_rebind_attempts = max;
        self
    }

    pub fn max_probe_bytes(mut self, max: u64) -> Self {
        self.params.max_probe_bytes = max;
        self
    }

    pub fn rtcp_mux(mut self, mux: bool) -> Self {
        self.params.rtcp_mux = mux;
        self
    }

    pub fn build(self) -> RtpSessionParams {
        self.params
    }
}

impl Default for RtpSessionParams {
    fn default() -> Self {
        Self {
            media_key: MediaKey::with_default_vhost("live", "default", None).unwrap_or_else(|_| {
                // This fallback is unreachable because "live" and "default" are valid.
                MediaKey {
                    vhost: crate::ids::VhostName::default_value(),
                    app: crate::ids::AppName::new("live").unwrap(),
                    stream: crate::ids::StreamName::new("default").unwrap(),
                    schema: None,
                }
            }),
            direction: RtpDirection::default(),
            transport: RtpTransport::default(),
            tcp_role: None,
            framing: RtpFraming::default(),
            container: MediaContainer::default(),
            profile: GbMediaCompatibilityProfile::default(),
            ssrc: None,
            payload_bindings: Vec::new(),
            source_binding_policy: SourceBindingPolicy::default(),
            remote_endpoint: None,
            local_endpoint_hint: None,
            max_rebind_attempts: 0,
            max_probe_bytes: 64 * 1024,
            rtcp_mux: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtp_session_query_page_size_clamped() {
        let mut q = RtpSessionQuery {
            page_size: 10_000,
            ..Default::default()
        };
        q.clamp_page_size();
        assert_eq!(q.page_size, RtpSessionQuery::MAX_PAGE_SIZE);
    }

    #[test]
    fn default_profile_is_gb_common() {
        assert_eq!(
            GbMediaCompatibilityProfile::default(),
            GbMediaCompatibilityProfile::GbCommon
        );
    }

    #[test]
    fn builder_produces_params() {
        let key = MediaKey::with_default_vhost("live", "test", None).unwrap();
        let params = RtpSessionParamsBuilder::new(key.clone(), RtpDirection::Receive)
            .transport(RtpTransport::Tcp)
            .tcp_role(TcpRole::Passive)
            .framing(RtpFraming::DollarPrefixed)
            .container(MediaContainer::Ps)
            .profile(GbMediaCompatibilityProfile::Zlm)
            .ssrc(0x12345678)
            .source_binding_policy(SourceBindingPolicy::AllowValidatedRebind)
            .rtcp_mux(true)
            .build();

        assert_eq!(params.media_key, key);
        assert_eq!(params.transport, RtpTransport::Tcp);
        assert_eq!(params.tcp_role, Some(TcpRole::Passive));
        assert_eq!(params.framing, RtpFraming::DollarPrefixed);
        assert_eq!(params.container, MediaContainer::Ps);
        assert_eq!(params.profile, GbMediaCompatibilityProfile::Zlm);
        assert_eq!(params.ssrc, Some(0x12345678));
        assert_eq!(
            params.source_binding_policy,
            SourceBindingPolicy::AllowValidatedRebind
        );
        assert!(params.rtcp_mux);
    }

    #[test]
    fn stop_session_serde_roundtrip() {
        let stop = StopRtpSession {
            session_ref: RtpSessionRef {
                session_id: RtpSessionId("sess-1".to_string()),
                expected_generation: RtpSessionGeneration(7),
            },
            release_lease: true,
        };
        let json = serde_json::to_string(&stop).unwrap();
        let decoded: StopRtpSession = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, stop);
    }

    #[test]
    fn session_resource_ref_round_trips_without_generation_collision() {
        let resource_ref = RtpSessionResourceRef {
            session_id: RtpSessionId("rtp-sess-42".to_string()),
            generation: RtpSessionGeneration(3),
            resource_ref: crate::fencing::ControlledResourceRef {
                tenant_id: crate::ids::TenantId::new("tenant-1").unwrap(),
                media_session_id: Some(
                    crate::ids::MediaSessionId::new("550e8400-e29b-41d4-a716-446655440000")
                        .unwrap(),
                ),
                media_binding_id: None,
                resource_kind: "rtp_session".to_string(),
                resource_handle: "rtp-sess-42".to_string(),
                owner_epoch: crate::ids::OwnerEpoch(9),
                node_instance_epoch: crate::ids::MediaNodeInstanceEpoch(4),
                generation: crate::ids::ResourceGeneration(3),
                origin: crate::fencing::ResourceOrigin::Local,
            },
        };
        let json = serde_json::to_string(&resource_ref).unwrap();
        let decoded: RtpSessionResourceRef = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, resource_ref);
        // Sanity check that both generation fields are present and distinct.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["generation"], 3);
        assert_eq!(value["resource_ref"]["generation"], 3);
    }
}
