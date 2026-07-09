use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cheetah_config::ConfigStore;
use cheetah_engine::{Engine, EngineBuilder};
use cheetah_rtmp_core::{RtmpClientState, RtmpEvent, RtmpMediaType, RtmpUrl};
use cheetah_rtmp_driver_tokio::{
    start_client, ClientDriverEvent, RtmpClientDriverConfig, RtmpClientHandle, RtmpClientMode,
};
use cheetah_rtmp_module::RtmpModuleFactory;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::CancellationToken;
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::timeout;

mod common;
use common::*;

fn reserve_listen_addr() -> SocketAddr {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);
    listen
}

async fn start_bridge_engine(
    rtsp_listen: SocketAddr,
    rtmp_listen: SocketAddr,
) -> (Arc<TokioRuntime>, Engine) {
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

    (runtime, engine)
}

async fn wait_for_client_state(
    client: &mut RtmpClientHandle,
    target: RtmpClientState,
    stage: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let now = Instant::now();
        assert!(
            now < deadline,
            "timeout waiting for rtmp client state {target:?} at {stage}"
        );
        let remaining = deadline.saturating_duration_since(now);
        let event = timeout(remaining, client.recv_event())
            .await
            .expect("timeout waiting rtmp event")
            .expect("rtmp client event stream closed unexpectedly");
        if let ClientDriverEvent::Core {
            event: RtmpEvent::ClientStateChanged { state },
        } = event
        {
            if state == target {
                return;
            }
        }
    }
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
            "timeout waiting for h264 coded video media event at {stage}"
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

fn is_aac_coded_audio_payload(payload: &[u8]) -> bool {
    payload.len() > 1 && payload[0] == 0xaf && payload[1] == 0x01
}

async fn recv_aac_coded_audio_timestamp_ms(client: &mut RtmpClientHandle, stage: &str) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let now = Instant::now();
        assert!(
            now < deadline,
            "timeout waiting for aac coded audio media event at {stage}"
        );
        let remaining = deadline.saturating_duration_since(now);
        let event = timeout(remaining, client.recv_event())
            .await
            .expect("timeout waiting rtmp audio media event")
            .expect("rtmp client event stream closed unexpectedly");
        if let ClientDriverEvent::Core {
            event:
                RtmpEvent::MediaData {
                    media_type: RtmpMediaType::Audio,
                    timestamp_ms,
                    payload,
                    ..
                },
        } = event
        {
            if is_aac_coded_audio_payload(&payload) {
                return timestamp_ms;
            }
        }
    }
}

async fn recv_aac_coded_audio_timestamps(
    client: &mut RtmpClientHandle,
    min_count: usize,
    stage: &str,
) -> Vec<u32> {
    let mut out = Vec::with_capacity(min_count);
    while out.len() < min_count {
        out.push(recv_aac_coded_audio_timestamp_ms(client, stage).await);
    }
    out
}

async fn recv_h264_coded_video_timestamp_at_least(
    client: &mut RtmpClientHandle,
    min_timestamp_ms: u32,
    stage: &str,
) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut best = 0u32;
    loop {
        let now = Instant::now();
        assert!(
            now < deadline,
            "timeout waiting for h264 coded video timestamp >= {min_timestamp_ms} at {stage}, best={best}"
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
            if !is_h264_coded_video_payload(&payload) {
                continue;
            }
            if timestamp_ms > best {
                best = timestamp_ms;
            }
            if timestamp_ms >= min_timestamp_ms {
                return timestamp_ms;
            }
        }
    }
}

