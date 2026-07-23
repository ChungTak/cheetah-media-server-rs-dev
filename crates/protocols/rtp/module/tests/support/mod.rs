use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind, RtpHeader, RtpPacket,
    Timebase, TrackId, TrackInfo,
};
use cheetah_config::ConfigStore;
use cheetah_engine::{Engine, EngineBuilder, EngineMediaFacade};
use cheetah_record_module::RecordModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::media_api::command::{OpenPlaybackRequest, PlaybackControl, PlaybackQuery};
use cheetah_sdk::media_api::error::{MediaError, Result as MediaResult};
use cheetah_sdk::media_api::ids::PlaybackSessionId;
use cheetah_sdk::media_api::model::{
    AdmissionRequest, Decision, OnlineState, Page, PlaybackSession, PlaybackSessionState,
};
use cheetah_sdk::media_api::port::{MediaAdmissionApi, MediaControlApi, PlaybackApi};
use cheetah_sdk::media_api::MediaCapabilitySet;
use cheetah_sdk::media_api::MediaRequestContext;
use cheetah_sdk::{
    PublisherOptions, PublisherSink, StreamKey, StreamManagerApi, SubscriberOptions,
    SubscriberSource,
};
use tokio::net::UdpSocket;
use tokio::time::{sleep, timeout};

pub struct Gb28181TestHarness {
    pub engine: Engine,
    #[allow(dead_code)]
    pub temp_dir: PathBuf,
}

impl Gb28181TestHarness {
    pub async fn start() -> Self {
        Self::start_with_rtp_config("").await
    }

