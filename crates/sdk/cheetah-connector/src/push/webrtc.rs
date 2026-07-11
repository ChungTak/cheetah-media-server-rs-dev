//! WebRTC push-side adapter implementing [`cheetah_sdk::PublisherSink`].
//!
//! Spawns a private `cheetah_webrtc_driver_tokio` driver, performs a WHIP
//! offer/answer exchange, and forwards `AVFrame` values into the driver as
//! `WebRtcDriverCommand::SendFrame`.
//!
//! WebRTC 推流端适配器，实现 [`cheetah_sdk::PublisherSink`]。

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use bytes::Bytes;
use cheetah_codec::{
    codec_default_samples_per_frame, codec_rtp_clock_rate, compute_rtp_timestamp, AVFrame, CodecId,
    FrameFlags, MediaKind, RtpTimestampInput, RtpTimestampMode, TrackId, TrackInfo,
};
use cheetah_runtime_api::{CancellationToken, JoinHandle};
use cheetah_sdk::{DispatchResult, PublisherSink, SdkError};
use cheetah_webrtc_core::{
    MidLabel, WebRtcCloseReason, WebRtcCodecKind, WebRtcCoreEvent, WebRtcMediaDirection,
    WebRtcMediaKind, WebRtcOfferDirection, WebRtcOfferSpec, WebRtcRtcpFeedback, WebRtcSendFrame,
    WebRtcSessionId, WebRtcSessionLifecycle, WebRtcSessionRole,
};
use cheetah_webrtc_driver_tokio::{
    spawn_driver, CandidateTransportPolicy, HttpClientError, HttpClientRequest, HttpClientResponse,
    WebRtcDriverCommand, WebRtcDriverEvent, WebRtcDriverHandle, WhipWhepHttpClient,
};
use futures::channel::mpsc;
use futures::future::{BoxFuture, Fuse, FusedFuture, OptionFuture};
use futures::{select_biased, FutureExt, StreamExt};
use parking_lot::Mutex;
use tracing::warn;

use crate::error::ConnectorError;
use crate::handles::PushHandle;
use crate::options::{ConnectorPushOptions, ProtocolPushExtras, WebRtcPushExtras};
use crate::protocol::Protocol;

const DEFAULT_COMMAND_QUEUE: usize = 256;
const DEFAULT_BUFFER_CAPACITY: usize = 256;

struct SinkState {
    closed: bool,
    ready: bool,
    tracks: Vec<TrackInfo>,
    buffer: VecDeque<Arc<AVFrame>>,
    buffer_capacity: usize,
    video_mid: Option<MidLabel>,
    audio_mid: Option<MidLabel>,
    keyframe_requests: u64,
}

impl SinkState {
    fn new(buffer_capacity: usize) -> Self {
        Self {
            closed: false,
            ready: false,
            tracks: Vec::new(),
            buffer: VecDeque::with_capacity(buffer_capacity),
            buffer_capacity,
            video_mid: None,
            audio_mid: None,
            keyframe_requests: 0,
        }
    }
}

enum WebRtcSinkCommand {
    UpdateTracks(Vec<TrackInfo>),
    SendFrame(Arc<AVFrame>),
}

/// Synchronous WebRTC publisher sink.
///
/// A background task (started by [`open_webrtc_push`]) performs the WHIP
/// handshake and forwards frames to the WebRTC driver.
pub struct WebRtcPublisherSink {
    cmd_tx: Mutex<mpsc::Sender<WebRtcSinkCommand>>,
    cancel: CancellationToken,
    state: Arc<Mutex<SinkState>>,
    _join: Mutex<Option<Box<dyn JoinHandle>>>,
}

impl WebRtcPublisherSink {
    fn new(
        cmd_tx: mpsc::Sender<WebRtcSinkCommand>,
        cancel: CancellationToken,
        state: Arc<Mutex<SinkState>>,
        join: Box<dyn JoinHandle>,
    ) -> Self {
        Self {
            cmd_tx: Mutex::new(cmd_tx),
            cancel,
            state,
            _join: Mutex::new(Some(join)),
        }
    }

