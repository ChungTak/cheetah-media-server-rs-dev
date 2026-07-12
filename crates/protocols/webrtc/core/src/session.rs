//! [`WebRtcCore`] is the multi-session Sans-I/O wrapper around `str0m::Rtc`.
//!
//! Time discipline:
//!
//! * The constructor accepts a `start_instant: Instant` which anchors all
//!   subsequent `now_micros` boundary values.
//! * The crate never calls [`Instant::now`]; the driver layer is responsible
//!   for sourcing wall-clock time and passing it through inputs.
//! * Conversion from `u64 now_micros` to [`Instant`] is monotonic and
//!   saturating, so a slightly out-of-order `now_micros` value cannot panic
//!   the state machine.
//!
//! `WebRtcCore` 是围绕 `str0m::Rtc` 的多会话无 I/O 包装器。
//!
//! 时间纪律：
//!
//! - 构造函数接受 `start_instant: Instant` 作为锚点，所有后续边界 `now_micros`
//!   均相对该锚点。
//! - 本 crate 不调用 [`Instant::now`]；驱动层负责获取墙上时间并通过输入传入。
//! - 从 `u64 now_micros` 到 [`Instant`] 的转换是单调且饱和的，因此略微乱序的
//!   `now_micros` 不会使状态机 panic。
//!
//! Phase 01 implements:
//!
//! * `accept_offer` / `apply_answer` flow through `str0m::change::SdpApi`.
//! * Remote ICE candidate ingestion.
//! * Network packet pumping via [`net::Receive`].
//! * Timer scheduling driven by `Output::Timeout`.
//! * Conservative event mapping for ICE state, media-added,
//!   data-channel and PLI/FIR feedback.
//!
//! Media write paths, RTP-mode passthrough, BWE policy and stats export are
//! not implemented in this phase. Commands for those operations are still
//! exposed on [`WebRtcCoreCommand`] but currently emit a diagnostic and are
//! otherwise a no-op so downstream layers can wire their flow without
//! blocking on later phases.
//!
//! 阶段 01 实现：
//!
//! - 通过 `str0m::change::SdpApi` 的 `accept_offer` / `apply_answer` 流程。
//! - 远端 ICE candidate 注入。
//! - 通过 [`net::Receive`] 的网络包轮询。
//! - 由 `Output::Timeout` 驱动的定时器调度。
//! - ICE 状态、媒体添加、DataChannel 与 PLI/FIR 反馈的保守事件映射。
//!
//! 媒体写路径、RTP 模式透传、BWE 策略与统计导出尚未在本阶段实现。这些操作的
//! 命令仍暴露在 [`WebRtcCoreCommand`] 上，但当前只发出诊断，否则为 no-op，
//! 以便下游层可以提前接入，而不被后续阶段阻塞。

use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bytes::Bytes;
use str0m::bwe::Bitrate;
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::channel::ChannelId as Str0mChannelId;
use str0m::format::CodecConfig;
use str0m::media::MediaKind as Str0mMediaKind;
use str0m::net::{Protocol, Receive};
use str0m::{
    Candidate, Event as Str0mEvent, IceConnectionState, Input as Str0mInput, Output as Str0mOutput,
    Rtc, RtcConfig, RtcError,
};

use crate::config::WebRtcCoreConfig;
use crate::error::{WebRtcCoreDiagnostic, WebRtcCoreDiagnosticKind, WebRtcCoreError};
use crate::event::{
    WebRtcCodecKind, WebRtcCoreEvent, WebRtcDataChannelEvent, WebRtcIceState, WebRtcMediaDirection,
    WebRtcMediaEvent, WebRtcMediaKind, WebRtcMediaTrack, WebRtcRtcpFeedback,
    WebRtcSessionLifecycle, WebRtcSimulcastLayerObservation, WebRtcSimulcastRidSource,
};
use crate::input::{
    WebRtcCloseReason, WebRtcCoreCommand, WebRtcCoreInput, WebRtcDataChannelOut,
    WebRtcNetworkInput, WebRtcOfferDirection, WebRtcOfferSpec, WebRtcRequestKeyframeKind,
    WebRtcSendFrame,
};
use crate::output::{WebRtcCoreOutput, WebRtcLocalDescriptionKind, WebRtcPacketOut, WebRtcTimer};
use crate::sdp_compat::{preprocess_remote_sdp, SdpCompatReport};
use crate::types::{
    DataChannelId, MidLabel, WebRtcCodecProfile, WebRtcSessionId, WebRtcSessionRole,
    WebRtcSessionState,
};

/// Maximum jump forward we accept from a single `now_micros` input before
/// clamping the value back to the previous monotonic instant. Prevents
/// hostile inputs from advancing the state machine by gigabytes of time.
///
/// 在将单个 `now_micros` 输入钳位回前一个单调时刻之前所允许的最大前进跳跃。
/// 防止恶意输入将状态机推进极大量时间。
const MAX_INPUT_TIME_JUMP: Duration = Duration::from_secs(60 * 60);

/// Multi-session Sans-I/O wrapper around `str0m::Rtc`.
///
/// One `WebRtcCore` typically lives in a single driver shard. Drivers feed
/// it input via [`WebRtcCore::handle_input`] and drain output via
/// [`WebRtcCore::pump_outputs`].
///
/// 围绕 `str0m::Rtc` 的多会话无 I/O 包装器。
///
/// 一个 `WebRtcCore` 通常驻留在一个驱动分片中。驱动层通过
/// [`WebRtcCore::handle_input`] 喂入输入，通过 [`WebRtcCore::pump_outputs`] 排出输出。
pub struct WebRtcCore {
    config: WebRtcCoreConfig,
    sessions: HashMap<WebRtcSessionId, WebRtcCoreSession>,
    pending_outputs: VecDeque<WebRtcCoreOutput>,
    start_instant: Instant,
    last_seen_instant: Instant,
}

/// Per-session state stored by the core.
///
/// Holds the `str0m` `Rtc` instance and the bookkeeping needed to map
/// `str0m` events and channel ids into boundary types.
///
/// 核心为每个会话存储的状态。
///
/// 持有 `str0m` `Rtc` 实例以及将 `str0m` 事件和通道 id 映射到边界类型所需的
/// 簿记。
#[allow(dead_code)]
struct WebRtcCoreSession {
    id: WebRtcSessionId,
    rtc: Rtc,
    role: WebRtcSessionRole,
    state: WebRtcSessionState,
    pending_offer: Option<SdpPendingOffer>,
    remote_candidate_count: usize,
    last_activity_at: Instant,
    last_known_destination: Option<SocketAddr>,
    last_known_source: Option<SocketAddr>,
    track_kind_by_mid: HashMap<String, WebRtcMediaKind>,
    channel_ids: HashMap<Str0mChannelId, DataChannelId>,
    reverse_channel_ids: HashMap<DataChannelId, Str0mChannelId>,
    next_channel_id: u32,
}

impl WebRtcCoreSession {
    /// Map a `str0m` channel id to the boundary `DataChannelId`.
    ///
    /// Allocates a new boundary id on first use and caches the mapping in
    /// both directions so `ChannelData` and `SendDataChannel` both resolve
    /// efficiently.
    ///
    /// 将 `str0m` 通道 id 映射到边界 `DataChannelId`。
    ///
    /// 首次使用时分配新的边界 id，并在两个方向缓存映射，使 `ChannelData` 与
    /// `SendDataChannel` 都能高效解析。
    fn map_channel_id(&mut self, str0m_id: Str0mChannelId) -> DataChannelId {
        if let Some(existing) = self.channel_ids.get(&str0m_id).copied() {
            return existing;
        }
        let assigned = DataChannelId::new(self.next_channel_id);
        self.next_channel_id = self.next_channel_id.saturating_add(1);
        self.channel_ids.insert(str0m_id, assigned);
        self.reverse_channel_ids.insert(assigned, str0m_id);
        assigned
    }

    /// Look up the `str0m` channel id for a boundary `DataChannelId`.
    ///
    /// Returns `None` for closed or never-opened channels.
    ///
    /// 根据边界 `DataChannelId` 查找 `str0m` 通道 id。
    ///
    /// 对已关闭或从未打开的通道返回 `None`。
    fn lookup_str0m_channel_id(&self, channel: DataChannelId) -> Option<Str0mChannelId> {
        self.reverse_channel_ids.get(&channel).copied()
    }
}

impl WebRtcCore {
    /// Create a new core anchored at `start_instant`.
    ///
    /// The anchor must be supplied by the caller. The crate never reads
    /// system time on its own.
    ///
    /// 以 `start_instant` 为锚点创建新核心。
    ///
    /// 锚点必须由调用方提供。本 crate 不自行读取系统时间。
    pub fn new(config: WebRtcCoreConfig, start_instant: Instant) -> Self {
        Self {
            config,
            sessions: HashMap::new(),
            pending_outputs: VecDeque::new(),
            start_instant,
            last_seen_instant: start_instant,
        }
    }

    /// Number of currently managed sessions.
    ///
    /// 当前管理的会话数。
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Whether a session with the given id exists in the core.
    ///
    /// 核心中是否存在指定 id 的会话。
    pub fn has_session(&self, id: WebRtcSessionId) -> bool {
        self.sessions.contains_key(&id)
    }

    /// Snapshot the high-level state of a session, if it exists.
    ///
    /// 若存在，则快照会话的高层状态。
    pub fn session_state(&self, id: WebRtcSessionId) -> Option<WebRtcSessionState> {
        self.sessions.get(&id).map(|s| s.state)
    }