    /// Start the RTP module with a custom `PlaybackApi` provider and capability set.
    /// The `record` module is omitted so the harness can control playback capability
    /// registration exactly.
    pub async fn start_with_playback(
        playback: Arc<dyn PlaybackApi>,
        playback_capabilities: MediaCapabilitySet,
        extra_rtp_config: &str,
    ) -> Self {
        let runtime = Arc::new(TokioRuntime::new());
        let temp_dir =
            std::env::temp_dir().join(format!("cheetah-gb28181-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let config = Arc::new(ConfigStore::new());
        let yaml = format!(
            "modules:\n  rtp:\n    enabled: true\n    listen_udp: \"0.0.0.0:0\"\n    listen_tcp: \"0.0.0.0:0\"\n{extra_rtp_config}"
        );
        config.load_yaml_str(&yaml).expect("load config");

        let engine = EngineBuilder::new(config.clone(), config.clone(), runtime.clone())
            .with_config_schema_registry(config)
            .register_module_factory(Arc::new(cheetah_rtp_module::RtpModuleFactory))
            .build()
            .expect("build engine");

        engine
            .media_services()
            .register_playback_with_capabilities(playback, playback_capabilities);

        engine.start().await.expect("start engine");

        sleep(Duration::from_millis(50)).await;
        tokio::task::yield_now().await;

        Self { engine, temp_dir }
    }

    pub async fn start_with_rtp_config(extra_rtp_config: &str) -> Self {
        let runtime = Arc::new(TokioRuntime::new());
        let temp_dir =
            std::env::temp_dir().join(format!("cheetah-gb28181-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let config = Arc::new(ConfigStore::new());
        let yaml = format!(
            "modules:\n  rtp:\n    enabled: true\n    listen_udp: \"0.0.0.0:0\"\n    listen_tcp: \"0.0.0.0:0\"\n{extra_rtp_config}  record:\n    enabled: true\n    root_path: \"{}\"\n",
            temp_dir.display()
        );
        config.load_yaml_str(&yaml).expect("load config");

        let engine = EngineBuilder::new(config.clone(), config.clone(), runtime.clone())
            .with_config_schema_registry(config)
            .register_module_factory(Arc::new(cheetah_rtp_module::RtpModuleFactory))
            .register_module_factory(Arc::new(RecordModuleFactory))
            .build()
            .expect("build engine");

        engine.start().await.expect("start engine");

        // Give the RTP driver a moment to bind its default sockets and let the
        // ingress worker task start before test logic runs.
        sleep(Duration::from_millis(50)).await;
        tokio::task::yield_now().await;

        Self { engine, temp_dir }
    }

    pub fn media_facade(&self) -> Arc<EngineMediaFacade> {
        self.engine.media_facade()
    }

    pub fn stream_manager(&self) -> Arc<dyn StreamManagerApi> {
        self.engine.stream_manager_api()
    }

    pub async fn wait_for_stream_online(&self, stream_key: &StreamKey, max_wait: Duration) {
        let deadline = tokio::time::Instant::now() + max_wait;
        let media = self.media_facade();
        let media_key = stream_key_to_media_key(stream_key);
        loop {
            let state = media
                .is_media_online(&MediaRequestContext::default(), &media_key)
                .await
                .expect("is_media_online");
            if state == OnlineState::Online {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("timeout waiting for stream {stream_key} to come online");
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn open_publisher(
        &self,
        stream_key: StreamKey,
        tracks: Vec<TrackInfo>,
    ) -> Box<dyn PublisherSink> {
        let sink = self
            .stream_manager()
            .open_publisher(stream_key, PublisherOptions::default())
            .await
            .expect("open publisher");
        sink.update_tracks(tracks).expect("update tracks");
        sink
    }

    pub async fn open_subscriber(&self, stream_key: StreamKey) -> Box<dyn SubscriberSource> {
        self.stream_manager()
            .open_subscriber(stream_key, SubscriberOptions::default())
            .await
            .expect("open subscriber")
    }

    pub fn set_admission_deny(&self, deny: bool) {
        let provider = Arc::new(FakeAdmissionProvider::new(deny));
        self.engine.media_services().register_admission(provider);
    }

    pub async fn stop(self) {
        self.engine.stop().await;
    }
}

pub struct FakeAdmissionProvider {
    deny: AtomicBool,
}

impl FakeAdmissionProvider {
    pub fn new(deny: bool) -> Self {
        Self {
            deny: AtomicBool::new(deny),
        }
    }
}

#[async_trait]
impl MediaAdmissionApi for FakeAdmissionProvider {
    async fn authorize(
        &self,
        _ctx: &MediaRequestContext,
        _request: AdmissionRequest,
    ) -> MediaResult<Decision> {
        if self.deny.load(Ordering::SeqCst) {
            Ok(Decision::Deny {
                code: cheetah_sdk::media_api::error::MediaErrorCode::PermissionDenied,
                reason: "admission denied by test".to_string(),
            })
        } else {
            Ok(Decision::Allow)
        }
    }
}

pub fn stream_key_to_media_key(stream_key: &StreamKey) -> cheetah_sdk::media_api::MediaKey {
    cheetah_sdk::media_api::MediaKey::with_default_vhost(
        &stream_key.namespace,
        &stream_key.path,
        None,
    )
    .expect("media key")
}

pub fn media_key_to_stream_key(media_key: &cheetah_sdk::media_api::MediaKey) -> StreamKey {
    let (namespace, path) =
        cheetah_sdk::media_api::ids::StreamKeyBridge::to_namespace_path(media_key);
    StreamKey::new(&namespace, &path)
}

pub fn make_video_track() -> TrackInfo {
    let mut track = TrackInfo::new(TrackId(0xE0), MediaKind::Video, CodecId::H264, 90_000);
    track.extradata = CodecExtradata::H264 {
        sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x0A])],
        pps: vec![Bytes::from_static(&[0x68, 0xCE, 0x38, 0x80])],
        avcc: None,
    };
    track.refresh_readiness();
    track
}

pub fn make_audio_track() -> TrackInfo {
    TrackInfo::new(TrackId(0xC0), MediaKind::Audio, CodecId::G711A, 8_000)
}

pub fn make_video_frame(pts_us: i64) -> AVFrame {
    let mut payload = vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x0A];
    // Annex-B IDR NAL (type 5) so the PS demuxer marks emitted frames as keyframes.
    payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65]);
    payload.extend_from_slice(b"video frame data");
    let mut frame = AVFrame::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        90_000,
        90_000,
        Timebase::new(1, 90_000),
        Bytes::from(payload),
    );
    frame.pts_us = pts_us;
    frame.dts_us = pts_us;
    frame.flags.insert(FrameFlags::KEY);
    frame
}

pub fn make_audio_frame(pts_us: i64) -> AVFrame {
    let mut frame = AVFrame::new(
        TrackId(0xC0),
        MediaKind::Audio,
        CodecId::G711A,
        FrameFormat::G711Packet,
        90_080,
        90_080,
        Timebase::new(1, 8_000),
        Bytes::from_static(b"audio frame data"),
    );
    frame.pts_us = pts_us;
    frame.dts_us = pts_us;
    frame
}

pub fn mux_ps_frame(frame: &AVFrame) -> Bytes {
    let mut muxer = cheetah_codec::PsMuxer::new();
    muxer.add_track(make_video_track());
    muxer.add_track(make_audio_track());
    muxer.mux(frame).expect("mux frame")
}

pub fn encode_rtp(
    payload: Bytes,
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
    payload_type: u8,
) -> Bytes {
    let packet = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type,
            sequence_number: sequence,
            timestamp,
            ssrc,
            marker: false,
        },
        payload,
    };
    packet.encode()
}

