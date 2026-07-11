use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use bytes::Bytes;
use cheetah_runtime_api::CancellationToken;
use cheetah_srt_core::{
    SrtEncryptionOptions, SrtKeyLength, SrtPayloadKind, SrtRole, SrtSessionOptions, SrtStreamMode,
};
use cheetah_srt_driver_tokio::{
    spawn_driver, SrtDriverCommand, SrtDriverConfig, SrtDriverEncryption, SrtDriverEvent, SrtPeerId,
};

fn driver_config() -> SrtDriverConfig {
    SrtDriverConfig {
        listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        max_connections: 16,
        idle_timeout_ms: 5_000,
        connect_timeout_ms: 5_000,
        latency_ms: 20,
        stats_interval_ms: 20,
        recv_buffer_packets: 1024,
        send_queue_capacity: 1024,
        srt_version: 0x0001_0500,
        encryption: SrtDriverEncryption::default(),
    }
}

fn caller_options() -> SrtSessionOptions {
    SrtSessionOptions {
        role: SrtRole::Caller,
        mode: SrtStreamMode::Publish,
        stream_key: "live/test".to_string(),
        latency_ms: 20,
        payload: SrtPayloadKind::MpegTs,
        encryption: SrtEncryptionOptions {
            enabled: false,
            passphrase: String::new(),
            key_length: SrtKeyLength::Aes128,
        },
    }
}

fn encrypted_driver_config(passphrase: &str, key_length: SrtKeyLength) -> SrtDriverConfig {
    let mut config = driver_config();
    config.stats_interval_ms = 0;
    config.encryption = SrtDriverEncryption {
        enabled: true,
        passphrase: passphrase.to_string(),
        key_length,
    };
    config
}

async fn recv_with_timeout(rx: &mut tokio::sync::mpsc::Receiver<SrtDriverEvent>) -> SrtDriverEvent {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("event timeout")
        .expect("event channel open")
}

async fn assert_encrypted_payload_roundtrip(key_length: SrtKeyLength) {
    let listener_cancel = CancellationToken::new();
    let caller_cancel = CancellationToken::new();
    let (listener_handle, mut listener_events) = spawn_driver(
        encrypted_driver_config("shared-test-passphrase", key_length),
        listener_cancel.clone(),
    );
    let (caller_handle, mut caller_events) = spawn_driver(
        encrypted_driver_config("shared-test-passphrase", key_length),
        caller_cancel.clone(),
    );

    let listener_addr = match recv_with_timeout(&mut listener_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first event: {other:?}"),
    };
    let _caller_addr = match recv_with_timeout(&mut caller_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first caller event: {other:?}"),
    };

    let caller_id = SrtPeerId(110);
    caller_handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id: caller_id,
            remote: listener_addr,
            stream_id: Some("#!::r=live/encrypted,m=publish".to_string()),
            options: caller_options(),
        })
        .await;

    let mut caller_connected = false;
    let mut listener_peer = None;
    for _ in 0..40 {
        tokio::select! {
            event = recv_with_timeout(&mut caller_events) => {
                if let SrtDriverEvent::Connected { peer_id, .. } = event {
                    if peer_id == caller_id {
                        caller_connected = true;
                    }
                }
            }
            event = recv_with_timeout(&mut listener_events) => {
                if let SrtDriverEvent::Connected { peer_id, stream_id, .. } = event {
                    listener_peer = Some(peer_id);
                    assert_eq!(stream_id.as_deref(), Some("#!::r=live/encrypted,m=publish"));
                }
            }
        }
        if caller_connected && listener_peer.is_some() {
            break;
        }
    }

    assert!(caller_connected, "encrypted caller should connect");
    let listener_peer = listener_peer.expect("encrypted listener peer should connect");

    listener_handle
        .send(SrtDriverCommand::SendPayload {
            peer_id: listener_peer,
            payload: Bytes::from_static(b"encrypted-hello"),
        })
        .await;

    for _ in 0..40 {
        if let SrtDriverEvent::Payload { peer_id, payload } =
            recv_with_timeout(&mut caller_events).await
        {
            if peer_id == caller_id && payload.as_ref() == b"encrypted-hello" {
                listener_cancel.cancel();
                caller_cancel.cancel();
                return;
            }
        }
    }

    panic!("encrypted caller did not receive listener payload");
}