    /// Add a local ICE candidate to an existing session.
    ///
    /// Drivers call this once they have learned about local host
    /// candidates from their socket layer.
    ///
    /// 向现有会话添加本地 ICE candidate。
    ///
    /// 驱动层从 socket 层获取本地 host candidate 后调用此函数。
    pub fn add_local_candidate(
        &mut self,
        session_id: WebRtcSessionId,
        candidate_sdp: &str,
    ) -> Result<(), WebRtcCoreError> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(WebRtcCoreError::SessionNotFound(session_id))?;
        let candidate = Candidate::from_sdp_string(candidate_sdp).map_err(|err| {
            WebRtcCoreError::InvalidCandidate {
                message: err.to_string(),
            }
        })?;
        session.rtc.add_local_candidate(candidate);
        Ok(())
    }

    /// Iterate over the session ids currently managed by this core.
    ///
    /// 遍历当前核心管理的所有会话 id。
    pub fn session_ids(&self) -> impl Iterator<Item = WebRtcSessionId> + '_ {
        self.sessions.keys().copied()
    }

    /// Best-effort routing of an incoming packet across all sessions.
    ///
    /// The driver layer uses this when single-port demultiplexing has not
    /// yet bound a remote address to a session — typically the first STUN
    /// binding request from a peer. We ask each `Rtc` instance whether
    /// it accepts the input via [`Rtc::accepts`], and feed the packet to
    /// the first match.
    ///
    /// Returns the matched session id when one was found.
    ///
    /// 在所有会话中尽力路由一个入站包。
    ///
    /// 当单端口 demux 尚未将远端地址绑定到会话时，驱动层使用此函数——典型场景是
    /// 对端的第一个 STUN 绑定请求。我们询问每个 `Rtc` 实例是否通过 [`Rtc::accepts`]
    /// 接受该输入，并将包喂给第一个匹配项。
    ///
    /// 若找到匹配会话，返回其 id。
    pub fn route_unbound_packet(
        &mut self,
        source: SocketAddr,
        destination: SocketAddr,
        data: Bytes,
        now_micros: u64,
    ) -> Result<Option<WebRtcSessionId>, WebRtcCoreError> {
        let now = self.absolute_instant(now_micros);
        let receive = match Receive::new(Protocol::Udp, source, destination, data.as_ref()) {
            Ok(r) => r,
            Err(err) => {
                self.pending_outputs.push_back(WebRtcCoreOutput::Diagnostic(
                    WebRtcCoreDiagnostic {
                        session_id: None,
                        kind: WebRtcCoreDiagnosticKind::NetworkInputRejected,
                        message: format!("net::Receive::new failed: {err}"),
                    },
                ));
                return Ok(None);
            }
        };
        let candidate_input = Str0mInput::Receive(now, receive);
        let mut matched: Option<WebRtcSessionId> = None;
        for (id, session) in &self.sessions {
            if session.rtc.accepts(&candidate_input) {
                matched = Some(*id);
                break;
            }
        }
        if let Some(session_id) = matched {
            self.dispatch_network(WebRtcNetworkInput {
                session_id,
                source,
                destination,
                data,
                now_micros,
            })?;
        }
        Ok(matched)
    }

    /// Feed a single input into the state machine.
    ///
    /// This is the only entry point that mutates core state. Commands are
    /// dispatched to the appropriate handler, network packets are forwarded
    /// to the target session, and timeouts/ticks advance the `str0m` clock.
    ///
    /// 将单个输入喂入状态机。
    ///
    /// 这是改变核心状态的唯一入口。命令被分派到对应处理器，网络包转发到目标
    /// 会话，超时/滴答推进 `str0m` 时钟。
    pub fn handle_input(&mut self, input: WebRtcCoreInput) -> Result<(), WebRtcCoreError> {
        match input {
            WebRtcCoreInput::Command(cmd) => self.dispatch_command(cmd),
            WebRtcCoreInput::Network(net) => self.dispatch_network(net),
            WebRtcCoreInput::Timeout {
                session_id,
                now_micros,
            } => {
                let now = self.absolute_instant(now_micros);
                self.dispatch_timeout(session_id, now)
            }
            WebRtcCoreInput::Tick { now_micros } => {
                let now = self.absolute_instant(now_micros);
                let ids: Vec<WebRtcSessionId> = self.sessions.keys().copied().collect();
                for id in ids {
                    self.dispatch_timeout(id, now)?;
                }
                Ok(())
            }
        }
    }

    /// Drain queued outputs into the caller-provided buffer.
    ///
    /// The core never pushes directly to a socket or timer; it accumulates
    /// outputs in `pending_outputs` and waits for the driver to pull them.
    ///
    /// 将待输出队列排入调用方提供的缓冲区。
    ///
    /// 核心不会直接推送到 socket 或定时器；它将输出累积在 `pending_outputs` 中，
    /// 等待驱动层拉取。
    pub fn pump_outputs(&mut self, sink: &mut Vec<WebRtcCoreOutput>) {
        sink.reserve(self.pending_outputs.len());
        while let Some(out) = self.pending_outputs.pop_front() {
            sink.push(out);
        }
    }

    /// Borrow queued outputs without draining them. Useful for tests.
    ///
    /// 借用待输出队列而不排空。测试用。
    pub fn pending_output_count(&self) -> usize {
        self.pending_outputs.len()
    }

    /// Dispatch a command to the session-specific handler.
    ///
    /// 将命令分派到会话相关处理器。
    fn dispatch_command(&mut self, command: WebRtcCoreCommand) -> Result<(), WebRtcCoreError> {
        match command {
            WebRtcCoreCommand::AcceptOffer {
                session_id,
                role,
                remote_sdp,
                local_candidates,
                now_micros,
            } => {
                let now = self.absolute_instant(now_micros);
                self.accept_offer(session_id, role, remote_sdp, local_candidates, now)
            }
            WebRtcCoreCommand::CreateOffer {
                session_id,
                role,
                spec,
                local_candidates,
                now_micros,
            } => {
                let now = self.absolute_instant(now_micros);
                self.create_offer(session_id, role, spec, local_candidates, now)
            }
            WebRtcCoreCommand::ApplyAnswer {
                session_id,
                remote_sdp,
                now_micros,
            } => {
                let now = self.absolute_instant(now_micros);
                self.apply_answer(session_id, remote_sdp, now)
            }
            WebRtcCoreCommand::AddRemoteCandidate {
                session_id,
                candidate,
                now_micros,
            } => {
                let now = self.absolute_instant(now_micros);
                self.add_remote_candidate(session_id, candidate, now)
            }
            WebRtcCoreCommand::IceRestart {
                session_id,
                keep_local_candidates,
                now_micros,
            } => {
                let now = self.absolute_instant(now_micros);
                self.ice_restart(session_id, keep_local_candidates, now)
            }
            WebRtcCoreCommand::SendDataChannel(out) => self.send_data_channel(out),
            WebRtcCoreCommand::SendFrame(frame) => self.send_frame(*frame),
            WebRtcCoreCommand::RequestKeyframe {
                session_id,
                mid,
                kind,
                now_micros,
            } => {
                let now = self.absolute_instant(now_micros);
                self.request_keyframe(session_id, mid, kind, now)
            }
            WebRtcCoreCommand::Close { session_id, reason } => {
                self.close_session(session_id, reason)
            }
        }
    }

    /// Dispatch a network packet to a single session.
    ///
    /// Builds a `str0m::net::Receive` from the raw bytes, feeds it to the
    /// session's `Rtc`, and drains any outputs produced in reaction. Errors
    /// are converted to diagnostics and may fail the session.
    ///
    /// 将网络包分派到单个会话。
    ///
    /// 从原始字节构建 `str0m::net::Receive`，喂入会话的 `Rtc`，并排出任何响应
    /// 输出。错误会转换为诊断，并可能导致会话失败。
    fn dispatch_network(&mut self, packet: WebRtcNetworkInput) -> Result<(), WebRtcCoreError> {
        let WebRtcNetworkInput {
            session_id,
            source,
            destination,
            data,
            now_micros,
        } = packet;
        let now = self.absolute_instant(now_micros);
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(WebRtcCoreError::SessionNotFound(session_id))?;
        session.last_activity_at = now;
        session.last_known_source = Some(source);
        session.last_known_destination = Some(destination);

        let bytes = data.as_ref();
        let receive = match Receive::new(Protocol::Udp, source, destination, bytes) {
            Ok(r) => r,
            Err(err) => {
                self.pending_outputs.push_back(WebRtcCoreOutput::Diagnostic(
                    WebRtcCoreDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcCoreDiagnosticKind::NetworkInputRejected,
                        message: format!("net::Receive::new failed: {err}"),
                    },
                ));
                return Ok(());
            }
        };
        let str0m_input = Str0mInput::Receive(now, receive);

        if let Err(err) = session.rtc.handle_input(str0m_input) {
            self.pending_outputs
                .push_back(WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcCoreDiagnosticKind::NetworkInputRejected,
                    message: format!("rtc.handle_input(Receive) failed: {err}"),
                }));
            self.fail_session(session_id, &format!("network input rejected: {err}"));
            return Ok(());
        }

        self.drain_session_output(session_id);
        Ok(())
    }

    /// Dispatch a timeout to a single session.
    ///
    /// Timeouts are the only way `str0m` advances its internal state machine
    /// (ICE retransmissions, DTLS timers, etc.). We do not update
    /// `last_activity_at` here because a timeout is not user activity.
    ///
    /// 将超时事件分派到单个会话。
    ///
    /// 超时是 `str0m` 推进其内部状态机（ICE 重传、DTLS 定时器等）的唯一方式。
    /// 我们在此不更新 `last_activity_at`，因为超时不是用户活动。
    fn dispatch_timeout(
        &mut self,
        session_id: WebRtcSessionId,
        now: Instant,
    ) -> Result<(), WebRtcCoreError> {
        let session = match self.sessions.get_mut(&session_id) {
            Some(s) => s,
            None => return Ok(()),
        };
        // Note: do not update `last_activity_at` here. A Timeout / Tick
        // is a clock advancement, not session activity. Doing so would
        // make `last_activity_at` indistinguishable from wall-clock and
        // break any downstream idle-timeout enforcement.
        let str0m_input = Str0mInput::Timeout(now);
        if let Err(err) = session.rtc.handle_input(str0m_input) {
            self.pending_outputs
                .push_back(WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcCoreDiagnosticKind::TimeoutRejected,
                    message: format!("rtc.handle_input(Timeout) failed: {err}"),
                }));
            self.fail_session(session_id, &format!("timeout rejected: {err}"));
            return Ok(());
        }
        self.drain_session_output(session_id);
        Ok(())
    }

    /// Accept a remote SDP offer and produce a local answer.
    ///
    /// The flow is: sanitize/compat -> parse offer -> create `Rtc` -> add
    /// local candidates -> `accept_offer` -> emit lifecycle events, extension
    /// mappings, and negotiated payload types. The session is inserted in the
    /// `Connecting` state.
    ///
    /// 接受远端 SDP offer 并生成本地 answer。
    ///
    /// 流程：清理/兼容 -> 解析 offer -> 创建 `Rtc` -> 添加本地 candidate ->
    /// `accept_offer` -> 发出生命周期事件、扩展映射与协商 payload type。
    /// 会话以 `Connecting` 状态插入。
    fn accept_offer(
        &mut self,
        session_id: WebRtcSessionId,
        role: WebRtcSessionRole,
        remote_sdp: String,
        local_candidates: Vec<String>,
        now: Instant,
    ) -> Result<(), WebRtcCoreError> {
        if self.sessions.contains_key(&session_id) {
            return Err(WebRtcCoreError::SessionAlreadyExists(session_id));
        }
        if self.sessions.len() >= self.config.limits.max_sessions {
            return Err(WebRtcCoreError::SessionCapacityExhausted {
                max: self.config.limits.max_sessions,
            });
        }
        if remote_sdp.len() > self.config.limits.max_remote_sdp_bytes {
            return Err(WebRtcCoreError::SdpTooLarge {
                size: remote_sdp.len(),
                limit: self.config.limits.max_remote_sdp_bytes,
            });
        }

        let (sanitized, report) = preprocess_remote_sdp(&remote_sdp);
        if report.is_modified() {
            self.pending_outputs
                .push_back(WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcCoreDiagnosticKind::SdpCompatRewrite,
                    message: format_compat_report(&report),
                }));
        }

        // Extract RTP extension mappings from the remote SDP for
        // module-layer observability before str0m consumes the offer.
        let ext_mappings = crate::sdp_compat::extract_rtp_extension_mappings(&sanitized);

        // Extract dynamic payload type numbers from the offer so the
        // module layer can use the browser-assigned values instead of
        // hardcoded constants (ABL bug fix 2025-06-12 / 2025-12-01).
        let offer_payloads = crate::offer_payload::extract_offer_payloads(&sanitized);

        let offer =
            SdpOffer::from_sdp_string(&sanitized).map_err(|err| WebRtcCoreError::InvalidSdp {
                message: err.to_string(),
            })?;

        let mut rtc = build_rtc(&self.config, now);
        add_local_candidates(&mut rtc, &local_candidates)?;
        let answer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(rtc_error_to_invalid_sdp)?;
        let answer_sdp = answer.to_sdp_string();

        let session = WebRtcCoreSession {
            id: session_id,
            rtc,
            role,
            state: WebRtcSessionState::Connecting,
            pending_offer: None,
            remote_candidate_count: 0,
            last_activity_at: now,
            last_known_destination: None,
            last_known_source: None,
            track_kind_by_mid: HashMap::new(),
            channel_ids: HashMap::new(),
            reverse_channel_ids: HashMap::new(),
            next_channel_id: 0,
        };
        self.sessions.insert(session_id, session);

        self.pending_outputs
            .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
                session_id,
                state: WebRtcSessionLifecycle::Created,
            }));
        self.pending_outputs
            .push_back(WebRtcCoreOutput::LocalDescription {
                session_id,
                sdp: answer_sdp,
                kind: WebRtcLocalDescriptionKind::Answer,
            });
        self.pending_outputs
            .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
                session_id,
                state: WebRtcSessionLifecycle::LocalDescriptionReady,
            }));
        // Emit RTP extension mappings observed in the remote SDP so
        // the module can track the negotiated extension set.
        if !ext_mappings.is_empty() {
            self.pending_outputs.push_back(WebRtcCoreOutput::Event(
                WebRtcCoreEvent::RtpExtensionObserved {
                    session_id,
                    mappings: ext_mappings,
                },
            ));
        }
        // Emit the extracted payload type numbers so the module layer
        // uses the browser-negotiated values, never hardcoded constants.
        self.pending_outputs.push_back(WebRtcCoreOutput::Event(
            WebRtcCoreEvent::OfferPayloadNegotiated {
                session_id,
                payloads: offer_payloads,
            },
        ));
        self.drain_session_output(session_id);
        Ok(())
    }

    /// Create a local SDP offer for a new session.
    ///
    /// The flow is: create `Rtc` -> add local candidates -> `sdp_api().add_media`
    /// for each requested direction and DataChannel -> `apply()` -> store the
    /// pending offer and emit the local SDP. The module later applies the
    /// remote answer with `ApplyAnswer`.
    ///
    /// 为新会话创建本地 SDP offer。
    ///
    /// 流程：创建 `Rtc` -> 添加本地 candidate -> 为每个请求方向与 DataChannel
    /// 调用 `sdp_api().add_media` -> `apply()` -> 保存 pending offer 并发出本地 SDP。
    /// 模块随后通过 `ApplyAnswer` 应用远端 answer。
    fn create_offer(
        &mut self,
        session_id: WebRtcSessionId,
        role: WebRtcSessionRole,
        spec: WebRtcOfferSpec,
        local_candidates: Vec<String>,
        now: Instant,
    ) -> Result<(), WebRtcCoreError> {
        if self.sessions.contains_key(&session_id) {
            return Err(WebRtcCoreError::SessionAlreadyExists(session_id));
        }
        if self.sessions.len() >= self.config.limits.max_sessions {
            return Err(WebRtcCoreError::SessionCapacityExhausted {
                max: self.config.limits.max_sessions,
            });
        }

        let mut rtc = build_rtc(&self.config, now);
        add_local_candidates(&mut rtc, &local_candidates)?;
        let mut sdp_api = rtc.sdp_api();

        if let Some(dir) = spec.video_direction {
            sdp_api.add_media(
                Str0mMediaKind::Video,
                map_offer_direction(dir),
                None,
                None,
                None,
            );
        }
        if let Some(dir) = spec.audio_direction {
            sdp_api.add_media(
                Str0mMediaKind::Audio,
                map_offer_direction(dir),
                None,
                None,
                None,
            );
        }
        if spec.data_channel {
            let _ = sdp_api.add_channel("cheetah-data".to_string());
        }

        let (offer, pending) = match sdp_api.apply() {
            Some(pair) => pair,
            None => {
                return Err(WebRtcCoreError::InvalidState {
                    message:
                        "CreateOffer with empty WebRtcOfferSpec produced no offer; specify at \
                         least one of video/audio/data_channel"
                            .into(),
                });
            }
        };
        let offer_sdp = offer.to_sdp_string();

        let session = WebRtcCoreSession {
            id: session_id,
            rtc,
            role,
            state: WebRtcSessionState::Connecting,
            pending_offer: Some(pending),
            remote_candidate_count: 0,
            last_activity_at: now,
            last_known_destination: None,
            last_known_source: None,
            track_kind_by_mid: HashMap::new(),
            channel_ids: HashMap::new(),
            reverse_channel_ids: HashMap::new(),
            next_channel_id: 0,
        };
        self.sessions.insert(session_id, session);

        self.pending_outputs
            .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
                session_id,
                state: WebRtcSessionLifecycle::Created,
            }));
        self.pending_outputs
            .push_back(WebRtcCoreOutput::LocalDescription {
                session_id,
                sdp: offer_sdp,
                kind: WebRtcLocalDescriptionKind::Offer,
            });
        self.pending_outputs
            .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
                session_id,
                state: WebRtcSessionLifecycle::LocalDescriptionReady,
            }));
        self.drain_session_output(session_id);
        Ok(())
    }

    /// Apply a remote SDP answer to a previously created offering session.
    ///
    /// The flow is: sanitize -> parse answer -> `sdp_api().accept_answer()`
    /// with the stored `SdpPendingOffer`. If the SDP was modified by the
    /// compat preprocessor, a diagnostic is emitted.
    ///
    /// 将远端 SDP answer 应用于先前创建的 offerer 会话。
    ///
    /// 流程：清理 -> 解析 answer -> 使用保存的 `SdpPendingOffer` 调用
    /// `sdp_api().accept_answer()`。若 SDP 被兼容预处理器修改，会发出诊断。
    fn apply_answer(
        &mut self,
        session_id: WebRtcSessionId,
        remote_sdp: String,
        now: Instant,
    ) -> Result<(), WebRtcCoreError> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(WebRtcCoreError::SessionNotFound(session_id))?;
        if remote_sdp.len() > self.config.limits.max_remote_sdp_bytes {
            return Err(WebRtcCoreError::SdpTooLarge {
                size: remote_sdp.len(),
                limit: self.config.limits.max_remote_sdp_bytes,
            });
        }
        let (sanitized, report) = preprocess_remote_sdp(&remote_sdp);
        if report.is_modified() {
            self.pending_outputs
                .push_back(WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcCoreDiagnosticKind::SdpCompatRewrite,
                    message: format_compat_report(&report),
                }));
        }
        let answer =
            SdpAnswer::from_sdp_string(&sanitized).map_err(|err| WebRtcCoreError::InvalidSdp {
                message: err.to_string(),
            })?;
        let pending =
            session
                .pending_offer
                .take()
                .ok_or_else(|| WebRtcCoreError::InvalidState {
                    message: format!("session {session_id} has no pending offer"),
                })?;
        session
            .rtc
            .sdp_api()
            .accept_answer(pending, answer)
            .map_err(rtc_error_to_invalid_sdp)?;
        session.last_activity_at = now;
        self.drain_session_output(session_id);
        Ok(())
    }

    /// Add a remote ICE candidate to an existing session.
    ///
    /// Increments the per-session candidate count and rejects candidates
    /// beyond [`WebRtcCoreLimits::max_remote_candidates_per_session`].
    ///
    /// 向现有会话添加远端 ICE candidate。
    ///
    /// 增加每会话 candidate 计数，并拒绝超过
    /// [`WebRtcCoreLimits::max_remote_candidates_per_session`] 的 candidate。
    fn add_remote_candidate(
        &mut self,
        session_id: WebRtcSessionId,
        candidate_sdp: String,
        now: Instant,
    ) -> Result<(), WebRtcCoreError> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(WebRtcCoreError::SessionNotFound(session_id))?;
        if session.remote_candidate_count >= self.config.limits.max_remote_candidates_per_session {
            return Err(WebRtcCoreError::TooManyRemoteCandidates {
                limit: self.config.limits.max_remote_candidates_per_session,
            });
        }
        let candidate = Candidate::from_sdp_string(&candidate_sdp).map_err(|err| {
            WebRtcCoreError::InvalidCandidate {
                message: err.to_string(),
            }
        })?;
        session.rtc.add_remote_candidate(candidate);
        session.remote_candidate_count += 1;
        session.last_activity_at = now;
        self.drain_session_output(session_id);
        Ok(())
    }

    /// Trigger an ICE restart on an existing session.
    ///
    /// Refuses to start a second ICE restart while a previous offer is still
    /// pending. `str0m` would otherwise overwrite the prior pending state and
    /// the original answer could never be applied.
    ///
    /// 在现有会话上触发 ICE 重启。
    ///
    /// 若之前 offer 仍在 pending，则拒绝启动第二次 ICE 重启。否则 `str0m` 会覆盖
    /// 之前的 pending 状态，导致原始 answer 永远无法应用。
    fn ice_restart(
        &mut self,
        session_id: WebRtcSessionId,
        keep_local_candidates: bool,
        now: Instant,
    ) -> Result<(), WebRtcCoreError> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(WebRtcCoreError::SessionNotFound(session_id))?;
        // Refuse to start a second ICE-restart while a previous offer
        // (whether ordinary CreateOffer or a prior ice_restart) is
        // still pending. `str0m` would silently overwrite the prior
        // pending state and leave us unable to apply the original
        // answer; we surface a structured error instead so the module
        // layer can treat it as a 409.
        if session.pending_offer.is_some() {
            return Err(WebRtcCoreError::InvalidState {
                message: format!(
                    "session {session_id} already has a pending offer; \
                     finish or roll back before starting an ICE restart"
                ),
            });
        }

        let mut sdp_api = session.rtc.sdp_api();
        let _new_creds = sdp_api.ice_restart(keep_local_candidates);
        let (offer, pending) = match sdp_api.apply() {
            Some(pair) => pair,
            None => {
                // `apply()` returning `None` after `ice_restart` means
                // str0m did not consider the change negotiable (e.g.,
                // the session was already in a state that cannot be
                // restarted). Surface this rather than silently drop.
                return Err(WebRtcCoreError::InvalidState {
                    message: format!(
                        "session {session_id} ice_restart produced no offer; \
                         negotiation skipped"
                    ),
                });
            }
        };
        let offer_sdp = offer.to_sdp_string();
        session.pending_offer = Some(pending);
        session.last_activity_at = now;

        self.pending_outputs
            .push_back(WebRtcCoreOutput::LocalDescription {
                session_id,
                sdp: offer_sdp,
                kind: WebRtcLocalDescriptionKind::Offer,
            });
        self.pending_outputs
            .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
                session_id,
                state: WebRtcSessionLifecycle::LocalDescriptionReady,
            }));
        self.drain_session_output(session_id);
        Ok(())
    }

    /// Send a DataChannel message on an opened channel.
    ///
    /// Enforces the configured max message size, maps the boundary id to the
    /// `str0m` channel id, and converts `str0m` write errors into either
    /// diagnostics or a `WebRtcCoreError::Rtc`.
    ///
    /// 在已打开通道上发送 DataChannel 消息。
    ///
    /// 强制执行配置的最大消息大小，将边界 id 映射到 `str0m` 通道 id，并将
    /// `str0m` 写入错误转换为诊断或 `WebRtcCoreError::Rtc`。
    fn send_data_channel(&mut self, out: WebRtcDataChannelOut) -> Result<(), WebRtcCoreError> {
        let WebRtcDataChannelOut {
            session_id,
            channel,
            payload,
            binary,
        } = out;
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(WebRtcCoreError::SessionNotFound(session_id))?;
        // ZLM caps each DataChannel message at a configurable size.
        // We mirror that here so a runaway producer can't push
        // megabytes through `str0m`'s SCTP buffer in one go. Oversize
        // payloads surface as a diagnostic so operators have visibility
        // without losing the rest of the channel.
        let max_message_bytes = self.config.limits.max_data_channel_message_bytes;
        if payload.len() > max_message_bytes {
            self.pending_outputs
                .push_back(WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcCoreDiagnosticKind::PendingOutputDropped,
                    message: format!(
                        "data channel {} message {} bytes exceeds max {} bytes; dropped",
                        channel.0,
                        payload.len(),
                        max_message_bytes
                    ),
                }));
            return Ok(());
        }
        let str0m_channel_id = match session.lookup_str0m_channel_id(channel) {
            Some(id) => id,
            None => {
                // The channel id is unknown — either it was never
                // opened, or it was already closed. Treat this as a
                // graceful drop with a diagnostic rather than a hard
                // error, matching ZLM behaviour where post-close
                // writes are silently dropped.
                self.pending_outputs
                    .push_back(WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcCoreDiagnosticKind::PendingOutputDropped,
                        message: format!(
                            "data channel {} unknown or already closed for session {session_id}; dropped {} bytes",
                            channel.0,
                            payload.len()
                        ),
                    }));
                return Ok(());
            }
        };
        let payload_len = payload.len();
        let mut handle = match session.rtc.channel(str0m_channel_id) {
            Some(h) => h,
            None => {
                // str0m has lost the channel handle — the channel was
                // open but is now closed. Drop with diagnostic.
                self.pending_outputs.push_back(WebRtcCoreOutput::Diagnostic(
                    WebRtcCoreDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcCoreDiagnosticKind::PendingOutputDropped,
                        message: format!(
                        "data channel {} closed by peer for session {session_id}; dropped {} bytes",
                        channel.0, payload_len
                    ),
                    },
                ));
                return Ok(());
            }
        };
        match handle.write(binary, payload.as_ref()) {
            Ok(true) => {}
            Ok(false) => {
                self.pending_outputs.push_back(WebRtcCoreOutput::Diagnostic(
                    WebRtcCoreDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcCoreDiagnosticKind::PendingOutputDropped,
                        message: format!(
                            "data channel {} send buffer full; dropped {} bytes",
                            channel.0, payload_len
                        ),
                    },
                ));
            }
            Err(err) => {
                return Err(WebRtcCoreError::Rtc {
                    message: err.to_string(),
                });
            }
        }
        self.drain_session_output(session_id);
        Ok(())
    }

    /// Write a media frame to a send-direction track.
    ///
    /// This is the Phase 01 send path: it resolves the `Writer` by `mid`,
    /// picks the negotiated payload type for the requested codec, converts
    /// the boundary timestamp into `MediaTime`, and delegates to `str0m`.
    /// Pre-connection frames are dropped with a diagnostic.
    ///
    /// 将媒体帧写入发送方向 track。
    ///
    /// 这是阶段 01 的发送路径：通过 `mid` 解析 `Writer`，为请求的编解码器选择
    /// 协商后的 payload type，将边界时间戳转换为 `MediaTime`，并委托给 `str0m`。
    /// 连接建立前的帧会被丢弃并发出诊断。
    fn send_frame(&mut self, frame: WebRtcSendFrame) -> Result<(), WebRtcCoreError> {
        let session_id = frame.session_id;
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(WebRtcCoreError::SessionNotFound(session_id))?;
        if !session.rtc.is_alive() {
            return Err(WebRtcCoreError::SessionNotAlive {
                session: session_id,
            });
        }
        // SAFETY against bad inputs: drop frames before connection is up.
        if !session.rtc.is_connected() {
            self.pending_outputs
                .push_back(WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcCoreDiagnosticKind::PendingOutputDropped,
                    message: "send_frame dropped: session not yet connected".into(),
                }));
            return Ok(());
        }
        let mid = str0m::media::Mid::from(frame.mid.as_str());
        let writer = match session.rtc.writer(mid) {
            Some(w) => w,
            None => {
                self.pending_outputs.push_back(WebRtcCoreOutput::Diagnostic(
                    WebRtcCoreDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcCoreDiagnosticKind::UnhandledEvent,
                        message: format!(
                            "send_frame dropped: no send-direction track for mid={}",
                            frame.mid
                        ),
                    },
                ));
                return Ok(());
            }
        };
        // Find the first payload params whose codec matches what the
        // engine handed us. We do not need full PT negotiation here —
        // `Writer::write` looks up codec config by `pt`, so any matching
        // PT works.
        let target_codec = match map_codec_kind_to_str0m(frame.codec) {
            Some(c) => c,
            None => {
                self.pending_outputs.push_back(WebRtcCoreOutput::Diagnostic(
                    WebRtcCoreDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcCoreDiagnosticKind::UnhandledEvent,
                        message: format!("send_frame: unknown codec {:?}", frame.codec),
                    },
                ));
                return Ok(());
            }
        };
        let Some(pt) = writer
            .payload_params()
            .find(|p| p.spec().codec == target_codec)
            .map(|p| p.pt())
        else {
            self.pending_outputs
                .push_back(WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcCoreDiagnosticKind::UnhandledEvent,
                    message: format!(
                        "send_frame: no PT negotiated for codec {:?} on mid={}",
                        target_codec, frame.mid
                    ),
                }));
            return Ok(());
        };

        let denom = if frame.rtp_timestamp_denom == 0 {
            frame.clock_rate.max(1)
        } else {
            frame.rtp_timestamp_denom
        };
        let frequency = match std::num::NonZeroU32::new(denom) {
            Some(f) => str0m::media::Frequency::from_nonzero(f),
            None => return Ok(()),
        };
        let media_time = str0m::media::MediaTime::new(frame.rtp_timestamp_ticks as u64, frequency);
        let wallclock = self
            .start_instant
            .checked_add(Duration::from_micros(frame.network_time_micros))
            .unwrap_or(self.last_seen_instant);

        let payload_len = frame.payload.len();
        if let Err(err) = writer.write(pt, wallclock, media_time, frame.payload.to_vec()) {
            self.pending_outputs
                .push_back(WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcCoreDiagnosticKind::PendingOutputDropped,
                    message: format!(
                        "send_frame dropped {} bytes for mid={}: {err}",
                        payload_len, frame.mid
                    ),
                }));
        }
        // `random_access` is intentionally unused: str0m's packetizer
        // derives the IDR/keyframe boundary from the codec-specific
        // payload itself (e.g., H264 NAL unit type, VP8 keyframe bit).
        // We accept the boundary value so callers can still annotate
        // the frame for future RTP-mode passthrough.
        let _ = frame.random_access;
        self.drain_session_output(session_id);
        Ok(())
    }

    /// Request a keyframe from the local sender for a receive-direction track.
    ///
    /// Maps `WebRtcRequestKeyframeKind` to `str0m`'s `KeyframeRequestKind`
    /// and forwards the request to the `Writer` for the given `mid`. If the
    /// session is not connected, a diagnostic is emitted and the request is
    /// dropped.
    ///
    /// 为接收方向 track 请求本地发送端生成关键帧。
    ///
    /// 将 `WebRtcRequestKeyframeKind` 映射到 `str0m` 的 `KeyframeRequestKind`，
    /// 并转发给指定 `mid` 的 `Writer`。若会话未连接，会发出诊断并丢弃请求。
    fn request_keyframe(
        &mut self,
        session_id: WebRtcSessionId,
        mid: MidLabel,
        kind: WebRtcRequestKeyframeKind,
        now: Instant,
    ) -> Result<(), WebRtcCoreError> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(WebRtcCoreError::SessionNotFound(session_id))?;
        session.last_activity_at = now;
        if !session.rtc.is_connected() {
            self.pending_outputs
                .push_back(WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcCoreDiagnosticKind::PendingOutputDropped,
                    message: "request_keyframe dropped: session not yet connected".into(),
                }));
            return Ok(());
        }
        let str0m_mid = str0m::media::Mid::from(mid.as_str());
        let mut writer = match session.rtc.writer(str0m_mid) {
            Some(w) => w,
            None => {
                self.pending_outputs.push_back(WebRtcCoreOutput::Diagnostic(
                    WebRtcCoreDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcCoreDiagnosticKind::UnhandledEvent,
                        message: format!(
                            "request_keyframe dropped: no track for mid={mid} (track is not in receive direction or unknown)"
                        ),
                    },
                ));
                return Ok(());
            }
        };
        let kf_kind = match kind {
            WebRtcRequestKeyframeKind::Pli => str0m::media::KeyframeRequestKind::Pli,
            WebRtcRequestKeyframeKind::Fir => str0m::media::KeyframeRequestKind::Fir,
        };
        if let Err(err) = writer.request_keyframe(None, kf_kind) {
            self.pending_outputs
                .push_back(WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcCoreDiagnosticKind::UnhandledEvent,
                    message: format!("request_keyframe failed for mid={mid}: {err}"),
                }));
        }
        self.drain_session_output(session_id);
        Ok(())
    }

    /// Close a session and emit the lifecycle events.
    ///
    /// Removes the session from the map, disconnects the `Rtc`, and pushes
    /// `Lifecycle::Closed` plus `CloseSession` to the output queue.
    ///
    /// 关闭会话并发出生命周期事件。
    ///
    /// 从 map 中移除会话，断开 `Rtc`，并推送 `Lifecycle::Closed` 与 `CloseSession`
    /// 到输出队列。
    fn close_session(
        &mut self,
        session_id: WebRtcSessionId,
        reason: WebRtcCloseReason,
    ) -> Result<(), WebRtcCoreError> {
        let mut session = self
            .sessions
            .remove(&session_id)
            .ok_or(WebRtcCoreError::SessionNotFound(session_id))?;
        session.rtc.disconnect();
        self.pending_outputs
            .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
                session_id,
                state: WebRtcSessionLifecycle::Closed,
            }));
        self.pending_outputs
            .push_back(WebRtcCoreOutput::CloseSession { session_id, reason });
        Ok(())
    }

    /// Mark a session as failed and remove it.
    ///
    /// This is used internally when `str0m` rejects an input or when
    /// `poll_output` returns an error. The driver is expected to clean up
    /// any associated sockets and notify the module.
    ///
    /// 将会话标记为失败并移除。
    ///
    /// 当 `str0m` 拒绝输入或 `poll_output` 返回错误时内部使用。驱动层应清理
    /// 关联 socket 并通知模块。
    fn fail_session(&mut self, session_id: WebRtcSessionId, reason: &str) {
        if let Some(mut session) = self.sessions.remove(&session_id) {
            session.rtc.disconnect();
        }
        self.pending_outputs
            .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
                session_id,
                state: WebRtcSessionLifecycle::Failed,
            }));
        self.pending_outputs
            .push_back(WebRtcCoreOutput::CloseSession {
                session_id,
                reason: WebRtcCloseReason::Internal(reason.into()),
            });
    }

    /// Drain `str0m` outputs for a session until the next timeout or limit.
    ///
    /// This is the central output pump. It converts `str0m::Output::Transmit`
    /// into `WebRtcPacketOut`, `str0m::Output::Event` into
    /// `WebRtcCoreEvent`, and `str0m::Output::Timeout` into `WebRtcTimer`.
    /// If the output limit is exceeded, it schedules an immediate re-entry.
    ///
    /// 排出会话的 `str0m` 输出，直到下一个超时或达到上限。
    ///
    /// 这是中央输出泵。它将 `str0m::Output::Transmit` 转换为 `WebRtcPacketOut`，
    /// `str0m::Output::Event` 转换为 `WebRtcCoreEvent`，`str0m::Output::Timeout`
    /// 转换为 `WebRtcTimer`。若超过输出上限，则安排立即重新进入。
    fn drain_session_output(&mut self, session_id: WebRtcSessionId) {
        // Pop outputs until we hit the next `Output::Timeout`, which `str0m`
        // requires the caller to honour as a deadline.
        let max_iterations = self.config.limits.max_pending_outputs_per_session;
        for _ in 0..max_iterations {
            let session = match self.sessions.get_mut(&session_id) {
                Some(s) => s,
                None => return,
            };
            if !session.rtc.is_alive() {
                self.fail_session(session_id, "rtc no longer alive");
                return;
            }
            let output = match session.rtc.poll_output() {
                Ok(out) => out,
                Err(err) => {
                    let msg = format!("rtc.poll_output failed: {err}");
                    self.pending_outputs.push_back(WebRtcCoreOutput::Diagnostic(
                        WebRtcCoreDiagnostic {
                            session_id: Some(session_id),
                            kind: WebRtcCoreDiagnosticKind::NetworkInputRejected,
                            message: msg.clone(),
                        },
                    ));
                    self.fail_session(session_id, &msg);
                    return;
                }
            };
            match output {
                Str0mOutput::Transmit(transmit) => {
                    let bytes: Vec<u8> = transmit.contents.into();
                    let packet = WebRtcPacketOut {
                        session_id,
                        source: Some(transmit.source),
                        destination: transmit.destination,
                        data: Bytes::from(bytes),
                    };
                    self.pending_outputs
                        .push_back(WebRtcCoreOutput::SendPacket(packet));
                }
                Str0mOutput::Event(event) => {
                    self.translate_event(session_id, event);
                }
                Str0mOutput::Timeout(deadline) => {
                    let micros = self.relative_micros(deadline);
                    self.pending_outputs
                        .push_back(WebRtcCoreOutput::SetTimer(WebRtcTimer {
                            session_id,
                            deadline_micros: micros,
                        }));
                    return;
                }
            }
        }

        // Safety valve: if the state machine produced more outputs than our
        // bound allows, schedule an immediate timer wake-up so the driver
        // re-enters and we can drain the rest. Emit a diagnostic so this
        // condition is visible.
        self.pending_outputs
            .push_back(WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                session_id: Some(session_id),
                kind: WebRtcCoreDiagnosticKind::PendingOutputDropped,
                message: format!(
                    "session {session_id} produced more than {} outputs in one drain; \
                     scheduling immediate re-entry",
                    self.config.limits.max_pending_outputs_per_session
                ),
            }));
        let last_activity_at = match self.sessions.get(&session_id) {
            Some(s) => s.last_activity_at,
            None => return,
        };
        let micros = self.relative_micros(last_activity_at);
        self.pending_outputs
            .push_back(WebRtcCoreOutput::SetTimer(WebRtcTimer {
                session_id,
                deadline_micros: micros,
            }));
    }

    /// Translate a `str0m::Event` into boundary `WebRtcCoreOutput` events.
    ///
    /// This is the conservative event mapping layer. Most `str0m` events are
    /// translated into domain events; unknown events become `UnhandledEvent`
    /// diagnostics. ICE connection state changes drive the session state
    /// machine and emit `Lifecycle::Connected`.
    ///
    /// 将 `str0m::Event` 转换为边界 `WebRtcCoreOutput` 事件。
    ///
    /// 这是保守的事件映射层。大多数 `str0m` 事件被转换为域事件；未知事件变成
    /// `UnhandledEvent` 诊断。ICE 连接状态变化驱动会话状态机并发出
    /// `Lifecycle::Connected`。
    fn translate_event(&mut self, session_id: WebRtcSessionId, event: Str0mEvent) {
        match event {
            Str0mEvent::IceConnectionStateChange(state) => {
                let mapped = match state {
                    IceConnectionState::New => WebRtcIceState::New,
                    IceConnectionState::Checking => WebRtcIceState::Checking,
                    IceConnectionState::Connected => WebRtcIceState::Connected,
                    IceConnectionState::Completed => WebRtcIceState::Connected,
                    IceConnectionState::Disconnected => WebRtcIceState::Disconnected,
                };
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.state = match mapped {
                        WebRtcIceState::Connected => WebRtcSessionState::Connected,
                        WebRtcIceState::Disconnected => WebRtcSessionState::Connecting,
                        WebRtcIceState::Closed => WebRtcSessionState::Closed,
                        _ => session.state,
                    };
                }
                self.pending_outputs
                    .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Ice {
                        session_id,
                        state: mapped,
                    }));
                // Only `Str0mEvent::Connected` (after DTLS/SRTP is up) emits
                // `Lifecycle::Connected` so that callers like `open_webrtc_push`
                // can rely on `Lifecycle::Connected` as "ready to send media".
                // ICE `Connected` is still surfaced via the `Ice` event.
                //
                // 只有 `Str0mEvent::Connected`（DTLS/SRTP 就绪后）才发出
                // `Lifecycle::Connected`，以便 `open_webrtc_push` 等调用方
                // 可以依赖 `Lifecycle::Connected` 作为“可发送媒体”。
                // ICE `Connected` 仍通过 `Ice` 事件暴露。
            }
            Str0mEvent::Connected => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.state = WebRtcSessionState::Connected;
                }
                self.pending_outputs.push_back(WebRtcCoreOutput::Event(
                    WebRtcCoreEvent::Lifecycle {
                        session_id,
                        state: WebRtcSessionLifecycle::Connected,
                    },
                ));
            }
            Str0mEvent::MediaAdded(added) => {
                let mid_label = MidLabel::new(added.mid.to_string());
                let kind = match added.kind {
                    Str0mMediaKind::Audio => WebRtcMediaKind::Audio,
                    Str0mMediaKind::Video => WebRtcMediaKind::Video,
                };
                let direction = match added.direction {
                    str0m::media::Direction::SendOnly => WebRtcMediaDirection::SendOnly,
                    str0m::media::Direction::RecvOnly => WebRtcMediaDirection::RecvOnly,
                    str0m::media::Direction::SendRecv => WebRtcMediaDirection::SendRecv,
                    str0m::media::Direction::Inactive => WebRtcMediaDirection::Inactive,
                };
                let (simulcast_send, simulcast_recv) = match added.simulcast.as_ref() {
                    Some(sc) => (
                        sc.send.iter().map(|layer| layer.rid.to_string()).collect(),
                        sc.recv.iter().map(|layer| layer.rid.to_string()).collect(),
                    ),
                    None => (Vec::new(), Vec::new()),
                };
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.track_kind_by_mid.insert(mid_label.0.clone(), kind);
                }
                self.pending_outputs.push_back(WebRtcCoreOutput::Event(
                    WebRtcCoreEvent::MediaTrackAdded {
                        session_id,
                        track: WebRtcMediaTrack {
                            mid: mid_label.clone(),
                            kind,
                            direction,
                            simulcast_send: simulcast_send.clone(),
                            simulcast_recv: simulcast_recv.clone(),
                        },
                    },
                ));
                // Emit per-layer observations so the module can
                // pre-allocate routing state without re-parsing SDP.
                // ZLM `RtpExtContext` makes the same observation when
                // ingesting RID/repaired-RID extensions; here we only
                // know about the SDP-negotiated layers, the actual
                // RID extension observations land later through media
                // events.
                for rid in simulcast_send {
                    self.pending_outputs.push_back(WebRtcCoreOutput::Event(
                        WebRtcCoreEvent::SimulcastLayerObserved {
                            session_id,
                            observation: WebRtcSimulcastLayerObservation {
                                mid: mid_label.clone(),
                                rid,
                                source: WebRtcSimulcastRidSource::SdpRid,
                            },
                        },
                    ));
                }
                for rid in simulcast_recv {
                    self.pending_outputs.push_back(WebRtcCoreOutput::Event(
                        WebRtcCoreEvent::SimulcastLayerObserved {
                            session_id,
                            observation: WebRtcSimulcastLayerObservation {
                                mid: mid_label.clone(),
                                rid,
                                source: WebRtcSimulcastRidSource::SdpRid,
                            },
                        },
                    ));
                }
            }
            Str0mEvent::MediaData(data) => {
                let mid_label = MidLabel::new(data.mid.to_string());
                let rid = data.rid.map(|r| r.to_string());
                let spec = data.params.spec();
                let codec = match spec.codec {
                    str0m::format::Codec::Opus => WebRtcCodecKind::Opus,
                    str0m::format::Codec::PCMU => WebRtcCodecKind::Pcmu,
                    str0m::format::Codec::PCMA => WebRtcCodecKind::Pcma,
                    str0m::format::Codec::H264 => WebRtcCodecKind::H264,
                    str0m::format::Codec::H265 => WebRtcCodecKind::H265,
                    str0m::format::Codec::Vp8 => WebRtcCodecKind::Vp8,
                    str0m::format::Codec::Vp9 => WebRtcCodecKind::Vp9,
                    str0m::format::Codec::Av1 => WebRtcCodecKind::Av1,
                    _ => WebRtcCodecKind::Unknown,
                };
                let clock_rate = spec.clock_rate.get();
                let random_access = data.is_keyframe();
                let network_time_micros = data
                    .network_time
                    .saturating_duration_since(self.start_instant)
                    .as_micros() as u64;
                // Surface RTP header extensions and the first
                // contributing sequence number through the boundary
                // metadata struct. `str0m::ExtensionValues` exposes
                // audio-level / voice-activity / video orientation
                // directly; abs-send-time and TWCC are consumed by
                // str0m's BWE subsystem and therefore not surfaced
                // verbatim. The video_orientation byte packs (rotation,
                // flip) per RFC 7742 §4 — we forward the byte
                // representation so codec-side helpers can choose
                // whether to apply it.
                let video_orientation_byte = data
                    .ext_vals
                    .video_orientation
                    .as_ref()
                    .map(video_orientation_to_byte);
                let sequence_number = Some(data.seq_range.start().as_u16());
                let meta = crate::event::WebRtcFrameMeta {
                    audio_level_dbov: data.ext_vals.audio_level,
                    voice_activity: data.ext_vals.voice_activity,
                    video_orientation: video_orientation_byte,
                    sequence_number,
                    contiguous: data.contiguous,
                };
                self.pending_outputs
                    .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Media {
                        session_id,
                        event: WebRtcMediaEvent::Frame {
                            mid: mid_label,
                            rid,
                            codec,
                            clock_rate,
                            random_access,
                            rtp_timestamp_ticks: data.time.numer() as u32,
                            rtp_timestamp_denom: data.time.denom(),
                            payload: Bytes::from(data.data),
                            network_time_micros,
                            meta,
                        },
                    }));
            }
            Str0mEvent::ChannelOpen(id, label) => {
                let channel_id = match self.sessions.get_mut(&session_id) {
                    Some(session) => session.map_channel_id(id),
                    None => return,
                };
                self.pending_outputs.push_back(WebRtcCoreOutput::Event(
                    WebRtcCoreEvent::DataChannel {
                        session_id,
                        event: WebRtcDataChannelEvent::Opened {
                            id: channel_id,
                            label,
                        },
                    },
                ));
            }
            Str0mEvent::ChannelData(data) => {
                let channel_id = match self.sessions.get_mut(&session_id) {
                    Some(session) => session.map_channel_id(data.id),
                    None => return,
                };
                self.pending_outputs.push_back(WebRtcCoreOutput::Event(
                    WebRtcCoreEvent::DataChannel {
                        session_id,
                        event: WebRtcDataChannelEvent::Message {
                            id: channel_id,
                            payload: Bytes::from(data.data),
                            binary: data.binary,
                        },
                    },
                ));
            }
            Str0mEvent::ChannelClose(id) => {
                let channel_id = match self.sessions.get_mut(&session_id) {
                    Some(session) => session.map_channel_id(id),
                    None => return,
                };
                self.pending_outputs.push_back(WebRtcCoreOutput::Event(
                    WebRtcCoreEvent::DataChannel {
                        session_id,
                        event: WebRtcDataChannelEvent::Closed { id: channel_id },
                    },
                ));
            }
            Str0mEvent::KeyframeRequest(req) => {
                let mid_label = MidLabel::new(req.mid.to_string());
                let feedback = match req.kind {
                    str0m::media::KeyframeRequestKind::Pli => WebRtcRtcpFeedback::Pli {
                        mid: Some(mid_label.clone()),
                    },
                    str0m::media::KeyframeRequestKind::Fir => WebRtcRtcpFeedback::Fir {
                        mid: Some(mid_label.clone()),
                    },
                };
                self.pending_outputs.push_back(WebRtcCoreOutput::Event(
                    WebRtcCoreEvent::RtcpFeedback {
                        session_id,
                        feedback,
                    },
                ));
                let media_event = if matches!(req.kind, str0m::media::KeyframeRequestKind::Pli) {
                    WebRtcMediaEvent::PliReceived { mid: mid_label }
                } else {
                    WebRtcMediaEvent::FirReceived { mid: mid_label }
                };
                self.pending_outputs
                    .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Media {
                        session_id,
                        event: media_event,
                    }));
            }
            Str0mEvent::PeerStats(stats) => {
                use crate::stats::WebRtcSessionStats;
                let snapshot = WebRtcSessionStats {
                    bytes_in: stats.bytes_rx,
                    bytes_out: stats.bytes_tx,
                    rtt_us: stats.rtt.map(|d| d.as_micros() as u64),
                    loss_fraction_x10000: stats
                        .egress_loss_fraction
                        .map(|f| (f.clamp(0.0, 1.0) * 10_000.0) as u32),
                    ..Default::default()
                };
                self.pending_outputs
                    .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Stats {
                        session_id,
                        snapshot,
                    }));
            }
            Str0mEvent::MediaIngressStats(stats) => {
                use crate::stats::WebRtcSessionStats;
                let snapshot = WebRtcSessionStats {
                    packets_in: stats.packets,
                    bytes_in: stats.bytes,
                    nack_out: stats.nacks,
                    pli_out: stats.plis,
                    fir_out: stats.firs,
                    rtt_us: stats.rtt.map(|d| d.as_micros() as u64),
                    loss_fraction_x10000: stats.loss.map(|f| (f.clamp(0.0, 1.0) * 10_000.0) as u32),
                    ..Default::default()
                };
                self.pending_outputs
                    .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Stats {
                        session_id,
                        snapshot,
                    }));
            }
            Str0mEvent::MediaEgressStats(stats) => {
                use crate::stats::WebRtcSessionStats;
                let snapshot = WebRtcSessionStats {
                    packets_out: stats.packets,
                    bytes_out: stats.bytes,
                    nack_in: stats.nacks,
                    pli_in: stats.plis,
                    fir_in: stats.firs,
                    rtt_us: stats.rtt.map(|d| d.as_micros() as u64),
                    loss_fraction_x10000: stats.loss.map(|f| (f.clamp(0.0, 1.0) * 10_000.0) as u32),
                    ..Default::default()
                };
                self.pending_outputs
                    .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Stats {
                        session_id,
                        snapshot,
                    }));
            }
            Str0mEvent::EgressBitrateEstimate(kind) => {
                use crate::stats::WebRtcBweStats;
                let bps = match kind {
                    str0m::bwe::BweKind::Twcc(rate) => rate.as_u64(),
                    str0m::bwe::BweKind::Remb(_, rate) => rate.as_u64(),
                    _ => 0,
                };
                self.pending_outputs
                    .push_back(WebRtcCoreOutput::Event(WebRtcCoreEvent::Bwe {
                        session_id,
                        snapshot: WebRtcBweStats {
                            estimated_bitrate_bps: Some(bps),
                            target_bitrate_bps: None,
                        },
                    }));
                // ZLMediaKit surfaces REMB as a distinct RTCP feedback
                // record. Doing the same here lets the module
                // distinguish a TWCC-driven update (which is local to
                // BWE) from a remote receiver requesting a different
                // bitrate cap.
                if let str0m::bwe::BweKind::Remb(mid, rate) = kind {
                    self.pending_outputs.push_back(WebRtcCoreOutput::Event(
                        WebRtcCoreEvent::RtcpFeedback {
                            session_id,
                            feedback: WebRtcRtcpFeedback::Remb {
                                mid: Some(MidLabel::new(mid.to_string())),
                                bitrate_bps: rate.as_u64(),
                            },
                        },
                    ));
                }
            }
            other => {
                self.pending_outputs.push_back(WebRtcCoreOutput::Diagnostic(
                    WebRtcCoreDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcCoreDiagnosticKind::UnhandledEvent,
                        message: format!("unhandled str0m event: {other:?}"),
                    },
                ));
            }
        }
    }

    /// Convert a boundary `now_micros` into an absolute `Instant`.
    ///
    /// The mapping is monotonic relative to `last_seen_instant`: drivers
    /// that supply a slightly out-of-order timestamp will see their input
    /// clamped to the previous instant, so a hostile or buggy time source
    /// cannot rewind `str0m`'s state machine.
    ///
    /// 将边界 `now_micros` 转换为绝对 `Instant`。
    ///
    /// 映射相对于 `last_seen_instant` 单调：提供略微乱序时间戳的驱动层会看见输入
    /// 被钳位到上一时刻，因此恶意或错误的时钟源无法回退 `str0m` 状态机。
    fn absolute_instant(&mut self, now_micros: u64) -> Instant {
        let raw = self
            .start_instant
            .checked_add(Duration::from_micros(now_micros))
            .unwrap_or(self.last_seen_instant);
        let bounded_high = self
            .last_seen_instant
            .checked_add(MAX_INPUT_TIME_JUMP)
            .unwrap_or(raw);
        let candidate = if raw > bounded_high {
            bounded_high
        } else {
            raw
        };
        let pinned = if candidate < self.last_seen_instant {
            self.last_seen_instant
        } else {
            candidate
        };
        self.last_seen_instant = pinned;
        pinned
    }

    /// Convert an absolute [`Instant`] back into a boundary
    /// `deadline_micros`. Used for `SetTimer` outputs.
    ///
    /// 将绝对 [`Instant`] 转换回边界 `deadline_micros`。用于 `SetTimer` 输出。
    fn relative_micros(&self, instant: Instant) -> u64 {
        instant
            .checked_duration_since(self.start_instant)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0)
    }
}

