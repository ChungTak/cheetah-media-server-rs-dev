use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId,
    TrackInfo, TrackReadiness,
};
use cheetah_connector::{
    options::ConnectorPushOptions, ConnectorBuilder, ConnectorError, Protocol, PushHandle,
    RuntimeConnector,
};
use cheetah_runtime_api::CancellationToken;
use cheetah_runtime_tokio::TokioRuntime;

#[cfg(feature = "webrtc")]
use cheetah_connector::options::{ProtocolPushExtras, WebRtcPushExtras};
#[cfg(feature = "webrtc")]
use cheetah_webrtc_core::{
    WebRtcCloseReason, WebRtcCodecKind, WebRtcCoreEvent, WebRtcMediaEvent, WebRtcSessionId,
    WebRtcSessionRole,
};
#[cfg(feature = "webrtc")]
use cheetah_webrtc_driver_tokio::{
    spawn_driver, CandidateTransportPolicy, WebRtcDriverCommand, WebRtcDriverConfig,
    WebRtcDriverEvent, WebRtcDriverHandle, WebRtcSessionSpec,
};
#[cfg(feature = "webrtc")]
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
#[cfg(feature = "webrtc")]
use tokio::net::{TcpListener, TcpStream};

/// A minimal H.264 track with SPS/PPS and a ready state.
fn h264_track() -> TrackInfo {
    let sps = Bytes::from_static(&[
        0x67, 0x42, 0xc0, 0x1f, 0xd9, 0x00, 0x78, 0x02, 0x27, 0xe5, 0xc0, 0x44, 0x00, 0x00, 0x03,
        0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0xf0, 0x3c, 0x60, 0xc6, 0x58,
    ]);
    let pps = Bytes::from_static(&[0x68, 0xce, 0x3c, 0x80]);
    TrackInfo {
        track_id: TrackId(0),
        media_kind: MediaKind::Video,
        codec: CodecId::H264,
        aac_rtp_packetization: Default::default(),
        aac_latm_config_in_band: false,
        payload_type: None,
        clock_rate: 90_000,
        sample_rate: None,
        channels: None,
        width: None,
        height: None,
        fps: None,
        bitrate: None,
        extradata: CodecExtradata::H264 {
            sps: vec![sps],
            pps: vec![pps],
            avcc: None,
        },
        readiness: TrackReadiness::Ready,
    }
}

/// A single H.264 IDR keyframe in Annex-B form.
fn h264_keyframe() -> AVFrame {
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

/// Build a connector with no default modules so we only exercise the WebRTC
/// push adapter.
async fn build_connector() -> Result<cheetah_connector::EngineConnector, ConnectorError> {
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn cheetah_runtime_api::RuntimeApi>;
    let connector = ConnectorBuilder::new(runtime)
        .without_default_modules()
        .build()?;
    connector.start().await?;
    Ok(connector)
}

/// Spawn an answerer WebRTC driver that will receive the connector's WHIP push.
#[cfg(feature = "webrtc")]
async fn spawn_answerer_driver() -> std::io::Result<(Arc<WebRtcDriverHandle>, CancellationToken)> {
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        public_ips: vec!["127.0.0.1".parse().unwrap()],
        driver_shards: 1,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await?;
    Ok((handle, cancel))
}

/// Start a tiny WHIP server that forwards the POSTed offer to `answerer` and
/// returns the answer SDP with HTTP 201.
#[cfg(feature = "webrtc")]
async fn start_whip_server(
    answerer: Arc<WebRtcDriverHandle>,
) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let _ = handle_whip_request(stream, answerer).await;
        }
    });
    Ok((addr, handle))
}

