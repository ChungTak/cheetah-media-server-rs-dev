use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId, TrackInfo,
    TrackReadiness,
};
use cheetah_connector::{
    ConnectorBuilder, ConnectorPullOptions, ConnectorPushOptions, LoopbackLayer, LoopbackOptions,
    Protocol, ProtocolPullExtras, ProtocolPushExtras, RuntimeConnector,
};
use cheetah_runtime_api::CancellationToken;
use cheetah_runtime_tokio::TokioRuntime;

#[cfg(feature = "http-flv")]
use cheetah_http_flv_module::pull::PullReadLimits;

#[cfg(feature = "rtmp")]
use cheetah_connector::RtmpPushExtras;

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

#[cfg(feature = "http-flv")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connector_pull_options_queue_capacity_zero_returns_invalid_argument(
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()?;
    connector.start().await?;

    let mut subscriber = cheetah_sdk::SubscriberOptions::default();
    subscriber.queue_capacity = 0;

    let err = connector
        .open_pull(
            Protocol::HttpFlv,
            "http://127.0.0.1:1/live/test.flv",
            ConnectorPullOptions {
                subscriber,
                cancel: None,
                protocol: ProtocolPullExtras::HttpFlv {
                    reconnect: None,
                    read_limits: None,
                    buffer_size: None,
                },
            },
        )
        .await
        .expect_err("queue_capacity zero must fail");

    assert!(matches!(
        err,
        cheetah_connector::ConnectorError::InvalidArgument(_)
    ));
    assert!(err.to_string().contains("queue_capacity"));

    connector.stop().await;
    Ok(())
}

#[cfg(feature = "http-flv")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connector_pull_options_http_flv_extras_reach_adapter(
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()?;
    connector.start().await?;

    let cancel = CancellationToken::new();
    let mut subscriber = cheetah_sdk::SubscriberOptions::default();
    subscriber.queue_capacity = 42;

    let mut subscriber = connector
        .open_pull(
            Protocol::HttpFlv,
            "http://127.0.0.1:1/live/test.flv",
            ConnectorPullOptions {
                subscriber,
                cancel: Some(cancel.clone()),
                protocol: ProtocolPullExtras::HttpFlv {
                    reconnect: None,
                    read_limits: Some(PullReadLimits {
                        max_response_header_bytes: 100,
                        read_buffer_size: 200,
                        max_demux_buffer_bytes: 300,
                        max_websocket_message_bytes: 400,
                    }),
                    buffer_size: Some(17),
                },
            },
        )
        .await?;

    assert!(subscriber.id().0 > 0);
    cancel.cancel();
    subscriber.close().await?;
    connector.stop().await;

    Ok(())
}

#[cfg(feature = "rtmp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connector_push_options_rtmp_extras_reach_adapter() -> Result<(), Box<dyn std::error::Error>>
{
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()?;
    connector.start().await?;

    let mut options = ConnectorPushOptions::default();
    options.protocol = ProtocolPushExtras::Rtmp(RtmpPushExtras {
        command_queue_capacity: Some(64),
        write_queue_capacity: Some(64),
        read_buffer_size: Some(1024),
        chunk_size: Some(128),
        ack_window_size: Some(1000),
    });

    let err = connector
        .open_push(Protocol::Rtmp, "rtmp://127.0.0.1:1/live/test", options)
        .await
        .expect_err("connect to port 1 must fail");

    assert!(matches!(
        err,
        cheetah_connector::ConnectorError::Connect {
            protocol: Protocol::Rtmp,
            ..
        }
    ));
    assert!(err.retryable());
    assert_eq!(err.protocol(), Some(Protocol::Rtmp));

    connector.stop().await;
    Ok(())
}

#[cfg(feature = "loopback")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_options_engine_only_respects_queue_capacity(
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()?;
    connector.start().await?;

    let mut zero_options = LoopbackOptions::default();
    zero_options.stream_name = "engine_zero".to_string();
    zero_options.preferred_layer = LoopbackLayer::EngineOnlyBypassWire;
    zero_options.queue_capacity = 0;

    let err = connector
        .open_in_memory_loopback(zero_options)
        .await
        .expect_err("queue_capacity zero must fail");
    assert!(matches!(
        err,
        cheetah_connector::ConnectorError::InvalidArgument(_)
    ));
    assert!(err.to_string().contains("queue_capacity"));

    let mut options = LoopbackOptions::default();
    options.stream_name = "engine".to_string();
    options.preferred_layer = LoopbackLayer::EngineOnlyBypassWire;
    options.tracks = vec![h264_track()];

    let mut pair = connector.open_in_memory_loopback(options).await?;
    assert_eq!(
        pair.layer,
        cheetah_connector::LoopbackLayer::EngineOnlyBypassWire
    );

    pair.publisher.wait_ready().await?;
    pair.publisher.push_frame(Arc::new(h264_frame()))?;

    let frame = tokio::time::timeout(Duration::from_secs(5), pair.subscriber.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(frame.codec, CodecId::H264);
    assert!(!frame.payload.is_empty());

    pair.publisher.close()?;
    pair.subscriber.close().await?;
    connector.stop().await;

    Ok(())
}