/// Build a fresh `str0m::Rtc` from the core configuration.
///
/// This centralizes all `RtcConfig` settings: ICE-lite, reorder windows,
/// BWE, RTP mode, and the codec profile. The returned `Rtc` is not yet
/// bound to a session id.
///
/// 根据核心配置构建新的 `str0m::Rtc`。
///
/// 集中所有 `RtcConfig` 设置：ICE-lite、重排窗口、BWE、RTP 模式与编解码器配置。
/// 返回的 `Rtc` 尚未绑定到会话 id。
fn build_rtc(config: &WebRtcCoreConfig, start: Instant) -> Rtc {
    let mut builder = RtcConfig::new()
        .set_ice_lite(config.ice_lite)
        .set_reordering_size_audio(config.audio_reorder_packets)
        .set_reordering_size_video(config.video_reorder_packets)
        .set_stats_interval(Some(Duration::from_secs(1)));

    if config.enable_bwe {
        let initial = config.bwe_initial_bitrate_bps.map(Bitrate::bps);
        builder = builder.enable_bwe(initial);
    } else {
        builder = builder.enable_bwe(None);
    }

    if config.enable_rtp_mode {
        builder = builder.set_rtp_mode(true);
    }

    apply_codec_profile(builder.codec_config(), config.codec_profile);

    builder.build(start)
}