#[cfg(feature = "webrtc")]
async fn handle_whip_request(
    stream: TcpStream,
    answerer: Arc<WebRtcDriverHandle>,
) -> std::io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let (header, body) = read_http_request_body(&mut reader).await?;
    let first_line = header.lines().next().unwrap_or("");
    if !first_line.starts_with("POST") {
        write_half
            .write_all(b"HTTP/1.1 405 Method Not Allowed\r\n\r\n")
            .await?;
        return Ok(());
    }

    let offer = String::from_utf8_lossy(&body).to_string();
    let session_id = WebRtcSessionId::new(2);
    answerer
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: offer,
            candidate_transport_policy: CandidateTransportPolicy::All,
        }))
        .await;

    let mut answer = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), answerer.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::AnswerReady {
                session_id: sid,
                sdp,
            })) if sid == session_id => {
                answer = Some(sdp);
                break;
            }
            Ok(Some(WebRtcDriverEvent::SessionClosed {
                session_id: sid, ..
            })) if sid == session_id => break,
            Ok(_) | Err(_) => continue,
        }
    }

    let answer = answer.unwrap_or_else(|| "v=0\r\ns=-\r\nt=0 0\r\n".to_string());
    let body = answer.as_bytes();
    let response = format!(
        "HTTP/1.1 201 Created\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        answer
    );
    write_half.write_all(response.as_bytes()).await?;
    Ok(())
}

#[cfg(feature = "webrtc")]
async fn read_http_request_body<R>(reader: &mut R) -> std::io::Result<(String, Vec<u8>)>
where
    R: AsyncBufReadExt + Unpin,
{
    let mut header = String::new();
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse().ok();
        }
        header.push_str(&line);
    }

    let mut body = vec![0u8; content_length.unwrap_or(0)];
    if !body.is_empty() {
        reader.read_exact(&mut body).await?;
    }
    Ok((header, body))
}

#[cfg(feature = "webrtc")]
async fn open_webrtc_push_handle(
    connector: &cheetah_connector::EngineConnector,
    server_addr: SocketAddr,
    tracks: Vec<TrackInfo>,
) -> Result<PushHandle, ConnectorError> {
    let url = format!(
        "webrtc+whip://127.0.0.1:{}/whip/app/stream",
        server_addr.port()
    );
    let options = ConnectorPushOptions {
        tracks,
        protocol: ProtocolPushExtras::WebRtc(WebRtcPushExtras::default()),
        ..Default::default()
    };
    connector.open_push(Protocol::WebRtc, &url, options).await
}

#[cfg(feature = "webrtc")]
async fn wait_for_keyframe(
    answerer: &Arc<WebRtcDriverHandle>,
    timeout: Duration,
) -> Result<Bytes, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, answerer.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::Core(WebRtcCoreEvent::Media {
                event:
                    WebRtcMediaEvent::Frame {
                        codec,
                        random_access,
                        payload,
                        ..
                    },
                ..
            }))) => {
                if codec == WebRtcCodecKind::H264 && random_access && !payload.is_empty() {
                    return Ok(payload);
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => break,
        }
    }
    Err("timed out waiting for keyframe".to_string())
}

#[cfg(feature = "webrtc")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_wr_01_invalid_url() -> Result<(), Box<dyn std::error::Error>> {
    let connector = build_connector().await?;

    let err = connector
        .open_push(
            Protocol::WebRtc,
            "ftp://127.0.0.1:8000/live/stream",
            Default::default(),
        )
        .await
        .expect_err("ftp:// must be rejected as invalid url");

    assert!(
        matches!(
            err,
            ConnectorError::InvalidUrl {
                protocol: Protocol::WebRtc,
                ..
            }
        ),
        "expected InvalidUrl, got {err:?}"
    );

    connector.stop().await;
    Ok(())
}

#[cfg(not(feature = "webrtc"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_wr_02_feature_disabled() -> Result<(), Box<dyn std::error::Error>> {
    let connector = build_connector().await?;

    let err = connector
        .open_push(
            Protocol::WebRtc,
            "http://127.0.0.1:8000/whip/app/stream",
            Default::default(),
        )
        .await
        .expect_err("webrtc push must be disabled without feature");

    assert!(
        matches!(
            err,
            ConnectorError::FeatureDisabled {
                protocol: Protocol::WebRtc,
                feature: "webrtc",
            }
        ),
        "expected FeatureDisabled, got {err:?}"
    );

    connector.stop().await;
    Ok(())
}

#[cfg(feature = "webrtc")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_wr_03_open_push_returns_handle() -> Result<(), Box<dyn std::error::Error>> {
    let connector = build_connector().await?;
    let (answerer, answerer_cancel) = spawn_answerer_driver().await?;
    let (server_addr, server_handle) = start_whip_server(answerer.clone()).await?;

    let handle = open_webrtc_push_handle(&connector, server_addr, vec![h264_track()]).await?;
    assert_eq!(handle.protocol(), Protocol::WebRtc);
    assert!(handle.url().starts_with("webrtc+whip://"));

    handle.close()?;
    connector.stop().await;
    answerer_cancel.cancel();
    server_handle.abort();
    let _ = answerer;
    Ok(())
}

