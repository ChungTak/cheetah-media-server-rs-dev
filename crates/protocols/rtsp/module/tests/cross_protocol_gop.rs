use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use cheetah_codec::RtpPacket;
use cheetah_config::ConfigStore;
use cheetah_engine::{Engine, EngineBuilder};
use cheetah_rtmp_core::{RtmpClientState, RtmpMessageStreamId, RtmpUrl};
use cheetah_rtmp_driver_tokio::{
    start_client, ClientDriverEvent, RtmpClientDriverConfig, RtmpClientHandle, RtmpClientMode,
    RtmpCoreCommand,
};
use cheetah_rtmp_module::RtmpModuleFactory;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{StreamKey, StreamManagerApi};
use tokio::time::{sleep, timeout};

mod common;
use common::*;

const RTMP_MEDIA_STREAM_ID: u32 = RtmpMessageStreamId::MEDIA.get();

fn reserve_listen_addr() -> SocketAddr {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);
    listen
}

async fn wait_for_publish_ready(client: &mut RtmpClientHandle, stage: &str) {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let now = Instant::now();
        assert!(
            now < deadline,
            "timeout waiting for rtmp publish ready at {stage}"
        );
        let remaining = deadline.saturating_duration_since(now);
        let event = timeout(remaining, client.recv_event())
            .await
            .expect("timeout waiting rtmp event")
            .expect("rtmp client event stream closed unexpectedly");
        if let ClientDriverEvent::Core {
            event: cheetah_rtmp_core::RtmpEvent::ClientStateChanged { state },
        } = event
        {
            if state == RtmpClientState::Publishing {
                return;
            }
        }
    }
}

async fn wait_for_stream_tracks(
    engine: &Engine,
    stream_key: &StreamKey,
    expected: usize,
    stage: &str,
) {
    let api: Arc<dyn StreamManagerApi> = engine.stream_manager_api();
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Ok(Some(snapshot)) = api.get_stream(stream_key).await {
            if snapshot.tracks.len() >= expected {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timeout waiting stream tracks at {stage}"
        );
        sleep(Duration::from_millis(20)).await;
    }
}

/// Minimal AVCC with one SPS (0x67, 0x42, 0x00, 0x1f) and one PPS (0x68, 0xce, 0x06, 0xe2).
/// Format: [version=1, profile=0x42, compat=0x00, level=0x1f, nal_length_size_minus1=0xff(4bytes),
///           num_sps=0xe1(1), sps_len=4, sps_data..., num_pps=1, pps_len=4, pps_data...]
fn h264_avcc_sequence_header() -> Bytes {
    let sps: &[u8] = &[0x67, 0x42, 0x00, 0x1f];
    let pps: &[u8] = &[0x68, 0xce, 0x06, 0xe2];
    let mut buf = Vec::new();
    // FLV video tag header: keyframe(1) + AVC(7) = 0x17, packet_type=0 (seq header), cts=0
    buf.push(0x17);
    buf.push(0x00);
    buf.extend_from_slice(&[0x00, 0x00, 0x00]); // CTS
                                                // AVCC box
    buf.push(0x01); // version
    buf.push(sps[1]); // profile
    buf.push(sps[2]); // compat
    buf.push(sps[3]); // level
    buf.push(0xff); // nal_length_size_minus1 = 3 (4 bytes)
    buf.push(0xe1); // num_sps = 1
    buf.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    buf.extend_from_slice(sps);
    buf.push(0x01); // num_pps = 1
    buf.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    buf.extend_from_slice(pps);
    Bytes::from(buf)
}