    fn send_command(&self, command: WebRtcSinkCommand) -> Result<(), SdkError> {
        let mut guard = self.cmd_tx.lock();
        if guard.is_closed() {
            return Err(SdkError::Internal(
                "webrtc push command channel closed".to_string(),
            ));
        }
        guard.try_send(command).map_err(|err| {
            if err.is_full() {
                SdkError::Internal("webrtc push command queue full".to_string())
            } else {
                SdkError::Internal("webrtc push command channel closed".to_string())
            }
        })
    }
}

impl PublisherSink for WebRtcPublisherSink {
    fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), SdkError> {
        {
            let mut guard = self.state.lock();
            if guard.closed {
                return Err(SdkError::Internal("webrtc push sink closed".to_string()));
            }
            guard.tracks = tracks;
        }
        self.send_command(WebRtcSinkCommand::UpdateTracks(
            self.state.lock().tracks.clone(),
        ))
    }

    fn push_frame(&self, frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError> {
        {
            let guard = self.state.lock();
            if guard.closed {
                return Ok(DispatchResult::RejectedClosed);
            }
        }
        match self.send_command(WebRtcSinkCommand::SendFrame(frame)) {
            Ok(()) => Ok(DispatchResult::Accepted),
            Err(SdkError::Internal(msg)) if msg.contains("queue full") => {
                Ok(DispatchResult::DroppedByPolicy)
            }
            Err(err) => Err(err),
        }
    }

    fn close(&self) -> Result<(), SdkError> {
        self.state.lock().closed = true;
        self.cancel.cancel();
        Ok(())
    }

    fn take_keyframe_requests(&self) -> u64 {
        let mut guard = self.state.lock();
        let requests = guard.keyframe_requests;
        guard.keyframe_requests = 0;
        requests
    }
}

/// Open a WebRTC push handle for `url` and `options`.
///
/// 为 `url` 和 `options` 打开 WebRTC 推流句柄。
pub async fn open_webrtc_push(
    engine: Arc<cheetah_engine::Engine>,
    url: &str,
    options: ConnectorPushOptions,
) -> Result<PushHandle, ConnectorError> {
    let protocol = Protocol::WebRtc;
    let (http_url, allow_private_ips) = normalize_whip_url(url)?;
    let runtime_api = engine.runtime_api();
    let cancel = options.cancel.clone().unwrap_or_default().child_token();

    let extras = match options.protocol {
        ProtocolPushExtras::WebRtc(extras) => extras,
        _ => WebRtcPushExtras::default(),
    };
    let allow_private_ips = extras.allow_private_ips.unwrap_or(allow_private_ips);

    let config = match extras.driver_config {
        Some(cfg) => *cfg,
        None => cheetah_webrtc_driver_tokio::WebRtcDriverConfig {
            listen_udp: SocketAddr::from(([127, 0, 0, 1], 0)),
            public_ips: vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
            ..Default::default()
        },
    };

    let driver =
        spawn_driver(config, cancel.clone())
            .await
            .map_err(|err| ConnectorError::Connect {
                protocol,
                endpoint: http_url.clone(),
                source: Box::new(err),
            })?;

    let (ready_tx, ready_rx) = tokio::sync::watch::channel(false);
    let (cmd_tx, cmd_rx) = mpsc::channel(DEFAULT_COMMAND_QUEUE);
    let state = Arc::new(Mutex::new(SinkState::new(DEFAULT_BUFFER_CAPACITY)));
    state.lock().tracks = options.tracks;

    let run = run_webrtc(
        driver,
        cmd_rx,
        http_url,
        allow_private_ips,
        cancel.clone(),
        state.clone(),
        ready_tx,
    );

    let join = runtime_api.spawn(Box::pin(run));

    let sink = WebRtcPublisherSink::new(cmd_tx, cancel, state, join);
    Ok(PushHandle::new(
        protocol,
        url.to_string(),
        Box::new(sink),
        Arc::new(ready_rx),
    ))
}

