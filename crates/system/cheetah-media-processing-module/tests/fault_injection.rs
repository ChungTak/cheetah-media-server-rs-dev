#![cfg(feature = "media-processing-cpu")]

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::net::{
    SocketAddr, TcpListener as StdTcpListener, TcpStream as StdTcpStream, UdpSocket as StdUdpSocket,
};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use cheetah_codec::MonoTime;
use cheetah_codec::{
    frame::{FrameFlags, FrameFormat, FrameOrigin},
    track::{MediaKind, TrackId, TrackInfo, TrackReadiness},
    AVFrame, CodecId, Rational32, Timebase,
};
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_api::error::MediaErrorCode;
use cheetah_media_api::ids::MediaKey;
use cheetah_media_api::port::MediaRequestContext;
use cheetah_media_api::processing::{
    AudioCodec, AudioTarget, CreateProcessingJob, ProcessingJob, ProcessingJobId,
    ProcessingJobSpec, ProcessingJobState, TrackSelection, VideoCodec, VideoTarget,
};
use cheetah_media_processing_module::config::MediaProcessingModuleConfig;
use cheetah_media_processing_module::MediaProcessingModuleFactory;
use cheetah_runtime_api::{
    AsyncTcpListener, AsyncTcpStream, AsyncTimer, AsyncUdpSocket, ConnectTcpFuture,
    ConnectTlsFuture, JoinHandle, ResolveHostFuture, RuntimeApi, SpawnError,
};
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{
    BackpressurePolicy, BootstrapMode, BootstrapPolicy, MediaFilter, ModuleId, PublisherOptions,
    StreamKey, SubscriberOptions,
};
use tokio::time::{sleep, timeout};

#[cfg(feature = "media-processing-cpu")]
use avcodec::{
    core::{CodecId as AvCodecId, Image, ImageInfo, PacketFlags, Poll, TimeBase},
    VideoEncoderRequest, VideoProfile, VideoSdk,
};

// ---------------------------------------------------------------------------
// Fault injection runtime wrapper
//
// Wraps TokioRuntime so tests can fail or panic selected `spawn_blocking`
// workers without changing production code. All other runtime primitives are
// delegated unchanged.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FaultState {
    fail_on: HashSet<String>,
    panic_on: HashSet<String>,
    fail_count: HashMap<String, usize>,
    panic_count: HashMap<String, usize>,
}

impl FaultState {
    fn should_fail(&mut self, name: &str) -> bool {
        if let Some(n) = self.fail_count.get_mut(name) {
            if *n > 0 {
                *n -= 1;
                return true;
            }
        }
        self.fail_on.contains(name)
    }

    fn should_panic(&mut self, name: &str) -> bool {
        if let Some(n) = self.panic_count.get_mut(name) {
            if *n > 0 {
                *n -= 1;
                return true;
            }
        }
        self.panic_on.contains(name)
    }
}

#[derive(Clone)]
struct FaultRuntime {
    inner: Arc<dyn RuntimeApi>,
    state: Arc<Mutex<FaultState>>,
}

impl FaultRuntime {
    fn new(inner: Arc<dyn RuntimeApi>) -> Self {
        Self {
            inner,
            state: Arc::new(Mutex::new(FaultState::default())),
        }
    }

    fn fail_spawn_blocking_on(&self, name: &str) {
        self.state.lock().unwrap().fail_on.insert(name.to_string());
    }

    fn panic_spawn_blocking_on(&self, name: &str) {
        self.state.lock().unwrap().panic_on.insert(name.to_string());
    }
}

impl RuntimeApi for FaultRuntime {
    fn now(&self) -> MonoTime {
        self.inner.now()
    }

    fn spawn(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
    ) -> Box<dyn JoinHandle> {
        self.inner.spawn(fut)
    }

    fn spawn_local(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + 'static>>,
    ) -> Result<Box<dyn JoinHandle>, SpawnError> {
        self.inner.spawn_local(fut)
    }

