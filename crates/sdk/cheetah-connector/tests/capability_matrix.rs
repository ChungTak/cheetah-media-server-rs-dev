use std::sync::Arc;

use cheetah_connector::{supports, ConnectorBuilder, Direction, Protocol, RuntimeConnector};
use cheetah_runtime_tokio::TokioRuntime;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supports_matrix_only_allows_four_pairs() {
    assert!(supports(Protocol::Rtsp, Direction::Pull));
    assert!(supports(Protocol::HttpFlv, Direction::Pull));
    assert!(supports(Protocol::Rtmp, Direction::Push));
    assert!(supports(Protocol::WebRtc, Direction::Push));

    assert!(!supports(Protocol::Rtmp, Direction::Pull));
    assert!(!supports(Protocol::WebRtc, Direction::Pull));
    assert!(!supports(Protocol::Rtsp, Direction::Push));
    assert!(!supports(Protocol::HttpFlv, Direction::Push));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_pull_rtmp_returns_unsupported_protocol() {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()
        .expect("build engine");
    connector.start().await.expect("start engine");

    let err = connector
        .open_pull(
            Protocol::Rtmp,
            "rtmp://127.0.0.1:1935/live/stream",
            Default::default(),
        )
        .await
        .expect_err("rtmp pull must be unsupported");

    assert!(matches!(
        err,
        cheetah_connector::ConnectorError::UnsupportedProtocol {
            protocol: Protocol::Rtmp,
            direction: Direction::Pull,
        }
    ));

    connector.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_push_rtsp_returns_unsupported_protocol() {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()
        .expect("build engine");
    connector.start().await.expect("start engine");

    let err = connector
        .open_push(
            Protocol::Rtsp,
            "rtsp://127.0.0.1:554/live/stream",
            Default::default(),
        )
        .await
        .expect_err("rtsp push must be unsupported");

    assert!(matches!(
        err,
        cheetah_connector::ConnectorError::UnsupportedProtocol {
            protocol: Protocol::Rtsp,
            direction: Direction::Push,
        }
    ));

    connector.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_push_http_flv_returns_unsupported_protocol() {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()
        .expect("build engine");
    connector.start().await.expect("start engine");

    let err = connector
        .open_push(
            Protocol::HttpFlv,
            "http://127.0.0.1:8080/live/stream.flv",
            Default::default(),
        )
        .await
        .expect_err("http-flv push must be unsupported");

    assert!(matches!(
        err,
        cheetah_connector::ConnectorError::UnsupportedProtocol {
            protocol: Protocol::HttpFlv,
            direction: Direction::Push,
        }
    ));

    connector.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn builder_with_default_modules_builds_and_stops() {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let config = Arc::new(cheetah_config::ConfigStore::new());
    config
        .load_yaml_str(
            r#"
modules:
  rtsp:
    enabled: false
  webrtc:
    enabled: false
  rtmp:
    enabled: false
  http_flv:
    enabled: false
"#,
        )
        .expect("load yaml");

    let connector = ConnectorBuilder::new(runtime)
        .with_config_provider(config as Arc<dyn cheetah_sdk::ConfigProvider>)
        .with_default_modules()
        .build()
        .expect("build engine");
    connector.start().await.expect("start engine");
    connector.stop().await;
}
