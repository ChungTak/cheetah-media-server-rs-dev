#![allow(unused_imports)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::{
    frame::{FrameFlags, FrameFormat, FrameOrigin},
    track::{MediaKind, TrackId, TrackInfo, TrackReadiness},
    AVFrame, CodecId, Rational32, Timebase,
};
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_api::ids::MediaKey;
use cheetah_media_api::port::MediaRequestContext;
use cheetah_media_api::processing::{
    CreateProcessingJob, MosaicCell, MosaicFit, MosaicLayout, ProcessingJobSpec,
    ProcessingJobState, VideoCodec, VideoMosaicInput,
};
use cheetah_media_processing_module::config::MediaProcessingModuleConfig;
use cheetah_media_processing_module::MediaProcessingModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{
    BackpressurePolicy, BootstrapPolicy, MediaFilter, ModuleId, PublisherOptions, StreamKey,
    SubscriberOptions,
};

#[cfg(feature = "media-processing-cpu")]
use avcodec::{
    core::{
        BitstreamFormat, CodecId as AvCodecId, Image, ImageInfo, Packet, PacketFlags, Poll,
        TimeBase,
    },
    VideoDecoderRequest, VideoEncoderRequest, VideoProfile, VideoSdk,
};

#[cfg(feature = "media-processing-cpu")]
fn encode_source_clip(width: u32, height: u32, fps: u32, count: usize) -> Vec<AVFrame> {
    let sdk = VideoSdk::new().expect("video sdk");
    let mut encoder = sdk
        .create_encoder(
            VideoProfile::NativeFree,
            VideoEncoderRequest {
                codec: AvCodecId::H264,
                width,
                height,
                format: ImageInfo::Yuv420p,
                time_base: TimeBase::new(1, fps),
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

    let mut out = Vec::new();
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
                Poll::Ready(packet) => out.push(packet),
                Poll::Pending => break,
                Poll::EndOfStream => break,
            }
        }
    }

    encoder.flush().expect("flush encoder");
    loop {
        match encoder.poll_packet().expect("poll packet after flush") {
            Poll::Ready(packet) => out.push(packet),
            Poll::Pending => break,
            Poll::EndOfStream => break,
        }
    }

    let tb = Timebase::new(1, fps);
    out.into_iter()
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

#[cfg(feature = "media-processing-cpu")]
async fn start_source_publisher(
    engine: &cheetah_engine::Engine,
    key: StreamKey,
    clip: Vec<AVFrame>,
) {
    let publisher_api = engine.publisher_api();
    let (lease, sink) = publisher_api
        .acquire_publisher(key, PublisherOptions::default())
        .await
        .expect("acquire source publisher");

    let mut track = TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::H264, 30);
    track.readiness = TrackReadiness::Ready;
    track.width = Some(160);
    track.height = Some(120);
    track.fps = Some(Rational32::new(30, 1));
    sink.update_tracks(vec![track])
        .expect("update source track");

    let publisher_api = Arc::clone(&publisher_api);
    tokio::spawn(async move {
        for frame in clip.iter().cycle().take(300) {
            let _ = sink.push_frame(Arc::new(frame.clone()));
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let _ = publisher_api.release_publisher(&lease).await;
    });
}

