//! Cheetah ↔ Janus ignored end-to-end test.
//!
//! Phase 06 (`plans-27-webrtc-zlm2/phase-06-external-interop-infra.md`).
//!
//! Drives Janus's REST API through a 3-step `create / attach /
//! message` echotest handshake and verifies the responses are
//! shape-compliant. Mirrors `cheetah_to_pion_interop.rs` and
//! `cheetah_to_zlm_interop.rs` so the harness contract stays
//! consistent across all three external-peer end-to-end tests.
//!
//! The test runs entirely against Janus's HTTP API — no SDP
//! exchange yet — because cheetah's WebRTC pipeline doesn't need
//! Janus for its primary feature set. The point is to keep the
//! Janus path covered by nightly CI so a refactor that breaks the
//! integration surface fires loudly.
//!
//! Run locally:
//!
//! ```bash
//! docker compose -f dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml \
//!   --profile janus up -d
//! export WEBRTC_INTEROP_JANUS_URL=http://127.0.0.1:8088/janus
//! cargo test -p cheetah-webrtc-module --test cheetah_to_janus_interop \
//!   -- --ignored cheetah_drives_janus_echotest
//! ```

mod interop_harness;

use std::time::Duration;

use interop_harness::{open_test, require_env, ENV_JANUS};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Tiny HTTP/1.1 POST client matching the one in
/// `cheetah_to_zlm_interop.rs`. Kept duplicated rather than shared
/// because the harness file lives in the test target tree, not in
/// the crate library — sharing would require a separate `tests/`
/// crate, which is heavier than 60 lines of duplication.
async fn http_post_json(url: &str, body: &str, timeout: Duration) -> Result<(u16, String), String> {
    let url = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("only http:// supported by the harness client, got {url:?}"))?;
    let (host_port, path_query) = match url.find('/') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url, "/"),
    };
    let (host, port) = match host_port.rfind(':') {
        Some(i) => {
            let port: u16 = host_port[i + 1..]
                .parse()
                .map_err(|_| format!("bad port in {host_port}"))?;
            (&host_port[..i], port)
        }
        None => (host_port, 80),
    };
    let request = format!(
        "POST {path_query} HTTP/1.1\r\n\
         Host: {host_port}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n{body}",
        len = body.len()
    );
    let stream = tokio::time::timeout(timeout, TcpStream::connect((host.to_string(), port)))
        .await
        .map_err(|_| format!("connect to {host}:{port} timed out"))?
        .map_err(|err| format!("connect to {host}:{port} failed: {err}"))?;

    let (mut read_half, mut write_half) = stream.into_split();
    write_half
        .write_all(request.as_bytes())
        .await
        .map_err(|err| format!("write request: {err}"))?;
    write_half
        .shutdown()
        .await
        .map_err(|err| format!("shutdown write half: {err}"))?;

    let mut buf = Vec::with_capacity(8192);
    tokio::time::timeout(timeout, read_half.read_to_end(&mut buf))
        .await
        .map_err(|_| "response read timed out".to_string())?
        .map_err(|err| format!("read response: {err}"))?;

    let response = String::from_utf8_lossy(&buf).to_string();
    let mut parts = response.splitn(2, "\r\n\r\n");
    let header = parts.next().unwrap_or("");
    let body = parts.next().unwrap_or("");
    let status_line = header.lines().next().unwrap_or("");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line: {status_line:?}"))?;
    Ok((status, body.to_string()))
}

