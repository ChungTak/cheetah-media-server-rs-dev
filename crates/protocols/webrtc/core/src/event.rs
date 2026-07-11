//! Events emitted by the core for the driver and module layers.
//!
//! Only a small forward-compatible subset of `str0m::Event` is mapped today;
//! later phases extend this enum with media-frame, RTP, RTCP and BWE
//! variants. The mapping is conservative: variants we do not understand
//! become diagnostic records rather than corrupting the event stream.
//!
//! 本模块包含核心向驱动层和模块层发出的事件。
//!
//! 当前只映射了 `str0m::Event` 中一个小的前向兼容子集；后续阶段会扩展媒体帧、
//! RTP、RTCP 与 BWE 变体。映射是保守的：无法理解的变体会变成诊断记录，
//! 而不是污染事件流。

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::types::{DataChannelId, MidLabel, WebRtcSessionId};

/// All boundary-visible events for a session.
///
/// Events are emitted to `pending_outputs` during `pump_outputs` and are
/// consumed by the driver, which routes lifecycle/ICE events to the module.
///
/// 会话的所有边界可见事件。
///
/// 事件在 `pump_outputs` 期间被推入 `pending_outputs`，由驱动层消费并路由
/// 生命周期/ICE 事件到模块。
#[derive(Debug, Clone)]
pub enum WebRtcCoreEvent {
    /// Session lifecycle transition (`Created`, `Connected`, `Failed`, ...).
    ///
    /// 会话生命周期迁移（`Created`、`Connected`、`Failed` 等）。
    Lifecycle {
        session_id: WebRtcSessionId,
        state: WebRtcSessionLifecycle,
    },
    /// ICE connection state change from the underlying `str0m` session.
    ///
    /// 底层 `str0m` 会话的 ICE 连接状态变化。
    Ice {
        session_id: WebRtcSessionId,
        state: WebRtcIceState,
    },
    /// A new media track has been negotiated for this session.
    ///
    /// 本会话已协商出一个新的媒体 track。
    MediaTrackAdded {
        session_id: WebRtcSessionId,
        track: WebRtcMediaTrack,
    },
    /// Media-level event (frame arrival, keyframe request, etc.).
    ///
    /// 媒体级事件（帧到达、关键帧请求等）。
    Media {
        session_id: WebRtcSessionId,
        event: WebRtcMediaEvent,
    },
    /// DataChannel open / message / close event.
    ///
    /// DataChannel 打开/消息/关闭事件。
    DataChannel {
        session_id: WebRtcSessionId,
        event: WebRtcDataChannelEvent,
    },
    /// Transport and per-stream stats snapshot.
    ///
    /// 传输与每流统计快照。
    Stats {
        session_id: WebRtcSessionId,
        snapshot: crate::stats::WebRtcSessionStats,
    },
    /// Bandwidth estimation snapshot.
    ///
    /// 带宽估计快照。
    Bwe {
        session_id: WebRtcSessionId,
        snapshot: crate::stats::WebRtcBweStats,
    },
    /// RTCP feedback from the remote peer (PLI, FIR, NACK, REMB, ...).
    ///
    /// 来自远端对端的 RTCP 反馈（PLI、FIR、NACK、REMB 等）。
    RtcpFeedback {
        session_id: WebRtcSessionId,
        feedback: WebRtcRtcpFeedback,
    },
    /// Simulcast layer surfaced by SDP negotiation or RID extension
    /// observation. Emitted once per RID per direction at track-add
    /// time so the module can pre-allocate per-layer routing state.
    ///
    /// SDP 协商或 RID 扩展观察暴露出的 simulcast 层。
    /// 在 track 添加时按方向每个 RID 发出一次，使模块可预先分配每层路由状态。
    SimulcastLayerObserved {
        session_id: WebRtcSessionId,
        observation: WebRtcSimulcastLayerObservation,
    },
    /// RTP extension mappings observed during SDP negotiation. Emitted
    /// once per session after `AcceptOffer` / `CreateOffer` so the
    /// module can track which extensions are active and their id/type
    /// mapping without re-parsing SDP.
    ///
    /// SDP 协商期间观察到的 RTP 扩展映射。
    ///
    /// 在 `AcceptOffer` / `CreateOffer` 后每个会话发出一次，模块无需重新解析
    /// SDP 即可跟踪哪些扩展激活以及 id/type 映射。
    RtpExtensionObserved {
        session_id: WebRtcSessionId,
        mappings: Vec<crate::sdp_compat::RtpExtensionMapping>,
    },
    /// Payload type numbers extracted from the remote SDP offer. Emitted
    /// once per session after `AcceptOffer` so the module layer knows
    /// which dynamic payload types the browser assigned to each codec.
    /// The answer SDP uses these negotiated values — never hardcoded
    /// constants.
    ///
    /// 从远端 SDP offer 提取的 payload type 数字。
    ///
    /// 在 `AcceptOffer` 后每个会话发出一次，模块层据此了解浏览器为每个编解码器
    /// 分配的动态 payload type。answer SDP 使用这些协商值，而非硬编码常量。
    OfferPayloadNegotiated {
        session_id: WebRtcSessionId,
        payloads: crate::offer_payload::OfferPayloads,
    },
}