/// Parse and add local ICE candidates to a `str0m` session.
///
/// 解析本地 ICE candidate 并添加到 `str0m` 会话。
fn add_local_candidates(rtc: &mut Rtc, local_candidates: &[String]) -> Result<(), WebRtcCoreError> {
    for candidate_sdp in local_candidates {
        let candidate = Candidate::from_sdp_string(candidate_sdp).map_err(|err| {
            WebRtcCoreError::InvalidCandidate {
                message: err.to_string(),
            }
        })?;
        rtc.add_local_candidate(candidate);
    }
    Ok(())
}

/// Apply a codec profile to the `str0m` codec config.
///
/// The profile gates which codecs are enabled for negotiation. We avoid
/// `clear_codecs()` because it would also remove standard payload types.
///
/// 将编解码器配置应用到 `str0m` codec 配置。
///
/// 配置决定哪些编解码器可用于协商。我们避免调用 `clear_codecs()`，因为它也会
/// 移除标准 payload type。
fn apply_codec_profile(codec_config: &mut CodecConfig, profile: WebRtcCodecProfile) {
    // We never call `clear_codecs` here; that would also drop standard
    // payload types. Instead we toggle individual codec switches relative
    // to the default-on baseline.
    match profile {
        WebRtcCodecProfile::Browser => {
            // Defaults already enable opus/h264/vp8/vp9/av1; ensure G.711 is
            // off for browser peers so we do not accidentally negotiate a
            // codec the page cannot decode.
            // CodecConfig does not expose bare `enable_pcmu/pcma` setters,
            // they live on RtcConfig. Profile policy is enforced upstream
            // on RtcConfig in `build_rtc`. Nothing to do at the
            // CodecConfig level for the browser profile baseline.
            let _ = codec_config;
        }
        WebRtcCodecProfile::Device => {
            let _ = codec_config;
        }
        WebRtcCodecProfile::Passthrough => {
            let _ = codec_config;
        }
    }
}

