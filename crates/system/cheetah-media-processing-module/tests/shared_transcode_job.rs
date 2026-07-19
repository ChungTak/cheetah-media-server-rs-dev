//! Shared fingerprint attach / ref_count lifecycle for Transcode jobs.
#![cfg(feature = "media-processing-cpu")]

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
    CreateProcessingJob, ProcessingJobSpec, ProcessingJobState, TrackSelection, VideoCodec,
    VideoTarget,
};
use cheetah_media_processing_module::config::MediaProcessingModuleConfig;
use cheetah_media_processing_module::MediaProcessingModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{ModuleId, PublisherOptions, StreamKey};
use tokio::time::sleep;

#[cfg(feature = "media-processing-cpu")]
use avcodec::{
    core::{CodecId as AvCodecId, Image, ImageInfo, PacketFlags, Poll, TimeBase},
    VideoEncoderRequest, VideoProfile, VideoSdk,
};

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

async fn build_engine() -> cheetah_engine::Engine {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(serde_json::json!({}));
    config.register_module_default(
        ModuleId::new("media-processing"),
        MediaProcessingModuleConfig::default_json(),
    );
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .register_module_factory(Arc::new(MediaProcessingModuleFactory))
        .build()
        .expect("engine build");
    engine.start().await.expect("engine start");
    engine
}

fn video_target() -> VideoTarget {
    VideoTarget {
        codec: VideoCodec::H264,
        width: Some(160),
        height: Some(120),
        frame_rate_num: Some(30),
        frame_rate_den: Some(1),
        bit_rate: Some(200_000),
        gop_size: None,
        profile: None,
    }
}

fn transcode_req(source: &str, target: &str) -> CreateProcessingJob {
    CreateProcessingJob {
        idempotency_key: None,
        deadline_ms: None,
        spec: ProcessingJobSpec::Transcode {
            source: MediaKey::with_default_vhost("app", source, None).unwrap(),
            target: MediaKey::with_default_vhost("app", target, None).unwrap(),
            track_selection: TrackSelection::VideoOnly,
            video: Some(video_target()),
            audio: None,
            overlays: Vec::new(),
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_transcode_job_attaches_and_refcount_stop() {
    let engine = build_engine().await;
    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider");

    let source_key = StreamKey::new("app", "src_shared");
    let clip = encode_h264_clip(160, 120, 8);
    let mut track = TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::H264, 90_000);
    track.width = Some(160);
    track.height = Some(120);
    track.fps = Some(Rational32::new(30, 1));
    track.readiness = TrackReadiness::Ready;

    let publisher_api = engine.publisher_api();
    let (lease, sink) = publisher_api
        .acquire_publisher(source_key.clone(), PublisherOptions::default())
        .await
        .expect("acquire source");
    sink.update_tracks(vec![track]).expect("tracks");
    let pub_api = Arc::clone(&publisher_api);
    let feeder = tokio::spawn(async move {
        for frame in clip {
            let _ = sink.push_frame(Arc::new(frame));
            sleep(Duration::from_millis(20)).await;
        }
        let _ = pub_api.release_publisher(&lease).await;
    });

    // Wait briefly for source announce.
    for _ in 0..40 {
        if let Ok(Some(s)) = engine.stream_manager_api().get_stream(&source_key).await {
            if s.publisher_active {
                break;
            }
        }
        sleep(Duration::from_millis(25)).await;
    }

    let ctx = MediaRequestContext::default();
    let job_a = processing
        .create_job(&ctx, transcode_req("src_shared", "derived_a"))
        .await
        .expect("create first job");
    assert_eq!(job_a.ref_count, 1);
    assert_eq!(job_a.state, ProcessingJobState::Running);

    // Different target name, same conversion fingerprint → attach.
    let job_b = processing
        .create_job(&ctx, transcode_req("src_shared", "derived_b"))
        .await
        .expect("attach second consumer");
    assert_eq!(job_b.job_id, job_a.job_id);
    assert_eq!(job_b.ref_count, 2);
    assert_eq!(job_b.output_keys, job_a.output_keys);

    // First stop only releases a reference.
    let after_stop = processing
        .stop_job(&ctx, &job_a.job_id)
        .await
        .expect("stop one consumer");
    assert_eq!(after_stop.ref_count, 1);
    assert_eq!(after_stop.state, ProcessingJobState::Running);

    // Second stop tears the job down.
    let final_stop = processing
        .stop_job(&ctx, &job_a.job_id)
        .await
        .expect("stop last consumer");
    assert_eq!(final_stop.ref_count, 0);
    assert_eq!(final_stop.state, ProcessingJobState::Stopped);

    let _ = feeder.await;
    engine.stop().await;
}