/// H.264 keyframe with a single IDR NALU (type 5 = 0x65), length-prefixed (4 bytes).
fn h264_keyframe_idr_only() -> Bytes {
    let idr_nalu: &[u8] = &[0x65, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
    let mut buf = Vec::new();
    // FLV video tag header: keyframe(1) + AVC(7) = 0x17, packet_type=1 (NALU), cts=0
    buf.push(0x17);
    buf.push(0x01);
    buf.extend_from_slice(&[0x00, 0x00, 0x00]); // CTS
                                                // Length-prefixed NALU
    buf.extend_from_slice(&(idr_nalu.len() as u32).to_be_bytes());
    buf.extend_from_slice(idr_nalu);
    Bytes::from(buf)
}

fn aac_sequence_header_48k_stereo() -> Bytes {
    // AF 00 = AAC, sequence header; 11 90 = ASC (AAC-LC, 48kHz, stereo)
    Bytes::from_static(&[0xaf, 0x00, 0x11, 0x90])
}

fn aac_raw_payload() -> Bytes {
    Bytes::from_static(&[0xaf, 0x01, 0x12, 0x10])
}

/// Returns the H.264 NAL unit type from a raw NALU byte (first byte & 0x1f).
fn h264_nalu_type(nalu: &[u8]) -> u8 {
    nalu.first().map(|b| b & 0x1f).unwrap_or(0)
}

/// Extract NAL units from an RTP packet payload (handles single NAL, STAP-A, FU-A).
fn extract_nalu_types_from_rtp(payload: &[u8]) -> Vec<u8> {
    if payload.is_empty() {
        return vec![];
    }
    let nal_type = payload[0] & 0x1f;
    match nal_type {
        // STAP-A: aggregation packet
        24 => {
            let mut types = vec![];
            let mut offset = 1;
            while offset + 2 <= payload.len() {
                let size = u16::from_be_bytes([payload[offset], payload[offset + 1]]) as usize;
                offset += 2;
                if offset + size <= payload.len() {
                    types.push(h264_nalu_type(&payload[offset..offset + size]));
                }
                offset += size;
            }
            types
        }
        // FU-A: fragmentation unit
        28 => {
            if payload.len() >= 2 {
                let fu_header = payload[1];
                let start = fu_header & 0x80 != 0;
                if start {
                    vec![fu_header & 0x1f]
                } else {
                    vec![]
                }
            } else {
                vec![]
            }
        }
        // Single NAL unit
        _ => vec![nal_type],
    }
}

/// Verifies that RTMP→RTSP cross-protocol play correctly prepends SPS/PPS to keyframes.
#[tokio::test(flavor = "current_thread")]
async fn rtmp_to_rtsp_keyframe_contains_parameter_sets() {
    let rtsp_listen = reserve_listen_addr();
    let rtmp_listen = reserve_listen_addr();
    let stream_name = "bridge-gop-ps";
    let rtsp_uri = format!("rtsp://{rtsp_listen}/live/{stream_name}");
    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/{stream_name}"))
        .expect("parse rtmp url");

    let config = Arc::new(ConfigStore::new());
    let config_yaml = format!(
        "modules:\n  rtmp:\n    listen: \"{rtmp_listen}\"\n  rtsp:\n    listen: \"{rtsp_listen}\"\n"
    );
    config.load_yaml_str(&config_yaml).expect("load config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime.clone())
        .register_module_factory(Arc::new(RtmpModuleFactory))
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    // --- RTMP publish with proper sequence header ---
    let mut rtmp_publisher = start_client(
        runtime,
        rtmp_url,
        RtmpClientMode::Publish,
        RtmpClientDriverConfig::default(),
        cheetah_sdk::CancellationToken::new(),
    )
    .expect("start rtmp publish client");
    wait_for_publish_ready(&mut rtmp_publisher, "gop-ps").await;

    let tx = rtmp_publisher.core_command_sender();

    // Send video sequence header (AVCC with SPS/PPS)
    tx.send_core(RtmpCoreCommand::SendVideo {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 0,
        payload: h264_avcc_sequence_header(),
    })
    .await
    .expect("send avcc sequence header");

    // Send audio sequence header
    tx.send_core(RtmpCoreCommand::SendAudio {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 0,
        payload: aac_sequence_header_48k_stereo(),
    })
    .await
    .expect("send aac sequence header");

    // Send keyframe (IDR only, no inline SPS/PPS)
    tx.send_core(RtmpCoreCommand::SendVideo {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 0,
        payload: h264_keyframe_idr_only(),
    })
    .await
    .expect("send h264 keyframe");

    // Send audio frame
    tx.send_core(RtmpCoreCommand::SendAudio {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 0,
        payload: aac_raw_payload(),
    })
    .await
    .expect("send aac frame");

    // Wait for tracks to be registered
    wait_for_stream_tracks(&engine, &StreamKey::new("live", stream_name), 2, "gop-ps").await;

    // --- RTSP DESCRIBE: verify SDP contains sprop-parameter-sets ---
    let mut player = connect_with_retry(rtsp_listen).await;
    let describe = build_request("DESCRIBE", &rtsp_uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "GOP-PS-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);

    let sdp_body = std::str::from_utf8(&describe_resp.body).expect("sdp utf8");
    assert!(
        sdp_body.contains("sprop-parameter-sets="),
        "SDP must contain sprop-parameter-sets from RTMP AVCC; got:\n{sdp_body}"
    );

    let session = describe_resp
        .header("Session")
        .expect("session header")
        .to_string();

    // --- SETUP video track with TCP interleaved ---
    let setup_video = build_request(
        "SETUP",
        &format!("{rtsp_uri}/trackID=0"),
        2,
        Some(&session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut player, &setup_video).await;
    let setup_resp = read_response(&mut player, "GOP-PS-SETUP-VIDEO").await;
    assert_eq!(setup_resp.status_code, 200);

    // --- PLAY ---
    let play = build_request("PLAY", &rtsp_uri, 3, Some(&session), &[], &[]);
    write_request(&mut player, &play).await;
    let play_resp = read_response(&mut player, "GOP-PS-PLAY").await;
    assert_eq!(play_resp.status_code, 200);

    // Send another keyframe to trigger bootstrap delivery
    tx.send_core(RtmpCoreCommand::SendVideo {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 100,
        payload: h264_keyframe_idr_only(),
    })
    .await
    .expect("send second keyframe");

    // --- Receive RTP packets and verify parameter sets are present ---
    let mut found_sps = false;
    let mut found_pps = false;
    let mut found_idr = false;

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !(found_sps && found_pps && found_idr) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let result = timeout(
            remaining.min(Duration::from_millis(500)),
            read_interleaved_frame(&mut player, "GOP-PS-RTP"),
        )
        .await;
        let Ok((channel, payload)) = result else {
            break;
        };
        if channel != 0 {
            continue;
        }
        let Some(pkt) = RtpPacket::parse(&payload) else {
            continue;
        };
        let nalu_types = extract_nalu_types_from_rtp(pkt.payload.as_ref());
        for nt in nalu_types {
            match nt {
                7 => found_sps = true, // SPS
                8 => found_pps = true, // PPS
                5 => found_idr = true, // IDR
                _ => {}
            }
        }
    }

    assert!(
        found_sps,
        "RTSP play must receive SPS in RTP stream (prepended to keyframe from RTMP source)"
    );
    assert!(
        found_pps,
        "RTSP play must receive PPS in RTP stream (prepended to keyframe from RTMP source)"
    );
    assert!(
        found_idr,
        "RTSP play must receive IDR keyframe in RTP stream"
    );

    // Cleanup
    let teardown = build_request("TEARDOWN", &rtsp_uri, 4, Some(&session), &[], &[]);
    write_request(&mut player, &teardown).await;
    let _ = read_response(&mut player, "GOP-PS-TEARDOWN").await;

    rtmp_publisher.shutdown();
    let _ = rtmp_publisher.wait().await;
    engine.stop().await;
}

/// Verifies that RTSP→RTMP cross-protocol play correctly generates AVCC sequence header
/// from SDP sprop-parameter-sets when the source is RTSP.
#[tokio::test(flavor = "current_thread")]
async fn rtsp_to_rtmp_generates_avcc_from_sdp_parameter_sets() {
    use cheetah_rtmp_core::{RtmpEvent, RtmpMediaType, RtmpUrl};
    use cheetah_rtmp_driver_tokio::{ClientDriverEvent, RtmpClientDriverConfig, RtmpClientMode};

    let rtsp_listen = reserve_listen_addr();
    let rtmp_listen = reserve_listen_addr();
    let stream_name = "bridge-rtsp-rtmp-avcc";
    let rtsp_uri = format!("rtsp://{rtsp_listen}/live/{stream_name}");
    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/{stream_name}"))
        .expect("parse rtmp url");

    let config = Arc::new(ConfigStore::new());
    let config_yaml = format!(
        "modules:\n  rtmp:\n    listen: \"{rtmp_listen}\"\n  rtsp:\n    listen: \"{rtsp_listen}\"\n"
    );
    config.load_yaml_str(&config_yaml).expect("load config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime.clone())
        .register_module_factory(Arc::new(RtmpModuleFactory))
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    // --- RTSP publish with SDP containing sprop-parameter-sets ---
    let mut publisher = connect_with_retry(rtsp_listen).await;

    // SDP with H264 sprop-parameter-sets (SPS=0x67,0x42,0x00,0x1f PPS=0x68,0xce,0x06,0xe2)
    let sdp = "v=0\r\n\
               o=- 0 0 IN IP4 127.0.0.1\r\n\
               s=test\r\n\
               t=0 0\r\n\
               m=video 0 RTP/AVP 96\r\n\
               a=rtpmap:96 H264/90000\r\n\
               a=fmtp:96 packetization-mode=1;sprop-parameter-sets=Z0IAHw==,aM4G4g==\r\n\
               a=control:trackID=0\r\n";

    let announce = build_request(
        "ANNOUNCE",
        &rtsp_uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        sdp.as_bytes(),
    );
    write_request(&mut publisher, &announce).await;
    let resp = read_response(&mut publisher, "RTSP-AVCC-ANNOUNCE").await;
    assert_eq!(resp.status_code, 200);
    let session = resp.header("Session").expect("session").to_string();

    let setup = build_request(
        "SETUP",
        &format!("{rtsp_uri}/trackID=0"),
        2,
        Some(&session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut publisher, &setup).await;
    let resp = read_response(&mut publisher, "RTSP-AVCC-SETUP").await;
    assert_eq!(resp.status_code, 200);

    let record = build_request("RECORD", &rtsp_uri, 3, Some(&session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let resp = read_response(&mut publisher, "RTSP-AVCC-RECORD").await;
    assert_eq!(resp.status_code, 200);

    // Send a keyframe via RTP (IDR only, no inline SPS/PPS)
    let rtp_pkt = cheetah_codec::RtpPacket {
        header: cheetah_codec::RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1000,
            timestamp: 90_000,
            ssrc: 0x1234_5678,
            marker: true,
        },
        payload: Bytes::from_static(&[0x65, 0xaa, 0xbb, 0xcc, 0xdd]),
    };
    let encoded = rtp_pkt.encode();
    send_interleaved_frame(&mut publisher, 0, &encoded).await;
    sleep(Duration::from_millis(100)).await;

    // --- RTMP play: verify we receive a video sequence header (AVCC) ---
    let mut player = cheetah_rtmp_driver_tokio::start_client(
        runtime.clone(),
        rtmp_url,
        RtmpClientMode::Play,
        RtmpClientDriverConfig::default(),
        cheetah_sdk::CancellationToken::new(),
    )
    .expect("start rtmp play client");

    // Wait for Playing state
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        assert!(
            Instant::now() < deadline,
            "timeout waiting for rtmp play ready"
        );
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = timeout(remaining, player.recv_event())
            .await
            .expect("timeout")
            .expect("event stream closed");
        if let ClientDriverEvent::Core {
            event: RtmpEvent::ClientStateChanged { state },
        } = event
        {
            if state == cheetah_rtmp_core::RtmpClientState::Playing {
                break;
            }
        }
    }

    // Receive media events — look for video sequence header
    let mut found_sequence_header = false;
    let mut found_coded_frame = false;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !(found_sequence_header && found_coded_frame) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Ok(event) = timeout(remaining, player.recv_event()).await else {
            break;
        };
        let Some(event) = event else { break };
        if let ClientDriverEvent::Core {
            event:
                RtmpEvent::MediaData {
                    media_type: RtmpMediaType::Video,
                    payload,
                    ..
                },
        } = event
        {
            if payload.len() >= 2 {
                if payload[0] == 0x17 && payload[1] == 0x00 {
                    // Video sequence header (keyframe + AVC + seq header)
                    found_sequence_header = true;
                    // Verify it contains AVCC data
                    assert!(
                        payload.len() > 5,
                        "sequence header payload too short: {:?}",
                        payload
                    );
                    // AVCC starts at offset 5: version byte should be 1
                    assert_eq!(
                        payload[5], 0x01,
                        "AVCC version must be 1, got {:02x}",
                        payload[5]
                    );
                } else if payload[0] == 0x17 && payload[1] == 0x01 {
                    found_coded_frame = true;
                }
            }
        }
    }

    assert!(
        found_sequence_header,
        "RTMP play must receive video sequence header (AVCC) from RTSP source with SDP parameter sets"
    );

    // Cleanup
    player.shutdown();
    let _ = player.wait().await;
    let teardown = build_request("TEARDOWN", &rtsp_uri, 4, Some(&session), &[], &[]);
    write_request(&mut publisher, &teardown).await;
    let _ = read_response(&mut publisher, "RTSP-AVCC-TEARDOWN").await;
    engine.stop().await;
}

/// Verifies H.265 RTSP publish → RTMP play generates Enhanced RTMP HEVC sequence header.
#[tokio::test(flavor = "current_thread")]
async fn rtsp_h265_to_rtmp_generates_enhanced_hevc_sequence_header() {
    use cheetah_rtmp_core::{RtmpEvent, RtmpUrl};
    use cheetah_rtmp_driver_tokio::{ClientDriverEvent, RtmpClientDriverConfig, RtmpClientMode};

    let rtsp_listen = reserve_listen_addr();
    let rtmp_listen = reserve_listen_addr();
    let stream_name = "bridge-h265-hevc";
    let rtsp_uri = format!("rtsp://{rtsp_listen}/live/{stream_name}");
    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/{stream_name}"))
        .expect("parse rtmp url");

    let config = Arc::new(ConfigStore::new());
    let config_yaml = format!(
        "modules:\n  rtmp:\n    listen: \"{rtmp_listen}\"\n  rtsp:\n    listen: \"{rtsp_listen}\"\n    track_ready_timeout_ms: 0\n"
    );
    config.load_yaml_str(&config_yaml).expect("load config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime.clone())
        .register_module_factory(Arc::new(RtmpModuleFactory))
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    // RTSP publish H.265 with SDP containing sprop-vps/sps/pps
    let mut publisher = connect_with_retry(rtsp_listen).await;
    let sdp = "v=0\r\n\
               o=- 0 0 IN IP4 127.0.0.1\r\n\
               s=test\r\n\
               t=0 0\r\n\
               m=video 0 RTP/AVP 96\r\n\
               a=rtpmap:96 H265/90000\r\n\
               a=fmtp:96 sprop-vps=QAEMAf//AUA=;sprop-sps=QgEBAUAAAAMAAAAAAw==;sprop-pps=RAHA\r\n\
               a=control:trackID=0\r\n";

    let announce = build_request(
        "ANNOUNCE",
        &rtsp_uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        sdp.as_bytes(),
    );
    write_request(&mut publisher, &announce).await;
    let resp = read_response(&mut publisher, "H265-ANNOUNCE").await;
    assert_eq!(resp.status_code, 200);
    let session = resp.header("Session").expect("session").to_string();

    let setup = build_request(
        "SETUP",
        &format!("{rtsp_uri}/trackID=0"),
        2,
        Some(&session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut publisher, &setup).await;
    let resp = read_response(&mut publisher, "H265-SETUP").await;
    assert_eq!(resp.status_code, 200);

    let record = build_request("RECORD", &rtsp_uri, 3, Some(&session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let resp = read_response(&mut publisher, "H265-RECORD").await;
    assert_eq!(resp.status_code, 200);

    // Send H.265 IDR keyframe via RTP (NAL type 19 = IDR_W_RADL)
    // H.265 NAL header is 2 bytes: (type << 1) in first byte
    let nal_type_idr: u8 = 19;
    let h265_nal_header = [(nal_type_idr << 1) | 0x01, 0x01]; // forbidden=0, type=19, layer=0, tid=1
    let mut h265_payload = Vec::new();
    h265_payload.extend_from_slice(&h265_nal_header);
    h265_payload.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee]);

    let rtp_pkt = cheetah_codec::RtpPacket {
        header: cheetah_codec::RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1000,
            timestamp: 90_000,
            ssrc: 0xabcd_ef01,
            marker: true,
        },
        payload: Bytes::from(h265_payload.clone()),
    };
    let encoded = rtp_pkt.encode();
    send_interleaved_frame(&mut publisher, 0, &encoded).await;

    // Wait for stream to be registered with tracks
    wait_for_stream_tracks(&engine, &StreamKey::new("live", stream_name), 1, "h265").await;

    // RTMP play: verify Enhanced RTMP HEVC sequence header
    let mut player = cheetah_rtmp_driver_tokio::start_client(
        runtime.clone(),
        rtmp_url,
        RtmpClientMode::Play,
        RtmpClientDriverConfig::default(),
        cheetah_sdk::CancellationToken::new(),
    )
    .expect("start rtmp play client");

    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        assert!(Instant::now() < deadline, "timeout waiting for playing");
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = timeout(remaining, player.recv_event())
            .await
            .expect("timeout")
            .expect("closed");
        if let ClientDriverEvent::Core {
            event: RtmpEvent::ClientStateChanged { state },
        } = event
        {
            if state == cheetah_rtmp_core::RtmpClientState::Playing {
                break;
            }
        }
    }

    // Send another keyframe after player is connected to ensure delivery
    let rtp_pkt2 = cheetah_codec::RtpPacket {
        header: cheetah_codec::RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1001,
            timestamp: 180_000,
            ssrc: 0xabcd_ef01,
            marker: true,
        },
        payload: Bytes::from(h265_payload),
    };
    send_interleaved_frame(&mut publisher, 0, &rtp_pkt2.encode()).await;

    // Look for Enhanced RTMP video sequence header (0x90 = enhanced + sequence start)
    // Note: The test RTMP client doesn't support Enhanced RTMP parsing, so it will
    // report a decode error when receiving the Enhanced RTMP HEVC packet. This confirms
    // the server IS sending Enhanced RTMP format for H.265.
    let mut received_enhanced_rtmp_error = false;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !received_enhanced_rtmp_error {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Ok(event) = timeout(remaining, player.recv_event()).await else {
            break;
        };
        let Some(event) = event else { break };
        if let ClientDriverEvent::Closed { reason } = &event {
            // The client fails to parse Enhanced RTMP video (frame_type=9 from 0x90 header)
            // This confirms the server sent Enhanced RTMP format
            if reason.contains("Invalid video frame type: 9") {
                received_enhanced_rtmp_error = true;
            }
        }
    }

    assert!(
        received_enhanced_rtmp_error,
        "RTMP server must send Enhanced RTMP HEVC video (0x90 header) for H.265 RTSP source, \
         which the legacy client parser rejects as 'Invalid video frame type: 9'"
    );

    player.shutdown();
    let _ = player.wait().await;
    let teardown = build_request("TEARDOWN", &rtsp_uri, 4, Some(&session), &[], &[]);
    write_request(&mut publisher, &teardown).await;
    let _ = read_response(&mut publisher, "H265-TEARDOWN").await;
    engine.stop().await;
}