async fn run_webrtc(
    driver: Arc<WebRtcDriverHandle>,
    mut cmd_rx: mpsc::Receiver<WebRtcSinkCommand>,
    http_url: String,
    allow_private_ips: bool,
    cancel: CancellationToken,
    state: Arc<Mutex<SinkState>>,
    ready_tx: tokio::sync::watch::Sender<bool>,
) {
    let session_id = WebRtcSessionId::new(1);
    let http_client = WhipWhepHttpClient::new();
    let mut connected = false;

    let initial_tracks = state.lock().tracks.clone();
    let offer_spec = build_offer_spec(&initial_tracks);
    driver
        .send_command(WebRtcDriverCommand::CreateOffer {
            session_id,
            role: WebRtcSessionRole::Player,
            spec: offer_spec,
            candidate_transport_policy: CandidateTransportPolicy::All,
        })
        .await;

    let mut send_fut: OptionFuture<Fuse<BoxFuture<'static, ()>>> = OptionFuture::from(None);
    let mut http_fut: OptionFuture<
        Fuse<BoxFuture<'static, Result<HttpClientResponse, HttpClientError>>>,
    > = OptionFuture::from(None);

    let try_send_next = |send_fut: &mut OptionFuture<Fuse<BoxFuture<'static, ()>>>| loop {
        if !send_fut.is_terminated() {
            return;
        }
        let frame = {
            let mut guard = state.lock();
            if guard.closed || !guard.ready {
                return;
            }
            guard.buffer.pop_front()
        };
        let Some(frame) = frame else {
            return;
        };
        let Some(send_frame) = build_send_frame(&frame, session_id, &state) else {
            continue;
        };
        let driver = driver.clone();
        let fut: BoxFuture<'static, ()> = Box::pin(async move {
            driver
                .send_command(WebRtcDriverCommand::SendFrame(Box::new(send_frame)))
                .await;
        });
        *send_fut = OptionFuture::from(Some(fut.fuse()));
        return;
    };

    let try_signal_ready = |connected: bool| {
        if !connected {
            return false;
        }
        let mut guard = state.lock();
        if guard.ready || (guard.video_mid.is_none() && guard.audio_mid.is_none()) {
            return false;
        }
        guard.ready = true;
        let _ = ready_tx.send(true);
        true
    };

    loop {
        select_biased! {
            _ = cancel.cancelled().fuse() => break,
            event = driver.recv_event().fuse() => {
                let Some(event) = event else { break; };
                match event {
                    WebRtcDriverEvent::OfferReady { session_id: id, sdp } if id == session_id => {
                        // `str0m` does not emit `MediaAdded` for locally added
                        // tracks, so the send-direction `mid` values must be
                        // extracted from the local offer SDP.
                        let (video_mid, audio_mid) = extract_mids_from_sdp(&sdp);
                        {
                            let mut guard = state.lock();
                            if video_mid.is_some() {
                                guard.video_mid = video_mid;
                            }
                            if audio_mid.is_some() {
                                guard.audio_mid = audio_mid;
                            }
                        }
                        if try_signal_ready(connected) {
                            try_send_next(&mut send_fut);
                        }

                        let mut req = HttpClientRequest::new_post_sdp(http_url.clone(), Bytes::from(sdp));
                        req.allow_private_ips = allow_private_ips;
                        let client = http_client.clone();
                        let fut: BoxFuture<'static, Result<HttpClientResponse, HttpClientError>> = Box::pin(async move {
                            client.send(req).await
                        });
                        http_fut = OptionFuture::from(Some(fut.fuse()));
                    }
                    WebRtcDriverEvent::Core(WebRtcCoreEvent::Lifecycle { session_id: id, state: s }) if id == session_id => {
                        match s {
                            WebRtcSessionLifecycle::Connected => {
                                connected = true;
                                if try_signal_ready(connected) {
                                    try_send_next(&mut send_fut);
                                }
                            }
                            WebRtcSessionLifecycle::Failed | WebRtcSessionLifecycle::Closed => {
                                break;
                            }
                            _ => {}
                        }
                    }
                    WebRtcDriverEvent::Core(WebRtcCoreEvent::Ice { session_id: id, state: s }) if id == session_id => {
                        // ICE connected is an intermediate step; `Lifecycle::Connected`
                        // (emitted after DTLS/SRTP is up) drives readiness.
                        let _ = s;
                    }
                    WebRtcDriverEvent::Core(WebRtcCoreEvent::MediaTrackAdded { session_id: id, track }) if id == session_id => {
                        if matches!(track.direction, WebRtcMediaDirection::SendOnly | WebRtcMediaDirection::SendRecv) {
                            let mut guard = state.lock();
                            let mid = track.mid;
                            match track.kind {
                                WebRtcMediaKind::Video => guard.video_mid = Some(mid),
                                WebRtcMediaKind::Audio => guard.audio_mid = Some(mid),
                            }
                            drop(guard);
                            if try_signal_ready(connected) {
                                try_send_next(&mut send_fut);
                            }
                        }
                    }
                    WebRtcDriverEvent::Core(WebRtcCoreEvent::RtcpFeedback { session_id: id, feedback }) if id == session_id => {
                        if matches!(feedback, WebRtcRtcpFeedback::Pli { .. } | WebRtcRtcpFeedback::Fir { .. }) {
                            state.lock().keyframe_requests += 1;
                        }
                    }
                    WebRtcDriverEvent::SessionClosed { session_id: id, .. } if id == session_id => {
                        break;
                    }
                    _ => {}
                }
            }
            cmd = cmd_rx.next().fuse() => {
                match cmd {
                    Some(WebRtcSinkCommand::UpdateTracks(tracks)) => {
                        state.lock().tracks = tracks;
                    }
                    Some(WebRtcSinkCommand::SendFrame(frame)) => {
                        let mut guard = state.lock();
                        if guard.closed {
                            drop(guard);
                        } else if guard.buffer.len() < guard.buffer_capacity {
                            guard.buffer.push_back(frame);
                            drop(guard);
                            try_send_next(&mut send_fut);
                        }
                    }
                    None => break,
                }
            }
            http = http_fut => {
                match http {
                    Some(Ok(resp)) => {
                        if resp.status != 201 {
                            break;
                        }
                        let answer_sdp = String::from_utf8_lossy(&resp.body).to_string();
                        driver.send_command(WebRtcDriverCommand::ApplyRemoteAnswer {
                            session_id,
                            remote_sdp: answer_sdp,
                        }).await;
                        http_fut = OptionFuture::from(None);
                    }
                    Some(Err(err)) => {
                        warn!("webrtc whip post failed: {err}");
                        break;
                    }
                    None => {}
                }
            }
            send = send_fut => {
                if send.is_some() {
                    try_send_next(&mut send_fut);
                }
            }
        }
    }

    let _ = driver
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;
    state.lock().closed = true;
    let _ = ready_tx.send(false);
}