/// High-level session lifecycle events.
///
/// These are synthetic transitions derived from `str0m` events and from
/// command-driven close operations. They are the canonical signal used by the
/// module to manage session lifecycle.
///
/// 高层会话生命周期事件。
///
/// 这些是从 `str0m` 事件与命令驱动的关闭操作派生的合成迁移。模块使用它们
/// 管理会话生命周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcSessionLifecycle {
    /// Session has been created in the core.
    ///
    /// 会话已在核心中创建。
    Created,
    /// Local SDP description is ready (offer or answer).
    ///
    /// 本地 SDP 描述已就绪（offer 或 answer）。
    LocalDescriptionReady,
    /// ICE+DTLS+SRTP up.
    ///
    /// ICE+DTLS+SRTP 已建立。
    Connected,
    /// ICE temporarily disconnected, may recover.
    ///
    /// ICE 暂时断开，可能恢复。
    Disconnected,
    /// Session has been closed.
    ///
    /// 会话已关闭。
    Closed,
    /// Session ended because of an unrecoverable error.
    ///
    /// 会话因不可恢复错误而结束。
    Failed,
}

/// ICE connection state as observed by the core.
///
/// Mirrors `str0m::IceConnectionState` with `Completed` collapsed into
/// `Connected` because the boundary only needs to know when media can flow.
///
/// 核心观察到的 ICE 连接状态。
///
/// 与 `str0m::IceConnectionState` 对齐，但将 `Completed` 合并为 `Connected`，
/// 因为边界只需知道媒体能否流通。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcIceState {
    /// ICE agent is idle, no checks started.
    ///
    /// ICE agent 空闲，尚未开始检查。
    New,
    /// ICE connectivity checks are in progress.
    ///
    /// ICE 连通性检查进行中。
    Checking,
    /// A working candidate pair has been selected.
    ///
    /// 已选择可用的 candidate pair。
    Connected,
    /// The selected candidate pair has temporarily lost connectivity.
    ///
    /// 已选 candidate pair 暂时失去连通性。
    Disconnected,
    /// ICE agent has shut down and released resources.
    ///
    /// ICE agent 已关闭并释放资源。
    Closed,
}

/// A media track newly negotiated for a session.
///
/// Combines the SDP m-line identifier, media kind, negotiated direction, and
/// simulcast RID lists for both directions.
///
/// 会话新协商出的媒体 track。
///
/// 组合 SDP m-line 标识符、媒体类型、协商方向与双向 simulcast RID 列表。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebRtcMediaTrack {
    pub mid: MidLabel,
    pub kind: WebRtcMediaKind,
    pub direction: WebRtcMediaDirection,
    /// Simulcast layer RIDs negotiated for this track, if any.
    ///
    /// 本 track 协商的 simulcast 层 RID（如有）。
    pub simulcast_send: Vec<String>,
    /// Simulcast layer RIDs the remote endpoint will send for this track.
    ///
    /// 远端对端将为本 track 发送的 simulcast 层 RID。
    pub simulcast_recv: Vec<String>,
}

