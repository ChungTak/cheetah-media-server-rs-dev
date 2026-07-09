use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtmp_core::{RtmpClientState, RtmpEvent, RtmpMediaType, RtmpMessageStreamId, RtmpUrl};
use cheetah_rtmp_driver_tokio::{
    start_client, ClientDriverEvent, RtmpClientDriverConfig, RtmpClientHandle, RtmpClientMode,
    RtmpCoreCommand,
};
use cheetah_rtmp_module::RtmpModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::CancellationToken;
use tokio::time::timeout;

const RTMP_MEDIA_STREAM_ID: u32 = RtmpMessageStreamId::MEDIA.get();

fn reserve_listen_addr() -> SocketAddr {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);
    listen
}

async fn wait_for_client_state(
    client: &mut RtmpClientHandle,
    target: RtmpClientState,
    stage: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut saw_connected = false;
    loop {
        let now = Instant::now();
        assert!(
            now < deadline,
            "timeout waiting for rtmp client state {target:?} at {stage}, saw_connected={saw_connected}"
        );
        let remaining = deadline.saturating_duration_since(now);
        let event = match timeout(remaining, client.recv_event()).await {
            Ok(Some(event)) => event,
            Ok(None) => panic!(
                "rtmp client event stream closed unexpectedly before {target:?} at {stage}, saw_connected={saw_connected}"
            ),
            Err(_) => panic!(
                "timeout waiting rtmp event before {target:?} at {stage}, saw_connected={saw_connected}"
            ),
        };
        if let ClientDriverEvent::Connected { .. } = event {
            saw_connected = true;
            continue;
        }
        if let ClientDriverEvent::Core {
            event: RtmpEvent::ClientStateChanged { state },
        } = event
        {
            if state == target {
                return;
            }
        }
        if let ClientDriverEvent::Closed { reason } = event {
            panic!("rtmp client closed before reaching {target:?} at {stage}: {reason}");
        }
    }
}

fn h264_keyframe_payload_with_cts(cts_ms: i32) -> Bytes {
    assert!((0..=0x7f_ffff).contains(&cts_ms));
    let cts = cts_ms as u32;
    Bytes::from(vec![
        0x17,
        0x01,
        ((cts >> 16) & 0xff) as u8,
        ((cts >> 8) & 0xff) as u8,
        (cts & 0xff) as u8,
        0x00,
        0x00,
        0x00,
        0x01,
        0x65,
    ])
}

fn aac_sequence_header_48k_stereo() -> Bytes {
    Bytes::from_static(&[0xaf, 0x00, 0x11, 0x90])
}

fn aac_raw_payload() -> Bytes {
    Bytes::from_static(&[0xaf, 0x01, 0x12, 0x10])
}

fn is_h264_coded_video_payload(payload: &[u8]) -> bool {
    payload.len() > 1 && payload[1] == 0x01
}

async fn recv_h264_coded_video_timestamp_ms(client: &mut RtmpClientHandle, stage: &str) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let now = Instant::now();
        assert!(
            now < deadline,
            "timeout waiting h264 coded video media event at {stage}"
        );
        let remaining = deadline.saturating_duration_since(now);
        let event = timeout(remaining, client.recv_event())
            .await
            .expect("timeout waiting rtmp media event")
            .expect("rtmp client event stream closed unexpectedly");
        if let ClientDriverEvent::Core {
            event:
                RtmpEvent::MediaData {
                    media_type: RtmpMediaType::Video,
                    timestamp_ms,
                    payload,
                    ..
                },
        } = event
        {
            if is_h264_coded_video_payload(&payload) {
                return timestamp_ms;
            }
        }
    }
}

async fn recv_h264_coded_video_timestamps(
    client: &mut RtmpClientHandle,
    min_count: usize,
    stage: &str,
) -> Vec<u32> {
    let mut out = Vec::with_capacity(min_count);
    while out.len() < min_count {
        out.push(recv_h264_coded_video_timestamp_ms(client, stage).await);
    }
    out
}

fn assert_monotonic_and_contains_100ms_step(timestamps: &[u32], stage: &str) {
    assert!(
        timestamps.len() >= 2,
        "need at least 2 timestamps at {stage}, got {}",
        timestamps.len()
    );
    let mut has_100ms_step = false;
    for window in timestamps.windows(2) {
        let first = window[0];
        let second = window[1];
        assert!(
            second >= first,
            "media timestamp must be monotonic at {stage}: first={first}, second={second}"
        );
        let delta = second - first;
        if (95..=105).contains(&delta) {
            has_100ms_step = true;
        }
    }
    assert!(
        has_100ms_step,
        "expected at least one ~100ms timestamp step at {stage}, got {timestamps:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rtmp_publish_to_rtmp_play_timestamp_regression() {
    let rtmp_listen = reserve_listen_addr();
    let stream_name = "matrix-rtmp-rtmp-h264-aac";
    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/{stream_name}"))
        .expect("parse rtmp url");

    let config = Arc::new(ConfigStore::new());
    let config_yaml = format!("modules:\n  rtmp:\n    listen: \"{rtmp_listen}\"\n");
    config.load_yaml_str(&config_yaml).expect("load config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime.clone())
        .register_module_factory(Arc::new(RtmpModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mut publisher = start_client(
        runtime.clone(),
        rtmp_url.clone(),
        RtmpClientMode::Publish,
        RtmpClientDriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start rtmp publish client");
    wait_for_client_state(&mut publisher, RtmpClientState::Publishing, "publish").await;

    let tx = publisher.core_command_sender();
    tx.send_core(RtmpCoreCommand::SendAudio {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 0,
        payload: aac_sequence_header_48k_stereo(),
    })
    .await
    .expect("send rtmp aac sequence header");
    tx.send_core(RtmpCoreCommand::SendVideo {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 0,
        payload: h264_keyframe_payload_with_cts(0),
    })
    .await
    .expect("send rtmp first h264 keyframe");
    tx.send_core(RtmpCoreCommand::SendAudio {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 0,
        payload: aac_raw_payload(),
    })
    .await
    .expect("send rtmp first aac raw");

    let mut player = start_client(
        runtime,
        rtmp_url,
        RtmpClientMode::Play,
        RtmpClientDriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start rtmp play client");
    wait_for_client_state(&mut player, RtmpClientState::Playing, "play").await;

    tx.send_core(RtmpCoreCommand::SendVideo {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 100,
        payload: h264_keyframe_payload_with_cts(0),
    })
    .await
    .expect("send rtmp second h264 keyframe");
    tx.send_core(RtmpCoreCommand::SendVideo {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 200,
        payload: h264_keyframe_payload_with_cts(0),
    })
    .await
    .expect("send rtmp third h264 keyframe");
    tx.send_core(RtmpCoreCommand::SendVideo {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 300,
        payload: h264_keyframe_payload_with_cts(0),
    })
    .await
    .expect("send rtmp fourth h264 keyframe");

    let video_timestamps = recv_h264_coded_video_timestamps(&mut player, 3, "rtmp->rtmp").await;
    assert_monotonic_and_contains_100ms_step(&video_timestamps, "rtmp->rtmp");

    publisher.shutdown();
    let _ = publisher.wait().await;
    player.shutdown();
    let _ = player.wait().await;
    engine.stop().await;
}
