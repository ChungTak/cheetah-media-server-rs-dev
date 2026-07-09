use std::sync::Arc;
use std::time::Duration;

use cheetah_rtmp_core::RtmpUrl;
use cheetah_rtmp_driver_tokio::tls::{RtmpTlsClientConfig, RtmpTlsConfig};
use cheetah_rtmp_driver_tokio::{
    start_tls_client, start_tls_server, ClientDriverEvent, DriverConfig, DriverEvent,
    RtmpClientDriverConfig, RtmpClientMode,
};
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use cheetah_runtime_tokio::TokioRuntime;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::time::timeout;

fn generate_self_signed_cert() -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("generate self-signed cert");
    let cert_der = CertificateDer::from(cert.cert.der().to_vec());
    let key_der = PrivateKeyDer::Pkcs8(cert.key_pair.serialize_der().into());
    (vec![cert_der], key_der)
}

#[tokio::test]
async fn rtmps_server_accepts_tls_handshake_and_rtmp_connection() {
    let (certs, key) = generate_self_signed_cert();

    let tls_config = RtmpTlsConfig::from_der(certs.clone(), key).expect("build tls config");

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let cancel = CancellationToken::new();

    let mut server = start_tls_server(
        runtime_api.clone(),
        listen,
        DriverConfig::default(),
        tls_config,
        Duration::from_secs(5),
        cancel.clone(),
    )
    .expect("start tls server");

    // Connect a TLS client
    let client_tls = RtmpTlsClientConfig::insecure_no_verify();
    let url = RtmpUrl::parse(&format!("rtmps://localhost:{}/live/test", listen.port()))
        .expect("parse url");

    let mut client = start_tls_client(
        runtime_api.clone(),
        url,
        RtmpClientMode::Play,
        RtmpClientDriverConfig::default(),
        client_tls,
        cancel.child_token(),
    )
    .expect("start tls client");

    // Server should see a connection open
    let conn_id = timeout(Duration::from_secs(3), async {
        loop {
            match server.recv_event().await {
                Some(DriverEvent::ConnectionOpened { connection_id, .. }) => {
                    break connection_id;
                }
                Some(_) => continue,
                None => panic!("server event channel closed"),
            }
        }
    })
    .await
    .expect("timeout waiting for connection open");

    assert!(conn_id > 0);

    // Client should receive Connected event (TLS handshake + TCP succeeded)
    let connected = timeout(Duration::from_secs(3), async {
        loop {
            match client.recv_event().await {
                Some(ClientDriverEvent::Connected { .. }) => break true,
                Some(ClientDriverEvent::Closed { reason }) => {
                    panic!("client closed unexpectedly: {reason}")
                }
                Some(_) => continue,
                None => break false,
            }
        }
    })
    .await
    .expect("timeout waiting for client connected");

    assert!(connected);

    // Cleanup
    cancel.cancel();
    let _ = server.wait().await;
}

#[tokio::test]
async fn rtmps_server_rejects_plain_tcp_connection() {
    let (certs, key) = generate_self_signed_cert();
    let tls_config = RtmpTlsConfig::from_der(certs, key).expect("build tls config");

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let cancel = CancellationToken::new();

    let mut server = start_tls_server(
        runtime_api.clone(),
        listen,
        DriverConfig::default(),
        tls_config,
        Duration::from_secs(2),
        cancel.clone(),
    )
    .expect("start tls server");

    // Connect with plain TCP (no TLS) - should fail TLS handshake
    let mut tcp = tokio::net::TcpStream::connect(listen)
        .await
        .expect("tcp connect");

    // Send garbage (RTMP C0C1 without TLS) - server should reject
    use tokio::io::AsyncWriteExt;
    let c0c1 = vec![3u8; 1537];
    let _ = tcp.write_all(&c0c1).await;

    // Server should NOT emit ConnectionOpened for a failed TLS handshake
    let result = timeout(Duration::from_secs(3), server.recv_event()).await;
    // Either timeout (no event) or no ConnectionOpened
    match result {
        Err(_) => {} // timeout = correct, no connection opened
        Ok(Some(DriverEvent::ConnectionOpened { .. })) => {
            panic!("plain TCP should not produce ConnectionOpened on TLS server")
        }
        Ok(_) => {} // other events are fine
    }

    cancel.cancel();
    let _ = server.wait().await;
}
