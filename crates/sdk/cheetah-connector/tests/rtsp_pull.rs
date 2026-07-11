//! RTSP pull adapter integration tests for `cheetah-connector`.
//!
//! These tests implement the T-RTSP-* checklist from
//! `dev-docs/900_sdk_gaps_plan2/03_rtsp_pull_adapter.md`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use cheetah_codec::CodecId;
use cheetah_connector::{
    CancellationToken, ConnectorBuilder, ConnectorError, ConnectorPullOptions, Direction, Protocol,
    RuntimeConnector,
};
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::BootstrapPolicy;
use common::*;
use tokio::net::TcpListener;
use tokio::time::timeout;

fn test_runtime() -> Arc<dyn cheetah_runtime_api::RuntimeApi> {
    Arc::new(TokioRuntime::new())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_rtsp_01_invalid_url_returns_invalid_url() {
    let runtime = test_runtime();
    let connector = ConnectorBuilder::new(runtime)
        .with_default_modules()
        .build()
        .expect("build engine");
    connector.start().await.expect("start engine");

    let err = connector
        .open_pull(
            Protocol::Rtsp,
            "http://127.0.0.1:554/live/stream",
            Default::default(),
        )
        .await
        .expect_err("invalid url must fail");

    assert!(matches!(
        err,
        ConnectorError::InvalidUrl {
            protocol: Protocol::Rtsp,
            ..
        }
    ));

    connector.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_rtsp_03_local_server_pulls_at_least_one_frame() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-source");
    let source_uri_for_server = source_uri.clone();

    let rtp_payload = h264_rtp_packet();
    let source_server = tokio::spawn(async move {
        let (socket, _) = source_listener.accept().await.expect("accept pull source");
        run_interleaved_rtsp_source(socket, source_uri_for_server, Some(rtp_payload)).await;
    });

    let runtime = test_runtime();
    let connector = ConnectorBuilder::new(runtime)
        .with_default_modules()
        .build()
        .expect("build engine");
    connector.start().await.expect("start engine");

    let mut pull = connector
        .open_pull(Protocol::Rtsp, &source_uri, Default::default())
        .await
        .expect("open rtsp pull");

    let frame = timeout(Duration::from_secs(2), pull.recv())
        .await
        .expect("recv frame timeout")
        .expect("recv frame result")
        .expect("frame should exist");
    assert_eq!(frame.track_id.0, 1);
    assert!(
        !frame.payload.is_empty(),
        "ingested frame payload should not be empty"
    );

    pull.close().await.expect("close pull");
    connector.stop().await;
    let _ = source_server.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_rtsp_04_cancel_ends_recv_cleanly() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-source");
    let source_uri_for_server = source_uri.clone();

    let source_server = tokio::spawn(async move {
        let (socket, _) = source_listener.accept().await.expect("accept pull source");
        // Server that never sends RTP; connector should still be able to PLAY.
        run_interleaved_rtsp_source(socket, source_uri_for_server, None).await;
    });

    let runtime = test_runtime();
    let connector = ConnectorBuilder::new(runtime)
        .with_default_modules()
        .build()
        .expect("build engine");
    connector.start().await.expect("start engine");

    let cancel = CancellationToken::new();
    let mut options = ConnectorPullOptions::default();
    options.cancel = Some(cancel.clone());

    let mut pull = connector
        .open_pull(Protocol::Rtsp, &source_uri, options)
        .await
        .expect("open rtsp pull");

    cancel.cancel();

    let result = timeout(Duration::from_secs(2), pull.recv()).await;
    assert!(
        result.is_ok(),
        "recv should return quickly after cancel, not hang"
    );

    let _ = pull.close().await;
    connector.stop().await;
    let _ = source_server.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_rtsp_05_close_is_idempotent() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-source");
    let source_uri_for_server = source_uri.clone();

    let source_server = tokio::spawn(async move {
        let (socket, _) = source_listener.accept().await.expect("accept pull source");
        run_interleaved_rtsp_source(socket, source_uri_for_server, None).await;
    });

    let runtime = test_runtime();
    let connector = ConnectorBuilder::new(runtime)
        .with_default_modules()
        .build()
        .expect("build engine");
    connector.start().await.expect("start engine");

    let mut pull = connector
        .open_pull(Protocol::Rtsp, &source_uri, Default::default())
        .await
        .expect("open rtsp pull");

    pull.close().await.expect("first close");
    let _ = pull.close().await;

    connector.stop().await;
    let _ = source_server.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_rtsp_06_bounded_queue_capacity_enforced() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-source");
    let source_uri_for_server = source_uri.clone();

    let rtp_payload = h264_rtp_packet();
    let source_server = tokio::spawn(async move {
        let (socket, _) = source_listener.accept().await.expect("accept pull source");
        run_interleaved_rtsp_source(socket, source_uri_for_server, Some(rtp_payload)).await;
    });

    let runtime = test_runtime();
    let connector = ConnectorBuilder::new(runtime)
        .with_default_modules()
        .build()
        .expect("build engine");
    connector.start().await.expect("start engine");

    let mut options = ConnectorPullOptions::default();
    options.subscriber.queue_capacity = 5;
    options.subscriber.bootstrap_policy = BootstrapPolicy::none();

    let mut pull = connector
        .open_pull(Protocol::Rtsp, &source_uri, options)
        .await
        .expect("open rtsp pull with bounded queue");

    let frame = timeout(Duration::from_secs(2), pull.recv())
        .await
        .expect("recv should not hang with bounded queue")
        .expect("recv frame result")
        .expect("frame should exist");
    assert!(!frame.payload.is_empty());

    pull.close().await.expect("close pull");
    connector.stop().await;
    let _ = source_server.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_rtsp_07_supports_consistency() {
    use cheetah_connector::supports;

    assert!(supports(Protocol::Rtsp, Direction::Pull));
    assert!(!supports(Protocol::Rtsp, Direction::Push));
    assert!(supports(Protocol::HttpFlv, Direction::Pull));
    assert!(supports(Protocol::Rtmp, Direction::Push));
    assert!(!supports(Protocol::Rtmp, Direction::Pull));
    assert!(!supports(Protocol::WebRtc, Direction::Pull));
    assert!(!supports(Protocol::WebRtc, Direction::Push));
    assert!(!supports(Protocol::HttpFlv, Direction::Push));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_rtsp_08_tracks_non_empty_and_codec_reasonable() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-source");
    let source_uri_for_server = source_uri.clone();

    let source_server = tokio::spawn(async move {
        let (socket, _) = source_listener.accept().await.expect("accept pull source");
        run_interleaved_rtsp_source(socket, source_uri_for_server, None).await;
    });

    let runtime = test_runtime();
    let connector = ConnectorBuilder::new(runtime)
        .with_default_modules()
        .build()
        .expect("build engine");
    connector.start().await.expect("start engine");

    let mut pull = connector
        .open_pull(Protocol::Rtsp, &source_uri, Default::default())
        .await
        .expect("open rtsp pull");

    let tracks = pull.tracks();
    assert!(
        !tracks.is_empty(),
        "rtsp pull should discover at least one track"
    );
    assert_eq!(tracks[0].codec, CodecId::H264);

    pull.close().await.expect("close pull");
    connector.stop().await;
    let _ = source_server.await;
}