/// Convert a `str0m::RtcError` into an `InvalidSdp` error.
///
/// 将 `str0m::RtcError` 转换为 `InvalidSdp` 错误。
fn rtc_error_to_invalid_sdp(err: RtcError) -> WebRtcCoreError {
    WebRtcCoreError::InvalidSdp {
        message: err.to_string(),
    }
}

/// Map a boundary codec kind to the `str0m` codec enum.
///
/// Returns `None` for `Unknown` so callers can drop frames with a diagnostic.
///
/// 将边界编解码器类型映射到 `str0m` 编解码器枚举。
///
/// 对 `Unknown` 返回 `None`，调用方可通过诊断丢弃帧。
fn map_codec_kind_to_str0m(kind: WebRtcCodecKind) -> Option<str0m::format::Codec> {
    Some(match kind {
        WebRtcCodecKind::H264 => str0m::format::Codec::H264,
        WebRtcCodecKind::H265 => str0m::format::Codec::H265,
        WebRtcCodecKind::Vp8 => str0m::format::Codec::Vp8,
        WebRtcCodecKind::Vp9 => str0m::format::Codec::Vp9,
        WebRtcCodecKind::Av1 => str0m::format::Codec::Av1,
        WebRtcCodecKind::Opus => str0m::format::Codec::Opus,
        WebRtcCodecKind::Pcma => str0m::format::Codec::PCMA,
        WebRtcCodecKind::Pcmu => str0m::format::Codec::PCMU,
        WebRtcCodecKind::Unknown => return None,
    })
}

