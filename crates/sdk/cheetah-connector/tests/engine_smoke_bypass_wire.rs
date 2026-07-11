//! This test BYPASSES protocol wire behavior.
//! It must NOT be counted as protocol loopback acceptance.
//!
//! 本测试绕过协议 wire 行为，不得被计为协议 loopback 验收。

use std::sync::Arc;

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId,
    TrackInfo, TrackReadiness,
};
use cheetah_connector::ConnectorBuilder;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{PublisherOptions, StreamKey, SubscriberOptions};

fn h264_track() -> TrackInfo {
    let mut track = TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::H264, 90_000);
    track.extradata = CodecExtradata::H264 {
        sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1f])],
        pps: vec![Bytes::from_static(&[0x68, 0xce, 0x3c, 0x80])],
        avcc: None,
    };
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_publish_subscribe_preserves_frame_fields() -> Result<(), Box<dyn std::error::Error>>
{
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()?;
    connector.start().await?;

    let stream_manager = connector.engine().stream_manager_api();
    let key = StreamKey::new("live", "stream");

    let publisher = stream_manager
        .open_publisher(key.clone(), PublisherOptions::default())
        .await?;
    let mut subscriber = stream_manager
        .open_subscriber(key.clone(), SubscriberOptions::default())
        .await?;

    publisher.update_tracks(vec![h264_track()])?;
    publisher.push_frame(Arc::new(h264_frame()))?;

    let frame = subscriber
        .recv()
        .await?
        .expect("engine subscriber should receive the frame");

    assert_eq!(frame.track_id, TrackId(0));
    assert_eq!(frame.media_kind, MediaKind::Video);
    assert_eq!(frame.codec, CodecId::H264);
    assert_eq!(frame.format, FrameFormat::CanonicalH26x);
    assert_eq!(frame.pts, 0);
    assert_eq!(frame.dts, 0);
    assert_eq!(frame.timebase, Timebase::new(1, 1_000));
    assert!(frame.flags.contains(FrameFlags::KEY));
    assert_eq!(frame.payload, h264_frame().payload);

    let snapshot = stream_manager
        .get_stream(&key)
        .await?
        .expect("stream should exist");
    assert!(snapshot.publisher_active);
    assert_eq!(snapshot.tracks.len(), 1);
    assert_eq!(snapshot.tracks[0].codec, CodecId::H264);
    assert_eq!(snapshot.tracks[0].track_id, TrackId(0));

    let _ = publisher.close();
    let _ = subscriber.close().await;
    connector.stop().await;

    Ok(())
}