/// Media kind for a `WebRtcMediaTrack`.
///
/// `WebRtcMediaTrack` 的媒体类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcMediaKind {
    /// Audio track.
    ///
    /// 音频 track。
    Audio,
    /// Video track.
    ///
    /// 视频 track。
    Video,
}

/// Direction negotiated for a media track.
///
/// 媒体 track 协商出的方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcMediaDirection {
    /// Local endpoint sends only.
    ///
    /// 本地端仅发送。
    SendOnly,
    /// Local endpoint receives only.
    ///
    /// 本地端仅接收。
    RecvOnly,
    /// Both endpoints send and receive.
    ///
    /// 双向收发。
    SendRecv,
    /// Track is not used for media.
    ///
    /// Track 不用于媒体。
    Inactive,
}

/// Media-level events surfaced from `str0m`.
///
/// Carries enough metadata for the module to push frames into engine
/// ingestion: codec, clock rate, RID for simulcast and the canonical
/// media time numerator / denominator.
///
/// 从 `str0m` 暴露的媒体级事件。
///
/// 携带足够元数据，使模块能够将帧推入引擎接入：编解码器、时钟率、simulcast 的
/// RID 以及规范媒体时间分子/分母。
#[derive(Debug, Clone)]
pub enum WebRtcMediaEvent {
    /// A frame arrived on the named track.
    ///
    /// 指定 track 上到达一帧。
    Frame {
        mid: MidLabel,
        rid: Option<String>,
        codec: WebRtcCodecKind,
        clock_rate: u32,
        random_access: bool,
        rtp_timestamp_ticks: u32,
        rtp_timestamp_denom: u32,
        payload: Bytes,
        network_time_micros: u64,
        /// RTP header / packet metadata for codec-side adapters.
        ///
        /// The boundary uses an explicit metadata struct rather than
        /// boolean / option overloads on the carrier so codec adapters
        /// stay decoupled from the str0m-level RTP stream surface.
        ///
        /// 用于编解码器侧适配器的 RTP 头/包元数据。
        ///
        /// 边界使用显式元数据结构而非在载体上重载布尔/选项，使编解码器适配器
        /// 与 str0m 级 RTP 流表面保持解耦。
        meta: WebRtcFrameMeta,
    },
    /// The remote requested a keyframe via PLI.
    ///
    /// 远端通过 PLI 请求关键帧。
    PliReceived { mid: MidLabel },
    /// The remote requested a keyframe via FIR.
    ///
    /// 远端通过 FIR 请求关键帧。
    FirReceived { mid: MidLabel },
}

/// Per-frame metadata carried alongside [`WebRtcMediaEvent::Frame`].
///
/// Each field maps onto a well-known RTP header extension that
/// `str0m::media::MediaData` exposes:
///
/// * `audio_level_dbov` — `urn:ietf:params:rtp-hdrext:ssrc-audio-level`
///   (RFC 6464). Negative dBOV; 0 is loudest, -127 is silence.
/// * `voice_activity` — companion bit in the audio-level extension.
/// * `video_orientation` — CVO extension (RFC 7742) bit-packed
///   `(rotation, flip)` byte.
/// * `sequence_number` — first RTP sequence number that contributed
///   to this access unit; usable as the canonical
///   `WebRtcIngressContractView::sequence_number`.
/// * `contiguous` — false when str0m's reorder buffer detected a gap
///   relative to the previously emitted frame on the same track. The
///   module forwards this to `cheetah-codec` so the timestamp
///   normalizer can mark the frame as discontinuous.
///
/// 伴随 [`WebRtcMediaEvent::Frame`] 携带的每帧元数据。
///
/// 每个字段映射到 `str0m::media::MediaData` 暴露的知名 RTP 头扩展：
///
/// - `audio_level_dbov` — `urn:ietf:params:rtp-hdrext:ssrc-audio-level`
///   （RFC 6464）。dBOV 负值；0 最大，-127 静音。
/// - `voice_activity` — 音频级别扩展的伴随位。
/// - `video_orientation` — CVO 扩展（RFC 7742）打包的字节 `(rotation, flip)`。
/// - `sequence_number` — 构成本访问单元的首个 RTP 序列号，可作为规范的
///   `WebRtcIngressContractView::sequence_number`。
/// - `contiguous` — 当 str0m 重排缓冲区检测到同一 track 上一帧缺失时为 false。
///   模块将其转发给 `cheetah-codec`，时间戳归一化器据此标记帧为不连续。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebRtcFrameMeta {
    pub audio_level_dbov: Option<i8>,
    pub voice_activity: Option<bool>,
    pub video_orientation: Option<u8>,
    pub sequence_number: Option<u16>,
    pub contiguous: bool,
}

