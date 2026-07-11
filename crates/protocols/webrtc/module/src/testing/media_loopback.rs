//! Deterministic in-process WebRTC media loopback harness.
//!
//! This harness BYPASSES ICE/DTLS/SRTP and UDP by driving two
//! [`WebRtcCore`] sessions directly, routing `SendPacket` outputs from the
//! offerer into the answerer and vice-versa. It is intended for W2 media
//! fixture tests and connector-level loopback tests.
//!
//! 本 harness 绕过 ICE/DTLS/SRTP 与 UDP，直接驱动两个 `WebRtcCore` 会话，
//! 将 offerer 的 `SendPacket` 输出路由到 answerer，反之亦然。
//! 用于 W2 媒体 fixture 测试与 connector 层 loopback 测试。

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_codec::{
    media_ts_to_rtp_ticks, AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId,
};
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use cheetah_sdk::{
    PublishLease, PublisherApi, PublisherSink, SdkError, StreamKey, StreamManagerApi,
    SubscriberSource,
};
use cheetah_webrtc_core::{
    MidLabel, WebRtcCloseReason, WebRtcCodecKind, WebRtcCore, WebRtcCoreCommand, WebRtcCoreConfig,
    WebRtcCoreError, WebRtcCoreEvent, WebRtcCoreInput, WebRtcCoreOutput,
    WebRtcLocalDescriptionKind, WebRtcMediaEvent, WebRtcNetworkInput, WebRtcOfferDirection,
    WebRtcOfferSpec, WebRtcPacketOut, WebRtcSendFrame, WebRtcSessionId, WebRtcSessionLifecycle,
    WebRtcSessionRole, WebRtcTimer,
};

use crate::bridge::WebRtcPublishBridge;
use crate::config::SimulcastPolicy;

/// Hard-coded source/destination addresses for the loopback pair.
const PLAYER_ADDR: SocketAddr = SocketAddr::V4(std::net::SocketAddrV4::new(
    std::net::Ipv4Addr::new(127, 0, 0, 1),
    5000,
));
const PUBLISHER_ADDR: SocketAddr = SocketAddr::V4(std::net::SocketAddrV4::new(
    std::net::Ipv4Addr::new(127, 0, 0, 1),
    5001,
));

/// How long we are willing to wait for handshake / media.
const LOOPBACK_TIMEOUT: Duration = Duration::from_secs(10);

/// In-process WebRTC media loopback harness.
///
/// Two `WebRtcCore` sessions are created and driven back-to-back:
/// * a `Player` (offerer, `SendOnly` video) that sends `WebRtcSendFrame`
///   inputs, and
/// * a `Publisher` (answerer, `RecvOnly` video) whose received `Frame`
///   events are converted to `AVFrame`s and pushed into the engine through
///   a `WebRtcPublishBridge`.
///
/// The harness can be wrapped in a `PushHandle`/`PullHandle` pair at the
/// connector layer.
pub struct MediaLoopbackHarness {
    runtime: Arc<dyn RuntimeApi>,
    cancel: CancellationToken,
    core: WebRtcCore,
    bridge: WebRtcPublishBridge,
    sink_subscriber: Box<dyn SubscriberSource>,

    start_instant: Instant,
    start_monotime: cheetah_codec::MonoTime,

    player_session: WebRtcSessionId,
    publisher_session: WebRtcSessionId,
    player_mid: Option<MidLabel>,

    player_connected: bool,
    publisher_connected: bool,
    media_received: bool,
    pending_frame: Option<Arc<AVFrame>>,

    timeout: Duration,
}

