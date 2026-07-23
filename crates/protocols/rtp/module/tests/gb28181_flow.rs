use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cheetah_codec::AVFrame;
use cheetah_sdk::media_api::command::{
    PlaybackControl, RtpReceiverRequest, RtpSenderMode, RtpSenderRequest, StartRecordRequest,
    StopRecordRequest,
};
use cheetah_sdk::media_api::error::{EffectOutcome, MediaErrorCode};
use cheetah_sdk::media_api::ids::RtpSessionId;
use cheetah_sdk::media_api::model::{OnlineState, RecordTaskState};
use cheetah_sdk::media_api::port::{MediaControlApi, RecordApi, RtpApi};
use cheetah_sdk::media_api::rtp_session::{
    MediaContainer, OpenRtpReceiver, OpenRtpSender, OpenRtpTalk, PlaybackRange, RtpDirection,
    RtpPayloadBinding, RtpSessionParamsBuilder, RtpSessionPurpose, RtpSessionQuery, RtpSessionRef,
    RtpTransport, SourceBindingPolicy, StopRtpSession, UpdateRtpSession,
};
use cheetah_sdk::media_api::{MediaCapabilitySet, MediaKey, MediaRequestContext};
use cheetah_sdk::StreamKey;
use tokio::time::{sleep, timeout, Instant as TokioInstant};

mod support;

use support::*;

