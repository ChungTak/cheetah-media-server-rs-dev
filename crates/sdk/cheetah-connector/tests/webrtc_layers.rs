use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId, TrackInfo,
    TrackReadiness,
};
use cheetah_connector::{
    ConnectorBuilder, Direction, LoopbackLayer, LoopbackOptions, LoopbackTopology, Protocol,
    RuntimeConnector,
};
use cheetah_runtime_tokio::TokioRuntime;

fn h264_track() -> TrackInfo {
    let mut track = TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::H264, 90_000);
    track.readiness = TrackReadiness::Ready;
    track
}

fn h264_frame() -> AVFrame {
    let payload = Bytes::from_static(&[
        0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00, 0x2f, 0xff, 0xff, 0x00, 0x04, 0x00, 0x00,
        0x04, 0x01,
    ]);
    let mut frame = AVFrame::new(
        TrackId(0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        0,
        0,
        Timebase::new(1, 1_000),
        payload,
    );
    frame.flags = FrameFlags::KEY;
    frame
}

#[cfg(all(feature = "webrtc", feature = "loopback"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn webrtc_same_protocol_fixture_loopback_roundtrips_h264(
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()?;
    connector.start().await?;

    let mut options = LoopbackOptions::default();
    options.stream_name = "webrtc_fixture".to_string();
    options.topology = LoopbackTopology::SameProtocol {
        protocol: Protocol::WebRtc,
    };
    options.preferred_layer = LoopbackLayer::WebRtcMediaFixture;
    options.tracks = vec![h264_track()];

    let mut pair = connector.open_in_memory_loopback(options).await?;
    assert_eq!(pair.layer, LoopbackLayer::WebRtcMediaFixture);

    pair.publisher.push_frame(Arc::new(h264_frame()))?;

    let frame = tokio::time::timeout(Duration::from_secs(15), pair.subscriber.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(frame.codec, CodecId::H264);
    assert_eq!(frame.media_kind, MediaKind::Video);
    assert!(!frame.payload.is_empty());
    assert!(frame.flags.contains(FrameFlags::KEY));

    pair.publisher.close()?;
    pair.subscriber.close().await?;
    connector.stop().await;

    Ok(())
}

#[cfg(all(feature = "webrtc", feature = "loopback"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn webrtc_pull_is_unsupported() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()?;
    connector.start().await?;

    let err = connector
        .open_pull(
            Protocol::WebRtc,
            "webrtc://127.0.0.1:8000/live/stream",
            Default::default(),
        )
        .await
        .expect_err("webrtc pull must be unsupported");

    assert!(matches!(
        err,
        cheetah_connector::ConnectorError::UnsupportedProtocol {
            protocol: Protocol::WebRtc,
            direction: Direction::Pull,
        }
    ));

    connector.stop().await;
    Ok(())
}