#[cfg(feature = "media-processing-cpu")]
async fn wait_for_stream(engine: &cheetah_engine::Engine, key: StreamKey) {
    let sm = engine.stream_manager_api();
    for _ in 0..40 {
        if let Ok(Some(snapshot)) = sm.get_stream(&key).await {
            if snapshot.publisher_active && !snapshot.tracks.is_empty() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("stream {key} did not become active in time");
}

#[cfg(feature = "media-processing-cpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn video_mosaic_job_publishes_decodable_output_and_releases_lease_on_stop() {
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

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .register_module_factory(Arc::new(MediaProcessingModuleFactory))
        .build()
        .expect("engine build");

    engine.start().await.expect("engine start");

    let clip = encode_source_clip(160, 120, 30, 10);
    let source1_key = StreamKey::new("app", "src1");
    let source2_key = StreamKey::new("app", "src2");
    let output_media_key =
        MediaKey::with_default_vhost("app", "mosaic", None).expect("output media key");
    let output_stream_key = StreamKey::new("app", "mosaic");

    start_source_publisher(&engine, source1_key.clone(), clip.clone()).await;
    start_source_publisher(&engine, source2_key.clone(), clip).await;

    wait_for_stream(&engine, source1_key).await;
    wait_for_stream(&engine, source2_key).await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let request = CreateProcessingJob {
        idempotency_key: None,
        deadline_ms: None,
        spec: ProcessingJobSpec::VideoMosaic {
            inputs: vec![
                VideoMosaicInput {
                    source: MediaKey::with_default_vhost("app", "src1", None).expect("source1 key"),
                    cell: MosaicCell {
                        column: 0,
                        row: 0,
                        z_order: 0,
                    },
                    audio_gain_db: None,
                    fit: Some(MosaicFit::Cover),
                    label: None,
                },
                VideoMosaicInput {
                    source: MediaKey::with_default_vhost("app", "src2", None).expect("source2 key"),
                    cell: MosaicCell {
                        column: 0,
                        row: 1,
                        z_order: 0,
                    },
                    audio_gain_db: None,
                    fit: Some(MosaicFit::Cover),
                    label: None,
                },
            ],
            target: output_media_key,
            layout: MosaicLayout {
                columns: 1,
                rows: 2,
                cell_width: 160,
                cell_height: 120,
                background: None,
                frame_rate_num: Some(30),
                frame_rate_den: Some(1),
                bit_rate: None,
                gop_size: None,
                video_codec: Some(VideoCodec::H264),
                fit: None,
            },
            audio_mix: None,
            overlays: vec![],
        },
    };

    let job = processing
        .create_job(&MediaRequestContext::default(), request)
        .await
        .expect("create mosaic job");
    assert_eq!(job.state, ProcessingJobState::Running);

    wait_for_stream(&engine, output_stream_key.clone()).await;

    let mut subscriber = engine
        .subscriber_api()
        .subscribe(
            output_stream_key.clone(),
            SubscriberOptions {
                queue_capacity: 8,
                backpressure: BackpressurePolicy::DropDroppableFirst,
                bootstrap_policy: BootstrapPolicy {
                    mode: cheetah_sdk::BootstrapMode::LiveTail,
                    max_bootstrap_age_ms: None,
                    max_bootstrap_frames: 8,
                    wait_for_next_random_access_point: true,
                },
                media_filter: MediaFilter {
                    enable_video: true,
                    enable_audio: false,
                },
            },
        )
        .await
        .expect("subscribe to output");

    let frame = tokio::time::timeout(Duration::from_secs(5), subscriber.recv())
        .await
        .expect("output frame arrived before timeout")
        .expect("subscriber recv")
        .expect("output frame");

    // Decode the produced mosaic output to verify it is a valid H.264 bitstream.
    let sdk = VideoSdk::new().expect("video sdk");
    let mut decoder = sdk
        .create_decoder(
            VideoProfile::NativeFree,
            VideoDecoderRequest::new(AvCodecId::H264, TimeBase::new(1, 30)).unwrap(),
        )
        .expect("create h264 decoder")
        .into_session();

    let mut packet = Packet::from_host_bytes(
        avcodec::core::utils::next_buffer_id(),
        AvCodecId::H264,
        BitstreamFormat::H264AnnexB,
        frame.payload.to_vec(),
    );
    packet.pts = Some(frame.pts);
    packet.dts = Some(frame.dts);
    packet.time_base = Some(TimeBase::new(frame.timebase.num, frame.timebase.den));
    decoder.submit_packet(packet).expect("submit output packet");

    let mut decoded = None;
    for _ in 0..20 {
        match decoder.poll_image().expect("poll decoded image") {
            Poll::Ready(img) => {
                decoded = Some(img);
                break;
            }
            Poll::Pending => {}
            Poll::EndOfStream => break,
        }
    }
    if decoded.is_none() {
        decoder.flush().expect("flush decoder");
        for _ in 0..20 {
            match decoder.poll_image().expect("poll flushed image") {
                Poll::Ready(img) => {
                    decoded = Some(img);
                    break;
                }
                Poll::Pending => {}
                Poll::EndOfStream => break,
            }
        }
    }
    let img = decoded.expect("decoded mosaic output frame");
    assert_eq!(img.visible.width, 160);
    assert_eq!(img.visible.height, 240);

    let stopped = processing
        .stop_job(&MediaRequestContext::default(), &job.job_id)
        .await
        .expect("stop job");
    assert_eq!(stopped.state, ProcessingJobState::Stopped);

    tokio::time::sleep(Duration::from_millis(200)).await;
    let snapshot = engine
        .stream_manager_api()
        .get_stream(&output_stream_key)
        .await
        .expect("get stream");
    assert!(
        snapshot.is_none() || !snapshot.unwrap().publisher_active,
        "output publisher lease should be released after stop"
    );

    let final_job = processing
        .get_job(&MediaRequestContext::default(), &job.job_id)
        .await
        .expect("get job");
    assert!(
        final_job.frames_out > 0,
        "mosaic should have produced frames"
    );
    assert!(
        final_job.frames_in > 0,
        "mosaic should have ingested frames"
    );
}