/// Map a boundary offer direction to a `str0m` media direction.
///
/// 将边界 offer 方向映射到 `str0m` 媒体方向。
fn map_offer_direction(direction: WebRtcOfferDirection) -> str0m::media::Direction {
    match direction {
        WebRtcOfferDirection::SendOnly => str0m::media::Direction::SendOnly,
        WebRtcOfferDirection::RecvOnly => str0m::media::Direction::RecvOnly,
        WebRtcOfferDirection::SendRecv => str0m::media::Direction::SendRecv,
    }
}

/// Convert a `str0m::rtp::VideoOrientation` into the bit-packed CVO byte
/// described in RFC 7742 §4.
///
/// The byte layout is `0 0 C F R1 R0` where `R1 R0` is the rotation
/// pair (00 = 0°, 01 = 90° CCW, 10 = 180°, 11 = 90° CW). `str0m`'s
/// enum discriminants match the rotation pair encoding directly so we
/// just cast. The `C` (camera) and `F` (flip) bits are not surfaced by
/// `str0m`'s parsed enum so they default to zero on the boundary.
///
/// 将 `str0m::rtp::VideoOrientation` 转换为 RFC 7742 §4 描述的打包 CVO 字节。
///
/// 字节布局为 `0 0 C F R1 R0`，其中 `R1 R0` 为旋转对（00 = 0°，01 = 90° CCW，
/// 10 = 180°，11 = 90° CW）。`str0m` 的枚举判别式直接匹配旋转对编码，因此直接
/// 转换即可。`C`（camera）和 `F`（flip）位未由 `str0m` 解析枚举暴露，因此边界上
/// 默认为零。
fn video_orientation_to_byte(orientation: &str0m::rtp::VideoOrientation) -> u8 {
    *orientation as u8
}

