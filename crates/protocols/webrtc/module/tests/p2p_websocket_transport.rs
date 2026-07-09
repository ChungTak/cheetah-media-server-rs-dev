//! Phase 05 follow-up — round 8: real WebSocket transport integration
//! test. Boots a tiny `tokio-tungstenite` server inside the test
//! process and drives [`WebSocketTransportFactory`] +
//! [`WebSocketP2pTransport`] through one round-trip:
//!
//! 1. Bind a `TcpListener` on `127.0.0.1:0`.
//! 2. Build a [`WebSocketTransportFactory`] with `allow_private_ips`
//!    so SSRF doesn't reject loopback.
//! 3. The factory connects via `connect_async` and yields a
//!    [`WebSocketP2pTransport`].
//! 4. The bridge sends a `check_in`; the server reads it, replies
//!    with `answer + bye`; transport surfaces both messages.
//! 5. Counters reflect the activity.

use std::time::Duration;

use cheetah_webrtc_module::p2p::message::{
    P2pDirection, P2pMessage, P2pMessageHeader, P2pStreamTuple,
};
use cheetah_webrtc_module::p2p::room::{P2pRoomKeeperConfig, P2pRoomKeeperRegistry};
use cheetah_webrtc_module::p2p::supervisor::KeeperTransportFactory;
use cheetah_webrtc_module::p2p::{
    snapshot_websocket_counters, P2pTransport, P2pTransportEvent, SignalingUrlPolicy,
    WebSocketTransportConfig, WebSocketTransportFactory,
};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_transport_round_trips_messages_against_local_server() {
    // 1. Bind a real TCP listener so we get a free port.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bound = listener.local_addr().unwrap();

    // Server task: accept one WebSocket, expect a `check_in`, reply
    // with `answer` + `bye`.
    let server = tokio::spawn(async move {
        let (stream, _addr) = listener.accept().await.expect("accept ok");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("ws accept");
        let raw = match ws.next().await {
            Some(Ok(WsMessage::Text(t))) => t,
            other => panic!("expected text frame, got {other:?}"),
        };
        let parsed: serde_json::Value = serde_json::from_str(raw.as_str()).unwrap();
        assert_eq!(parsed["type"], "check_in");
        let header = parsed.as_object().unwrap();
        let room_id = header["room_id"].as_str().unwrap().to_string();
        let peer_id = header["peer_id"].as_str().unwrap().to_string();
        let transport_id = header["transport_id"].as_str().unwrap().to_string();
        let answer = serde_json::json!({
            "type": "answer",
            "room_id": peer_id,
            "peer_id": room_id,
            "transport_id": transport_id,
            "sdp": "v=0\r\nanswer",
        });
        ws.send(WsMessage::Text(answer.to_string().into()))
            .await
            .unwrap();
        let bye = serde_json::json!({
            "type": "bye",
            "room_id": "x",
            "peer_id": "y",
            "transport_id": "z",
            "reason": "done",
        });
        ws.send(WsMessage::Text(bye.to_string().into()))
            .await
            .unwrap();
        // Drain whatever the client sends back (e.g. close).
        let _ = ws.close(None).await;
        while ws.next().await.is_some() {}
    });

    // 2. Build the factory pointed at the bound address.
    let factory = WebSocketTransportFactory::new(WebSocketTransportConfig {
        url_policy: SignalingUrlPolicy {
            allow_private_ips: true,
            ..Default::default()
        },
        url_override: Some(format!("ws://{}/p2p", bound)),
        connect_timeout: Duration::from_secs(2),
        ..Default::default()
    });

    // 3. Build a stub keeper snapshot (registry isn't really used —
    //    the factory's `url_override` short-circuits the host derivation).
    let registry = P2pRoomKeeperRegistry::default();
    let key = registry
        .add(P2pRoomKeeperConfig {
            server_host: bound.ip().to_string(),
            server_port: bound.port(),
            room_id: "room42".into(),
            vhost: None,
            app: Some("live".into()),
            stream: Some("demo".into()),
            ssl: false,
        })
        .unwrap();
    let snapshot = registry
        .list()
        .into_iter()
        .find(|s| s.key == key)
        .expect("snapshot");

    // 4. Connect.
    let transport = factory.connect(&snapshot).await.expect("connect ok");
    let counters = transport.counters.clone();

    // 5. Send check_in.
    transport
        .send(P2pMessage::CheckIn {
            header: P2pMessageHeader {
                room_id: Some("room42".into()),
                peer_id: Some("ringing".into()),
                transport_id: Some("tr1".into()),
            },
            direction: P2pDirection::Pull,
            stream: P2pStreamTuple {
                vhost: "v".into(),
                app: "live".into(),
                stream: "demo".into(),
            },
            sdp: Some("v=0\r\noffer".into()),
        })
        .await
        .expect("send ok");

    // 6. Receive the two server-side messages.
    match transport.recv().await.unwrap() {
        P2pTransportEvent::Message(P2pMessage::Answer { sdp, .. }) => {
            assert!(sdp.contains("answer"), "unexpected sdp: {sdp}");
        }
        other => panic!("expected Answer, got {other:?}"),
    }
    match transport.recv().await.unwrap() {
        P2pTransportEvent::Message(P2pMessage::Bye { reason, .. }) => {
            assert_eq!(reason.as_deref(), Some("done"));
        }
        other => panic!("expected Bye, got {other:?}"),
    }

    // 7. Server closes the socket; transport surfaces `Closed`.
    match transport.recv().await.unwrap() {
        P2pTransportEvent::Closed | P2pTransportEvent::Error(_) => {}
        other => panic!("expected Closed/Error after server close, got {other:?}"),
    }
    transport.close().await;

    let snap = snapshot_websocket_counters(&counters);
    assert!(snap.messages_sent >= 1, "snap = {snap:?}");
    assert!(snap.messages_received >= 2, "snap = {snap:?}");

    server.await.unwrap();
}