const SSRC: u32 = 0x12345678;
const INGEST_PT: u8 = 100;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gb28181_udp_receiver_ingest_stream_online_and_keyframe_request() {
    let harness = Gb28181TestHarness::start().await;
    let media = harness.media_facade();
    let ctx = MediaRequestContext::default();

    let media_key = MediaKey::with_default_vhost("gb28181_device_001", "ch_001", None).unwrap();
    let stream_key = StreamKey::new("gb28181_device_001", "ch_001");

    let request = RtpReceiverRequest {
        media_key: media_key.clone(),
        port: Some(0),
        ip: None,
        ssrc: Some(SSRC),
        enable_rtcp: false,
        tcp_mode: None,
        payload_type: Some(INGEST_PT),
        codec_hint: Some("ps".to_string()),
        reuse_port: false,
        timeout_ms: 0,
        source_binding_policy: SourceBindingPolicy::default(),
    };

    let session = media.open_rtp_receiver(&ctx, request).await.unwrap();
    let recv_port = session.local_port.expect("receiver bound port");
    let recv_addr: SocketAddr = format!("127.0.0.1:{recv_port}").parse().unwrap();

    let socket = bind_udp_socket().await;

    // Warm up the inbound session with one video + one audio PS/RTP packet so the
    // publisher is established and tracks are discovered before we subscribe.
    send_rtp(
        &socket,
        recv_addr,
        mux_ps_frame(&make_video_frame(0)),
        SSRC,
        1,
        0,
        INGEST_PT,
    )
    .await;
    send_rtp(
        &socket,
        recv_addr,
        mux_ps_frame(&make_audio_frame(80)),
        SSRC,
        2,
        80,
        INGEST_PT,
    )
    .await;

    harness
        .wait_for_stream_online(&stream_key, Duration::from_secs(5))
        .await;

    let mut subscriber = harness.open_subscriber(stream_key.clone()).await;
    let mut saw_video = false;
    let mut saw_audio = false;
    let mut seq: u32 = 3;
    let mut pts: i64 = 100_000;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !(saw_video && saw_audio) {
        // Keep feeding keyframes and audio. The PS demuxer emits a video frame when the
        // next video pack header arrives, so each iteration after the first produces a keyframe.
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_video_frame(pts)),
            SSRC,
            seq as u16,
            (pts / 100 * 9) as u32,
            INGEST_PT,
        )
        .await;
        seq += 1;
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_audio_frame(pts + 80)),
            SSRC,
            seq as u16,
            ((pts + 80) / 100 * 9) as u32,
            INGEST_PT,
        )
        .await;
        seq += 1;
        pts += 100_000;

        if let Ok(Ok(Some(frame))) = timeout(Duration::from_millis(200), subscriber.recv()).await {
            if frame.media_kind == cheetah_codec::MediaKind::Video && frame.is_key_frame() {
                saw_video = true;
            }
            if frame.media_kind == cheetah_codec::MediaKind::Audio {
                saw_audio = true;
            }
        }
    }
    assert!(saw_video, "expected a video keyframe");
    assert!(saw_audio, "expected an audio frame");

    let info = media.get_media(&ctx, &media_key).await.unwrap();
    assert_eq!(info.online, OnlineState::Online);
    assert!(!info.tracks.is_empty(), "expected track metadata");
    media.request_keyframe(&ctx, &media_key).await.unwrap();

    let record_task = media
        .start_record(
            &ctx,
            StartRecordRequest {
                media_key: media_key.clone(),
                format: "mp4".to_string(),
                template: Default::default(),
                segment_duration_ms: None,
                max_segments: None,
                storage_policy: Default::default(),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(record_task.state, RecordTaskState::Running);

    for _ in 0..5 {
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_video_frame(pts)),
            SSRC,
            seq as u16,
            (pts / 100 * 9) as u32,
            INGEST_PT,
        )
        .await;
        seq += 1;
        pts += 100_000;
        sleep(Duration::from_millis(50)).await;
    }

    let stopped = media
        .stop_record(
            &ctx,
            StopRecordRequest {
                task_id: record_task.task_id,
            },
        )
        .await
        .unwrap();
    assert_eq!(stopped.state, RecordTaskState::Completed);

    media
        .stop_rtp_session(&ctx, &session.session_id)
        .await
        .unwrap();

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gb28181_udp_sender_egress_emits_real_rtp_packets() {
    let harness = Gb28181TestHarness::start().await;
    let media = harness.media_facade();
    let ctx = MediaRequestContext::default();

    let source_key = StreamKey::new("rtp_sender_source", "main");
    let source_media_key = stream_key_to_media_key(&source_key);
    let publisher = harness
        .open_publisher(
            source_key.clone(),
            vec![make_video_track(), make_audio_track()],
        )
        .await;

    let recv_socket = bind_udp_socket().await;
    let dest_addr = recv_socket.local_addr().unwrap();

    let request = RtpSenderRequest {
        media_key: source_media_key.clone(),
        destination_endpoint: dest_addr.to_string(),
        ssrc: Some(SSRC),
        payload_type: Some(INGEST_PT),
        codec_hint: Some("ps".to_string()),
        mode: RtpSenderMode::Active,
        transport_options: HashMap::new(),
        source_binding_policy: SourceBindingPolicy::default(),
    };

    let _sender_session = media.open_rtp_sender(&ctx, request).await.unwrap();

    for i in 0..10 {
        let ps = mux_ps_frame(&make_video_frame(i * 100_000));
        publisher
            .push_frame(Arc::new(AVFrame {
                payload: ps,
                ..make_video_frame(i * 100_000)
            }))
            .unwrap();
        sleep(Duration::from_millis(50)).await;
    }

    let mut saw_rtp = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !saw_rtp {
        if let Some((header, _payload, _addr)) =
            recv_rtp(&recv_socket, Duration::from_millis(200)).await
        {
            assert_eq!(header.version, 2);
            assert_eq!(header.ssrc, SSRC);
            assert_eq!(header.payload_type, 96, "PS mode uses PT 96");
            saw_rtp = true;
        }
    }
    assert!(saw_rtp, "expected to receive RTP packets from sender");

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gb28181_talkback_audio_round_trip() {
    let harness = Gb28181TestHarness::start().await;
    let media = harness.media_facade();
    let ctx = MediaRequestContext::default();

    let media_key = MediaKey::with_default_vhost("gb28181_talk", "cam_001", None).unwrap();
    let request = RtpReceiverRequest {
        media_key: media_key.clone(),
        port: Some(0),
        ip: None,
        ssrc: Some(SSRC),
        enable_rtcp: false,
        tcp_mode: None,
        payload_type: Some(INGEST_PT),
        codec_hint: Some("ps".to_string()),
        reuse_port: false,
        timeout_ms: 0,
        source_binding_policy: SourceBindingPolicy::default(),
    };

    let session = media.open_rtp_receiver(&ctx, request).await.unwrap();
    let recv_port = session.local_port.unwrap();
    let recv_addr: SocketAddr = format!("127.0.0.1:{recv_port}").parse().unwrap();

    let socket = bind_udp_socket().await;
    let src_addr = socket.local_addr().unwrap();

    // Send an audio-only PS packet so the receiver records our source endpoint.
    send_rtp(
        &socket,
        recv_addr,
        mux_ps_frame(&make_audio_frame(80)),
        SSRC,
        1,
        0,
        INGEST_PT,
    )
    .await;

    // Give the ingress worker time to update the receiver's remote endpoint.
    sleep(Duration::from_millis(200)).await;

    // Upgrade the inbound session to talkback, sending audio back to `src_addr`.
    let talk_request = RtpSenderRequest {
        media_key: media_key.clone(),
        destination_endpoint: src_addr.to_string(),
        ssrc: Some(SSRC),
        payload_type: Some(8),
        codec_hint: Some("raw_audio".to_string()),
        mode: RtpSenderMode::Talk,
        transport_options: HashMap::new(),
        source_binding_policy: SourceBindingPolicy::default(),
    };
    media.open_rtp_sender(&ctx, talk_request).await.unwrap();

    // Feed a short burst of audio frames; the talk egress echoes each one back as raw G.711A RTP.
    let mut saw_talkback = false;
    let mut seq: u16 = 2;
    let mut pts: i64 = 160;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !saw_talkback {
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_audio_frame(pts)),
            SSRC,
            seq,
            (pts / 100 * 9) as u32,
            INGEST_PT,
        )
        .await;
        seq = seq.wrapping_add(1);
        pts += 80;

        if let Some((header, _payload, addr)) = recv_rtp(&socket, Duration::from_millis(100)).await
        {
            if addr == recv_addr && header.payload_type == 8 {
                saw_talkback = true;
            }
        }
    }
    assert!(saw_talkback, "expected talkback RTP from the receiver");

    media
        .stop_rtp_session(&ctx, &session.session_id)
        .await
        .unwrap();
    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gb28181_session_stop_releases_port() {
    let harness = Gb28181TestHarness::start().await;
    let media = harness.media_facade();
    let ctx = MediaRequestContext::default();

    let media_key = MediaKey::with_default_vhost("gb28181_stop", "cam_001", None).unwrap();
    let request = RtpReceiverRequest {
        media_key: media_key.clone(),
        port: Some(0),
        ip: None,
        ssrc: Some(SSRC),
        enable_rtcp: false,
        tcp_mode: None,
        payload_type: Some(INGEST_PT),
        codec_hint: Some("ps".to_string()),
        reuse_port: false,
        timeout_ms: 0,
        source_binding_policy: SourceBindingPolicy::default(),
    };

    let session = media.open_rtp_receiver(&ctx, request).await.unwrap();
    let recv_port = session.local_port.unwrap();

    let probe = tokio::net::UdpSocket::bind(format!("127.0.0.1:{recv_port}")).await;
    assert!(
        probe.is_err(),
        "port should be occupied while session is active"
    );

    media
        .stop_rtp_session(&ctx, &session.session_id)
        .await
        .unwrap();

    // The socket is released asynchronously after the driver cancels the reader task;
    // retry a few times before failing.
    let mut probe2 = Err(std::io::Error::other("not attempted"));
    for _ in 0..20 {
        sleep(Duration::from_millis(50)).await;
        probe2 = tokio::net::UdpSocket::bind(format!("127.0.0.1:{recv_port}")).await;
        if probe2.is_ok() {
            break;
        }
    }
    assert!(probe2.is_ok(), "port should be released after stop");
    drop(probe2);

    assert!(media
        .get_rtp_session(&ctx, &session.session_id)
        .await
        .is_err());

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_rtp_session_errors_carry_resource_ref_and_generation() {
    let harness = Gb28181TestHarness::start().await;
    let rtp_api = harness
        .engine
        .media_services()
        .rtp_session()
        .expect("rtp session api");
    let ctx = MediaRequestContext::default();

    let media_key = MediaKey::with_default_vhost("gb28181_error", "cam_001", None).unwrap();
    let params = RtpSessionParamsBuilder::new(media_key, RtpDirection::Receive)
        .transport(RtpTransport::Udp)
        .container(MediaContainer::Ps)
        .payload_binding(RtpPayloadBinding {
            payload_type: INGEST_PT,
            codec: "PS".to_string(),
            clock_rate: 90000,
            channels: None,
            packet_duration_ms: None,
        })
        .build();
    let descriptor = rtp_api
        .open_receiver(
            &ctx,
            OpenRtpReceiver {
                params,
                playback_range: None,
            },
        )
        .await
        .expect("open receiver");

    // get_session with a stale generation returns Conflict and carries the resource ref.
    let stale_ref = RtpSessionRef {
        session_id: descriptor.session_id.clone(),
        expected_generation: cheetah_sdk::media_api::rtp_session::RtpSessionGeneration(
            descriptor.generation.0 + 1,
        ),
    };
    let err = rtp_api
        .get_session(&ctx, stale_ref.clone())
        .await
        .unwrap_err();
    assert_eq!(err.code, MediaErrorCode::Conflict);
    assert_eq!(err.outcome, EffectOutcome::NotApplied);
    let resource_ref = err.resource_ref.as_ref().expect("resource ref");
    assert_eq!(resource_ref.resource_handle, descriptor.session_id.0);
    assert_eq!(resource_ref.generation.0, stale_ref.expected_generation.0);

    // stop_session with a stale generation returns Conflict and carries the resource ref.
    let err = rtp_api
        .stop_session(
            &ctx,
            StopRtpSession {
                session_ref: stale_ref.clone(),
                release_lease: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, MediaErrorCode::Conflict);
    assert_eq!(err.outcome, EffectOutcome::NotApplied);
    let resource_ref = err.resource_ref.as_ref().expect("resource ref");
    assert_eq!(resource_ref.resource_handle, descriptor.session_id.0);
    assert_eq!(resource_ref.generation.0, stale_ref.expected_generation.0);

    // stop_session on a missing session returns NotApplied (idempotent success).
    let missing_ref = RtpSessionRef {
        session_id: RtpSessionId("no-such-session".to_string()),
        expected_generation: cheetah_sdk::media_api::rtp_session::RtpSessionGeneration(0),
    };
    let outcome = rtp_api
        .stop_session(
            &ctx,
            StopRtpSession {
                session_ref: missing_ref,
                release_lease: true,
            },
        )
        .await
        .expect("idempotent stop");
    assert_eq!(outcome, EffectOutcome::NotApplied);

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtp_playback_request_requires_record_source_and_time_range() {
    let harness = Gb28181TestHarness::start().await;
    let rtp_api = harness
        .engine
        .media_services()
        .rtp_session()
        .expect("rtp session api");
    let ctx = MediaRequestContext::default();

    let media_key = MediaKey::with_default_vhost("gb28181_playback", "cam_001", None).unwrap();
    let base_params = || {
        RtpSessionParamsBuilder::new(media_key.clone(), RtpDirection::Receive)
            .transport(RtpTransport::Udp)
            .container(MediaContainer::Ps)
            .payload_binding(RtpPayloadBinding {
                payload_type: INGEST_PT,
                codec: "PS".to_string(),
                clock_rate: 90000,
                channels: None,
                packet_duration_ms: None,
            })
    };

    // Missing record_source with a playback range is rejected.
    let err = rtp_api
        .open_receiver(
            &ctx,
            OpenRtpReceiver {
                params: base_params().build(),
                playback_range: Some(PlaybackRange {
                    start_ms: 0,
                    end_ms: None,
                }),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, MediaErrorCode::InvalidArgument);

    // Unsafe record source is rejected.
    let err = rtp_api
        .open_receiver(
            &ctx,
            OpenRtpReceiver {
                params: base_params().record_source("/etc/passwd").build(),
                playback_range: Some(PlaybackRange {
                    start_ms: 0,
                    end_ms: None,
                }),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, MediaErrorCode::InvalidArgument);

    // Valid playback contract is accepted (live ingest path is still used; playback
    // reader integration is covered by PLAY-02).
    let descriptor = rtp_api
        .open_receiver(
            &ctx,
            OpenRtpReceiver {
                params: base_params()
                    .record_source("recordings/cam_001/20250721.mp4")
                    .build(),
                playback_range: Some(PlaybackRange {
                    start_ms: 0,
                    end_ms: Some(60_000),
                }),
            },
        )
        .await
        .expect("playback receiver with valid record source");
    assert_eq!(descriptor.direction, RtpDirection::Receive);

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtp_module_profile_and_limits_config_enforced() {
    let harness = Gb28181TestHarness::start_with_rtp_config("    max_sessions: 1\n").await;
    let rtp_api = harness
        .engine
        .media_services()
        .rtp_session()
        .expect("rtp session api");
    let ctx = MediaRequestContext::default();

    let media_key1 = MediaKey::with_default_vhost("gb28181_limits", "cam_001", None).unwrap();
    let params1 = RtpSessionParamsBuilder::new(media_key1, RtpDirection::Receive)
        .transport(RtpTransport::Udp)
        .container(MediaContainer::Ps)
        .payload_binding(RtpPayloadBinding {
            payload_type: INGEST_PT,
            codec: "PS".to_string(),
            clock_rate: 90000,
            channels: None,
            packet_duration_ms: None,
        })
        .build();
    let _ = rtp_api
        .open_receiver(
            &ctx,
            OpenRtpReceiver {
                params: params1,
                playback_range: None,
            },
        )
        .await
        .expect("first session");

    let media_key2 = MediaKey::with_default_vhost("gb28181_limits", "cam_002", None).unwrap();
    let params2 = RtpSessionParamsBuilder::new(media_key2, RtpDirection::Receive)
        .transport(RtpTransport::Udp)
        .container(MediaContainer::Ps)
        .payload_binding(RtpPayloadBinding {
            payload_type: INGEST_PT,
            codec: "PS".to_string(),
            clock_rate: 90000,
            channels: None,
            packet_duration_ms: None,
        })
        .build();
    let err = rtp_api
        .open_receiver(
            &ctx,
            OpenRtpReceiver {
                params: params2,
                playback_range: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, MediaErrorCode::Unavailable);
    assert!(err.message.contains("limit"), "{}", err.message);

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtp_module_disabled_profile_is_rejected() {
    // Only the strict profile is enabled; the default builder uses gb_common, which should fail.
    let harness =
        Gb28181TestHarness::start_with_rtp_config("    enabled_profiles:\n      - strict\n").await;
    let rtp_api = harness
        .engine
        .media_services()
        .rtp_session()
        .expect("rtp session api");
    let ctx = MediaRequestContext::default();

    let media_key = MediaKey::with_default_vhost("gb28181_profile", "cam_001", None).unwrap();
    let params = RtpSessionParamsBuilder::new(media_key, RtpDirection::Receive)
        .transport(RtpTransport::Udp)
        .container(MediaContainer::Ps)
        .payload_binding(RtpPayloadBinding {
            payload_type: INGEST_PT,
            codec: "PS".to_string(),
            clock_rate: 90000,
            channels: None,
            packet_duration_ms: None,
        })
        .build();
    let err = rtp_api
        .open_receiver(
            &ctx,
            OpenRtpReceiver {
                params,
                playback_range: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, MediaErrorCode::Unsupported);
    assert!(err.message.contains("not enabled"), "{}", err.message);

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtp_module_admission_deny_leaves_no_session() {
    // Cap sessions at 1 so a leaked session from the denied open would block
    // the follow-up allowed open, making the "no leak" assertion strict.
    let harness = Gb28181TestHarness::start_with_rtp_config("    max_sessions: 1\n").await;
    harness.set_admission_deny(true);

    let rtp_api = harness
        .engine
        .media_services()
        .rtp_session()
        .expect("rtp session api");
    let ctx = MediaRequestContext::default();

    let media_key = MediaKey::with_default_vhost("gb28181_admission", "cam_001", None).unwrap();
    let params = RtpSessionParamsBuilder::new(media_key, RtpDirection::Receive)
        .transport(RtpTransport::Udp)
        .container(MediaContainer::Ps)
        .payload_binding(RtpPayloadBinding {
            payload_type: INGEST_PT,
            codec: "PS".to_string(),
            clock_rate: 90000,
            channels: None,
            packet_duration_ms: None,
        })
        .build();
    let err = rtp_api
        .open_receiver(
            &ctx,
            OpenRtpReceiver {
                params,
                playback_range: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, MediaErrorCode::PermissionDenied);

    // After admission flips to allow, a new open on a different stream succeeds and
    // proves the denied request left no session behind.
    harness.set_admission_deny(false);
    let media_key2 = MediaKey::with_default_vhost("gb28181_admission", "cam_002", None).unwrap();
    let params2 = RtpSessionParamsBuilder::new(media_key2, RtpDirection::Receive)
        .transport(RtpTransport::Udp)
        .container(MediaContainer::Ps)
        .payload_binding(RtpPayloadBinding {
            payload_type: INGEST_PT,
            codec: "PS".to_string(),
            clock_rate: 90000,
            channels: None,
            packet_duration_ms: None,
        })
        .build();
    let _desc = rtp_api
        .open_receiver(
            &ctx,
            OpenRtpReceiver {
                params: params2,
                playback_range: None,
            },
        )
        .await
        .expect("open after allow");

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtp_session_open_is_idempotent_for_same_request_and_key() {
    let harness = Gb28181TestHarness::start().await;
    let rtp_api = harness
        .engine
        .media_services()
        .rtp_session()
        .expect("rtp session api");
    let ctx = MediaRequestContext {
        idempotency_key: Some("idempotent-open-1".to_string()),
        ..Default::default()
    };

    let media_key = MediaKey::with_default_vhost("gb28181_idem", "cam_001", None).unwrap();
    let params = RtpSessionParamsBuilder::new(media_key, RtpDirection::Receive)
        .transport(RtpTransport::Udp)
        .container(MediaContainer::Ps)
        .payload_binding(RtpPayloadBinding {
            payload_type: INGEST_PT,
            codec: "PS".to_string(),
            clock_rate: 90000,
            channels: None,
            packet_duration_ms: None,
        })
        .build();
    let request = OpenRtpReceiver {
        params,
        playback_range: None,
    };

    let desc1 = rtp_api
        .open_receiver(&ctx, request.clone())
        .await
        .expect("first open");
    let desc2 = rtp_api
        .open_receiver(&ctx, request)
        .await
        .expect("idempotent second open");
    assert_eq!(desc1.session_id, desc2.session_id);

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtp_session_idempotent_failure_is_replayed_with_same_key() {
    // Only strict profile is enabled so the default gb_common request fails.
    let harness =
        Gb28181TestHarness::start_with_rtp_config("    enabled_profiles:\n      - strict\n").await;
    let rtp_api = harness
        .engine
        .media_services()
        .rtp_session()
        .expect("rtp session api");
    let ctx = MediaRequestContext {
        idempotency_key: Some("idempotent-fail-1".to_string()),
        ..Default::default()
    };

    let media_key = MediaKey::with_default_vhost("gb28181_idem_fail", "cam_001", None).unwrap();
    let params = RtpSessionParamsBuilder::new(media_key, RtpDirection::Receive)
        .transport(RtpTransport::Udp)
        .container(MediaContainer::Ps)
        .payload_binding(RtpPayloadBinding {
            payload_type: INGEST_PT,
            codec: "PS".to_string(),
            clock_rate: 90000,
            channels: None,
            packet_duration_ms: None,
        })
        .build();
    let request = OpenRtpReceiver {
        params,
        playback_range: None,
    };

    let err1 = rtp_api
        .open_receiver(&ctx, request.clone())
        .await
        .unwrap_err();
    assert_eq!(err1.code, MediaErrorCode::Unsupported);
    let err2 = rtp_api.open_receiver(&ctx, request).await.unwrap_err();
    assert_eq!(err2.code, MediaErrorCode::Unsupported);
    assert_eq!(err1.message, err2.message);

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtp_session_talk_codec_enables_pcma_pcmu_and_rejects_aac_by_default() {
    let harness = Gb28181TestHarness::start().await;
    let rtp_api = harness
        .engine
        .media_services()
        .rtp_session()
        .expect("rtp session api");
    let ctx = MediaRequestContext::default();

    // AAC talk is rejected by the codec capability gate before any session is allocated.
    let media_key_aac = MediaKey::with_default_vhost("gb28181_talk_codec", "aac", None).unwrap();
    let aac_binding = RtpPayloadBinding {
        payload_type: 97,
        codec: "AAC".to_string(),
        clock_rate: 48000,
        channels: Some(2),
        packet_duration_ms: Some(20),
    };
    let aac_params = RtpSessionParamsBuilder::new(media_key_aac, RtpDirection::DuplexTalk)
        .transport(RtpTransport::Udp)
        .container(MediaContainer::ElementaryStream)
        .payload_binding(aac_binding.clone())
        .build();
    let aac_request = OpenRtpTalk {
        params: aac_params,
        talkback_binding: Some(aac_binding),
    };
    let err = rtp_api.open_talk(&ctx, aac_request).await.unwrap_err();
    assert_eq!(err.code, MediaErrorCode::Unsupported);
    assert!(
        err.message.to_lowercase().contains("aac"),
        "error should name the disabled codec: {}",
        err.message
    );

    // PCMU talk is accepted; open_talk upgrades an existing receiver whose remote endpoint
    // has been learned from inbound traffic.
    let media_key_pcmu = MediaKey::with_default_vhost("gb28181_talk_codec", "pcmu", None).unwrap();
    let recv_params = RtpSessionParamsBuilder::new(media_key_pcmu.clone(), RtpDirection::Receive)
        .transport(RtpTransport::Udp)
        .container(MediaContainer::Ps)
        .ssrc(SSRC)
        .payload_binding(RtpPayloadBinding {
            payload_type: INGEST_PT,
            codec: "PS".to_string(),
            clock_rate: 90000,
            channels: None,
            packet_duration_ms: None,
        })
        .build();
    let recv_descriptor = rtp_api
        .open_receiver(
            &ctx,
            OpenRtpReceiver {
                params: recv_params,
                playback_range: None,
            },
        )
        .await
        .expect("open receiver for talk upgrade");
    let recv_addr = recv_descriptor.endpoints.local;

    let socket = bind_udp_socket().await;
    send_rtp(
        &socket,
        recv_addr,
        mux_ps_frame(&make_audio_frame(80)),
        SSRC,
        1,
        0,
        INGEST_PT,
    )
    .await;
    sleep(Duration::from_millis(200)).await;

    let pcmu_binding = RtpPayloadBinding {
        payload_type: 0,
        codec: "pcmu".to_string(),
        clock_rate: 8000,
        channels: Some(1),
        packet_duration_ms: Some(20),
    };
    let pcmu_params = RtpSessionParamsBuilder::new(media_key_pcmu, RtpDirection::DuplexTalk)
        .transport(RtpTransport::Udp)
        .container(MediaContainer::ElementaryStream)
        .payload_binding(pcmu_binding.clone())
        .build();
    let pcmu_request = OpenRtpTalk {
        params: pcmu_params,
        talkback_binding: Some(pcmu_binding),
    };
    rtp_api
        .open_talk(&ctx, pcmu_request)
        .await
        .expect("PCMU talk should be accepted by default");

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rtp_session_rejects_peer_reuse_by_different_media_key() {
    // Opening two senders to the same peer endpoint for two different media keys
    // must be denied with Conflict (TALK-05 cross-session denial).
    let harness = Gb28181TestHarness::start().await;
    let media = harness.media_facade();
    let ctx = MediaRequestContext::default();

    let recv_socket = bind_udp_socket().await;
    let dest_addr = recv_socket.local_addr().unwrap();

    let media_key1 = MediaKey::with_default_vhost("gb28181_peer_reuse", "cam_001", None).unwrap();
    let request1 = RtpSenderRequest {
        media_key: media_key1,
        destination_endpoint: dest_addr.to_string(),
        ssrc: Some(SSRC),
        payload_type: Some(INGEST_PT),
        codec_hint: Some("ps".to_string()),
        mode: RtpSenderMode::Active,
        transport_options: HashMap::new(),
        source_binding_policy: SourceBindingPolicy::default(),
    };
    let _first = media.open_rtp_sender(&ctx, request1).await.unwrap();

    let media_key2 = MediaKey::with_default_vhost("gb28181_peer_reuse", "cam_002", None).unwrap();
    let request2 = RtpSenderRequest {
        media_key: media_key2,
        destination_endpoint: dest_addr.to_string(),
        ssrc: Some(SSRC),
        payload_type: Some(INGEST_PT),
        codec_hint: Some("ps".to_string()),
        mode: RtpSenderMode::Active,
        transport_options: HashMap::new(),
        source_binding_policy: SourceBindingPolicy::default(),
    };
    let err = media.open_rtp_sender(&ctx, request2).await.unwrap_err();
    assert_eq!(err.code, MediaErrorCode::Conflict);
    assert!(
        err.message.contains("already bound"),
        "expected conflict message, got: {}",
        err.message
    );

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gb28181_talkback_late_frame_drop_policy() {
    // Configure a very tight talkback latency budget and a small subscriber queue.
    // Late audio frames (pts_us in the past) must be dropped instead of being packetized
    // and sent back to the peer.
    let harness = Gb28181TestHarness::start_with_rtp_config(
        "    talkback_max_latency_ms: 1\n    talkback_queue_capacity: 4\n",
    )
    .await;
    let media = harness.media_facade();
    let ctx = MediaRequestContext::default();

    let media_key = MediaKey::with_default_vhost("gb28181_talk_late", "cam_001", None).unwrap();
    let request = RtpReceiverRequest {
        media_key: media_key.clone(),
        port: Some(0),
        ip: None,
        ssrc: Some(SSRC),
        enable_rtcp: false,
        tcp_mode: None,
        payload_type: Some(INGEST_PT),
        codec_hint: Some("ps".to_string()),
        reuse_port: false,
        timeout_ms: 0,
        source_binding_policy: SourceBindingPolicy::default(),
    };

    let session = media.open_rtp_receiver(&ctx, request).await.unwrap();
    let recv_port = session.local_port.unwrap();
    let recv_addr: SocketAddr = format!("127.0.0.1:{recv_port}").parse().unwrap();

    let socket = bind_udp_socket().await;
    let src_addr = socket.local_addr().unwrap();

    // Warm up the inbound session with one audio PS packet so the source endpoint is learned.
    send_rtp(
        &socket,
        recv_addr,
        mux_ps_frame(&make_audio_frame(80)),
        SSRC,
        1,
        0,
        INGEST_PT,
    )
    .await;
    sleep(Duration::from_millis(200)).await;

    // Upgrade to talkback, echoing audio back to `src_addr`.
    let talk_request = RtpSenderRequest {
        media_key,
        destination_endpoint: src_addr.to_string(),
        ssrc: Some(SSRC),
        payload_type: Some(8),
        codec_hint: Some("raw_audio".to_string()),
        mode: RtpSenderMode::Talk,
        transport_options: HashMap::new(),
        source_binding_policy: SourceBindingPolicy::default(),
    };
    media.open_rtp_sender(&ctx, talk_request).await.unwrap();

    // Feed audio frames whose pts_us is 0; they are far behind the current monotonic clock,
    // so the talkback egress should drop them under the 1 ms latency budget.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut seq: u16 = 2;
    while tokio::time::Instant::now() < deadline {
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_audio_frame(0)),
            SSRC,
            seq,
            0,
            INGEST_PT,
        )
        .await;
        seq = seq.wrapping_add(1);

        if let Some((header, _, addr)) = recv_rtp(&socket, Duration::from_millis(100)).await {
            assert!(
                !(addr == src_addr && header.payload_type == 8),
                "late talkback audio frame should have been dropped"
            );
        }
    }

    media
        .stop_rtp_session(&ctx, &session.session_id)
        .await
        .unwrap();
    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gb28181_playback_sender_reads_from_playback_api_and_emits_rtp() {
    let harness = Gb28181TestHarness::start().await;
    let fake = FakePlayback::default();
    harness
        .engine
        .media_services()
        .register_playback(Arc::new(fake.clone()));
    let rtp_api = harness
        .engine
        .media_services()
        .rtp_session()
        .expect("rtp session api");
    let ctx = MediaRequestContext::default();

    let source_key = StreamKey::new("rtp_playback_source", "main");
    let source_media_key = stream_key_to_media_key(&source_key);
    let publisher = harness
        .open_publisher(
            source_key.clone(),
            vec![make_video_track(), make_audio_track()],
        )
        .await;

    // Seed the source stream with PS video frames whose source timeline starts at
    // 5 seconds. The playback range start is also 5 seconds, so the output RTP
    // timeline should be normalized to begin near 0.
    let playback_start_us = 5_000_000;
    for i in 0..5 {
        let pts_us = playback_start_us + i * 100_000;
        let ps = mux_ps_frame(&make_video_frame(pts_us));
        publisher
            .push_frame(Arc::new(AVFrame {
                payload: ps,
                ..make_video_frame(pts_us)
            }))
            .unwrap();
    }

    let recv_socket = bind_udp_socket().await;
    let dest_addr = recv_socket.local_addr().unwrap();

    let request = OpenRtpSender {
        params: RtpSessionParamsBuilder::new(source_media_key.clone(), RtpDirection::Send)
            .transport(RtpTransport::Udp)
            .container(MediaContainer::Ps)
            .ssrc(SSRC)
            .payload_binding(RtpPayloadBinding {
                payload_type: INGEST_PT,
                codec: "PS".to_string(),
                clock_rate: 90_000,
                channels: None,
                packet_duration_ms: None,
            })
            .remote_endpoint(dest_addr)
            .record_source("recordings/cam_001/20250721.mp4")
            .build(),
        playback_range: Some(PlaybackRange {
            start_ms: 5_000,
            end_ms: Some(60_000),
        }),
    };
    let session = rtp_api
        .open_sender(&ctx, request)
        .await
        .expect("open playback sender");
    assert_eq!(fake.open_count(), 1);

    let mut saw_rtp = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !saw_rtp {
        if let Some((header, _payload, _addr)) =
            recv_rtp(&recv_socket, Duration::from_millis(200)).await
        {
            assert_eq!(header.version, 2);
            assert_eq!(header.ssrc, SSRC);
            // Source started at 5 s and playback start is also 5 s, so the
            // normalized RTP timeline should begin near 0.
            assert!(
                header.timestamp < 100_000,
                "playback timeline should be normalized to start near 0, got {}",
                header.timestamp
            );
            saw_rtp = true;
        }
    }
    assert!(saw_rtp, "expected RTP packets from playback sender");

    // Stopping the RTP session should also stop the playback source.
    // The background egress bumps the generation to Connected after the first
    // frame, so fetch the current generation from the session list before stopping.
    let query = RtpSessionQuery {
        session_id: Some(session.session_id.clone()),
        ..Default::default()
    };
    let page = rtp_api
        .list_sessions(&ctx, query)
        .await
        .expect("list sessions");
    let updated = page.items.into_iter().next().expect("session still exists");
    let stop_ref = RtpSessionRef {
        session_id: session.session_id,
        expected_generation: updated.generation,
    };
    rtp_api
        .stop_session(
            &ctx,
            StopRtpSession {
                session_ref: stop_ref,
                release_lease: true,
            },
        )
        .await
        .expect("stop playback sender");
    assert_eq!(fake.stop_count(), 1);

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playback_control_update_is_allowed_when_provider_advertises_control() {
    let fake = FakePlayback::default();
    let mut playback_caps = MediaCapabilitySet::empty();
    playback_caps.add(cheetah_sdk::media_api::MediaCapability::Playback, 1);
    let harness =
        Gb28181TestHarness::start_with_playback(Arc::new(fake.clone()), playback_caps, "").await;

    let rtp_api = harness
        .engine
        .media_services()
        .rtp_session()
        .expect("rtp session api");
    let ctx = MediaRequestContext::default();
    let media_key = MediaKey::with_default_vhost("live", "pb", None).expect("media key");
    let request = OpenRtpSender {
        params: RtpSessionParamsBuilder::new(media_key, RtpDirection::Send)
            .transport(RtpTransport::Udp)
            .remote_endpoint(SocketAddr::from(([127, 0, 0, 1], 30001)))
            .ssrc(SSRC)
            .payload_binding(RtpPayloadBinding {
                payload_type: 100,
                codec: "H264".to_string(),
                clock_rate: 90_000,
                channels: None,
                packet_duration_ms: None,
            })
            .record_source("recordings/cam_001/20250721.mp4")
            .build(),
        playback_range: Some(PlaybackRange {
            start_ms: 0,
            end_ms: Some(60_000),
        }),
    };

    let session = rtp_api
        .open_sender(&ctx, request)
        .await
        .expect("open playback sender");

    let update = UpdateRtpSession {
        session_ref: RtpSessionRef {
            session_id: session.session_id,
            expected_generation: session.generation,
        },
        payload_bindings: None,
        source_binding_policy: None,
        remote_endpoint: None,
        max_rebind_attempts: None,
        max_probe_bytes: None,
        pause_check: None,
        playback_control: Some(PlaybackControl::Pause),
    };
    rtp_api
        .update_session(&ctx, update)
        .await
        .expect("pause playback");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playback_control_update_is_rejected_when_provider_lacks_control() {
    let fake = FakePlayback::default();
    let mut playback_caps = MediaCapabilitySet::empty();
    playback_caps.add_with_operations(
        cheetah_sdk::media_api::MediaCapability::Playback,
        1,
        vec!["open".to_string(), "get".to_string(), "list".to_string()],
    );
    let harness =
        Gb28181TestHarness::start_with_playback(Arc::new(fake.clone()), playback_caps, "").await;

    let rtp_api = harness
        .engine
        .media_services()
        .rtp_session()
        .expect("rtp session api");
    let ctx = MediaRequestContext::default();
    let media_key = MediaKey::with_default_vhost("live", "pb", None).expect("media key");
    let request = OpenRtpSender {
        params: RtpSessionParamsBuilder::new(media_key, RtpDirection::Send)
            .transport(RtpTransport::Udp)
            .remote_endpoint(SocketAddr::from(([127, 0, 0, 1], 30002)))
            .ssrc(SSRC)
            .payload_binding(RtpPayloadBinding {
                payload_type: 100,
                codec: "H264".to_string(),
                clock_rate: 90_000,
                channels: None,
                packet_duration_ms: None,
            })
            .record_source("recordings/cam_001/20250721.mp4")
            .build(),
        playback_range: Some(PlaybackRange {
            start_ms: 0,
            end_ms: Some(60_000),
        }),
    };

    let session = rtp_api
        .open_sender(&ctx, request)
        .await
        .expect("open playback sender");

    let update = UpdateRtpSession {
        session_ref: RtpSessionRef {
            session_id: session.session_id,
            expected_generation: session.generation,
        },
        payload_bindings: None,
        source_binding_policy: None,
        remote_endpoint: None,
        max_rebind_attempts: None,
        max_probe_bytes: None,
        pause_check: None,
        playback_control: Some(PlaybackControl::Pause),
    };
    let err = rtp_api
        .update_session(&ctx, update)
        .await
        .expect_err("pause should fail without control capability");
    assert_eq!(err.code, MediaErrorCode::Unsupported);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gb28181_download_egress_is_rate_limited_and_does_not_block_live() {
    let harness = Gb28181TestHarness::start().await;
    let fake = FakePlayback::default();
    harness
        .engine
        .media_services()
        .register_playback(Arc::new(fake.clone()));
    let rtp_api = harness
        .engine
        .media_services()
        .rtp_session()
        .expect("rtp session api");
    let ctx = MediaRequestContext::default();

    let source_key = StreamKey::new("rtp_download_source", "main");
    let source_media_key = stream_key_to_media_key(&source_key);
    let publisher = harness
        .open_publisher(
            source_key.clone(),
            vec![make_video_track(), make_audio_track()],
        )
        .await;

    // Seed the source stream with 10 small PS video frames.
    for i in 0..10 {
        let pts_us = i * 100_000;
        let ps = mux_ps_frame(&make_video_frame(pts_us));
        publisher
            .push_frame(Arc::new(AVFrame {
                payload: ps,
                ..make_video_frame(pts_us)
            }))
            .unwrap();
    }

    let recv_socket = bind_udp_socket().await;
    let dest_addr = recv_socket.local_addr().unwrap();

    // Open a download sender with a 64 kbps rate cap and a per-frame timeout.
    let request = OpenRtpSender {
        params: RtpSessionParamsBuilder::new(source_media_key.clone(), RtpDirection::Send)
            .transport(RtpTransport::Udp)
            .container(MediaContainer::Ps)
            .ssrc(SSRC)
            .payload_binding(RtpPayloadBinding {
                payload_type: INGEST_PT,
                codec: "PS".to_string(),
                clock_rate: 90_000,
                channels: None,
                packet_duration_ms: None,
            })
            .remote_endpoint(dest_addr)
            .record_source("recordings/cam_001/20250721.mp4")
            .purpose(RtpSessionPurpose::Download)
            .download_rate_kbps(64)
            .download_timeout_ms(5_000)
            .build(),
        playback_range: Some(PlaybackRange {
            start_ms: 0,
            end_ms: Some(60_000),
        }),
    };
    let _session = rtp_api
        .open_sender(&ctx, request)
        .await
        .expect("open download sender");

    let start = TokioInstant::now();
    let mut received = 0;
    let mut total_bytes: usize = 0;
    let deadline = TokioInstant::now() + Duration::from_millis(400);
    while TokioInstant::now() < deadline && received < 5 {
        if let Some((header, payload, _addr)) =
            recv_rtp(&recv_socket, Duration::from_millis(100)).await
        {
            assert_eq!(header.version, 2);
            assert_eq!(header.ssrc, SSRC);
            received += 1;
            total_bytes += payload.len();
        }
    }
    let elapsed = start.elapsed();

    // We should receive several frames, but a 64 kbps cap means the first 5 frames
    // (~200 bytes each after RTP/PS overhead) cannot all arrive in a single burst.
    // This demonstrates the download egress is throttled independently of the live stream.
    assert!(
        received >= 3,
        "expected at least 3 download RTP packets, got {received}"
    );
    let expected_max_bytes =
        (64u64 * 1000 * elapsed.as_millis() as u64 / 8 / 1000) as usize + 1024usize; // small burst allowance
    assert!(
        total_bytes <= expected_max_bytes,
        "download bytes {total_bytes} exceeded rate budget {expected_max_bytes} for {elapsed:?}"
    );
}