#[tokio::test]
async fn listener_started_reports_bound_addr() {
    let cancel = CancellationToken::new();
    let (_handle, mut events) = spawn_driver(driver_config(), cancel.clone());

    let event = recv_with_timeout(&mut events).await;
    let SrtDriverEvent::ListenerStarted { local_addr } = event else {
        panic!("unexpected event: {event:?}");
    };
    assert_eq!(local_addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_ne!(local_addr.port(), 0);

    cancel.cancel();
}

#[tokio::test]
async fn encryption_enabled_requires_passphrase() {
    let mut config = driver_config();
    config.encryption = SrtDriverEncryption {
        enabled: true,
        passphrase: String::new(),
        key_length: SrtKeyLength::Aes128,
    };
    let cancel = CancellationToken::new();
    let (_handle, mut events) = spawn_driver(config, cancel.clone());

    let event = recv_with_timeout(&mut events).await;
    match event {
        SrtDriverEvent::Error { peer_id, message } => {
            assert_eq!(peer_id, None);
            assert_eq!(message, "SRT encryption passphrase must not be empty");
        }
        other => panic!("unexpected event: {other:?}"),
    }

    assert!(
        tokio::time::timeout(Duration::from_millis(50), events.recv())
            .await
            .expect("driver should stop after invalid encryption config")
            .is_none()
    );
    cancel.cancel();
}

#[tokio::test]
async fn aes128_encrypted_payload_roundtrip() {
    assert_encrypted_payload_roundtrip(SrtKeyLength::Aes128).await;
}

#[tokio::test]
async fn aes256_encrypted_payload_roundtrip() {
    assert_encrypted_payload_roundtrip(SrtKeyLength::Aes256).await;
}

#[tokio::test]
async fn caller_command_uses_session_encryption_options() {
    let listener_cancel = CancellationToken::new();
    let caller_cancel = CancellationToken::new();
    let (listener_handle, mut listener_events) = spawn_driver(
        encrypted_driver_config("per-job-secret", SrtKeyLength::Aes128),
        listener_cancel.clone(),
    );
    let (caller_handle, mut caller_events) = spawn_driver(driver_config(), caller_cancel.clone());

    let listener_addr = match recv_with_timeout(&mut listener_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first event: {other:?}"),
    };
    let _caller_addr = match recv_with_timeout(&mut caller_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first caller event: {other:?}"),
    };

    let caller_id = SrtPeerId(130);
    let mut options = caller_options();
    options.encryption = SrtEncryptionOptions {
        enabled: true,
        passphrase: "per-job-secret".to_string(),
        key_length: SrtKeyLength::Aes128,
    };
    caller_handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id: caller_id,
            remote: listener_addr,
            stream_id: Some("#!::r=live/per-job-encryption,m=publish".to_string()),
            options,
        })
        .await;

    let mut caller_connected = false;
    let mut listener_peer = None;
    for _ in 0..40 {
        tokio::select! {
            event = recv_with_timeout(&mut caller_events) => {
                if let SrtDriverEvent::Connected { peer_id, .. } = event {
                    if peer_id == caller_id {
                        caller_connected = true;
                    }
                }
            }
            event = recv_with_timeout(&mut listener_events) => {
                if let SrtDriverEvent::Connected { peer_id, .. } = event {
                    listener_peer = Some(peer_id);
                }
            }
        }
        if caller_connected && listener_peer.is_some() {
            break;
        }
    }

    assert!(
        caller_connected,
        "caller should connect using command encryption options"
    );
    let listener_peer = listener_peer.expect("listener peer should connect");

    listener_handle
        .send(SrtDriverCommand::SendPayload {
            peer_id: listener_peer,
            payload: Bytes::from_static(b"per-job-secret-payload"),
        })
        .await;

    for _ in 0..40 {
        if let SrtDriverEvent::Payload { peer_id, payload } =
            recv_with_timeout(&mut caller_events).await
        {
            if peer_id == caller_id && payload.as_ref() == b"per-job-secret-payload" {
                listener_cancel.cancel();
                caller_cancel.cancel();
                return;
            }
        }
    }

    panic!("caller did not receive encrypted payload with command encryption options");
}