async fn setup_rtsp_tcp_publisher(listen: SocketAddr, uri: &str) -> (TcpStream, String) {
    let mut publisher = connect_with_retry(listen).await;

    let announce = build_request(
        "ANNOUNCE",
        uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(&mut publisher, &announce).await;
    let announce_resp = read_response(&mut publisher, "RTSP-TCP-PUBLISHER-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let session = announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut publisher, &setup).await;
    let setup_resp = read_response(&mut publisher, "RTSP-TCP-PUBLISHER-SETUP").await;
    assert_eq!(setup_resp.status_code, 200);

    let record = build_request("RECORD", uri, 3, Some(&session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let record_resp = read_response(&mut publisher, "RTSP-TCP-PUBLISHER-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    (publisher, session)
}

async fn setup_rtsp_tcp_publisher_with_av_tracks(
    listen: SocketAddr,
    uri: &str,
) -> (TcpStream, String) {
    let mut publisher = connect_with_retry(listen).await;

    let announce = build_request(
        "ANNOUNCE",
        uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(&mut publisher, &announce).await;
    let announce_resp = read_response(&mut publisher, "RTSP-TCP-AV-PUBLISHER-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let session = announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let setup_video = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut publisher, &setup_video).await;
    let setup_video_resp = read_response(&mut publisher, "RTSP-TCP-AV-PUBLISHER-SETUP-VIDEO").await;
    assert_eq!(setup_video_resp.status_code, 200);

    let setup_audio = build_request(
        "SETUP",
        &format!("{uri}/trackID=1"),
        3,
        Some(&session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3")],
        &[],
    );
    write_request(&mut publisher, &setup_audio).await;
    let setup_audio_resp = read_response(&mut publisher, "RTSP-TCP-AV-PUBLISHER-SETUP-AUDIO").await;
    assert_eq!(setup_audio_resp.status_code, 200);

    let record = build_request("RECORD", uri, 4, Some(&session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let record_resp = read_response(&mut publisher, "RTSP-TCP-AV-PUBLISHER-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    (publisher, session)
}

async fn setup_rtsp_udp_publisher(
    listen: SocketAddr,
    uri: &str,
) -> (TcpStream, String, UdpSocket, UdpSocket, u16) {
    let mut publisher = connect_with_retry(listen).await;

    let announce = build_request(
        "ANNOUNCE",
        uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(&mut publisher, &announce).await;
    let announce_resp = read_response(&mut publisher, "RTSP-UDP-PUBLISHER-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let session = announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let publisher_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind publisher rtp");
    let publisher_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind publisher rtcp");
    let setup_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        publisher_rtp
            .local_addr()
            .expect("publisher rtp addr")
            .port(),
        publisher_rtcp
            .local_addr()
            .expect("publisher rtcp addr")
            .port()
    );

    let setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&session),
        &[("Transport", &setup_transport)],
        &[],
    );
    write_request(&mut publisher, &setup).await;
    let setup_resp = read_response(&mut publisher, "RTSP-UDP-PUBLISHER-SETUP").await;
    assert_eq!(setup_resp.status_code, 200);
    let transport = setup_resp.header("Transport").expect("publisher transport");
    let (server_rtp_port, _server_rtcp_port) =
        parse_transport_server_ports(transport).expect("parse server ports");

    let record = build_request("RECORD", uri, 3, Some(&session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let record_resp = read_response(&mut publisher, "RTSP-UDP-PUBLISHER-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    (
        publisher,
        session,
        publisher_rtp,
        publisher_rtcp,
        server_rtp_port,
    )
}

async fn teardown_rtsp_publisher(
    publisher: &mut TcpStream,
    uri: &str,
    session: &str,
    cseq: u32,
    stage: &str,
) {
    let teardown = build_request("TEARDOWN", uri, cseq, Some(session), &[], &[]);
    write_request(publisher, &teardown).await;
    let teardown_resp = read_response(publisher, stage).await;
    assert_eq!(teardown_resp.status_code, 200);
}

async fn collect_rtmp_video_timestamps_for_rtsp_publish(
    runtime: &Arc<TokioRuntime>,
    rtsp_listen: SocketAddr,
    rtmp_listen: SocketAddr,
    stream_name: &str,
    rtp_timestamps: &[u32],
    use_udp_publish: bool,
) -> Vec<u32> {
    let rtsp_uri = format!("rtsp://{rtsp_listen}/live/{stream_name}");
    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/{stream_name}"))
        .expect("parse rtmp url");

    let timestamps = if use_udp_publish {
        let (mut publisher, publisher_session, publisher_rtp, _publisher_rtcp, server_rtp_port) =
            setup_rtsp_udp_publisher(rtsp_listen, &rtsp_uri).await;
        let mut player = start_client(
            runtime.clone(),
            rtmp_url,
            RtmpClientMode::Play,
            RtmpClientDriverConfig::default(),
            CancellationToken::new(),
        )
        .expect("start rtmp play client");
        wait_for_client_state(&mut player, RtmpClientState::Playing, stream_name).await;
        for (idx, ts) in rtp_timestamps.iter().copied().enumerate() {
            send_publish_udp_rtp_frame(
                &publisher_rtp,
                server_rtp_port,
                7000u16.wrapping_add(idx as u16),
                ts,
                0x3355_7799,
            )
            .await;
        }
        let out =
            recv_h264_coded_video_timestamps(&mut player, rtp_timestamps.len(), "udp consistency")
                .await;
        player.shutdown();
        let _ = player.wait().await;
        teardown_rtsp_publisher(
            &mut publisher,
            &rtsp_uri,
            &publisher_session,
            4,
            "RTSP-UDP-CONSISTENCY-PUBLISHER-TEARDOWN",
        )
        .await;
        out
    } else {
        let (mut publisher, publisher_session) =
            setup_rtsp_tcp_publisher(rtsp_listen, &rtsp_uri).await;
        let mut player = start_client(
            runtime.clone(),
            rtmp_url,
            RtmpClientMode::Play,
            RtmpClientDriverConfig::default(),
            CancellationToken::new(),
        )
        .expect("start rtmp play client");
        wait_for_client_state(&mut player, RtmpClientState::Playing, stream_name).await;
        for (idx, ts) in rtp_timestamps.iter().copied().enumerate() {
            let packet = build_publish_h264_rtp(6000u16.wrapping_add(idx as u16), ts, 0x2244_6688);
            send_interleaved_frame(&mut publisher, 0, &packet).await;
        }
        let out =
            recv_h264_coded_video_timestamps(&mut player, rtp_timestamps.len(), "tcp consistency")
                .await;
        player.shutdown();
        let _ = player.wait().await;
        teardown_rtsp_publisher(
            &mut publisher,
            &rtsp_uri,
            &publisher_session,
            4,
            "RTSP-TCP-CONSISTENCY-PUBLISHER-TEARDOWN",
        )
        .await;
        out
    };
    timestamps
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

fn assert_first_timestamp_is_epoch_normalized(timestamps: &[u32], stage: &str) {
    let first = *timestamps
        .first()
        .unwrap_or_else(|| panic!("need at least one timestamp at {stage}"));
    assert!(
        first <= 1_000,
        "first RTMP media timestamp must not leak random RTP epoch at {stage}: first={first}, all={timestamps:?}"
    );
}

fn build_publish_aac_rtp(
    sequence_number: u16,
    timestamp: u32,
    ssrc: u32,
    au_payload: &[u8],
) -> bytes::Bytes {
    let au_size = u16::try_from(au_payload.len()).expect("test au fits u16");
    let au_header = au_size << 3;
    let mut payload = Vec::with_capacity(4 + au_payload.len());
    payload.extend_from_slice(&16u16.to_be_bytes());
    payload.extend_from_slice(&au_header.to_be_bytes());
    payload.extend_from_slice(au_payload);
    let pkt = cheetah_codec::RtpPacket {
        header: cheetah_codec::RtpHeader {
            version: 2,
            payload_type: 97,
            sequence_number,
            timestamp,
            ssrc,
            marker: true,
        },
        payload: bytes::Bytes::from(payload),
    };
    pkt.encode()
}

#[tokio::test(flavor = "current_thread")]
async fn rtsp_tcp_to_rtmp_preserves_aac_rtp_audio_clock() {
    let rtsp_listen = reserve_listen_addr();
    let rtmp_listen = reserve_listen_addr();
    let stream_name = "bridge-rtsp-rtmp-aac-clock";
    let rtsp_uri = format!("rtsp://{rtsp_listen}/live/{stream_name}");
    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/{stream_name}"))
        .expect("parse rtmp url");

    let (runtime, engine) = start_bridge_engine(rtsp_listen, rtmp_listen).await;
    let (mut publisher, publisher_session) =
        setup_rtsp_tcp_publisher_with_av_tracks(rtsp_listen, &rtsp_uri).await;

    let mut player = start_client(
        runtime.clone(),
        rtmp_url,
        RtmpClientMode::Play,
        RtmpClientDriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start rtmp play client");
    wait_for_client_state(&mut player, RtmpClientState::Playing, "rtsp-aac-clock").await;

    let video = build_publish_h264_rtp(10_000, 3_231_885_519, 0x2244_6688);
    send_interleaved_frame(&mut publisher, 0, &video).await;

    let audio_base = 3_427_178_613u32;
    let audio_timestamps = [
        audio_base,
        audio_base.wrapping_add(3_064),
        audio_base.wrapping_add(7_160),
        audio_base.wrapping_add(11_256),
    ];
    for (idx, timestamp) in audio_timestamps.into_iter().enumerate() {
        let packet = build_publish_aac_rtp(
            20_000u16.wrapping_add(idx as u16),
            timestamp,
            0x5566_7788,
            &[0x21, 0x16, 0xc5, idx as u8],
        );
        send_interleaved_frame(&mut publisher, 2, &packet).await;
    }

    let timestamps = recv_aac_coded_audio_timestamps(&mut player, 4, "rtsp-aac-clock").await;
    assert_eq!(
        timestamps[0], 0,
        "first AAC media timestamp should be normalized to zero"
    );
    assert!(
        (60..=68).contains(&(timestamps[1] - timestamps[0])),
        "3064 ticks at 48kHz should map to about 64ms, got {timestamps:?}"
    );
    assert!(
        (80..=90).contains(&(timestamps[2] - timestamps[1])),
        "4096 ticks at 48kHz should map to about 85ms, got {timestamps:?}"
    );

    player.shutdown();
    let _ = player.wait().await;
    teardown_rtsp_publisher(
        &mut publisher,
        &rtsp_uri,
        &publisher_session,
        5,
        "RTSP-TCP-AAC-CLOCK-PUBLISHER-TEARDOWN",
    )
    .await;

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn rtsp_tcp_to_rtmp_replay_and_long_run_timestamp_regression() {
    let rtsp_listen = reserve_listen_addr();
    let rtmp_listen = reserve_listen_addr();
    let stream_name = "bridge-rtsp-rtmp-tcp";
    let rtsp_uri = format!("rtsp://{rtsp_listen}/live/{stream_name}");
    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/{stream_name}"))
        .expect("parse rtmp url");

    let (runtime, engine) = start_bridge_engine(rtsp_listen, rtmp_listen).await;
    let (mut publisher, publisher_session) = setup_rtsp_tcp_publisher(rtsp_listen, &rtsp_uri).await;

    for attempt in 0..3u32 {
        let mut player = start_client(
            runtime.clone(),
            rtmp_url.clone(),
            RtmpClientMode::Play,
            RtmpClientDriverConfig::default(),
            CancellationToken::new(),
        )
        .expect("start rtmp play client");
        wait_for_client_state(&mut player, RtmpClientState::Playing, "rtsp-tcp->rtmp").await;

        let seq_base = 4000u16.wrapping_add((attempt as u16) * 10);
        let ts0 = 3_895_818_000u32.wrapping_add(attempt * 180_000);
        let ts1 = ts0 + 9_000;
        let ts2 = ts1 + 9_000;

        let pkt0 = build_publish_h264_rtp(seq_base, ts0, 0x2244_6688);
        send_interleaved_frame(&mut publisher, 0, &pkt0).await;
        let pkt1 = build_publish_h264_rtp(seq_base.wrapping_add(1), ts1, 0x2244_6688);
        send_interleaved_frame(&mut publisher, 0, &pkt1).await;
        let pkt2 = build_publish_h264_rtp(seq_base.wrapping_add(2), ts2, 0x2244_6688);
        send_interleaved_frame(&mut publisher, 0, &pkt2).await;

        let timestamps =
            recv_h264_coded_video_timestamps(&mut player, 3, "rtsp-tcp->rtmp samples").await;
        if attempt == 0 {
            assert_first_timestamp_is_epoch_normalized(&timestamps, "rtsp-tcp->rtmp");
        }
        assert_monotonic_and_contains_100ms_step(&timestamps, "rtsp-tcp->rtmp");

        if attempt == 2 {
            let long_ts = ts2 + 162_009_000;
            let long_pkt = build_publish_h264_rtp(seq_base.wrapping_add(3), long_ts, 0x2244_6688);
            send_interleaved_frame(&mut publisher, 0, &long_pkt).await;
            let second_ms = timestamps[timestamps.len() - 1];
            let min_long_ms = second_ms.saturating_add(1_800_000);
            let long_ms = recv_h264_coded_video_timestamp_at_least(
                &mut player,
                min_long_ms,
                "rtsp-tcp->rtmp long run",
            )
            .await;
            assert!(
                long_ms >= second_ms,
                "long-run timestamp must stay monotonic: second={second_ms}, long={long_ms}"
            );
            let long_delta = long_ms - second_ms;
            assert!(
                long_delta >= 1_800_000,
                "long-run regression must cover >=30min timeline in ms: got delta={long_delta}"
            );
        }

        player.shutdown();
        let _ = player.wait().await;
    }

    teardown_rtsp_publisher(
        &mut publisher,
        &rtsp_uri,
        &publisher_session,
        4,
        "RTSP-TCP-PUBLISHER-TEARDOWN",
    )
    .await;

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn rtsp_tcp_to_rtmp_late_joiner_rebases_bootstrap_to_zero() {
    let rtsp_listen = reserve_listen_addr();
    let rtmp_listen = reserve_listen_addr();
    let stream_name = "bridge-rtsp-rtmp-late-join";
    let rtsp_uri = format!("rtsp://{rtsp_listen}/live/{stream_name}");
    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/{stream_name}"))
        .expect("parse rtmp url");

    let (runtime, engine) = start_bridge_engine(rtsp_listen, rtmp_listen).await;
    let (mut publisher, publisher_session) = setup_rtsp_tcp_publisher(rtsp_listen, &rtsp_uri).await;

    let ts0 = 3_895_818_000u32;
    let late_ts = ts0 + 2_459_970;
    let first = build_publish_h264_rtp(9000, ts0, 0x4466_88aa);
    send_interleaved_frame(&mut publisher, 0, &first).await;
    let late_key = build_publish_h264_rtp(9001, late_ts, 0x4466_88aa);
    send_interleaved_frame(&mut publisher, 0, &late_key).await;

    let mut player = start_client(
        runtime.clone(),
        rtmp_url,
        RtmpClientMode::Play,
        RtmpClientDriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start rtmp play client");
    wait_for_client_state(
        &mut player,
        RtmpClientState::Playing,
        "late-join rtsp-tcp->rtmp",
    )
    .await;

    let first_video_ms =
        recv_h264_coded_video_timestamp_ms(&mut player, "late-join bootstrap").await;
    assert_eq!(
        first_video_ms, 0,
        "late RTMP joiner must receive bootstrap media rebased to zero"
    );

    player.shutdown();
    let _ = player.wait().await;
    teardown_rtsp_publisher(
        &mut publisher,
        &rtsp_uri,
        &publisher_session,
        4,
        "RTSP-TCP-LATE-JOIN-PUBLISHER-TEARDOWN",
    )
    .await;

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn rtsp_udp_to_rtmp_replay_and_long_run_timestamp_regression() {
    let rtsp_listen = reserve_listen_addr();
    let rtmp_listen = reserve_listen_addr();
    let stream_name = "bridge-rtsp-rtmp-udp";
    let rtsp_uri = format!("rtsp://{rtsp_listen}/live/{stream_name}");
    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/{stream_name}"))
        .expect("parse rtmp url");

    let (runtime, engine) = start_bridge_engine(rtsp_listen, rtmp_listen).await;
    let (mut publisher, publisher_session, publisher_rtp, _publisher_rtcp, server_rtp_port) =
        setup_rtsp_udp_publisher(rtsp_listen, &rtsp_uri).await;

    for attempt in 0..3u32 {
        let mut player = start_client(
            runtime.clone(),
            rtmp_url.clone(),
            RtmpClientMode::Play,
            RtmpClientDriverConfig::default(),
            CancellationToken::new(),
        )
        .expect("start rtmp play client");
        wait_for_client_state(&mut player, RtmpClientState::Playing, "rtsp-udp->rtmp").await;

        let seq_base = 5000u16.wrapping_add((attempt as u16) * 10);
        let ts0 = 3_895_818_000u32.wrapping_add(attempt * 180_000);
        let ts1 = ts0 + 9_000;
        let ts2 = ts1 + 9_000;

        send_publish_udp_rtp_frame(&publisher_rtp, server_rtp_port, seq_base, ts0, 0x3355_7799)
            .await;
        send_publish_udp_rtp_frame(
            &publisher_rtp,
            server_rtp_port,
            seq_base.wrapping_add(1),
            ts1,
            0x3355_7799,
        )
        .await;
        send_publish_udp_rtp_frame(
            &publisher_rtp,
            server_rtp_port,
            seq_base.wrapping_add(2),
            ts2,
            0x3355_7799,
        )
        .await;

        let timestamps =
            recv_h264_coded_video_timestamps(&mut player, 3, "rtsp-udp->rtmp samples").await;
        if attempt == 0 {
            assert_first_timestamp_is_epoch_normalized(&timestamps, "rtsp-udp->rtmp");
        }
        assert_monotonic_and_contains_100ms_step(&timestamps, "rtsp-udp->rtmp");

        if attempt == 2 {
            let long_ts = ts2 + 162_009_000;
            send_publish_udp_rtp_frame(
                &publisher_rtp,
                server_rtp_port,
                seq_base.wrapping_add(3),
                long_ts,
                0x3355_7799,
            )
            .await;
            let second_ms = timestamps[timestamps.len() - 1];
            let min_long_ms = second_ms.saturating_add(1_800_000);
            let long_ms = recv_h264_coded_video_timestamp_at_least(
                &mut player,
                min_long_ms,
                "rtsp-udp->rtmp long run",
            )
            .await;
            assert!(
                long_ms >= second_ms,
                "long-run timestamp must stay monotonic: second={second_ms}, long={long_ms}"
            );
            let long_delta = long_ms - second_ms;
            assert!(
                long_delta >= 1_800_000,
                "long-run regression must cover >=30min timeline in ms: got delta={long_delta}"
            );
        }

        player.shutdown();
        let _ = player.wait().await;
    }

    teardown_rtsp_publisher(
        &mut publisher,
        &rtsp_uri,
        &publisher_session,
        4,
        "RTSP-UDP-PUBLISHER-TEARDOWN",
    )
    .await;

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn rtsp_tcp_and_udp_publish_produce_consistent_rtmp_timestamps() {
    let rtsp_listen = reserve_listen_addr();
    let rtmp_listen = reserve_listen_addr();
    let (runtime, engine) = start_bridge_engine(rtsp_listen, rtmp_listen).await;

    let base = 3_895_818_000u32;
    let rtp_timestamps = [
        base,
        base + 9_000,
        base + 18_000,
        base + 27_000,
        base + 36_000,
    ];

    let tcp_timestamps = collect_rtmp_video_timestamps_for_rtsp_publish(
        &runtime,
        rtsp_listen,
        rtmp_listen,
        "bridge-rtsp-rtmp-consistency-tcp",
        &rtp_timestamps,
        false,
    )
    .await;
    let udp_timestamps = collect_rtmp_video_timestamps_for_rtsp_publish(
        &runtime,
        rtsp_listen,
        rtmp_listen,
        "bridge-rtsp-rtmp-consistency-udp",
        &rtp_timestamps,
        true,
    )
    .await;

    assert_eq!(
        tcp_timestamps, udp_timestamps,
        "rtsp tcp/udp publish must map to identical canonical RTMP timestamp sequence"
    );
    assert_first_timestamp_is_epoch_normalized(&tcp_timestamps, "consistency-tcp");
    assert_monotonic_and_contains_100ms_step(&tcp_timestamps, "consistency-tcp");
    assert_first_timestamp_is_epoch_normalized(&udp_timestamps, "consistency-udp");
    assert_monotonic_and_contains_100ms_step(&udp_timestamps, "consistency-udp");

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn rtsp_udp_publish_loss_and_reorder_keeps_canonical_timeline_monotonic() {
    let rtsp_listen = reserve_listen_addr();
    let rtmp_listen = reserve_listen_addr();
    let stream_name = "bridge-rtsp-rtmp-udp-reorder-loss";
    let rtsp_uri = format!("rtsp://{rtsp_listen}/live/{stream_name}");
    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/{stream_name}"))
        .expect("parse rtmp url");

    let (runtime, engine) = start_bridge_engine(rtsp_listen, rtmp_listen).await;
    let (mut publisher, publisher_session, publisher_rtp, _publisher_rtcp, server_rtp_port) =
        setup_rtsp_udp_publisher(rtsp_listen, &rtsp_uri).await;

    let mut player = start_client(
        runtime.clone(),
        rtmp_url,
        RtmpClientMode::Play,
        RtmpClientDriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start rtmp play client");
    wait_for_client_state(
        &mut player,
        RtmpClientState::Playing,
        "rtsp-udp-reorder-loss",
    )
    .await;

    let base = 3_895_818_000u32;
    // seq 8001 intentionally dropped; send 8002 before 8003 to emulate disorder.
    let sample_packets = [
        (8000u16, base),
        (8002u16, base + 18_000),
        (8003u16, base + 9_000),
        (8004u16, base + 27_000),
    ];
    for (seq, ts) in sample_packets {
        send_publish_udp_rtp_frame(&publisher_rtp, server_rtp_port, seq, ts, 0x8899_AABB).await;
    }

    let timestamps = recv_h264_coded_video_timestamps(&mut player, 4, "udp reorder/loss").await;
    assert_first_timestamp_is_epoch_normalized(&timestamps, "udp reorder/loss");
    for window in timestamps.windows(2) {
        assert!(
            window[1] >= window[0],
            "udp reorder/loss must keep canonical timeline monotonic: {timestamps:?}"
        );
    }

    player.shutdown();
    let _ = player.wait().await;
    teardown_rtsp_publisher(
        &mut publisher,
        &rtsp_uri,
        &publisher_session,
        4,
        "RTSP-UDP-REORDER-LOSS-PUBLISHER-TEARDOWN",
    )
    .await;

    engine.stop().await;
}