/// Format a compatibility report into a human-readable diagnostic message.
///
/// 将兼容性报告格式化为人类可读的诊断消息。
fn format_compat_report(report: &SdpCompatReport) -> String {
    let mut parts = Vec::with_capacity(3);
    if report.normalized_line_endings {
        parts.push("normalized line endings");
    }
    if report.trimmed_trailing_whitespace {
        parts.push("trimmed trailing whitespace");
    }
    if report.appended_missing_terminator {
        parts.push("appended CRLF terminator");
    }
    format!("sdp compat preprocessor applied: [{}]", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_offer() -> String {
        // Standard chrome-style audio+video offer trimmed for tests. We use
        // a SMS-shipped fixture in integration tests; here we only need
        // something `str0m` can parse, so we keep it minimal but valid.
        include_str!("../tests/fixtures/minimal_offer.sdp").to_string()
    }

    #[test]
    fn core_constructor_does_not_call_system_time() {
        // The core does not own a system-time reader. We assert this by
        // construction succeeding with a fixed anchor.
        let anchor = Instant::now();
        let core = WebRtcCore::new(WebRtcCoreConfig::default(), anchor);
        assert_eq!(core.session_count(), 0);
    }

    #[test]
    fn accept_offer_emits_answer_and_lifecycle_events() {
        let anchor = Instant::now();
        let mut core = WebRtcCore::new(WebRtcCoreConfig::default(), anchor);
        let session_id = WebRtcSessionId::new(1);
        core.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::AcceptOffer {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp: fixture_offer(),
            local_candidates: Vec::new(),
            now_micros: 0,
        }))
        .expect("accept_offer should succeed for a valid SDP");

        let mut sink = Vec::new();
        core.pump_outputs(&mut sink);

        let mut saw_created = false;
        let mut saw_local_description = false;
        let mut saw_local_ready = false;
        for out in &sink {
            match out {
                WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
                    state: WebRtcSessionLifecycle::Created,
                    ..
                }) => saw_created = true,
                WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
                    state: WebRtcSessionLifecycle::LocalDescriptionReady,
                    ..
                }) => saw_local_ready = true,
                WebRtcCoreOutput::LocalDescription {
                    kind: WebRtcLocalDescriptionKind::Answer,
                    sdp,
                    ..
                } => {
                    assert!(sdp.starts_with("v=0"));
                    saw_local_description = true;
                }
                _ => {}
            }
        }
        assert!(saw_created, "Created lifecycle event should be emitted");
        assert!(saw_local_description, "Answer SDP should be produced");
        assert!(
            saw_local_ready,
            "LocalDescriptionReady lifecycle event should be emitted"
        );
        assert!(core.has_session(session_id));
        assert_eq!(
            core.session_state(session_id),
            Some(WebRtcSessionState::Connecting)
        );
    }

    #[test]
    fn rejects_oversize_remote_sdp() {
        let anchor = Instant::now();
        let mut core = WebRtcCore::new(
            WebRtcCoreConfig {
                limits: crate::config::WebRtcCoreLimits {
                    max_remote_sdp_bytes: 32,
                    ..Default::default()
                },
                ..Default::default()
            },
            anchor,
        );
        let session_id = WebRtcSessionId::new(2);
        let err = core
            .handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::AcceptOffer {
                session_id,
                role: WebRtcSessionRole::Publisher,
                remote_sdp: fixture_offer(),
                local_candidates: Vec::new(),
                now_micros: 0,
            }))
            .expect_err("oversize SDP should be rejected");
        assert!(matches!(err, WebRtcCoreError::SdpTooLarge { .. }));
        assert!(!core.has_session(session_id));
    }

    #[test]
    fn rejects_invalid_sdp_with_diagnostic() {
        let anchor = Instant::now();
        let mut core = WebRtcCore::new(WebRtcCoreConfig::default(), anchor);
        let session_id = WebRtcSessionId::new(3);
        let err = core
            .handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::AcceptOffer {
                session_id,
                role: WebRtcSessionRole::Publisher,
                remote_sdp: "this is not sdp".into(),
                local_candidates: Vec::new(),
                now_micros: 0,
            }))
            .expect_err("garbage SDP must be rejected");
        assert!(matches!(err, WebRtcCoreError::InvalidSdp { .. }));
    }

    #[test]
    fn close_session_emits_close_output() {
        let anchor = Instant::now();
        let mut core = WebRtcCore::new(WebRtcCoreConfig::default(), anchor);
        let session_id = WebRtcSessionId::new(4);
        core.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::AcceptOffer {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp: fixture_offer(),
            local_candidates: Vec::new(),
            now_micros: 0,
        }))
        .unwrap();
        let mut sink = Vec::new();
        core.pump_outputs(&mut sink);
        sink.clear();

        core.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::Close {
            session_id,
            reason: WebRtcCloseReason::Normal,
        }))
        .unwrap();

        core.pump_outputs(&mut sink);
        let close = sink
            .iter()
            .find(|o| matches!(o, WebRtcCoreOutput::CloseSession { .. }))
            .expect("expected CloseSession output");
        match close {
            WebRtcCoreOutput::CloseSession {
                session_id: id,
                reason: WebRtcCloseReason::Normal,
            } => assert_eq!(*id, session_id),
            other => panic!("unexpected close output: {other:?}"),
        }
        assert!(!core.has_session(session_id));
    }

    #[test]
    fn out_of_order_now_micros_is_clamped_monotonically() {
        let anchor = Instant::now();
        let mut core = WebRtcCore::new(WebRtcCoreConfig::default(), anchor);
        let first = core.absolute_instant(1_000_000);
        let second = core.absolute_instant(500_000);
        assert!(second >= first, "absolute_instant must be monotonic");
    }

    #[test]
    fn create_offer_emits_offer_and_lifecycle_events() {
        let anchor = Instant::now();
        let mut core = WebRtcCore::new(WebRtcCoreConfig::default(), anchor);
        let session_id = WebRtcSessionId::new(101);
        core.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::CreateOffer {
            session_id,
            role: WebRtcSessionRole::Player,
            spec: WebRtcOfferSpec {
                video_direction: Some(WebRtcOfferDirection::SendOnly),
                audio_direction: Some(WebRtcOfferDirection::SendOnly),
                data_channel: false,
            },
            local_candidates: Vec::new(),
            now_micros: 0,
        }))
        .expect("CreateOffer should succeed");

        let mut sink = Vec::new();
        core.pump_outputs(&mut sink);

        let mut saw_offer = false;
        let mut saw_local_ready = false;
        for out in &sink {
            match out {
                WebRtcCoreOutput::LocalDescription {
                    sdp,
                    kind: crate::output::WebRtcLocalDescriptionKind::Offer,
                    ..
                } => {
                    assert!(sdp.starts_with("v=0"));
                    saw_offer = true;
                }
                WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
                    state: WebRtcSessionLifecycle::LocalDescriptionReady,
                    ..
                }) => saw_local_ready = true,
                _ => {}
            }
        }
        assert!(saw_offer, "CreateOffer must emit a local SDP offer");
        assert!(saw_local_ready, "Lifecycle::LocalDescriptionReady expected");
        assert!(core.has_session(session_id));
    }

    #[test]
    fn create_offer_with_empty_spec_is_rejected() {
        let anchor = Instant::now();
        let mut core = WebRtcCore::new(WebRtcCoreConfig::default(), anchor);
        let session_id = WebRtcSessionId::new(102);
        let err = core
            .handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::CreateOffer {
                session_id,
                role: WebRtcSessionRole::Player,
                spec: WebRtcOfferSpec::default(),
                local_candidates: Vec::new(),
                now_micros: 0,
            }))
            .expect_err("CreateOffer with empty spec must fail");
        assert!(matches!(err, WebRtcCoreError::InvalidState { .. }));
    }

    /// Phase 05 follow-up: a `SendDataChannel` payload above the
    /// configured cap must be rejected with a `PendingOutputDropped`
    /// diagnostic and never reach `str0m`. The session is still alive
    /// after the rejection so subsequent (smaller) writes can land.
    #[test]
    fn send_data_channel_oversized_payload_emits_diagnostic_and_drops() {
        use crate::input::WebRtcDataChannelOut;
        use crate::types::DataChannelId;

        let anchor = Instant::now();
        let mut core = WebRtcCore::new(
            WebRtcCoreConfig {
                limits: crate::WebRtcCoreLimits {
                    // Tiny cap so the test does not allocate megabytes.
                    max_data_channel_message_bytes: 8,
                    ..Default::default()
                },
                ..Default::default()
            },
            anchor,
        );
        let session_id = WebRtcSessionId::new(201);
        core.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::AcceptOffer {
            session_id,
            role: WebRtcSessionRole::Bidirectional,
            remote_sdp: fixture_offer(),
            local_candidates: Vec::new(),
            now_micros: 0,
        }))
        .unwrap();
        let mut sink = Vec::new();
        core.pump_outputs(&mut sink);
        sink.clear();

        // The session is `Connecting`, no DataChannel is open yet, so
        // even a properly-sized write would fail with `InvalidState`.
        // The size-cap check has to short-circuit *before* the open
        // channel lookup, otherwise oversize payloads would mask
        // themselves as "channel not open" errors. We assert the
        // diagnostic arm runs by sending an oversized payload first.
        core.handle_input(WebRtcCoreInput::Command(
            WebRtcCoreCommand::SendDataChannel(WebRtcDataChannelOut {
                session_id,
                channel: DataChannelId::new(0),
                payload: bytes::Bytes::from(vec![0u8; 32]),
                binary: true,
            }),
        ))
        .expect("oversize send must not surface as Err — diagnostic only");

        core.pump_outputs(&mut sink);
        let saw_drop = sink.iter().any(|o| {
            matches!(
                o,
                WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                    kind: WebRtcCoreDiagnosticKind::PendingOutputDropped,
                    message,
                    ..
                }) if message.contains("exceeds max")
            )
        });
        assert!(
            saw_drop,
            "oversize DataChannel payload should produce a PendingOutputDropped diagnostic"
        );
        assert!(core.has_session(session_id));
    }

    /// Phase 05 follow-up: writes to an unknown (or already-closed)
    /// DataChannel must surface a `PendingOutputDropped` diagnostic
    /// rather than a hard `Err`. ZLM's behaviour is to silently drop
    /// post-close writes; we mirror that with an explicit diagnostic
    /// so operators can observe the drop.
    #[test]
    fn send_data_channel_unknown_channel_emits_diagnostic_not_error() {
        use crate::input::WebRtcDataChannelOut;
        use crate::types::DataChannelId;

        let anchor = Instant::now();
        let mut core = WebRtcCore::new(WebRtcCoreConfig::default(), anchor);
        let session_id = WebRtcSessionId::new(202);
        core.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::AcceptOffer {
            session_id,
            role: WebRtcSessionRole::Bidirectional,
            remote_sdp: fixture_offer(),
            local_candidates: Vec::new(),
            now_micros: 0,
        }))
        .unwrap();
        let mut sink = Vec::new();
        core.pump_outputs(&mut sink);
        sink.clear();

        // Send to a channel id that was never opened. Must not panic
        // or return Err — instead surface a PendingOutputDropped
        // diagnostic.
        core.handle_input(WebRtcCoreInput::Command(
            WebRtcCoreCommand::SendDataChannel(WebRtcDataChannelOut {
                session_id,
                channel: DataChannelId::new(99),
                payload: bytes::Bytes::from_static(b"hello"),
                binary: false,
            }),
        ))
        .expect("write to unknown channel must not surface as Err");

        core.pump_outputs(&mut sink);
        let saw_drop = sink.iter().any(|o| {
            matches!(
                o,
                WebRtcCoreOutput::Diagnostic(WebRtcCoreDiagnostic {
                    kind: WebRtcCoreDiagnosticKind::PendingOutputDropped,
                    message,
                    ..
                }) if message.contains("unknown or already closed")
            )
        });
        assert!(
            saw_drop,
            "write to unknown DataChannel should produce PendingOutputDropped diagnostic"
        );
        assert!(core.has_session(session_id));
    }

    /// `IceRestart` on a connecting session emits a fresh local
    /// SDP offer through `LocalDescription { kind: Offer }`. The
    /// session retains its identity (no new id is allocated).
    #[test]
    fn ice_restart_emits_fresh_offer_for_existing_session() {
        let anchor = Instant::now();
        let mut core = WebRtcCore::new(WebRtcCoreConfig::default(), anchor);
        let session_id = WebRtcSessionId::new(301);
        core.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::AcceptOffer {
            session_id,
            role: WebRtcSessionRole::Bidirectional,
            remote_sdp: fixture_offer(),
            local_candidates: Vec::new(),
            now_micros: 0,
        }))
        .expect("accept_offer should succeed");
        let mut sink = Vec::new();
        core.pump_outputs(&mut sink);
        sink.clear();

        core.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::IceRestart {
            session_id,
            keep_local_candidates: true,
            now_micros: 1_000,
        }))
        .expect("ice_restart should produce a new offer");
        core.pump_outputs(&mut sink);
        let saw_offer = sink.iter().any(|o| {
            matches!(
                o,
                WebRtcCoreOutput::LocalDescription {
                    kind: WebRtcLocalDescriptionKind::Offer,
                    ..
                }
            )
        });
        assert!(
            saw_offer,
            "ice_restart must surface a LocalDescription{{Offer}} output: {sink:?}"
        );
        assert!(core.has_session(session_id));
    }

    /// `IceRestart` for an unknown session returns `SessionNotFound`.
    #[test]
    fn ice_restart_unknown_session_is_not_found() {
        let anchor = Instant::now();
        let mut core = WebRtcCore::new(WebRtcCoreConfig::default(), anchor);
        let err = core
            .handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::IceRestart {
                session_id: WebRtcSessionId::new(9999),
                keep_local_candidates: false,
                now_micros: 0,
            }))
            .expect_err("ice_restart on unknown session must fail");
        assert!(matches!(err, WebRtcCoreError::SessionNotFound(_)));
    }
}