/// Codec carried in [`WebRtcMediaEvent::Frame`].
///
/// We intentionally keep this in `cheetah-webrtc-core` rather than
/// re-using `cheetah-codec::CodecId` so that core stays decoupled from the
/// fuller codec model. The module layer maps this to `CodecId` when
/// pushing into engine.
///
/// [`WebRtcMediaEvent::Frame`] 携带的编解码器。
///
/// 我们刻意将其保留在 `cheetah-webrtc-core` 而非复用 `cheetah-codec::CodecId`，
/// 使核心与完整编解码器模型保持解耦。模块层在推入引擎时将其映射到 `CodecId`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcCodecKind {
    /// Opus audio codec.
    ///
    /// Opus 音频编解码器。
    Opus,
    /// G.711 A-law audio codec.
    ///
    /// G.711 A-law 音频编解码器。
    Pcma,
    /// G.711 mu-law audio codec.
    ///
    /// G.711 mu-law 音频编解码器。
    Pcmu,
    /// H.264 video codec.
    ///
    /// H.264 视频编解码器。
    H264,
    /// H.265 / HEVC video codec.
    ///
    /// H.265 / HEVC 视频编解码器。
    H265,
    /// VP8 video codec.
    ///
    /// VP8 视频编解码器。
    Vp8,
    /// VP9 video codec.
    ///
    /// VP9 视频编解码器。
    Vp9,
    /// AV1 video codec.
    ///
    /// AV1 视频编解码器。
    Av1,
    /// Codec not recognized by this mapping.
    ///
    /// 此映射无法识别的编解码器。
    Unknown,
}

/// DataChannel events emitted by the core.
///
/// Channel ids are mapped from `str0m::channel::ChannelId` to the boundary
/// `DataChannelId` so the module layer does not depend on `str0m` internals.
///
/// 核心发出的 DataChannel 事件。
///
/// 通道 id 从 `str0m::channel::ChannelId` 映射到边界 `DataChannelId`，
/// 模块层无需依赖 `str0m` 内部。
#[derive(Debug, Clone)]
pub enum WebRtcDataChannelEvent {
    /// A DataChannel has been opened by the remote peer.
    ///
    /// 远端对端打开了一个 DataChannel。
    Opened { id: DataChannelId, label: String },
    /// A DataChannel message has been received.
    ///
    /// 收到 DataChannel 消息。
    Message {
        id: DataChannelId,
        payload: Bytes,
        binary: bool,
    },
    /// A DataChannel has been closed.
    ///
    /// DataChannel 已关闭。
    Closed { id: DataChannelId },
}