#[tokio::test]
async fn encryption_passphrase_mismatch_disconnects_caller() {
    let mut listener_config = encrypted_driver_config("listener-secret", SrtKeyLength::Aes128);
    listener_config.idle_timeout_ms = 0;
    listener_config.connect_timeout_ms = 150;

    let mut caller_config = encrypted_driver_config("caller-secret", SrtKeyLength::Aes128);
    caller_config.idle_timeout_ms = 0;
    caller_config.connect_timeout_ms = 150;

    let listener_cancel = CancellationToken::new();
    let caller_cancel = CancellationToken::new();
    let (_listener_handle, mut listener_events) =
        spawn_driver(listener_config, listener_cancel.clone());
    let (caller_handle, mut caller_events) = spawn_driver(caller_config, caller_cancel.clone());

    let listener_addr = match recv_with_timeout(&mut listener_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first event: {other:?}"),
    };
    let _caller_addr = match recv_with_timeout(&mut caller_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first caller event: {other:?}"),
    };

    let caller_id = SrtPeerId(120);
    caller_handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id: caller_id,
            remote: listener_addr,
            stream_id: Some("#!::r=live/encrypted,m=publish".to_string()),
            options: caller_options(),
        })
        .await;

    let mut saw_connecting = false;
    for _ in 0..20 {
        let event = tokio::time::timeout(Duration::from_secs(1), caller_events.recv())
            .await
            .expect("passphrase mismatch event timeout")
            .expect("caller event channel open");
        match event {
            SrtDriverEvent::CallerConnecting { peer_id, .. } if peer_id == caller_id => {
                saw_connecting = true;
            }
            SrtDriverEvent::Connected { peer_id, .. } if peer_id == caller_id => {
                panic!("caller unexpectedly connected with mismatched passphrase");
            }
            SrtDriverEvent::Disconnected { peer_id, reason } if peer_id == caller_id => {
                assert!(saw_connecting, "caller should report connecting first");
                assert_eq!(reason, "connect timeout");
                listener_cancel.cancel();
                caller_cancel.cancel();
                return;
            }
            SrtDriverEvent::Error { peer_id, .. } if peer_id == Some(caller_id) => {
                assert!(saw_connecting, "caller should report connecting first");
                listener_cancel.cancel();
                caller_cancel.cancel();
                return;
            }
            SrtDriverEvent::Stats { .. } => {}
            _ => {}
        }
    }

    panic!("caller did not fail after passphrase mismatch");
}

#[tokio::test]
async fn caller_payload_reaches_listener() {
    let listener_cancel = CancellationToken::new();
    let caller_cancel = CancellationToken::new();
    let (_listener_handle, mut listener_events) =
        spawn_driver(driver_config(), listener_cancel.clone());
    let (caller_handle, mut caller_events) = spawn_driver(driver_config(), caller_cancel.clone());

    let listener_addr = match recv_with_timeout(&mut listener_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first event: {other:?}"),
    };
    let _caller_addr = match recv_with_timeout(&mut caller_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first caller event: {other:?}"),
    };

    let caller_id = SrtPeerId(100);
    caller_handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id: caller_id,
            remote: listener_addr,
            stream_id: Some("#!::r=live/test,m=publish".to_string()),
            options: caller_options(),
        })
        .await;

    let mut caller_connected = false;
    let mut listener_peer = None;
    for _ in 0..20 {
        tokio::select! {
            event = recv_with_timeout(&mut caller_events) => match event {
            SrtDriverEvent::Connected {
                peer_id,
                stream_id,
                ..
            } if peer_id == caller_id => {
                caller_connected = true;
                assert_eq!(stream_id.as_deref(), Some("#!::r=live/test,m=publish"));
            }
            SrtDriverEvent::Connected {
                peer_id,
                stream_id,
                ..
            } => {
                listener_peer = Some(peer_id);
                assert_eq!(stream_id.as_deref(), Some("#!::r=live/test,m=publish"));
            }
            _ => {}
            },
            event = recv_with_timeout(&mut listener_events) => match event {
            SrtDriverEvent::Connected {
                peer_id,
                stream_id,
                ..
            } => {
                listener_peer = Some(peer_id);
                assert_eq!(stream_id.as_deref(), Some("#!::r=live/test,m=publish"));
            }
            _ => {}
            }
        }
        if caller_connected && listener_peer.is_some() {
            break;
        }
    }

    assert!(caller_connected, "caller should connect");
    let listener_peer = listener_peer.expect("listener peer should connect");

    caller_handle
        .send(SrtDriverCommand::SendPayload {
            peer_id: caller_id,
            payload: Bytes::from_static(b"hello-srt"),
        })
        .await;

    for _ in 0..20 {
        if let SrtDriverEvent::Payload { peer_id, payload } =
            recv_with_timeout(&mut listener_events).await
        {
            if peer_id == listener_peer && payload.as_ref() == b"hello-srt" {
                listener_cancel.cancel();
                caller_cancel.cancel();
                return;
            }
        }
    }

    panic!("listener did not receive caller payload");
}

