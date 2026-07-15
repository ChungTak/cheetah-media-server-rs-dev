use cheetah_runtime_api::RuntimeApi;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_webhook_dispatcher::security::{ParsedUrl, WebhookUrlVerdict};
use cheetah_webhook_dispatcher::sender::{RuntimeHttpClient, WebhookHttpRequest, WebhookSender};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn runtime_http_client_posts_and_reads_response() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        let received = String::from_utf8_lossy(&buf[..n]);
        assert!(received.contains("POST /hook HTTP/1.1"));
        assert!(received.contains("Content-Type: application/json"));
        assert!(received.contains("{\"hello\":\"world\"}"));
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
        stream.write_all(response).await.unwrap();
        stream.flush().await.unwrap();
    });

    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let client = RuntimeHttpClient::new(runtime_api);

    let verdict = WebhookUrlVerdict::Allow(
        addr,
        ParsedUrl {
            scheme: "http".to_string(),
            host: addr.ip().to_string(),
            port: addr.port(),
            path_and_query: "/hook".to_string(),
        },
    );

    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    let request = WebhookHttpRequest {
        verdict,
        headers,
        body: br#"{"hello":"world"}"#.to_vec(),
        timeout: Duration::from_secs(5),
    };

    let response = client.send(request).await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "ok");
}

#[tokio::test]
async fn runtime_http_client_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf).await.unwrap();
        // Never respond.
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
    let client = RuntimeHttpClient::new(runtime_api);

    let verdict = WebhookUrlVerdict::Allow(
        addr,
        ParsedUrl {
            scheme: "http".to_string(),
            host: addr.ip().to_string(),
            port: addr.port(),
            path_and_query: "/slow".to_string(),
        },
    );

    let request = WebhookHttpRequest {
        verdict,
        headers: HashMap::new(),
        body: b"{}".to_vec(),
        timeout: Duration::from_millis(50),
    };

    let err = client.send(request).await.unwrap_err();
    assert!(err.to_string().contains("timeout"));
}
