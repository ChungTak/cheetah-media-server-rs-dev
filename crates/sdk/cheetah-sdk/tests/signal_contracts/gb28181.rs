use std::sync::Arc;

use cheetah_media_api::command::*;
use cheetah_media_api::error::MediaErrorCode;
use cheetah_media_api::ids::*;
use cheetah_media_api::model::*;
use cheetah_sdk::MediaServices;

use crate::support::{ctx, FakeMediaProvider};

/// Simulate a GB28181 project that opens an RTP receiver, queries media status,
/// stops the session, and uses RTP talk for two-way audio.
///
/// 模拟一个 GB28181 项目：打开 RTP 接收端、查询媒体状态、停止会话、使用 RTP talk 进行双向语音。
async fn gb28181_flow(services: MediaServices) {
    let ctx = ctx();
    let key = MediaKey::with_default_vhost("gb28181", "34020000001320000001_1", None)
        .expect("valid GB28181 key");

    let receiver = services
        .rtp()
        .expect("RTP provider available")
        .open_rtp_receiver(
            &ctx,
            RtpReceiverRequest {
                media_key: key.clone(),
                port: Some(10000),
                ip: Some("192.0.2.1".to_string()),
                ssrc: Some(0x12345678),
                enable_rtcp: true,
                tcp_mode: Some(RtpTcpMode::Passive),
                payload_type: Some(96),
                codec_hint: Some("H264".to_string()),
                reuse_port: false,
                timeout_ms: 30_000,
            },
        )
        .await
        .expect("open RTP receiver");
    assert_eq!(receiver.media_key, key);
    assert_eq!(receiver.kind, RtpSessionKind::Receiver);
    assert_eq!(receiver.state, RtpSessionState::Listening);
    assert_eq!(receiver.local_port, Some(10000));

    let online = services
        .control()
        .expect("control provider available")
        .is_media_online(&ctx, &key)
        .await
        .expect("query online state");
    assert_eq!(online, OnlineState::Online);

    services
        .rtp()
        .expect("RTP provider available")
        .stop_rtp_session(&ctx, &receiver.session_id)
        .await
        .expect("stop RTP session");

    services
        .record()
        .expect("record provider available")
        .control_record_playback(
            &ctx,
            &RecordFileId("file-1".to_string()),
            RecordPlaybackCommand::Pause,
        )
        .await
        .expect("control record playback");

    let talk = services
        .rtp()
        .expect("RTP provider available")
        .open_rtp_sender(
            &ctx,
            RtpSenderRequest {
                media_key: key.clone(),
                destination_endpoint: "192.0.2.1:20000".to_string(),
                ssrc: Some(0x87654321),
                payload_type: Some(97),
                mode: RtpSenderMode::Talk,
                transport_options: std::collections::HashMap::new(),
            },
        )
        .await
        .expect("open RTP talk sender");
    assert_eq!(talk.kind, RtpSessionKind::Talk);
    assert_eq!(talk.media_key, key);
}

#[tokio::test]
async fn gb28181_can_complete_media_call_flow() {
    let services = MediaServices::unavailable();
    let provider = Arc::new(FakeMediaProvider::new());
    services.register_rtp(provider.clone());
    services.register_control(provider.clone());
    services.register_record(provider);

    gb28181_flow(services).await;
}

#[tokio::test]
async fn gb28181_unsupported_capability_is_distinguishable_from_unavailable() {
    let services = MediaServices::unavailable();
    let provider = Arc::new(FakeMediaProvider::new().with_talk(false));
    services.register_rtp(provider);

    let ctx = ctx();
    let key = MediaKey::with_default_vhost("gb28181", "34020000001320000001_1", None)
        .expect("valid GB28181 key");

    let rtp = services.rtp().expect("RTP provider available");
    let err = rtp
        .open_rtp_sender(
            &ctx,
            RtpSenderRequest {
                media_key: key,
                destination_endpoint: "192.0.2.1:20000".to_string(),
                ssrc: Some(0x87654321),
                payload_type: Some(97),
                mode: RtpSenderMode::Talk,
                transport_options: std::collections::HashMap::new(),
            },
        )
        .await
        .expect_err("talk should be unsupported");
    assert_eq!(err.code, MediaErrorCode::Unsupported);

    let unavailable_services = MediaServices::unavailable();
    assert!(
        unavailable_services.rtp().is_none(),
        "missing provider must be unavailable, not unsupported"
    );
}