#[tokio::test]
async fn listener_payload_reaches_caller() {
    let listener_cancel = CancellationToken::new();
    let caller_cancel = CancellationToken::new();
    let (listener_handle, mut listener_events) =
        spawn_driver(driver_config(), listener_cancel.clone());
    let (caller_handle, mut caller_events) = spawn_driver(driver_config(), caller_cancel.clone());

    let listener_addr = match recv_with_timeout(&mut listener_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first event: {other:?}"),
    };
    let _caller_addr = match recv_with_timeout(&mut caller_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first caller event: {other:?}"),
    };

    let caller_id = SrtPeerId(101);
    caller_handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id: caller_id,
            remote: listener_addr,
            stream_id: Some("#!::r=live/test,m=request".to_string()),
            options: caller_options(),
        })
        .await;

    let mut caller_connected = false;
    let mut listener_peer = None;
    for _ in 0..20 {
        tokio::select! {
            event = recv_with_timeout(&mut caller_events) => {
                if let SrtDriverEvent::Connected { peer_id, .. } = event {
                    if peer_id == caller_id {
                        caller_connected = true;
                    }
                }
            }
            event = recv_with_timeout(&mut listener_events) => {
                if let SrtDriverEvent::Connected { peer_id, stream_id, .. } = event {
                    listener_peer = Some(peer_id);
                    assert_eq!(stream_id.as_deref(), Some("#!::r=live/test,m=request"));
                }
            }
        }
        if caller_connected && listener_peer.is_some() {
            break;
        }
    }

    assert!(caller_connected, "caller should connect");
    let listener_peer = listener_peer.expect("listener peer should connect");

    listener_handle
        .send(SrtDriverCommand::SendPayload {
            peer_id: listener_peer,
            payload: Bytes::from_static(b"hello-caller"),
        })
        .await;

    for _ in 0..20 {
        if let SrtDriverEvent::Payload { peer_id, payload } =
            recv_with_timeout(&mut caller_events).await
        {
            if peer_id == caller_id && payload.as_ref() == b"hello-caller" {
                listener_cancel.cancel();
                caller_cancel.cancel();
                return;
            }
        }
    }

    panic!("caller did not receive listener payload");
}

#[tokio::test]
async fn connected_peer_emits_periodic_stats() {
    let listener_cancel = CancellationToken::new();
    let caller_cancel = CancellationToken::new();
    let (_listener_handle, mut listener_events) =
        spawn_driver(driver_config(), listener_cancel.clone());
    let (caller_handle, mut caller_events) = spawn_driver(driver_config(), caller_cancel.clone());

    let listener_addr = match recv_with_timeout(&mut listener_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first event: {other:?}"),
    };
    let _caller_addr = match recv_with_timeout(&mut caller_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first caller event: {other:?}"),
    };

    caller_handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id: SrtPeerId(200),
            remote: listener_addr,
            stream_id: Some("#!::r=live/test,m=publish".to_string()),
            options: caller_options(),
        })
        .await;

    let mut caller_connected = false;
    let mut listener_peer = None;
    for _ in 0..20 {
        tokio::select! {
            event = recv_with_timeout(&mut listener_events) => {
                if let SrtDriverEvent::Connected { peer_id, .. } = event {
                    listener_peer = Some(peer_id);
                }
            }
            event = recv_with_timeout(&mut caller_events) => {
                if let SrtDriverEvent::Connected { peer_id, .. } = event {
                    if peer_id == SrtPeerId(200) {
                        caller_connected = true;
                    }
                }
            }
        }
        if caller_connected && listener_peer.is_some() {
            break;
        }
    }

    assert!(
        caller_connected,
        "caller should connect before stats payload"
    );
    let listener_peer = listener_peer.expect("listener peer should connect");
    caller_handle
        .send(SrtDriverCommand::SendPayload {
            peer_id: SrtPeerId(200),
            payload: Bytes::from_static(b"stats-payload"),
        })
        .await;

    let mut payload_received = false;
    for _ in 0..20 {
        if let SrtDriverEvent::Payload { peer_id, payload } =
            recv_with_timeout(&mut listener_events).await
        {
            if peer_id == listener_peer && payload.as_ref() == b"stats-payload" {
                payload_received = true;
                break;
            }
        }
    }
    assert!(
        payload_received,
        "listener should receive payload before stats assertion"
    );

    for _ in 0..20 {
        if let SrtDriverEvent::Stats { peer_id, stats } =
            recv_with_timeout(&mut listener_events).await
        {
            if peer_id == listener_peer {
                assert!(
                    stats.bytes_in > 0,
                    "stats should include received handshake bytes"
                );
                assert!(
                    stats.packets_in > 0,
                    "stats should include received handshake packets"
                );
                assert!(
                    stats.receiver_total_received > 0,
                    "stats should include receiver packet totals from shiguredo_srt"
                );
                assert!(
                    stats.receiver_total_bytes_received > 0,
                    "stats should include receiver byte totals from shiguredo_srt"
                );
                let _ = stats.receiver_rtt_micros;
                let _ = stats.receiver_jitter_micros;
                listener_cancel.cancel();
                caller_cancel.cancel();
                return;
            }
        }
    }

    panic!("listener did not emit stats for connected peer");
}

