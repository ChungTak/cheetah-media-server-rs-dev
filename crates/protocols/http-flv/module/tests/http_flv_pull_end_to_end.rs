mod support;

use std::sync::Arc;

use cheetah_codec::{FlvDemuxEvent, FlvDemuxer, FlvTagType};
use cheetah_http_flv_module::pull::{pull_http_flv_once, pull_ws_flv_once, PullReadLimits};
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use cheetah_runtime_tokio::TokioRuntime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn encode_ws_binary_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 16);
    out.push(0x82);
    if payload.len() <= 125 {
        out.push(payload.len() as u8);
    } else if payload.len() <= 0xFFFF {
        out.push(126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    out.extend_from_slice(payload);
    out
}

fn count_video_audio(bytes: &[u8]) -> (usize, usize) {
    let mut demuxer = FlvDemuxer::default();
    let events = demuxer.push(bytes).expect("demux");
    let mut video = 0usize;
    let mut audio = 0usize;
    for event in events {
        if let FlvDemuxEvent::Tag(tag) = event {
            match tag.tag_type {
                FlvTagType::Video => video += 1,
                FlvTagType::Audio => audio += 1,
                FlvTagType::Script => {}
            }
        }
    }
    (video, audio)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_and_ws_pull_can_parse_standard_fixtures() {
    let cases = support::load_manifest_cases();
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>;
    let limits = PullReadLimits::default();

    for case in cases
        .into_iter()
        .filter(|item| item.role == "standard_play")
    {
        let fixture = support::load_fixture_bytes(&case.fixture);
        let (expect_video, expect_audio) = count_video_audio(&fixture);

        let http_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind http");
        let http_addr = http_listener.local_addr().expect("http addr");
        let http_body = fixture.clone();
        tokio::spawn(async move {
            let (mut socket, _) = http_listener.accept().await.expect("accept http");
            let mut req = vec![0u8; 4096];
            let _ = socket.read(&mut req).await.expect("read request");
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/x-flv\r\n\r\n")
                .await
                .expect("write head");
            socket.write_all(&http_body).await.expect("write body");
            let _ = socket.shutdown().await;
        });

        let cancel = CancellationToken::new();
        let http_result = pull_http_flv_once(
            runtime.clone(),
            &format!("http://{http_addr}/live/stream.flv"),
            &cancel,
            limits,
        )
        .await
        .expect("http pull");
        assert!(http_result.header.is_some(), "http header missing");
        assert!(
            http_result
                .tags
                .iter()
                .filter(|tag| tag.tag_type == FlvTagType::Video)
                .count()
                >= expect_video
        );
        assert!(
            http_result
                .tags
                .iter()
                .filter(|tag| tag.tag_type == FlvTagType::Audio)
                .count()
                >= expect_audio
        );

        let ws_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind ws");
        let ws_addr = ws_listener.local_addr().expect("ws addr");
        let ws_payload = encode_ws_binary_frame(&fixture);
        tokio::spawn(async move {
            let (mut socket, _) = ws_listener.accept().await.expect("accept ws");
            let mut req = vec![0u8; 4096];
            let _ = socket.read(&mut req).await.expect("read ws request");
            socket
                .write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\n\
Upgrade: websocket\r\n\
Connection: Upgrade\r\n\
Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
                )
                .await
                .expect("write handshake");
            socket.write_all(&ws_payload).await.expect("write ws frame");
            let _ = socket.shutdown().await;
        });

        let cancel = CancellationToken::new();
        let ws_result = pull_ws_flv_once(
            runtime.clone(),
            &format!("ws://{ws_addr}/live/stream.flv"),
            &cancel,
            limits,
        )
        .await
        .expect("ws pull");
        assert!(ws_result.header.is_some(), "ws header missing");
        assert!(
            ws_result
                .tags
                .iter()
                .filter(|tag| tag.tag_type == FlvTagType::Video)
                .count()
                >= expect_video
        );
        assert!(
            ws_result
                .tags
                .iter()
                .filter(|tag| tag.tag_type == FlvTagType::Audio)
                .count()
                >= expect_audio
        );
    }
}