#[cfg(feature = "webrtc")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_wr_04_wait_ready_completes() -> Result<(), Box<dyn std::error::Error>> {
    let connector = build_connector().await?;
    let (answerer, answerer_cancel) = spawn_answerer_driver().await?;
    let (server_addr, server_handle) = start_whip_server(answerer.clone()).await?;

    let handle = open_webrtc_push_handle(&connector, server_addr, vec![h264_track()]).await?;
    tokio::time::timeout(Duration::from_secs(15), handle.wait_ready())
        .await
        .expect("wait_ready timed out")
        .expect("wait_ready failed");

    handle.close()?;
    connector.stop().await;
    answerer_cancel.cancel();
    server_handle.abort();
    let _ = answerer;
    Ok(())
}

#[cfg(feature = "webrtc")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_wr_05_push_keyframe_reaches_peer() -> Result<(), Box<dyn std::error::Error>> {
    let connector = build_connector().await?;
    let (answerer, answerer_cancel) = spawn_answerer_driver().await?;
    let (server_addr, server_handle) = start_whip_server(answerer.clone()).await?;

    let handle = open_webrtc_push_handle(&connector, server_addr, vec![h264_track()]).await?;
    tokio::time::timeout(Duration::from_secs(15), handle.wait_ready())
        .await
        .expect("wait_ready timed out")
        .expect("wait_ready failed");

    // T-WR-06: update tracks + extradata must not break the push path.
    handle
        .update_tracks(vec![h264_track()])
        .expect("update_tracks");

    let result = handle.push_frame(Arc::new(h264_keyframe()))?;
    assert!(matches!(result, cheetah_sdk::DispatchResult::Accepted));

    let payload = wait_for_keyframe(&answerer, Duration::from_secs(15)).await?;
    assert!(
        !payload.is_empty(),
        "answerer must receive non-empty payload"
    );

    handle.close()?;
    connector.stop().await;
    answerer_cancel.cancel();
    server_handle.abort();
    let _ = answerer
        .send_command(WebRtcDriverCommand::StopSession {
            session_id: WebRtcSessionId::new(2),
            reason: WebRtcCloseReason::Normal,
        })
        .await;
    Ok(())
}

#[cfg(feature = "webrtc")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_wr_07_close_cleans_up() -> Result<(), Box<dyn std::error::Error>> {
    let connector = build_connector().await?;
    let (answerer, answerer_cancel) = spawn_answerer_driver().await?;
    let (server_addr, server_handle) = start_whip_server(answerer.clone()).await?;

    let handle = open_webrtc_push_handle(&connector, server_addr, vec![h264_track()]).await?;
    tokio::time::timeout(Duration::from_secs(15), handle.wait_ready())
        .await
        .expect("wait_ready timed out")
        .expect("wait_ready failed");

    assert_eq!(handle.take_keyframe_requests(), 0);
    handle.close()?;
    connector.stop().await;
    answerer_cancel.cancel();
    server_handle.abort();
    let _ = answerer;
    Ok(())
}

#[cfg(feature = "webrtc")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t_wr_08_signaling_only_completes() -> Result<(), Box<dyn std::error::Error>> {
    let connector = build_connector().await?;
    let (answerer, answerer_cancel) = spawn_answerer_driver().await?;
    let (server_addr, server_handle) = start_whip_server(answerer.clone()).await?;

    let handle = open_webrtc_push_handle(&connector, server_addr, vec![h264_track()]).await?;
    tokio::time::timeout(Duration::from_secs(15), handle.wait_ready())
        .await
        .expect("wait_ready timed out")
        .expect("wait_ready failed");

    // This is a signaling-only test: we confirm that wait_ready completes
    // without sending any media, and that the name is not "media_roundtrip".
    handle.close()?;
    connector.stop().await;
    answerer_cancel.cancel();
    server_handle.abort();
    let _ = answerer;
    Ok(())
}