impl MediaLoopbackHarness {
    /// Create and connect a loopback harness for `stream_key`.
    pub async fn new(
        runtime: Arc<dyn RuntimeApi>,
        stream_manager: Arc<dyn StreamManagerApi>,
        stream_key: StreamKey,
        cancel: CancellationToken,
    ) -> Result<Self, SdkError> {
        if cancel.is_cancelled() {
            return Err(SdkError::Unavailable("loopback cancelled".to_string()));
        }

        let start_instant = Instant::now();
        let start_monotime = runtime.now();

        // Build a wrapper so `WebRtcPublishBridge` can acquire a publisher sink.
        let publisher_api: Arc<dyn PublisherApi> =
            Arc::new(StreamManagerApiPublisherApi(stream_manager.clone()));

        let bridge = WebRtcPublishBridge::acquire(
            &publisher_api,
            stream_key.clone(),
            SimulcastPolicy::default(),
            (0, 0),
            false,
        )
        .await
        .map_err(|e| SdkError::Internal(format!("WebRtcPublishBridge acquire failed: {e}")))?;

        let sink_subscriber = stream_manager
            .open_subscriber(stream_key, cheetah_sdk::SubscriberOptions::default())
            .await?;

        let core = WebRtcCore::new(WebRtcCoreConfig::default(), start_instant);

        let mut harness = Self {
            runtime,
            cancel,
            core,
            bridge,
            sink_subscriber,
            start_instant,
            start_monotime,
            player_session: WebRtcSessionId(1),
            publisher_session: WebRtcSessionId(2),
            player_mid: None,
            player_connected: false,
            publisher_connected: false,
            media_received: false,
            pending_frame: None,
            timeout: LOOPBACK_TIMEOUT,
        };

        // Create offer/answer and connect both sessions.
        let player_offer = harness.create_player_offer()?;
        let publisher_answer = harness.accept_publisher_offer(player_offer)?;
        harness.apply_player_answer(publisher_answer)?;

        harness.drive_until(DriveTarget::Connected).await?;

        // Send a small probe frame and wait for it to traverse the whole
        // DTLS/SRTP path. This ensures `is_connected()` is true on the
        // player side before the caller tries to push real media.
        let probe = AVFrame::new(
            TrackId(0),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 1_000),
            Bytes::from_static(&[
                0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00, 0x2f, 0xff, 0xff, 0x00, 0x04,
            ]),
        );
        harness.pending_frame = Some(Arc::new(probe));
        harness.drive_until(DriveTarget::MediaReceived).await?;
        let _ = harness.recv().await?;
        harness.pending_frame = None;