#[tokio::test]
async fn idle_peer_is_disconnected() {
    let mut listener_config = driver_config();
    listener_config.idle_timeout_ms = 30;
    listener_config.stats_interval_ms = 0;

    let mut caller_config = driver_config();
    caller_config.idle_timeout_ms = 30;
    caller_config.stats_interval_ms = 0;

    let listener_cancel = CancellationToken::new();
    let caller_cancel = CancellationToken::new();
    let (_listener_handle, mut listener_events) =
        spawn_driver(listener_config, listener_cancel.clone());
    let (caller_handle, mut caller_events) = spawn_driver(caller_config, caller_cancel.clone());

    let listener_addr = match recv_with_timeout(&mut listener_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first event: {other:?}"),
    };
    let _caller_addr = match recv_with_timeout(&mut caller_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first caller event: {other:?}"),
    };

    caller_handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id: SrtPeerId(300),
            remote: listener_addr,
            stream_id: Some("#!::r=live/test,m=publish".to_string()),
            options: caller_options(),
        })
        .await;

    let mut listener_peer = None;
    for _ in 0..20 {
        tokio::select! {
            event = recv_with_timeout(&mut listener_events) => {
                if let SrtDriverEvent::Connected { peer_id, .. } = event {
                    listener_peer = Some(peer_id);
                }
            }
            _event = recv_with_timeout(&mut caller_events) => {}
        }
        if listener_peer.is_some() {
            break;
        }
    }

    let listener_peer = listener_peer.expect("listener peer should connect");
    caller_cancel.cancel();

    let event = tokio::time::timeout(Duration::from_secs(1), listener_events.recv())
        .await
        .expect("idle disconnect timeout")
        .expect("event channel open");
    match event {
        SrtDriverEvent::Disconnected { peer_id, reason } => {
            assert_eq!(peer_id, listener_peer);
            assert_eq!(reason, "idle timeout");
        }
        other => panic!("unexpected event: {other:?}"),
    }

    listener_cancel.cancel();
    caller_cancel.cancel();
}

#[tokio::test]
async fn caller_connect_timeout_disconnects() {
    let unused_socket =
        tokio::net::UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .expect("bind unused udp socket");
    let unused_addr = unused_socket.local_addr().expect("unused local addr");
    drop(unused_socket);

    let mut config = driver_config();
    config.connect_timeout_ms = 30;
    config.idle_timeout_ms = 0;
    config.stats_interval_ms = 0;

    let cancel = CancellationToken::new();
    let (handle, mut events) = spawn_driver(config, cancel.clone());
    let _local_addr = match recv_with_timeout(&mut events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first event: {other:?}"),
    };

    let peer_id = SrtPeerId(400);
    handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id,
            remote: unused_addr,
            stream_id: Some("#!::r=live/test,m=publish".to_string()),
            options: caller_options(),
        })
        .await;

    let mut saw_connecting = false;
    for _ in 0..10 {
        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("connect timeout event timeout")
            .expect("event channel open");
        match event {
            SrtDriverEvent::CallerConnecting {
                peer_id: event_peer,
                remote,
            } => {
                assert_eq!(event_peer, peer_id);
                assert_eq!(remote, unused_addr);
                saw_connecting = true;
            }
            SrtDriverEvent::Disconnected {
                peer_id: event_peer,
                reason,
            } => {
                assert!(saw_connecting, "caller should report connecting first");
                assert_eq!(event_peer, peer_id);
                assert_eq!(reason, "connect timeout");
                cancel.cancel();
                return;
            }
            SrtDriverEvent::Stats { .. } => {}
            other => panic!("unexpected event: {other:?}"),
        }
    }

    panic!("caller did not disconnect after connect timeout");
}