    fn spawn_blocking(
        &self,
        name: &str,
        task: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<Box<dyn JoinHandle>, SpawnError> {
        let mut state = self.state.lock().unwrap();
        if state.should_fail(name) {
            return Err(SpawnError::RuntimeUnavailable(format!(
                "injected spawn_blocking failure for {name}"
            )));
        }
        if state.should_panic(name) {
            drop(state);
            let name_for_spawn = name.to_string();
            let name_for_panic = name.to_string();
            return self.inner.spawn_blocking(
                &name_for_spawn,
                Box::new(move || {
                    let _ = task;
                    panic!("injected panic for blocking worker {name_for_panic}");
                }),
            );
        }
        drop(state);
        self.inner.spawn_blocking(name, task)
    }

    fn bind_udp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncUdpSocket>> {
        self.inner.bind_udp(addr)
    }

    fn connect_tcp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpStream>> {
        self.inner.connect_tcp(addr)
    }

    fn connect_tcp_async<'a>(&'a self, addr: SocketAddr) -> ConnectTcpFuture<'a> {
        self.inner.connect_tcp_async(addr)
    }

    fn connect_tls<'a>(&'a self, addr: SocketAddr, server_name: &str) -> ConnectTlsFuture<'a> {
        self.inner.connect_tls(addr, server_name)
    }

    fn bind_tcp(&self, addr: SocketAddr) -> io::Result<Box<dyn AsyncTcpListener>> {
        self.inner.bind_tcp(addr)
    }

    fn wrap_udp_socket(&self, socket: StdUdpSocket) -> io::Result<Box<dyn AsyncUdpSocket>> {
        self.inner.wrap_udp_socket(socket)
    }

    fn wrap_tcp_listener(&self, listener: StdTcpListener) -> io::Result<Box<dyn AsyncTcpListener>> {
        self.inner.wrap_tcp_listener(listener)
    }

    fn wrap_tcp_stream(&self, stream: StdTcpStream) -> io::Result<Box<dyn AsyncTcpStream>> {
        self.inner.wrap_tcp_stream(stream)
    }

    fn sleep_until(&self, deadline: MonoTime) -> Box<dyn AsyncTimer> {
        self.inner.sleep_until(deadline)
    }

    fn resolve_host(&self, host: &str) -> ResolveHostFuture<'_> {
        self.inner.resolve_host(host)
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn make_engine(runtime: Arc<dyn RuntimeApi>) -> cheetah_engine::Engine {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(serde_json::json!({}));
    let module_config = MediaProcessingModuleConfig {
        profile: "native-free".to_string(),
        ..Default::default()
    };
    config.register_module_default(
        ModuleId::new("media-processing"),
        serde_json::to_value(module_config).expect("module config"),
    );
    EngineBuilder::new(config.clone(), config.clone(), runtime)
        .register_module_factory(Arc::new(MediaProcessingModuleFactory))
        .build()
        .expect("engine build")
}

fn h264_video_track(width: u32, height: u32) -> TrackInfo {
    let mut track = TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::H264, 90_000);
    track.readiness = TrackReadiness::Ready;
    track.width = Some(width);
    track.height = Some(height);
    track.fps = Some(Rational32::new(30, 1));
    track
}

fn g711_audio_track() -> TrackInfo {
    let mut track = TrackInfo::new(TrackId(0), MediaKind::Audio, CodecId::G711A, 8_000);
    track.readiness = TrackReadiness::Ready;
    track.sample_rate = Some(8_000);
    track.channels = Some(1);
    track
}

fn g711_frame(pts: i64) -> AVFrame {
    let mut frame = AVFrame::new(
        TrackId(0),
        MediaKind::Audio,
        CodecId::G711A,
        FrameFormat::G711Packet,
        pts,
        pts,
        Timebase::new(1, 8_000),
        Bytes::from_static(&[0; 64]),
    );
    frame.origin = FrameOrigin::Ingest;
    frame
}

