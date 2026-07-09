mod support;

use cheetah_codec::{FlvDemuxEvent, FlvDemuxer, FlvTagType};
use cheetah_http_flv_module::pull::{
    pull_http_flv_once, pull_ws_flv_once, HttpFlvPullError, PullReadLimits,
};
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use cheetah_runtime_tokio::TokioRuntime;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn fold_demux_counts(events: Vec<FlvDemuxEvent>) -> (usize, usize, usize) {
    let mut header = 0usize;
    let mut video = 0usize;
    let mut audio = 0usize;
    for event in events {
        match event {
            FlvDemuxEvent::Header(_) => header += 1,
            FlvDemuxEvent::Tag(tag) => match tag.tag_type {
                FlvTagType::Video => video += 1,
                FlvTagType::Audio => audio += 1,
                FlvTagType::Script => {}
            },
            FlvDemuxEvent::PreviousTagSizeMismatch(_) => {}
        }
    }
    (header, video, audio)
}

#[test]
fn flv_fault_views_are_bounded_and_strong_views_keep_counts() {
    let fixture = support::load_fixture_bytes("standard/h264_aac.flvstream");
    let baseline_events = FlvDemuxer::default()
        .push(&fixture)
        .expect("baseline demux");
    let baseline = fold_demux_counts(baseline_events);

    let views = support::fault_views::build_flv_fault_views(&fixture);
    for view in views {
        let mut demuxer = FlvDemuxer::new(1024 * 1024);
        let mut collected = Vec::new();
        let mut had_error = false;
        for chunk in &view.chunks {
            match demuxer.push(chunk) {
                Ok(events) => collected.extend(events),
                Err(_) => {
                    had_error = true;
                    break;
                }
            }
        }
        let counts = fold_demux_counts(collected);
        if matches!(
            view.name,
            "single_buffer" | "one_byte_chunks" | "coalesced_4"
        ) {
            assert!(!had_error, "strong view should not fail: {}", view.name);
            assert_eq!(counts, baseline, "strong view mismatch: {}", view.name);
        } else {
            assert!(
                had_error || counts.0 <= baseline.0.saturating_add(1),
                "fault view should stay bounded: {}",
                view.name
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chunked_split_every_byte_and_ws_fragmented_binary_are_bounded() {
    let fixture = support::load_fixture_bytes("standard/h264_aac.flvstream");
    let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>;
    let limits = PullReadLimits::default();

    let http_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind http");
    let http_addr = http_listener.local_addr().expect("http addr");
    tokio::spawn(async move {
        let (mut socket, _) = http_listener.accept().await.expect("accept");
        let mut req = vec![0u8; 4096];
        let _ = socket.read(&mut req).await.expect("read request");
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await
            .expect("head");
        for byte in fixture {
            socket.write_all(b"1\r\n").await.expect("chunk-size");
            socket.write_all(&[byte]).await.expect("chunk-data");
            socket.write_all(b"\r\n").await.expect("chunk-end");
        }
        socket.write_all(b"0\r\n\r\n").await.expect("end");
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
    .expect("chunked-by-byte should be dechunked and parsed");
    assert!(http_result.header.is_some());
    assert!(!http_result.tags.is_empty());

    let ws_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind ws");
    let ws_addr = ws_listener.local_addr().expect("ws addr");
    tokio::spawn(async move {
        let (mut socket, _) = ws_listener.accept().await.expect("accept");
        let mut req = vec![0u8; 4096];
        let _ = socket.read(&mut req).await.expect("read request");
        socket
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\n\
Upgrade: websocket\r\n\
Connection: Upgrade\r\n\
Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
            )
            .await
            .expect("hs");
        socket
            .write_all(&[0x02, 0x02, 0x46, 0x4c]) // FIN=0 fragmented binary start
            .await
            .expect("frag");
        let _ = socket.shutdown().await;
    });

    let cancel = CancellationToken::new();
    let ws_err = pull_ws_flv_once(
        runtime,
        &format!("ws://{ws_addr}/live/stream.flv"),
        &cancel,
        limits,
    )
    .await
    .expect_err("fragmented ws frame should be rejected");
    assert!(matches!(ws_err, HttpFlvPullError::WebSocketProtocol(_)));
}
