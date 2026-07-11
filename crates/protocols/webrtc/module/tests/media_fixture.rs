//! WebRTC media fixture test that bypasses ICE/DTLS/SRTP.
//!
//! This test drives `MediaLoopbackHarness` directly and confirms that a single
//! H.264 access unit survives the `packetize -> depacketize -> AVFrame` path.
//!
//! 本测试绕过 ICE/DTLS/SRTP，直接驱动 `MediaLoopbackHarness`，确认单个 H.264
//! 访问单元在 `packetize -> depacketize -> AVFrame` 路径后语义保持一致。

use std::sync::Arc;

use bytes::Bytes;
use cheetah_codec::{AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId};
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::StreamKey;
use cheetah_webrtc_media_loopback::MediaLoopbackHarness;

fn h264_frame() -> AVFrame {
    // Annex-B access unit: start-code + IDR NAL.
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
async fn media_fixture_bypass_packetize_to_depacketize_preserves_h264_frame(
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>;
    let config = Arc::new(ConfigStore::new());

    let engine = EngineBuilder::new(config.clone(), config, runtime.clone()).build()?;
    engine.start().await?;

    let stream_key = StreamKey::new("live", "media_fixture");
    let mut harness = MediaLoopbackHarness::new(
        runtime,
        engine.stream_manager_api(),
        stream_key,
        CancellationToken::new(),
    )
    .await?;

    harness.push_frame(Arc::new(h264_frame()))?;

    let frame = harness
        .recv()
        .await?
        .expect("h264 frame should be received");

    assert_eq!(frame.track_id, TrackId(1));
    assert_eq!(frame.media_kind, MediaKind::Video);
    assert_eq!(frame.codec, CodecId::H264);
    assert_eq!(frame.format, FrameFormat::CanonicalH26x);
    assert_eq!(frame.pts, 0);
    assert_eq!(frame.dts, 0);
    assert_eq!(frame.timebase, Timebase::new(1, 90_000));
    assert!(frame.flags.contains(FrameFlags::KEY));
    assert!(!frame.payload.is_empty());
    assert!(frame.payload[4] == 0x65);

    // The depacketized payload is the NAL with the Annex-B start code retained.
    let expected = h264_frame().payload.slice(4..);
    assert!(frame
        .payload
        .as_ref()
        .windows(expected.len())
        .any(|w| w == expected.as_ref()));

    let _ = harness.close().await;
    engine.stop().await;

    Ok(())
}
