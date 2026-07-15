//! GB28181 production contract tests.
//!
//! These tests run against a real Engine, RtpModule and the golden fixture,
//! opening both receiver and sender RTP sessions and verifying packets on the wire.
//!
//! 本测试针对真实 Engine、RtpModule 与 golden fixture，
//! 分别打开 RTP 接收端与发送端会话，并验证真实 UDP 包是否到达。

use bytes::Bytes;
use cheetah_codec::{RtpHeader, RtpPacket};
use cheetah_media_api::command::{RtpReceiverRequest, RtpSenderMode, RtpSenderRequest};
use cheetah_media_api::model::{OnlineState, RtpSessionKind, RtpSessionState};
use cheetah_media_api::{MediaControlApi, RtpApi};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;

use crate::production_support::{ctx, golden_key, media_facade, production_engine, wait_ms};

#[tokio::test(flavor = "current_thread")]
async fn gb28181_can_open_receiver_and_sender_sessions() {
    let engine = production_engine().await;
    let facade = media_facade(&engine);

    // Verify the golden fixture stream is reported online.
    let online = facade
        .is_media_online(&ctx(), &golden_key())
        .await
        .expect("is_media_online");
    assert_eq!(online, OnlineState::Online);

    // Open a UDP RTP receiver with an ephemeral port.
    let recv_session = facade
        .open_rtp_receiver(
            &ctx(),
            RtpReceiverRequest {
                media_key: golden_key(),
                port: Some(0),
                ip: Some("127.0.0.1".to_string()),
                ssrc: Some(0x12345678),
                enable_rtcp: false,
                tcp_mode: None,
                payload_type: Some(96),
                codec_hint: None,
                reuse_port: false,
                timeout_ms: 5000,
            },
        )
        .await
        .expect("open_rtp_receiver");
    let recv_port = recv_session.local_port.expect("receiver has a local port");
    assert_ne!(recv_port, 0);
    assert_eq!(recv_session.kind, RtpSessionKind::Receiver);
    assert_eq!(recv_session.state, RtpSessionState::Listening);

    // Send a valid ES RTP packet to the receiver port.
    let test_socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind test socket");
    let packet = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 0,
            ssrc: 0x12345678,
            marker: true,
        },
        payload: Bytes::from_static(b"vp8"),
    }
    .encode();
    test_socket
        .send_to(&packet, format!("127.0.0.1:{recv_port}"))
        .await
        .expect("send rtp");
    wait_ms(200).await;

    let session = facade
        .get_rtp_session(&ctx(), &recv_session.session_id)
        .await
        .expect("get_rtp_session");
    assert_eq!(session.kind, RtpSessionKind::Receiver);

    // Open an RTP sender to a local sink socket and verify a packet is emitted.
    let sink_socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind sink socket");
    let sink_addr = sink_socket.local_addr().unwrap();

    let send_session = facade
        .open_rtp_sender(
            &ctx(),
            RtpSenderRequest {
                media_key: golden_key(),
                destination_endpoint: sink_addr.to_string(),
                ssrc: Some(0x12345678),
                payload_type: Some(96),
                codec_hint: None,
                mode: RtpSenderMode::Active,
                transport_options: Default::default(),
            },
        )
        .await
        .expect("open_rtp_sender");
    assert_eq!(send_session.kind, RtpSessionKind::Sender);

    let mut buf = [0u8; 2048];
    let (n, _) = timeout(Duration::from_millis(3000), sink_socket.recv_from(&mut buf))
        .await
        .expect("wait for rtp timeout")
        .expect("recv_from");
    assert!(n >= 12, "RTP packet should be at least 12 bytes");
    assert_eq!(buf[0] >> 6, 2, "RTP version should be 2");

    facade
        .stop_rtp_session(&ctx(), &recv_session.session_id)
        .await
        .expect("stop receiver");
    facade
        .stop_rtp_session(&ctx(), &send_session.session_id)
        .await
        .expect("stop sender");
}
