use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, AacAudioSpecificConfig, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind,
    Timebase, TrackId, TrackInfo, TrackReadiness,
};
use cheetah_connector::{ConnectorBuilder, LoopbackLayer, LoopbackOptions};
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
async fn rtmp_to_http_flv_loopback_preserves_frames() -> Result<(), Box<dyn std::error::Error>> {
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
    options.stream_name = "loopback".to_string();
    options.tracks = vec![h264_track(), aac_track()];

    let mut pair = connector.open_in_memory_loopback(options).await?;
    assert_eq!(pair.layer, LoopbackLayer::ProtocolFraming);

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

    let tracks = pair.subscriber.tracks();
    assert!(tracks.iter().any(|t| t.codec == CodecId::H264));
    assert!(tracks.iter().any(|t| t.codec == CodecId::AAC));

    pair.publisher.close()?;
    pair.subscriber.close().await?;
    connector.stop().await;

    Ok(())
}
