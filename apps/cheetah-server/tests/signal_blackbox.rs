//! Native HTTP black-box tests for the full `cheetah-server` binary.
//!
//! These tests spawn the real server process, configure the native HTTP adapter
//! to allow anonymous requests, and exercise the control/media endpoints over
//! TCP. They are intentionally independent of any in-process engine handle.
//!
//! `cheetah-server` 的 native HTTP 黑盒测试。本测试启动真实的服务器进程，
//! 通过 TCP 访问控制/媒体端点，不依赖进程内引擎句柄。

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};

fn server_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cheetah-server"))
}

async fn free_local_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn write_config(control_port: u16, temp_dir: &std::path::Path) -> PathBuf {
    let config_path = temp_dir.join("cheetah.yaml");
    let yaml = format!(
        r#"global:
  control:
    listen: "127.0.0.1:{control_port}"
  media:
    native:
      auth:
        mode: "none"
modules:
  rtmp:
    enabled: false
  webhook-dispatcher:
    profiles: []
"#,
    );
    std::fs::write(&config_path, yaml).unwrap();
    config_path
}

async fn spawn_server(config_path: &std::path::Path) -> Child {
    let mut child = Command::new(server_bin())
        .env("CHEETAH_CONFIG", config_path)
        .env("RUST_LOG", "error")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cheetah-server");

    // Give the process a moment to fail before returning.
    sleep(Duration::from_millis(200)).await;
    if let Ok(Some(status)) = child.try_wait() {
        panic!("cheetah-server exited early: {status}");
    }
    child
}

async fn wait_for_server(port: u16) {
    let deadline = Duration::from_secs(15);
    timeout(deadline, async {
        loop {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                // Wait until the server accepts a full request, not just the socket.
                sleep(Duration::from_millis(200)).await;
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("server did not start in time");
}

async fn http_get(host: &str, port: u16, path: &str) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect((host, port))
        .await
        .unwrap_or_else(|e| panic!("connect to {host}:{port}: {e}"));
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    read_http_response(&mut stream).await
}

async fn read_http_response(stream: &mut TcpStream) -> (u16, Vec<u8>) {
    let mut buffer = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut content_length: Option<usize> = None;
    let mut header_end: Option<usize> = None;
    let mut status: Option<u16> = None;

    loop {
        let n = stream.read(&mut tmp).await.expect("read response");
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&tmp[..n]);

        if header_end.is_none() {
            if let Some(idx) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                header_end = Some(idx + 4);
                let headers = std::str::from_utf8(&buffer[..idx]).unwrap();
                for line in headers.lines() {
                    let lower = line.to_ascii_lowercase();
                    if lower.starts_with("content-length:") {
                        content_length =
                            lower.split(':').nth(1).and_then(|v| v.trim().parse().ok());
                    }
                }
                // First line is "HTTP/1.1 200 OK".
                if let Some(first) = headers.lines().next() {
                    status = first.split(' ').nth(1).and_then(|s| s.parse().ok());
                }
            }
        }

        if let Some(header_end) = header_end {
            let body_len = content_length.unwrap_or(0);
            if buffer.len() >= header_end + body_len {
                break;
            }
        }
    }

    let header_end = header_end.unwrap_or(buffer.len());
    let body = buffer[header_end..].to_vec();
    (status.unwrap_or(0), body)
}

async fn stop_server(mut child: Child) {
    let _ = child.start_kill();
    let _ = timeout(Duration::from_secs(5), child.wait()).await;
}

#[tokio::test(flavor = "current_thread")]
async fn server_exposes_media_capabilities_over_http() {
    let control_port = free_local_port().await;
    let temp_dir = std::env::temp_dir().join(format!("cheetah_blackbox_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let config_path = write_config(control_port, &temp_dir);
    let child = spawn_server(&config_path).await;
    wait_for_server(control_port).await;

    let (status, body) = http_get("127.0.0.1", control_port, "/api/v1/media/capabilities").await;
    assert_eq!(
        status,
        200,
        "capabilities should return 200: {}",
        String::from_utf8_lossy(&body)
    );

    let json: serde_json::Value = serde_json::from_slice(&body).expect("capabilities json");
    assert!(
        json.get("capabilities").is_some(),
        "capabilities field missing: {json}"
    );
    assert!(
        json.get("version").is_some(),
        "version field missing: {json}"
    );

    stop_server(child).await;
}