#[cfg(feature = "media-processing-cpu")]
fn encode_h264_clip(width: u32, height: u32, count: usize) -> Vec<AVFrame> {
    let sdk = VideoSdk::new().expect("video sdk");
    let mut encoder = sdk
        .create_encoder(
            VideoProfile::NativeFree,
            VideoEncoderRequest {
                codec: AvCodecId::H264,
                width,
                height,
                format: ImageInfo::Yuv420p,
                time_base: TimeBase::new(1, 30),
                bitrate: 200_000,
            },
        )
        .expect("create h264 encoder")
        .into_session();

    let y_size = (width * height) as usize;
    let uv_size = y_size / 4;
    let y = vec![128u8; y_size];
    let u = vec![128u8; uv_size];
    let v = vec![128u8; uv_size];

    let mut packets = Vec::new();
    for pts in 0..count {
        let mut img = Image::from_host_i420(
            width,
            height,
            &y,
            width as usize,
            &u,
            (width / 2) as usize,
            &v,
            (width / 2) as usize,
        )
        .expect("build yuv420p image");
        img.pts = Some(pts as i64);
        img.dts = Some(pts as i64);
        encoder.submit_image(img).expect("submit image");

        loop {
            match encoder.poll_packet().expect("poll packet") {
                Poll::Ready(packet) => packets.push(packet),
                Poll::Pending => break,
                Poll::EndOfStream => break,
            }
        }
    }

    encoder.flush().expect("flush encoder");
    loop {
        match encoder.poll_packet().expect("poll packet after flush") {
            Poll::Ready(packet) => packets.push(packet),
            Poll::Pending => break,
            Poll::EndOfStream => break,
        }
    }

    let tb = Timebase::new(1, 30);
    packets
        .into_iter()
        .enumerate()
        .map(|(i, packet)| {
            let payload = packet
                .data
                .host_bytes()
                .expect("host bytes")
                .expect("some")
                .to_vec();
            let key = packet.flags.contains(PacketFlags::KEY);
            let mut frame = AVFrame::new(
                TrackId(0),
                MediaKind::Video,
                CodecId::H264,
                FrameFormat::CanonicalH26x,
                i as i64,
                i as i64,
                tb,
                Bytes::from(payload),
            );
            if key {
                frame.flags.insert(FrameFlags::KEY);
            }
            frame.origin = FrameOrigin::Ingest;
            frame
        })
        .collect()
}

