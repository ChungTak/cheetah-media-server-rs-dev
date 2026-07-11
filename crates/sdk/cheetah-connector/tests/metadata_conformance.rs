use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, AacAudioSpecificConfig, CodecExtradata, CodecId, FrameFlags, FrameFormat, FrameOrigin,
    FrameSideData, MediaKind, Timebase, TrackId, TrackInfo, TrackReadiness,
};
use cheetah_connector::{ConnectorBuilder, LoopbackOptions, WIRE_METADATA_NOT_PRESERVED};
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
    frame.flags = FrameFlags::KEY | FrameFlags::DISCONTINUITY;
    frame.origin = FrameOrigin::Relay;
    frame.set_duration(1).expect("valid duration");
    frame.side_data.push(FrameSideData::SequenceNumber(42));
    frame
}

fn aac_frame() -> AVFrame {
    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Audio,
        CodecId::AAC,
        FrameFormat::AacRaw,
        0,
        0,
        Timebase::new(1, 1_000),
        Bytes::from_static(&[0x12, 0x34, 0x56, 0x78]),
    );
    frame.origin = FrameOrigin::Relay;
    frame.set_duration(1).expect("valid duration");
    frame.side_data.push(FrameSideData::SequenceNumber(99));
    frame
}

#[cfg(feature = "loopback")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtmp_http_flv_loopback_preserves_track_and_frame_metadata(
) -> Result<(), Box<dyn std::error::Error>> {
    assert!(WIRE_METADATA_NOT_PRESERVED.contains(&"duration"));

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
    options.stream_name = "metadata".to_string();
    options.tracks = vec![h264_track(), aac_track()];

    let mut pair = connector.open_in_memory_loopback(options).await?;

    pair.publisher.wait_ready().await?;
    pair.publisher.push_frame(Arc::new(h264_frame()))?;
    pair.publisher.push_frame(Arc::new(aac_frame()))?;

    let video = tokio::time::timeout(Duration::from_secs(5), pair.subscriber.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(video.codec, CodecId::H264);
    assert_eq!(video.media_kind, MediaKind::Video);
    assert_eq!(video.track_id, TrackId(0));
    assert_eq!(video.format, FrameFormat::CanonicalH26x);
    assert_eq!(video.pts, 0);
    assert_eq!(video.dts, 0);
    assert_eq!(video.timebase, Timebase::new(1, 1_000));
    assert!(video.flags.contains(FrameFlags::KEY));
    assert!(video
        .flags
        .contains(FrameFlags::START_OF_AU | FrameFlags::END_OF_AU));
    assert!(!video.flags.contains(FrameFlags::DISCONTINUITY));
    assert_eq!(video.payload, h264_frame().payload);
    assert_eq!(video.duration, 0);
    assert_eq!(video.duration_us, 0);
    assert_eq!(video.origin, FrameOrigin::Ingest);
    assert!(!video.side_data.contains(&FrameSideData::SequenceNumber(42)));

    let audio = tokio::time::timeout(Duration::from_secs(5), pair.subscriber.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(audio.codec, CodecId::AAC);
    assert_eq!(audio.media_kind, MediaKind::Audio);
    assert_eq!(audio.track_id, TrackId(1));
    assert_eq!(audio.format, FrameFormat::AacRaw);
    assert_eq!(audio.pts, 0);
    assert_eq!(audio.dts, 0);
    assert_eq!(audio.payload, aac_frame().payload);
    assert!(audio
        .flags
        .contains(FrameFlags::START_OF_AU | FrameFlags::END_OF_AU));
    assert!(!audio.flags.contains(FrameFlags::DISCONTINUITY));
    assert_eq!(audio.duration, 0);
    assert_eq!(audio.duration_us, 0);
    assert_eq!(audio.origin, FrameOrigin::Ingest);
    assert!(!audio.side_data.contains(&FrameSideData::SequenceNumber(99)));

    let tracks = pair.subscriber.tracks();
    let video_track = tracks
        .iter()
        .find(|t| t.media_kind == MediaKind::Video)
        .expect("video track present");
    assert_eq!(video_track.codec, CodecId::H264);
    assert_eq!(video_track.track_id, TrackId(0));
    assert_eq!(video_track.clock_rate, 90_000);
    assert!(matches!(
        &video_track.extradata,
        CodecExtradata::H264 { sps, pps, avcc: Some(_) }
            if sps.len() == 1 && pps.len() == 1
    ));

    let audio_track = tracks
        .iter()
        .find(|t| t.media_kind == MediaKind::Audio)
        .expect("audio track present");
    assert_eq!(audio_track.codec, CodecId::AAC);
    assert_eq!(audio_track.track_id, TrackId(1));
    assert_eq!(audio_track.clock_rate, 44_100);
    assert_eq!(audio_track.sample_rate, Some(44_100));
    assert_eq!(audio_track.channels, Some(2));
    assert!(matches!(
        &audio_track.extradata,
        CodecExtradata::AAC { asc } if asc.as_ref() == [0x12, 0x10]
    ));

    pair.publisher.close()?;
    pair.subscriber.close().await?;
    connector.stop().await;

    Ok(())
}