fn fresh_txn() -> String {
    // Janus requires a transaction id for correlation. Use a tiny
    // counter-based id so the harness doesn't pull `rand` for one
    // test.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("cheetah-tx-{n}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires running Janus peer; set WEBRTC_INTEROP_JANUS_URL to run"]
async fn cheetah_drives_janus_echotest() {
    let Some(artifact) = open_test("cheetah_drives_janus_echotest", Some(ENV_JANUS)) else {
        return;
    };
    let url = require_env(ENV_JANUS).unwrap();
    let timeout = interop_harness::timeout();

    // Step 1: create session.
    let create_body = format!(r#"{{"janus":"create","transaction":"{}"}}"#, fresh_txn());
    let (create_status, create_body_resp) = match http_post_json(&url, &create_body, timeout).await
    {
        Ok(r) => r,
        Err(err) => {
            artifact.set_failure(format!("janus create transport failed: {err}"));
            panic!("{err}");
        }
    };
    artifact
        .write("step1-create.json", create_body_resp.as_bytes())
        .expect("write step1");
    if !(200..300).contains(&create_status) {
        artifact.set_failure(format!("janus create non-2xx: {create_status}"));
        panic!("janus create non-2xx");
    }
    let create_json: Value = match serde_json::from_str(&create_body_resp) {
        Ok(v) => v,
        Err(err) => {
            artifact.set_failure(format!("janus create body not JSON: {err}"));
            panic!("{err}");
        }
    };
    let session_id = match create_json
        .get("data")
        .and_then(|d| d.get("id"))
        .and_then(|v| v.as_u64())
    {
        Some(id) => id,
        None => {
            artifact.set_failure(format!(
                "janus create response missing data.id: {create_body_resp}"
            ));
            panic!("missing session id");
        }
    };

    // Step 2: attach echotest plugin.
    let attach_url = format!("{url}/{session_id}");
    let attach_body = format!(
        r#"{{"janus":"attach","plugin":"janus.plugin.echotest","transaction":"{}"}}"#,
        fresh_txn()
    );
    let (attach_status, attach_body_resp) =
        match http_post_json(&attach_url, &attach_body, timeout).await {
            Ok(r) => r,
            Err(err) => {
                artifact.set_failure(format!("janus attach transport failed: {err}"));
                panic!("{err}");
            }
        };
    artifact
        .write("step2-attach.json", attach_body_resp.as_bytes())
        .expect("write step2");
    if !(200..300).contains(&attach_status) {
        artifact.set_failure(format!("janus attach non-2xx: {attach_status}"));
        panic!("janus attach non-2xx");
    }
    let attach_json: Value = serde_json::from_str(&attach_body_resp)
        .map_err(|err| format!("attach body not JSON: {err}"))
        .unwrap_or_else(|err| {
            artifact.set_failure(err.clone());
            panic!("{err}");
        });
    let handle_id = attach_json
        .get("data")
        .and_then(|d| d.get("id"))
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| {
            artifact.set_failure(format!(
                "janus attach response missing data.id: {attach_body_resp}"
            ));
            panic!("missing handle id");
        });

    // Step 3: send echotest noop message.
    let msg_url = format!("{url}/{session_id}/{handle_id}");
    let msg_body = format!(
        r#"{{"janus":"message","body":{{"video":true,"audio":true,"bitrate":64000}},"transaction":"{}"}}"#,
        fresh_txn()
    );
    let (msg_status, msg_body_resp) = match http_post_json(&msg_url, &msg_body, timeout).await {
        Ok(r) => r,
        Err(err) => {
            artifact.set_failure(format!("janus message transport failed: {err}"));
            panic!("{err}");
        }
    };
    artifact
        .write("step3-message.json", msg_body_resp.as_bytes())
        .expect("write step3");
    if !(200..300).contains(&msg_status) {
        artifact.set_failure(format!("janus message non-2xx: {msg_status}"));
        panic!("janus message non-2xx");
    }
    // Janus message response is `{"janus":"ack"}` for async plugins
    // or `{"janus":"event", ...}` for sync ones. Accept either.
    let msg_json: Value = serde_json::from_str(&msg_body_resp)
        .map_err(|err| format!("message body not JSON: {err}"))
        .unwrap_or_else(|err| {
            artifact.set_failure(err.clone());
            panic!("{err}");
        });
    let janus_kind = msg_json.get("janus").and_then(|v| v.as_str()).unwrap_or("");
    if janus_kind != "ack" && janus_kind != "event" && janus_kind != "success" {
        artifact.set_failure(format!(
            "unexpected janus message kind {janus_kind:?} (expected ack/event/success): {msg_body_resp}"
        ));
        panic!("unexpected message kind");
    }

    artifact
        .append(
            "module-events.log",
            &format!(
                "janus echotest OK url={url} session={session_id} handle={handle_id} \
                 msg_kind={janus_kind}"
            ),
        )
        .expect("append log");
}