#[tokio::test]
async fn caller_command_respects_max_connections() {
    let unused_socket =
        tokio::net::UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .expect("bind unused udp socket");
    let unused_addr = unused_socket.local_addr().expect("unused local addr");
    drop(unused_socket);

    let mut config = driver_config();
    config.max_connections = 1;
    config.connect_timeout_ms = 5_000;
    config.idle_timeout_ms = 0;
    config.stats_interval_ms = 0;

    let cancel = CancellationToken::new();
    let (handle, mut events) = spawn_driver(config, cancel.clone());
    let _local_addr = match recv_with_timeout(&mut events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first event: {other:?}"),
    };

    handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id: SrtPeerId(500),
            remote: unused_addr,
            stream_id: Some("#!::r=live/one,m=publish".to_string()),
            options: caller_options(),
        })
        .await;
    let event = recv_with_timeout(&mut events).await;
    assert!(
        matches!(event, SrtDriverEvent::CallerConnecting { peer_id, .. } if peer_id == SrtPeerId(500)),
        "unexpected first caller event: {event:?}"
    );

    handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id: SrtPeerId(501),
            remote: unused_addr,
            stream_id: Some("#!::r=live/two,m=publish".to_string()),
            options: caller_options(),
        })
        .await;

    let event = recv_with_timeout(&mut events).await;
    match event {
        SrtDriverEvent::Error { peer_id, message } => {
            assert_eq!(peer_id, Some(SrtPeerId(501)));
            assert_eq!(message, "SRT max_connections reached");
        }
        other => panic!("unexpected max connection event: {other:?}"),
    }

    cancel.cancel();
}

#[tokio::test]
async fn remote_disconnect_releases_listener_connection_slot() {
    let listener_cancel = CancellationToken::new();
    let first_caller_cancel = CancellationToken::new();
    let second_caller_cancel = CancellationToken::new();

    let mut listener_config = driver_config();
    listener_config.max_connections = 1;
    listener_config.idle_timeout_ms = 0;
    listener_config.stats_interval_ms = 0;

    let (listener_handle, mut listener_events) =
        spawn_driver(listener_config, listener_cancel.clone());
    let (first_caller_handle, mut first_caller_events) =
        spawn_driver(driver_config(), first_caller_cancel.clone());
    let (second_caller_handle, mut second_caller_events) =
        spawn_driver(driver_config(), second_caller_cancel.clone());

    let listener_addr = match recv_with_timeout(&mut listener_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected listener event: {other:?}"),
    };
    let _first_caller_addr = match recv_with_timeout(&mut first_caller_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first caller event: {other:?}"),
    };
    let _second_caller_addr = match recv_with_timeout(&mut second_caller_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected second caller event: {other:?}"),
    };

    let first_caller_id = SrtPeerId(700);
    first_caller_handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id: first_caller_id,
            remote: listener_addr,
            stream_id: Some("#!::r=live/first,m=publish".to_string()),
            options: caller_options(),
        })
        .await;

    let mut first_caller_connected = false;
    let mut first_listener_peer = None;
    for _ in 0..20 {
        tokio::select! {
            event = recv_with_timeout(&mut first_caller_events) => {
                if let SrtDriverEvent::Connected { peer_id, .. } = event {
                    if peer_id == first_caller_id {
                        first_caller_connected = true;
                    }
                }
            }
            event = recv_with_timeout(&mut listener_events) => {
                if let SrtDriverEvent::Connected { peer_id, .. } = event {
                    first_listener_peer = Some(peer_id);
                }
            }
        }
        if first_caller_connected && first_listener_peer.is_some() {
            break;
        }
    }
    assert!(first_caller_connected, "first caller should connect");
    let first_listener_peer = first_listener_peer.expect("first listener peer should connect");

    first_caller_handle
        .send(SrtDriverCommand::Close {
            peer_id: first_caller_id,
            reason: "test close".to_string(),
        })
        .await;

    for _ in 0..20 {
        if let SrtDriverEvent::Disconnected { peer_id, .. } =
            recv_with_timeout(&mut listener_events).await
        {
            if peer_id == first_listener_peer {
                break;
            }
        }
    }

    let second_caller_id = SrtPeerId(701);
    second_caller_handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id: second_caller_id,
            remote: listener_addr,
            stream_id: Some("#!::r=live/second,m=publish".to_string()),
            options: caller_options(),
        })
        .await;

    let mut second_caller_connected = false;
    let mut second_listener_connected = false;
    for _ in 0..30 {
        tokio::select! {
            event = recv_with_timeout(&mut second_caller_events) => {
                if let SrtDriverEvent::Connected { peer_id, .. } = event {
                    if peer_id == second_caller_id {
                        second_caller_connected = true;
                    }
                }
            }
            event = recv_with_timeout(&mut listener_events) => match event {
                SrtDriverEvent::Connected { .. } => {
                    second_listener_connected = true;
                }
                SrtDriverEvent::Error { message, .. } if message == "SRT max_connections reached" => {
                    panic!("listener kept stale slot after remote disconnect");
                }
                _ => {}
            }
        }
        if second_caller_connected && second_listener_connected {
            listener_cancel.cancel();
            first_caller_cancel.cancel();
            second_caller_cancel.cancel();
            return;
        }
    }

    listener_cancel.cancel();
    first_caller_cancel.cancel();
    second_caller_cancel.cancel();
    let _ = listener_handle;
    panic!("second caller did not connect after first remote disconnected");
}