        Ok(harness)
    }

    /// Push one `AVFrame` into the player side.
    pub fn push_frame(
        &mut self,
        frame: Arc<AVFrame>,
    ) -> Result<cheetah_sdk::DispatchResult, SdkError> {
        self.media_received = false;
        self.pending_frame = None;
        self.send_frame(&frame)?;
        self.drive_sync()?;
        Ok(cheetah_sdk::DispatchResult::Accepted)
    }

    /// Receive the next `AVFrame` from the engine subscriber.
    pub async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, SdkError> {
        self.drive_until(DriveTarget::MediaReceived).await?;
        self.media_received = false;
        self.sink_subscriber.recv().await
    }

    /// Close the harness and release all sinks.
    pub async fn close(&mut self) -> Result<(), SdkError> {
        let _ = self.close_sink();
        self.sink_subscriber.close().await
    }

    /// Synchronous close for use through a `PublisherSink` wrapper.
    pub fn close_sink(&mut self) -> Result<(), SdkError> {
        let _ = self
            .core
            .handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::Close {
                session_id: self.player_session,
                reason: WebRtcCloseReason::Normal,
            }));
        let _ = self
            .core
            .handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::Close {
                session_id: self.publisher_session,
                reason: WebRtcCloseReason::Normal,
            }));
        self.bridge.close();
        Ok(())
    }

    /// Synchronous update for `PublisherSink` wrapper.
    pub fn update_tracks(
        &mut self,
        _tracks: Vec<cheetah_codec::TrackInfo>,
    ) -> Result<(), SdkError> {
        // The bridge builds track snapshots on the fly from `MediaTrackAdded` /
        // `Media` events.
        Ok(())
    }

    /// Return the current number of keyframe requests.
    pub fn take_keyframe_requests(&self) -> u64 {
        0
    }

    /// Subscriber id for `SubscriberSource` wrapper.
    pub fn id(&self) -> cheetah_sdk::SubscriberId {
        cheetah_sdk::SubscriberId(0)
    }

    /// Snapshot of tracks discovered by the engine subscriber.
    pub fn tracks(&self) -> Vec<cheetah_codec::TrackInfo> {
        self.sink_subscriber.tracks()
    }

    // ------------------------------------------------------------------
    // Offer/answer handshake
    // ------------------------------------------------------------------

    fn create_player_offer(&mut self) -> Result<String, SdkError> {
        let candidate = make_candidate(PLAYER_ADDR);
        self.core
            .handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::CreateOffer {
                session_id: self.player_session,
                role: WebRtcSessionRole::Player,
                spec: WebRtcOfferSpec {
                    video_direction: Some(WebRtcOfferDirection::SendOnly),
                    audio_direction: None,
                    data_channel: false,
                },
                local_candidates: vec![candidate],
                now_micros: 0,
            }))
            .map_err(map_core_error)?;

        let mut outputs = Vec::new();
        self.core.pump_outputs(&mut outputs);
        let local_sdp = take_local_description(
            &mut outputs,
            self.player_session,
            WebRtcLocalDescriptionKind::Offer,
        )
        .ok_or_else(|| SdkError::Internal("player offer not produced".to_string()))?;

        if let Some(mid_line) = local_sdp.lines().find(|l| l.starts_with("a=mid:")) {
            let mid = mid_line.strip_prefix("a=mid:").unwrap_or(mid_line);
            self.player_mid = Some(MidLabel::new(mid.to_string()));
        }

        eprintln!("[create_player_offer]\n{local_sdp}");

        let _ = self.process_outputs(&mut outputs)?;
        Ok(local_sdp)
    }

    fn accept_publisher_offer(&mut self, offer: String) -> Result<String, SdkError> {
        eprintln!(
            "[accept_publisher_offer] incoming offer mid={}",
            offer
                .lines()
                .find(|l| l.starts_with("a=mid:"))
                .unwrap_or("")
        );
        let candidate = make_candidate(PUBLISHER_ADDR);
        self.core
            .handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::AcceptOffer {
                session_id: self.publisher_session,
                role: WebRtcSessionRole::Publisher,
                remote_sdp: offer,
                local_candidates: vec![candidate],
                now_micros: 0,
            }))
            .map_err(map_core_error)?;

        let mut outputs = Vec::new();
        self.core.pump_outputs(&mut outputs);
        let local_sdp = take_local_description(
            &mut outputs,
            self.publisher_session,
            WebRtcLocalDescriptionKind::Answer,
        )
        .ok_or_else(|| SdkError::Internal("publisher answer not produced".to_string()))?;

        let _ = self.process_outputs(&mut outputs)?;
        eprintln!("[accept_publisher_offer]\n{local_sdp}");
        Ok(local_sdp)
    }

    fn apply_player_answer(&mut self, answer: String) -> Result<(), SdkError> {
        self.core
            .handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::ApplyAnswer {
                session_id: self.player_session,
                remote_sdp: answer,
                now_micros: 0,
            }))
            .map_err(map_core_error)?;

        let mut outputs = Vec::new();
        self.core.pump_outputs(&mut outputs);
        let _ = self.process_outputs(&mut outputs)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Drive loop
    // ------------------------------------------------------------------

    async fn drive_until(&mut self, target: DriveTarget) -> Result<(), SdkError> {
        if self.already(target) {
            return Ok(());
        }

        let start = Instant::now();
        while start.elapsed() < self.timeout {
            if self.cancel.is_cancelled() {
                return Err(SdkError::Unavailable("loopback cancelled".to_string()));
            }

            self.send_frame_if_pending()?;
            match self.drive_sync()? {
                DriveOutcome::Connected if matches!(target, DriveTarget::Connected) => {
                    return Ok(())
                }
                DriveOutcome::MediaReceived if matches!(target, DriveTarget::MediaReceived) => {
                    return Ok(())
                }
                DriveOutcome::Timer(t) => {
                    let deadline = self.start_instant + Duration::from_micros(t.deadline_micros);
                    let sleep_target = deadline.max(Instant::now());
                    self.sleep_to(sleep_target).await;
                    self.tick_to_instant(Instant::now())?;
                }
                DriveOutcome::Idle => {
                    let now = Instant::now();
                    self.sleep_to(now + Duration::from_millis(1)).await;
                    self.tick_to_instant(Instant::now())?;
                }
                _ => {}
            }

            if self.already(target) {
                return Ok(());
            }
        }

        Err(SdkError::Internal(format!(
            "drive_until {:?} timeout",
            target
        )))
    }

    fn already(&self, target: DriveTarget) -> bool {
        match target {
            DriveTarget::Connected => self.is_connected(),
            DriveTarget::MediaReceived => self.media_received,
        }
    }

    fn drive_sync(&mut self) -> Result<DriveOutcome, SdkError> {
        let mut outputs = Vec::new();
        self.core.pump_outputs(&mut outputs);
        let outcome = self.process_outputs(&mut outputs)?;
        if matches!(outcome, DriveOutcome::Idle) && self.media_received {
            return Ok(DriveOutcome::MediaReceived);
        }
        Ok(outcome)
    }

    fn process_outputs(
        &mut self,
        outputs: &mut Vec<WebRtcCoreOutput>,
    ) -> Result<DriveOutcome, SdkError> {
        let mut earliest_timer: Option<WebRtcTimer> = None;

        while !outputs.is_empty() {
            let output = outputs.remove(0);
            eprintln!("[process_outputs] output={output:?}");
            match output {
                WebRtcCoreOutput::SendPacket(packet) => {
                    self.route_packet(packet)?;
                    self.core.pump_outputs(outputs);
                }
                WebRtcCoreOutput::SetTimer(timer) => {
                    let deadline =
                        self.start_instant + Duration::from_micros(timer.deadline_micros);
                    if deadline < Instant::now() {
                        self.tick_to_instant(Instant::now())?;
                        self.core.pump_outputs(outputs);
                    } else {
                        earliest_timer = earliest_timer.map_or(Some(timer), |current| {
                            if timer.deadline_micros < current.deadline_micros {
                                Some(timer)
                            } else {
                                Some(current)
                            }
                        });
                    }
                }
                WebRtcCoreOutput::Event(event) => match event {
                    WebRtcCoreEvent::Lifecycle {
                        session_id,
                        state: WebRtcSessionLifecycle::Connected,
                    } => {
                        if session_id == self.player_session {
                            self.player_connected = true;
                        }
                        if session_id == self.publisher_session {
                            self.publisher_connected = true;
                        }
                        if self.is_connected() {
                            return Ok(DriveOutcome::Connected);
                        }
                    }
                    WebRtcCoreEvent::Lifecycle {
                        session_id,
                        state: WebRtcSessionLifecycle::Failed,
                    } => {
                        return Err(SdkError::Internal(format!(
                            "session {session_id:?} failed during loopback handshake"
                        )));
                    }
                    WebRtcCoreEvent::MediaTrackAdded { session_id, track } => {
                        if session_id == self.player_session {
                            self.player_mid = Some(track.mid);
                        }
                        if self.is_connected() {
                            return Ok(DriveOutcome::Connected);
                        }
                    }
                    WebRtcCoreEvent::Media { session_id, event }
                        if session_id == self.publisher_session =>
                    {
                        if let WebRtcMediaEvent::Frame { .. } = event {
                            self.bridge.push_frame(event);
                            self.media_received = true;
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        if let Some(timer) = earliest_timer {
            return Ok(DriveOutcome::Timer(timer));
        }

        Ok(DriveOutcome::Idle)
    }

    fn route_packet(&mut self, packet: WebRtcPacketOut) -> Result<(), SdkError> {
        let dst = if packet.session_id == self.player_session {
            self.publisher_session
        } else if packet.session_id == self.publisher_session {
            self.player_session
        } else {
            return Ok(());
        };

        let source = packet
            .source
            .unwrap_or_else(|| self.local_addr(packet.session_id));
        let destination = self.local_addr(dst);
        let now_micros = Instant::now()
            .duration_since(self.start_instant)
            .as_micros() as u64;

        eprintln!(
            "[route_packet] packet.session_id={:?} dst={:?} source={:?} destination={:?} len={}",
            packet.session_id,
            dst,
            source,
            destination,
            packet.data.len()
        );

        if let Err(err) = self
            .core
            .handle_input(WebRtcCoreInput::Network(WebRtcNetworkInput {
                session_id: dst,
                source,
                destination,
                data: packet.data,
                now_micros,
            }))
        {
            if matches!(err, WebRtcCoreError::SessionNotFound(_)) {
                return Ok(());
            }
            eprintln!("[route_packet] handle_input error: {err:?}");
            return Err(map_core_error(err));
        }

        Ok(())
    }

    fn send_frame_if_pending(&mut self) -> Result<(), SdkError> {
        if self.media_received {
            self.pending_frame = None;
            return Ok(());
        }
        if let Some(frame) = self.pending_frame.clone() {
            self.send_frame(&frame)?;
        }
        Ok(())
    }

    fn send_frame(&mut self, frame: &AVFrame) -> Result<(), SdkError> {
        let mid = self
            .player_mid
            .clone()
            .ok_or_else(|| SdkError::Internal("player mid not ready".to_string()))?;

        let codec = map_codec_id(frame.codec);
        let clock_rate = if frame.media_kind == MediaKind::Video {
            90_000
        } else {
            48_000
        };
        let rtp_timestamp_ticks =
            media_ts_to_rtp_ticks(frame.pts, frame.dts, frame.timebase, clock_rate);

        let send_frame = WebRtcSendFrame {
            session_id: self.player_session,
            mid,
            codec,
            clock_rate,
            rtp_timestamp_ticks,
            rtp_timestamp_denom: clock_rate,
            random_access: frame.flags.contains(FrameFlags::KEY),
            payload: frame.payload.clone(),
            network_time_micros: 0,
        };

        self.core
            .handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::SendFrame(
                Box::new(send_frame),
            )))
            .map_err(map_core_error)?;

        Ok(())
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn is_connected(&self) -> bool {
        self.player_connected && self.publisher_connected
    }

    fn local_addr(&self, session_id: WebRtcSessionId) -> SocketAddr {
        if session_id == self.player_session {
            PLAYER_ADDR
        } else {
            PUBLISHER_ADDR
        }
    }

    async fn sleep_to(&mut self, instant: Instant) {
        if instant <= Instant::now() {
            return;
        }
        let duration = instant.duration_since(self.start_instant);
        let deadline_micros = self
            .start_monotime
            .as_micros()
            .saturating_add(duration.as_micros() as u64);
        let deadline = cheetah_codec::MonoTime::from_micros(deadline_micros);
        let mut timer = self.runtime.sleep_until(deadline);
        timer.wait().await;
    }

    fn tick_to_instant(&mut self, instant: Instant) -> Result<(), SdkError> {
        let now_micros = instant.duration_since(self.start_instant).as_micros() as u64;
        self.core
            .handle_input(WebRtcCoreInput::Tick { now_micros })
            .map_err(map_core_error)
    }
}

// ----------------------------------------------------------------------
// Wrapper that exposes `StreamManagerApi` as `PublisherApi`
// ----------------------------------------------------------------------

struct StreamManagerApiPublisherApi(Arc<dyn StreamManagerApi>);

#[async_trait]
impl PublisherApi for StreamManagerApiPublisherApi {
    async fn acquire_publisher(
        &self,
        stream_key: StreamKey,
        _options: cheetah_sdk::PublisherOptions,
    ) -> Result<(PublishLease, Box<dyn PublisherSink>), SdkError> {
        let sink = self
            .0
            .open_publisher(stream_key.clone(), cheetah_sdk::PublisherOptions::default())
            .await?;
        let lease = PublishLease {
            stream_id: cheetah_sdk::StreamId(0),
            stream_key,
            lease_id: 0,
        };
        Ok((lease, sink))
    }

    async fn release_publisher(&self, _lease: &PublishLease) -> Result<(), SdkError> {
        Ok(())
    }
}

// ----------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------

fn make_candidate(addr: SocketAddr) -> String {
    format!(
        "candidate:1 1 udp 2113937151 {} {} typ host",
        addr.ip(),
        addr.port()
    )
}

fn take_local_description(
    outputs: &mut Vec<WebRtcCoreOutput>,
    session_id: WebRtcSessionId,
    kind: WebRtcLocalDescriptionKind,
) -> Option<String> {
    let pos = outputs.iter().position(|o| match o {
        WebRtcCoreOutput::LocalDescription {
            session_id: sid,
            kind: k,
            ..
        } => *sid == session_id && *k == kind,
        _ => false,
    })?;
    match outputs.swap_remove(pos) {
        WebRtcCoreOutput::LocalDescription { sdp, .. } => Some(sdp),
        _ => None,
    }
}

fn map_codec_id(codec: CodecId) -> WebRtcCodecKind {
    match codec {
        CodecId::H264 => WebRtcCodecKind::H264,
        CodecId::H265 => WebRtcCodecKind::H265,
        CodecId::Opus => WebRtcCodecKind::Opus,
        CodecId::G711A => WebRtcCodecKind::Pcma,
        CodecId::G711U => WebRtcCodecKind::Pcmu,
        CodecId::AV1 => WebRtcCodecKind::Av1,
        CodecId::VP8 => WebRtcCodecKind::Vp8,
        CodecId::VP9 => WebRtcCodecKind::Vp9,
        _ => WebRtcCodecKind::Unknown,
    }
}

fn map_core_error(err: WebRtcCoreError) -> SdkError {
    SdkError::Internal(err.to_string())
}

#[derive(Debug, Clone, Copy)]
enum DriveTarget {
    Connected,
    MediaReceived,
}

enum DriveOutcome {
    Connected,
    MediaReceived,
    Timer(WebRtcTimer),
    Idle,
}