fn build_send_frame(
    frame: &AVFrame,
    session_id: WebRtcSessionId,
    state: &Arc<Mutex<SinkState>>,
) -> Option<WebRtcSendFrame> {
    let webrtc_kind = match frame.media_kind {
        MediaKind::Video => WebRtcMediaKind::Video,
        MediaKind::Audio => WebRtcMediaKind::Audio,
        _ => return None,
    };

    let guard = state.lock();
    let mid = match webrtc_kind {
        WebRtcMediaKind::Video => guard.video_mid.clone(),
        WebRtcMediaKind::Audio => guard.audio_mid.clone(),
    };
    let mid = mid?;
    let codec = map_codec_id(frame.codec)?;
    let clock_rate = track_clock_rate(&guard.tracks, frame.track_id, frame.codec);
    let payload = frame.payload.clone();
    drop(guard);

    let rtp_ticks = compute_rtp_timestamp(&RtpTimestampInput {
        pts: frame.pts,
        dts: frame.dts,
        timebase: frame.timebase,
        media_kind: frame.media_kind,
        codec: frame.codec,
        clock_rate,
        mode: RtpTimestampMode::Live,
        source_frame_number: None,
        source_pts: None,
        source_timebase: None,
        samples_per_frame: codec_default_samples_per_frame(frame.codec),
    });

    Some(WebRtcSendFrame {
        session_id,
        mid,
        codec,
        clock_rate,
        rtp_timestamp_ticks: rtp_ticks,
        rtp_timestamp_denom: clock_rate,
        random_access: frame.flags.contains(FrameFlags::KEY),
        payload,
        network_time_micros: 0,
    })
}