#[tokio::test]
async fn send_payload_respects_zero_send_queue_capacity() {
    let listener_cancel = CancellationToken::new();
    let caller_cancel = CancellationToken::new();

    let listener_config = driver_config();
    let mut caller_config = driver_config();
    caller_config.send_queue_capacity = 0;
    caller_config.stats_interval_ms = 0;

    let (_listener_handle, mut listener_events) =
        spawn_driver(listener_config, listener_cancel.clone());
    let (caller_handle, mut caller_events) = spawn_driver(caller_config, caller_cancel.clone());

    let listener_addr = match recv_with_timeout(&mut listener_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first event: {other:?}"),
    };
    let _caller_addr = match recv_with_timeout(&mut caller_events).await {
        SrtDriverEvent::ListenerStarted { local_addr } => local_addr,
        other => panic!("unexpected first caller event: {other:?}"),
    };

    let caller_id = SrtPeerId(600);
    caller_handle
        .send(SrtDriverCommand::ConnectCaller {
            peer_id: caller_id,
            remote: listener_addr,
            stream_id: Some("#!::r=live/queue,m=publish".to_string()),
            options: caller_options(),
        })
        .await;

    let mut caller_connected = false;
    let mut listener_connected = false;
    for _ in 0..20 {
        tokio::select! {
            event = recv_with_timeout(&mut caller_events) => {
                if let SrtDriverEvent::Connected { peer_id, .. } = event {
                    if peer_id == caller_id {
                        caller_connected = true;
                    }
                }
            }
            event = recv_with_timeout(&mut listener_events) => {
                if matches!(event, SrtDriverEvent::Connected { .. }) {
                    listener_connected = true;
                }
            }
        }
        if caller_connected && listener_connected {
            break;
        }
    }
    assert!(caller_connected, "caller should connect");
    assert!(listener_connected, "listener should connect");

    caller_handle
        .send(SrtDriverCommand::SendPayload {
            peer_id: caller_id,
            payload: Bytes::from_static(b"blocked"),
        })
        .await;

    for _ in 0..10 {
        let event = recv_with_timeout(&mut caller_events).await;
        if let SrtDriverEvent::Error { peer_id, message } = event {
            assert_eq!(peer_id, Some(caller_id));
            assert_eq!(message, "SRT send queue full");
            listener_cancel.cancel();
            caller_cancel.cancel();
            return;
        }
    }

    panic!("send queue overflow did not emit an error");
}