async fn wait_for_job_state(
    engine: &cheetah_engine::Engine,
    id: &ProcessingJobId,
    want: ProcessingJobState,
) -> ProcessingJob {
    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");
    for _ in 0..80 {
        if let Ok(job) = processing
            .get_job(&MediaRequestContext::default(), id)
            .await
        {
            if job.state == want {
                return job;
            }
            if matches!(
                job.state,
                ProcessingJobState::Stopped | ProcessingJobState::Failed
            ) && want != ProcessingJobState::Running
            {
                return job;
            }
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("job {id} did not reach state {want:?} in time");
}

async fn wait_for_job_terminal(
    engine: &cheetah_engine::Engine,
    id: &ProcessingJobId,
) -> ProcessingJob {
    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");
    for _ in 0..80 {
        if let Ok(job) = processing
            .get_job(&MediaRequestContext::default(), id)
            .await
        {
            if matches!(
                job.state,
                ProcessingJobState::Stopped | ProcessingJobState::Failed
            ) {
                return job;
            }
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("job {id} did not finish in time");
}

async fn assert_clean_leak_report(engine: &cheetah_engine::Engine) {
    let report = engine.resource_leak_report().await.expect("leak report");
    assert!(
        report.is_clean(),
        "expected clean leak report, got {report:?}"
    );
}

async fn start_source_publisher(
    engine: &cheetah_engine::Engine,
    key: StreamKey,
    mut clip: Vec<AVFrame>,
    track: TrackInfo,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    let publisher_api = engine.publisher_api();
    let (lease, sink) = publisher_api
        .acquire_publisher(key, PublisherOptions::default())
        .await
        .expect("acquire source publisher");

    sink.update_tracks(vec![track])
        .expect("update source track");

    let publisher_api = Arc::clone(&publisher_api);
    tokio::spawn(async move {
        for frame in clip.drain(..) {
            let _ = sink.push_frame(Arc::new(frame));
            if interval > Duration::ZERO {
                sleep(interval).await;
            }
        }
        let _ = publisher_api.release_publisher(&lease).await;
    })
}

fn transcode_request(
    source: MediaKey,
    target: MediaKey,
    video: Option<VideoTarget>,
    audio: Option<AudioTarget>,
) -> CreateProcessingJob {
    CreateProcessingJob {
        idempotency_key: None,
        deadline_ms: None,
        spec: ProcessingJobSpec::Transcode {
            source,
            target,
            track_selection: TrackSelection::All,
            video,
            audio,
            overlays: Vec::new(),
        },
    }
}

fn video_h264_target() -> VideoTarget {
    VideoTarget {
        codec: VideoCodec::H264,
        width: None,
        height: None,
        frame_rate_num: None,
        frame_rate_den: None,
        bit_rate: None,
        gop_size: None,
        profile: None,
    }
}

fn audio_mp3_target() -> AudioTarget {
    AudioTarget {
        codec: AudioCodec::Mp3,
        sample_rate: Some(44_100),
        channels: Some(2),
        bit_rate: None,
    }
}

fn video_mjpeg_target() -> VideoTarget {
    VideoTarget {
        codec: VideoCodec::MJPEG,
        width: Some(160),
        height: Some(120),
        frame_rate_num: Some(30),
        frame_rate_den: Some(1),
        bit_rate: None,
        gop_size: None,
        profile: None,
    }
}

fn ctx_with_past_deadline() -> MediaRequestContext {
    let past = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    MediaRequestContext {
        deadline: Some(past.saturating_sub(10_000)),
        ..MediaRequestContext::default()
    }
}

// ---------------------------------------------------------------------------
// Fault scenarios
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_lease_conflict_rolls_back_and_does_not_leak() {
    let runtime = Arc::new(TokioRuntime::new());
    let engine = make_engine(runtime);
    engine.start().await.expect("engine start");

    let _source_key = StreamKey::new("app", "src");
    let _target_key = StreamKey::new("app", "out");
    let source = MediaKey::with_default_vhost("app", "src", None).expect("source key");
    let target = MediaKey::with_default_vhost("app", "out", None).expect("target key");

    // Pre-acquire the target stream publisher so the processing job cannot.
    let (lease, sink) = engine
        .publisher_api()
        .acquire_publisher(_target_key.clone(), PublisherOptions::default())
        .await
        .expect("acquire target publisher");

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let result = processing
        .create_job(
            &MediaRequestContext::default(),
            transcode_request(source, target, Some(video_h264_target()), None),
        )
        .await;
    assert!(
        result.is_err(),
        "expected target conflict error, got {result:?}"
    );

    // Release the target publisher before checking for leaks.
    drop(sink);
    drop(lease);
    engine.stop().await;
    assert_clean_leak_report(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backend_selection_failure_reports_failed_job() {
    let runtime = Arc::new(TokioRuntime::new());
    let engine = make_engine(runtime);
    engine.start().await.expect("engine start");

    let _source_key = StreamKey::new("app", "g711src");
    let _target_key = StreamKey::new("app", "mp3out");
    let source = MediaKey::with_default_vhost("app", "g711src", None).expect("source key");
    let target = MediaKey::with_default_vhost("app", "mp3out", None).expect("target key");

    // MP3 encode is rejected at create (not a supported output target).
    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let err = processing
        .create_job(
            &MediaRequestContext::default(),
            transcode_request(source, target, None, Some(audio_mp3_target())),
        )
        .await
        .expect_err("MP3 encode target must be rejected at create");
    assert_eq!(err.code, MediaErrorCode::Unsupported);
    assert!(
        err.message.to_ascii_lowercase().contains("mp3"),
        "error should mention MP3: {err}"
    );

    let _ = _source_key;
    engine.stop().await;
    assert_clean_leak_report(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_video_output_codec_fails_job() {
    let runtime = Arc::new(TokioRuntime::new());
    let engine = make_engine(runtime);
    engine.start().await.expect("engine start");

    let _source_key = StreamKey::new("app", "h264src");
    let _target_key = StreamKey::new("app", "mjpegout");
    let source = MediaKey::with_default_vhost("app", "h264src", None).expect("source key");
    let target = MediaKey::with_default_vhost("app", "mjpegout", None).expect("target key");

    let clip = encode_h264_clip(160, 120, 3);
    let feed = start_source_publisher(
        &engine,
        _source_key.clone(),
        clip,
        h264_video_track(160, 120),
        Duration::from_millis(50),
    )
    .await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let job = processing
        .create_job(
            &MediaRequestContext::default(),
            transcode_request(source, target, Some(video_mjpeg_target()), None),
        )
        .await
        .expect("create job");

    let job = wait_for_job_terminal(&engine, &job.job_id).await;
    assert!(
        matches!(job.state, ProcessingJobState::Failed),
        "expected failed job for unsupported output codec, got {job:?}"
    );

    let _ = timeout(Duration::from_secs(3), feed).await;
    engine.stop().await;
    assert_clean_leak_report(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_video_packet_stops_job_without_process_panic() {
    let runtime = Arc::new(TokioRuntime::new());
    let engine = make_engine(runtime);
    engine.start().await.expect("engine start");

    let _source_key = StreamKey::new("app", "corruptsrc");
    let _target_key = StreamKey::new("app", "corruptout");
    let source = MediaKey::with_default_vhost("app", "corruptsrc", None).expect("source key");
    let target = MediaKey::with_default_vhost("app", "corruptout", None).expect("target key");

    let mut frame = AVFrame::new(
        TrackId(0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        0,
        0,
        Timebase::new(1, 30),
        Bytes::from_static(&[0xff; 128]),
    );
    frame.flags.insert(FrameFlags::KEY);
    frame.flags.insert(FrameFlags::CORRUPTED);
    frame.origin = FrameOrigin::Ingest;

    let track = h264_video_track(160, 120);
    let feed = start_source_publisher(
        &engine,
        _source_key.clone(),
        vec![frame],
        track,
        Duration::from_millis(20),
    )
    .await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let job = processing
        .create_job(
            &MediaRequestContext::default(),
            transcode_request(source, target, Some(video_h264_target()), None),
        )
        .await
        .expect("create job");

    let job = wait_for_job_terminal(&engine, &job.job_id).await;
    // Corrupt payloads may be ignored or decoded; the important property is
    // that the worker does not panic and terminates cleanly.
    assert!(
        matches!(
            job.state,
            ProcessingJobState::Stopped | ProcessingJobState::Failed
        ),
        "expected terminal job after corrupt packet, got {job:?}"
    );

    let _ = timeout(Duration::from_secs(3), feed).await;
    engine.stop().await;
    assert_clean_leak_report(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_spawn_failure_is_handled_and_releases_lease() {
    let tokio_rt = Arc::new(TokioRuntime::new());
    let fault_rt = Arc::new(FaultRuntime::new(tokio_rt));
    fault_rt.fail_spawn_blocking_on("transcode-worker");
    let engine = make_engine(fault_rt);
    engine.start().await.expect("engine start");

    let _source_key = StreamKey::new("app", "src");
    let _target_key = StreamKey::new("app", "out");
    let source = MediaKey::with_default_vhost("app", "src", None).expect("source key");
    let target = MediaKey::with_default_vhost("app", "out", None).expect("target key");

    let clip = encode_h264_clip(160, 120, 3);
    let feed = start_source_publisher(
        &engine,
        _source_key.clone(),
        clip,
        h264_video_track(160, 120),
        Duration::from_millis(50),
    )
    .await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let job = processing
        .create_job(
            &MediaRequestContext::default(),
            transcode_request(source, target, Some(video_h264_target()), None),
        )
        .await
        .expect("create job");

    let job = wait_for_job_terminal(&engine, &job.job_id).await;
    assert!(
        matches!(job.state, ProcessingJobState::Failed),
        "expected failed job after worker spawn failure, got {job:?}"
    );

    let _ = timeout(Duration::from_secs(3), feed).await;
    engine.stop().await;
    assert_clean_leak_report(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_panic_is_handled_and_releases_lease() {
    let tokio_rt = Arc::new(TokioRuntime::new());
    let fault_rt = Arc::new(FaultRuntime::new(tokio_rt));
    fault_rt.panic_spawn_blocking_on("transcode-worker");
    let engine = make_engine(fault_rt);
    engine.start().await.expect("engine start");

    let _source_key = StreamKey::new("app", "src");
    let _target_key = StreamKey::new("app", "out");
    let source = MediaKey::with_default_vhost("app", "src", None).expect("source key");
    let target = MediaKey::with_default_vhost("app", "out", None).expect("target key");

    let clip = encode_h264_clip(160, 120, 3);
    let feed = start_source_publisher(
        &engine,
        _source_key.clone(),
        clip,
        h264_video_track(160, 120),
        Duration::from_millis(50),
    )
    .await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let job = processing
        .create_job(
            &MediaRequestContext::default(),
            transcode_request(source, target, Some(video_h264_target()), None),
        )
        .await
        .expect("create job");

    let job = wait_for_job_terminal(&engine, &job.job_id).await;
    assert!(
        matches!(job.state, ProcessingJobState::Failed),
        "expected failed job after worker panic, got {job:?}"
    );

    let _ = timeout(Duration::from_secs(3), feed).await;
    engine.stop().await;
    assert_clean_leak_report(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frame_drops_under_output_pressure() {
    let runtime = Arc::new(TokioRuntime::new());
    let engine = make_engine(runtime);
    engine.start().await.expect("engine start");

    let _source_key = StreamKey::new("app", "src");
    let _target_key = StreamKey::new("app", "out");
    let source = MediaKey::with_default_vhost("app", "src", None).expect("source key");
    let target = MediaKey::with_default_vhost("app", "out", None).expect("target key");

    let clip = encode_h264_clip(160, 120, 120);
    // Feed frames quickly to pressure the worker input/output queues.
    let feed = start_source_publisher(
        &engine,
        _source_key.clone(),
        clip,
        h264_video_track(160, 120),
        Duration::from_millis(2),
    )
    .await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let job = processing
        .create_job(
            &MediaRequestContext::default(),
            transcode_request(source, target, Some(video_h264_target()), None),
        )
        .await
        .expect("create job");

    // Attach a slow subscriber once the target publisher has been acquired.
    // DropUntilNextKeyframe makes the dispatcher drop non-key output frames
    // instead of removing the subscriber or erroring out the worker.
    let subscriber_options = SubscriberOptions {
        queue_capacity: 1,
        backpressure: BackpressurePolicy::DropUntilNextKeyframe,
        bootstrap_policy: BootstrapPolicy {
            mode: BootstrapMode::None,
            max_bootstrap_age_ms: None,
            max_bootstrap_frames: 0,
            wait_for_next_random_access_point: false,
        },
        media_filter: MediaFilter::default(),
    };
    let mut subscriber = None;
    for _ in 0..50 {
        match engine
            .subscriber_api()
            .subscribe(_target_key.clone(), subscriber_options.clone())
            .await
        {
            Ok(s) => {
                subscriber = Some(s);
                break;
            }
            Err(_) => sleep(Duration::from_millis(50)).await,
        }
    }
    let mut subscriber = subscriber.expect("target stream did not become subscribable");
    let reader = tokio::spawn(async move {
        loop {
            match subscriber.recv().await {
                Ok(None) | Err(_) => break,
                _ => sleep(Duration::from_millis(10)).await,
            }
        }
    });

    // Wait until the job has observed frame drops.
    let mut final_job = job.clone();
    for _ in 0..80 {
        if let Ok(j) = processing
            .get_job(&MediaRequestContext::default(), &job.job_id)
            .await
        {
            if j.drops > 0 {
                final_job = j;
                break;
            }
        }
        sleep(Duration::from_millis(50)).await;
    }

    assert!(
        final_job.drops > 0,
        "expected frame drops under pressure, got job {final_job:?}"
    );

    reader.abort();
    let _ = timeout(Duration::from_secs(2), feed).await;
    let _ = processing
        .stop_job(&MediaRequestContext::default(), &job.job_id)
        .await;
    let _ = wait_for_job_state(&engine, &job.job_id, ProcessingJobState::Stopped).await;
    engine.stop().await;
    assert_clean_leak_report(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deadline_exceeded_at_create_is_rejected() {
    let runtime = Arc::new(TokioRuntime::new());
    let engine = make_engine(runtime);
    engine.start().await.expect("engine start");

    let source = MediaKey::with_default_vhost("app", "src", None).expect("source key");
    let target = MediaKey::with_default_vhost("app", "out", None).expect("target key");

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let result = processing
        .create_job(
            &ctx_with_past_deadline(),
            transcode_request(source, target, Some(video_h264_target()), None),
        )
        .await;
    assert!(
        result.is_err(),
        "expected deadline exceeded, got {result:?}"
    );

    engine.stop().await;
    assert_clean_leak_report(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_after_start_stops_job_and_releases_resources() {
    let runtime = Arc::new(TokioRuntime::new());
    let engine = make_engine(runtime);
    engine.start().await.expect("engine start");

    let _source_key = StreamKey::new("app", "src");
    let _target_key = StreamKey::new("app", "out");
    let source = MediaKey::with_default_vhost("app", "src", None).expect("source key");
    let target = MediaKey::with_default_vhost("app", "out", None).expect("target key");

    let clip = encode_h264_clip(160, 120, 300);
    let feed = start_source_publisher(
        &engine,
        _source_key.clone(),
        clip,
        h264_video_track(160, 120),
        Duration::from_millis(20),
    )
    .await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let job = processing
        .create_job(
            &MediaRequestContext::default(),
            transcode_request(source, target, Some(video_h264_target()), None),
        )
        .await
        .expect("create job");

    // Give the worker a moment to start, then cancel it.
    sleep(Duration::from_millis(200)).await;
    let stopped = processing
        .stop_job(&MediaRequestContext::default(), &job.job_id)
        .await
        .expect("stop job");
    assert!(
        matches!(stopped.state, ProcessingJobState::Stopped),
        "expected stopped job, got {stopped:?}"
    );

    feed.abort();
    let _ = timeout(Duration::from_secs(2), feed).await;
    engine.stop().await;
    assert_clean_leak_report(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn module_restart_cleans_running_jobs() {
    let runtime = Arc::new(TokioRuntime::new());
    let engine = make_engine(runtime);
    engine.start().await.expect("engine start");

    let _source_key = StreamKey::new("app", "src");
    let _target_key = StreamKey::new("app", "out");
    let source = MediaKey::with_default_vhost("app", "src", None).expect("source key");
    let target = MediaKey::with_default_vhost("app", "out", None).expect("target key");

    let clip = encode_h264_clip(160, 120, 300);
    let feed = start_source_publisher(
        &engine,
        _source_key.clone(),
        clip,
        h264_video_track(160, 120),
        Duration::from_millis(20),
    )
    .await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let job = processing
        .create_job(
            &MediaRequestContext::default(),
            transcode_request(source, target, Some(video_h264_target()), None),
        )
        .await
        .expect("create job");

    sleep(Duration::from_millis(200)).await;

    engine
        .module_manager_api()
        .restart_module(&ModuleId::new("media-processing"))
        .await
        .expect("restart media-processing module");

    // After restart the old provider is replaced; the job should no longer be
    // tracked and the engine must not leak resources.
    let processing = engine.media_services().processing();
    if let Some(p) = processing {
        let lookup = p
            .get_job(&MediaRequestContext::default(), &job.job_id)
            .await;
        assert!(
            lookup.is_err(),
            "expected old job to be gone after module restart, got {lookup:?}"
        );
    }

    feed.abort();
    let _ = timeout(Duration::from_secs(2), feed).await;
    engine.stop().await;
    assert_clean_leak_report(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_shutdown_after_job_leaves_no_leaks() {
    let runtime = Arc::new(TokioRuntime::new());
    let engine = make_engine(runtime);
    engine.start().await.expect("engine start");

    let _source_key = StreamKey::new("app", "src");
    let _target_key = StreamKey::new("app", "out");
    let source = MediaKey::with_default_vhost("app", "src", None).expect("source key");
    let target = MediaKey::with_default_vhost("app", "out", None).expect("target key");

    // Keep source stream short so the feed task finishes before shutdown.
    let clip = encode_h264_clip(160, 120, 5);
    let feed = start_source_publisher(
        &engine,
        _source_key.clone(),
        clip,
        h264_video_track(160, 120),
        Duration::from_millis(20),
    )
    .await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let _job = processing
        .create_job(
            &MediaRequestContext::default(),
            transcode_request(source, target, Some(video_h264_target()), None),
        )
        .await
        .expect("create job");

    // Wait for feed to finish before stopping the engine.
    let _ = timeout(Duration::from_secs(5), feed).await;
    engine.stop().await;
    assert_clean_leak_report(&engine).await;
}