fn build_offer_spec(tracks: &[TrackInfo]) -> WebRtcOfferSpec {
    let has_video = tracks.iter().any(|t| t.media_kind == MediaKind::Video);
    let has_audio = tracks.iter().any(|t| t.media_kind == MediaKind::Audio);
    WebRtcOfferSpec {
        video_direction: if has_video {
            Some(WebRtcOfferDirection::SendOnly)
        } else {
            None
        },
        audio_direction: if has_audio {
            Some(WebRtcOfferDirection::SendOnly)
        } else {
            None
        },
        data_channel: false,
    }
}

fn track_clock_rate(tracks: &[TrackInfo], track_id: TrackId, codec: CodecId) -> u32 {
    tracks
        .iter()
        .find(|t| t.track_id == track_id)
        .map(|t| t.clock_rate)
        .filter(|&r| r != 0)
        .unwrap_or_else(|| codec_rtp_clock_rate(codec))
}

fn extract_mids_from_sdp(sdp: &str) -> (Option<MidLabel>, Option<MidLabel>) {
    let mut video = None;
    let mut audio = None;
    let mut current_kind = None;

    for line in sdp.lines() {
        if let Some(rest) = line.strip_prefix("m=") {
            current_kind = rest.split_whitespace().next();
        } else if let Some(mid) = line.strip_prefix("a=mid:") {
            match current_kind {
                Some("video") if video.is_none() => video = Some(MidLabel::new(mid.to_string())),
                Some("audio") if audio.is_none() => audio = Some(MidLabel::new(mid.to_string())),
                _ => {}
            }
        }
    }

    (video, audio)
}

fn map_codec_id(codec: CodecId) -> Option<WebRtcCodecKind> {
    Some(match codec {
        CodecId::H264 => WebRtcCodecKind::H264,
        CodecId::H265 => WebRtcCodecKind::H265,
        CodecId::VP8 => WebRtcCodecKind::Vp8,
        CodecId::VP9 => WebRtcCodecKind::Vp9,
        CodecId::AV1 => WebRtcCodecKind::Av1,
        CodecId::Opus => WebRtcCodecKind::Opus,
        CodecId::G711A => WebRtcCodecKind::Pcma,
        CodecId::G711U => WebRtcCodecKind::Pcmu,
        _ => return None,
    })
}

fn normalize_whip_url(url: &str) -> Result<(String, bool), ConnectorError> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(ConnectorError::InvalidUrl {
            protocol: Protocol::WebRtc,
            url: url.to_string(),
            reason: "missing scheme".to_string(),
        });
    };

    let (http_scheme, allow_private) = match scheme {
        "http" => ("http", false),
        "https" => ("https", false),
        "webrtc+whip" => ("http", false),
        "whip" => ("http", false),
        _ => {
            return Err(ConnectorError::InvalidUrl {
                protocol: Protocol::WebRtc,
                url: url.to_string(),
                reason: format!("unsupported scheme {scheme}"),
            });
        }
    };

    if rest.is_empty() {
        return Err(ConnectorError::InvalidUrl {
            protocol: Protocol::WebRtc,
            url: url.to_string(),
            reason: "missing host".to_string(),
        });
    }

    let host = extract_host(rest);
    let allow_private = allow_private
        || host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]";

    Ok((format!("{http_scheme}://{rest}"), allow_private))
}

fn extract_host(rest: &str) -> String {
    let authority = rest.split_once('/').map(|(a, _)| a).unwrap_or(rest);
    if let Some(end) = authority.find(']') {
        if authority.starts_with('[') {
            return authority[1..end].to_string();
        }
    }
    authority
        .split_once(':')
        .map(|(h, _)| h)
        .unwrap_or(authority)
        .to_string()
}