/// RTCP feedback events surfaced by the core.
///
/// These are the feedback messages that the module may act on: keyframe
/// requests (PLI/FIR), loss reports (NACK), bitrate estimates (REMB), and
/// the `str0m` BWE path (TWCC).
///
/// 核心暴露的 RTCP 反馈事件。
///
/// 这些模块可能需要处理的反馈消息：关键帧请求（PLI/FIR）、丢包报告（NACK）、
/// 码率估计（REMB）以及 `str0m` BWE 路径（TWCC）。
#[derive(Debug, Clone)]
pub enum WebRtcRtcpFeedback {
    /// RTCP sender report.
    ///
    /// RTCP 发送者报告。
    SenderReport,
    /// RTCP receiver report.
    ///
    /// RTCP 接收者报告。
    ReceiverReport,
    /// Picture Loss Indication for a specific track.
    ///
    /// 指定 track 的图像丢失指示。
    Pli { mid: Option<MidLabel> },
    /// Full Intra Request for a specific track.
    ///
    /// 指定 track 的完整帧内请求。
    Fir { mid: Option<MidLabel> },
    /// NACK with a cumulative count for a specific track.
    ///
    /// 指定 track 的 NACK，含累积计数。
    Nack { mid: Option<MidLabel>, count: u32 },
    /// Transport Wide Congestion Control feedback.
    ///
    /// 传输级拥塞控制反馈。
    Twcc,
    /// Receiver Estimated Maximum Bitrate, surfaced from `str0m`'s BWE
    /// subsystem (`Event::EgressBitrateEstimate(BweKind::Remb)`). The
    /// `mid` identifies which media this estimate applies to and the
    /// `bitrate_bps` is the raw estimate in bits per second.
    ///
    /// 接收端估计最大码率（REMB），从 `str0m` 的 BWE 子系统暴露
    /// （`Event::EgressBitrateEstimate(BweKind::Remb)`）。`mid` 标识该估计适用于
    /// 哪个媒体，`bitrate_bps` 为原始估计比特每秒。
    Remb {
        mid: Option<MidLabel>,
        bitrate_bps: u64,
    },
    /// Remote endpoint terminated the session via RTCP BYE. We surface
    /// this as a hint to module observability — the actual session
    /// teardown is driven by `Lifecycle::Closed`.
    ///
    /// 远端通过 RTCP BYE 终止会话。我们将其作为模块可观测性提示暴露——
    /// 实际会话拆除由 `Lifecycle::Closed` 驱动。
    Bye,
}

/// Observation of a simulcast layer becoming active or inactive.
///
/// Phase 04 will use these to drive layer selection. Phase 01 only emits
/// them on simulcast track addition for visibility.
///
/// simulcast 层激活或不活跃的观察。
///
/// 阶段 04 将用这些来驱动层选择。阶段 01 仅在 simulcast track 添加时
/// 发出以提供可见性。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcSimulcastLayerObservation {
    pub mid: MidLabel,
    pub rid: String,
    pub source: WebRtcSimulcastRidSource,
}

/// Where the RID label was derived from.
///
/// Mirrors ZLMediaKit's RID fallback chain in `RtpExtContext`: peers may
/// signal the RID via the `rid` extension, the `repaired-rid` extension
/// for retransmissions, the SSRC group `SIM` map, or — when the offer
/// has been munged to drop RID lines — the receiving side has to
/// generate a stable label from SSRC ordering.
///
/// RID 标签来源。
///
/// 与 ZLMediaKit 在 `RtpExtContext` 中的 RID 回退链对齐：对端可能通过 `rid`
/// 扩展、重传用的 `repaired-rid` 扩展、SSRC group `SIM` 映射发送 RID；
/// 当 offer 被篡改掉 RID 行时，接收端必须按 SSRC 顺序生成稳定标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcSimulcastRidSource {
    /// `urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id`.
    ///
    /// `urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id`。
    RidExt,
    /// `urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id`.
    ///
    /// `urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id`。
    RepairedRidExt,
    /// `a=ssrc-group:SIM` mapping.
    ///
    /// `a=ssrc-group:SIM` 映射。
    SsrcSimGroup,
    /// SDP munging stripped RID, label generated from SSRC order.
    ///
    /// SDP 被篡改去掉 RID，按 SSRC 顺序生成标签。
    Generated,
    /// Negotiated through `a=rid` lines and surfaced by `str0m`.
    ///
    /// 通过 `a=rid` 行协商并由 `str0m` 暴露。
    SdpRid,
}
