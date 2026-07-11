use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, AacAudioSpecificConfig, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind,
    Timebase, TrackId, TrackInfo, TrackReadiness,
};
use cheetah_connector::{
    ConnectorBuilder, Direction, LoopbackLayer, LoopbackOptions, LoopbackTopology, Protocol,
};
use cheetah_runtime_tokio::TokioRuntime;

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

fn aac_track() -> TrackInfo {
    let asc = AacAudioSpecificConfig {
        audio_object_type: 2,
        sampling_frequency_index: 4,
        channel_configuration: 2,
    };
    let mut track = TrackInfo::new(TrackId(1), MediaKind::Audio, CodecId::AAC, 44_100);
    track.sample_rate = Some(44_100);
    track.channels = Some(2);
    track.extradata = CodecExtradata::AAC {
        asc: Bytes::copy_from_slice(&asc.to_bytes()),
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

fn aac_frame() -> AVFrame {
    AVFrame::new(
        TrackId(1),
        MediaKind::Audio,
        CodecId::AAC,
        FrameFormat::AacRaw,
        0,
        0,
        Timebase::new(1, 1_000),
        Bytes::from_static(&[0x12, 0x34, 0x56, 0x78]),
    )
}

#[cfg(feature = "loopback")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_only_bypass_wire_roundtrips_frames() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()?;
    connector.start().await?;

    let mut options = LoopbackOptions::default();
    options.stream_name = "engine_layer".to_string();
    options.topology = LoopbackTopology::SameProtocol {
        protocol: Protocol::Rtmp,
    };
    options.preferred_layer = LoopbackLayer::EngineOnlyBypassWire;
    options.tracks = vec![h264_track(), aac_track()];

    let mut pair = connector.open_in_memory_loopback(options).await?;
    assert_eq!(pair.layer, LoopbackLayer::EngineOnlyBypassWire);
    assert_eq!(pair.publisher.protocol(), Protocol::Rtmp);
    assert_eq!(pair.subscriber.protocol(), Protocol::Rtmp);

    pair.publisher.wait_ready().await?;
    pair.publisher
        .push_frame(std::sync::Arc::new(h264_frame()))?;
    pair.publisher
        .push_frame(std::sync::Arc::new(aac_frame()))?;

    let video = tokio::time::timeout(Duration::from_secs(5), pair.subscriber.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(video.codec, CodecId::H264);
    assert_eq!(video.media_kind, MediaKind::Video);
    assert!(!video.payload.is_empty());

    let audio = tokio::time::timeout(Duration::from_secs(5), pair.subscriber.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(audio.codec, CodecId::AAC);
    assert_eq!(audio.media_kind, MediaKind::Audio);
    assert!(!audio.payload.is_empty());

    pair.publisher.close()?;
    pair.subscriber.close().await?;
    connector.stop().await;

    Ok(())
}

#[cfg(feature = "loopback")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protocol_framing_loopback_uses_protocol_framing_layer(
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let config = Arc::new(cheetah_config::ConfigStore::new());
    config.load_yaml_str(
        r#"
modules:
  rtsp:
    enabled: false
  webrtc:
    enabled: false
  rtmp:
    enabled: true
    listen: "127.0.0.1:0"
  http_flv:
    enabled: true
    listen: "127.0.0.1:0"
"#,
    )?;

    let connector = ConnectorBuilder::new(runtime)
        .with_config_provider(config.clone() as Arc<dyn cheetah_sdk::ConfigProvider>)
        .with_config_apply(config.clone() as Arc<dyn cheetah_sdk::ConfigApplyApi>)
        .build()?;
    connector.start().await?;

    let mut options = LoopbackOptions::default();
    options.stream_name = "framing_layer".to_string();
    options.preferred_layer = LoopbackLayer::ProtocolFraming;
    options.tracks = vec![h264_track(), aac_track()];

    let mut pair = connector.open_in_memory_loopback(options).await?;
    assert_eq!(pair.layer, LoopbackLayer::ProtocolFraming);
    assert_eq!(pair.publisher.protocol(), Protocol::Rtmp);
    assert_eq!(pair.subscriber.protocol(), Protocol::HttpFlv);

    pair.publisher.wait_ready().await?;
    pair.publisher
        .push_frame(std::sync::Arc::new(h264_frame()))?;
    pair.publisher
        .push_frame(std::sync::Arc::new(aac_frame()))?;

    let video = tokio::time::timeout(Duration::from_secs(5), pair.subscriber.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(video.codec, CodecId::H264);

    pair.publisher.close()?;
    pair.subscriber.close().await?;
    connector.stop().await;

    Ok(())
}

#[cfg(feature = "loopback")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_protocol_framing_for_webrtc_same_protocol(
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()?;
    connector.start().await?;

    let mut options = LoopbackOptions::default();
    options.stream_name = "webrtc_default".to_string();
    options.topology = LoopbackTopology::SameProtocol {
        protocol: Protocol::WebRtc,
    };
    // default preferred_layer is ProtocolFraming, which is not supported for WebRTC

    let err = connector
        .open_in_memory_loopback(options)
        .await
        .expect_err("webrtc protocol framing must be unsupported");

    assert!(matches!(
        err,
        cheetah_connector::ConnectorError::UnsupportedProtocol {
            protocol: Protocol::WebRtc,
            direction: Direction::Push,
        }
    ));

    connector.stop().await;
    Ok(())
}