pub async fn bind_udp_socket() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").await.expect("bind udp")
}

pub async fn send_rtp(
    socket: &UdpSocket,
    dest: SocketAddr,
    payload: Bytes,
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
    payload_type: u8,
) {
    let packet = encode_rtp(payload, ssrc, sequence, timestamp, payload_type);
    socket.send_to(&packet, dest).await.expect("send rtp");
}

/// Minimal fake `PlaybackApi` for tests that need playback source support.
#[derive(Clone, Default)]
pub struct FakePlayback {
    sessions: Arc<Mutex<HashMap<String, PlaybackSession>>>,
    open_count: Arc<AtomicUsize>,
    stop_count: Arc<AtomicUsize>,
}

impl FakePlayback {
    pub fn open_count(&self) -> usize {
        self.open_count.load(Ordering::SeqCst)
    }

    pub fn stop_count(&self) -> usize {
        self.stop_count.load(Ordering::SeqCst)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[async_trait]
impl PlaybackApi for FakePlayback {
    async fn open_playback(
        &self,
        _ctx: &MediaRequestContext,
        request: OpenPlaybackRequest,
    ) -> MediaResult<PlaybackSession> {
        self.open_count.fetch_add(1, Ordering::SeqCst);
        let id = format!("pb-{}", self.open_count.load(Ordering::SeqCst));
        // Playback output uses an independent stream key so it never overwrites
        // or bypasses the live source publisher lease.
        let output_key = cheetah_sdk::media_api::MediaKey::new(
            request.media_key.vhost.0.clone(),
            request.media_key.app.0.clone(),
            format!("playback_{}", request.media_key.stream.0),
            None,
        )
        .ok()
        .or(Some(request.media_key.clone()));
        let session = PlaybackSession {
            session_id: PlaybackSessionId(id.clone()),
            media_key: request.media_key.clone(),
            file_handle: request.file_handle.clone(),
            state: PlaybackSessionState::Playing,
            duration_ms: 0,
            position_ms: request.start_position_ms,
            scale: request.scale,
            generation: 1,
            output_key,
            last_error: None,
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        self.sessions.lock().unwrap().insert(id, session.clone());
        Ok(session)
    }

    async fn get_playback(
        &self,
        _ctx: &MediaRequestContext,
        id: &PlaybackSessionId,
    ) -> MediaResult<PlaybackSession> {
        self.sessions
            .lock()
            .unwrap()
            .get(&id.0)
            .cloned()
            .ok_or_else(|| MediaError::not_found(format!("playback session not found: {}", id.0)))
    }

    async fn list_playbacks(
        &self,
        _ctx: &MediaRequestContext,
        _query: PlaybackQuery,
    ) -> MediaResult<Page<PlaybackSession>> {
        let items: Vec<_> = self.sessions.lock().unwrap().values().cloned().collect();
        let total = items.len() as u64;
        Ok(Page {
            items,
            page: 1,
            page_size: total.max(1),
            total,
            next_cursor: None,
        })
    }

    async fn control_playback(
        &self,
        _ctx: &MediaRequestContext,
        id: &PlaybackSessionId,
        command: PlaybackControl,
    ) -> MediaResult<PlaybackSession> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions.get_mut(&id.0).ok_or_else(|| {
            MediaError::not_found(format!("playback session not found: {}", id.0))
        })?;
        match command {
            PlaybackControl::Pause => session.state = PlaybackSessionState::Paused,
            PlaybackControl::Resume => session.state = PlaybackSessionState::Playing,
            PlaybackControl::Seek { position_ms } => session.position_ms = position_ms,
            PlaybackControl::SetScale { scale } => session.scale = scale,
        }
        session.updated_at = now_ms();
        Ok(session.clone())
    }

    async fn stop_playback(
        &self,
        _ctx: &MediaRequestContext,
        id: &PlaybackSessionId,
    ) -> MediaResult<()> {
        self.stop_count.fetch_add(1, Ordering::SeqCst);
        self.sessions.lock().unwrap().remove(&id.0);
        Ok(())
    }
}

pub async fn recv_rtp(
    socket: &UdpSocket,
    timeout_after: Duration,
) -> Option<(RtpHeader, Bytes, SocketAddr)> {
    let mut buf = vec![0u8; 2048];
    match timeout(timeout_after, socket.recv_from(&mut buf)).await {
        Ok(Ok((len, addr))) => {
            let bytes = Bytes::copy_from_slice(&buf[..len]);
            RtpPacket::parse(&bytes).map(|p| (p.header, p.payload, addr))
        }
        _ => None,
    }
}
