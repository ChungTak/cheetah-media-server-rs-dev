use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind, RtpHeader, RtpPacket,
    Timebase, TrackId, TrackInfo,
};
use cheetah_config::ConfigStore;
use cheetah_engine::{Engine, EngineBuilder, EngineMediaFacade};
use cheetah_record_module::RecordModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::media_api::model::OnlineState;
use cheetah_sdk::media_api::port::MediaControlApi;
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
        let runtime = Arc::new(TokioRuntime::new());
        let temp_dir =
            std::env::temp_dir().join(format!("cheetah-gb28181-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let config = Arc::new(ConfigStore::new());
        let yaml = format!(
            "modules:\n  rtp:\n    enabled: true\n    listen_udp: \"0.0.0.0:0\"\n    listen_tcp: \"0.0.0.0:0\"\n  record:\n    enabled: true\n    root_path: \"{}\"\n",
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

        // Give the RTP driver a moment to bind its default sockets.
        sleep(Duration::from_millis(50)).await;

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

    pub async fn stop(self) {
        self.engine.stop().await;
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
