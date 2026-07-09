//! Cheetah → ZLM ignored end-to-end interop test.
//!
//! Phase 06 (`plans-27-webrtc-zlm2/phase-06-external-interop-infra.md`).
//!
//! This test drives the full assertion-helper pipeline against a
//! running ZLMediaKit instance:
//!
//! 1. Generate a WHIP offer in-process via the cheetah `WebRtcModule`.
//! 2. Read cheetah's locally-produced answer (proves cheetah's WHIP
//!    pipeline is healthy without leaving the harness).
//! 3. POST the same offer SDP to a ZLM WHIP endpoint via raw HTTP.
//! 4. Run the `assert_answer_well_formed` helper against ZLM's answer.
//! 5. Drop both SDPs into the artifact directory for triage.
//!
//! The test is `#[ignore]` because it requires a running ZLM
//! instance reachable from `WEBRTC_INTEROP_ZLM_WHIP_URL`. It runs
//! green in the nightly lab and surfaces real wire-shape regressions
//! between cheetah and ZLM.
//!
//! Run locally:
//!
//! ```bash
//! export WEBRTC_INTEROP_ZLM_WHIP_URL='http://127.0.0.1/index/api/webrtc?app=live&stream=sample&type=push'
//! cargo test -p cheetah-webrtc-module --test cheetah_to_zlm_interop \
//!   -- --ignored cheetah_offer_to_zlm_whip
//! ```

mod interop_harness;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{HttpHeader, HttpMethod, HttpRequest};
use cheetah_webrtc_module::WebRtcModuleFactory;
use interop_harness::assertions::assert_answer_well_formed;
use interop_harness::{open_test, require_env, ENV_ZLM_WHIP};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn fixture_offer() -> String {
    include_str!("fixtures/minimal_offer.sdp").to_string()
}

/// Tiny HTTP/1.1 POST client. Avoids pulling reqwest into the dev
/// graph for a single ignored test — the harness already keeps its
/// dependencies minimal. Returns `(status_code, body)`.
async fn http_post_sdp(url: &str, body: &str, timeout: Duration) -> Result<(u16, String), String> {
    // url = "http://host[:port]/path?query"
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
         Content-Type: application/sdp\r\n\
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
    // `HTTP/1.1 201 Created`
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line: {status_line:?}"))?;
    Ok((status, body.to_string()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires running ZLMediaKit peer; set WEBRTC_INTEROP_ZLM_WHIP_URL to run"]
async fn cheetah_offer_to_zlm_whip() {
    let Some(artifact) = open_test("cheetah_offer_to_zlm_whip", Some(ENV_ZLM_WHIP)) else {
        return;
    };
    let url = require_env(ENV_ZLM_WHIP).unwrap();

    // Step 1: spawn cheetah WebRtcModule in-process and grab its
    // HTTP service.
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");
    let mounts = engine.module_manager_api().http_mounts();
    let svc = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount")
        .service
        .clone();

    let offer = fixture_offer();
    artifact
        .write("request-offer.sdp", offer.as_bytes())
        .expect("write offer");

    // Step 2: cheetah-side WHIP confirms the offer is valid.
    let local_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("app=live&stream=ckhk".into()),
            headers: vec![HttpHeader {
                name: "Content-Type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::copy_from_slice(offer.as_bytes()),
        })
        .await
        .expect("local whip handler");
    if local_resp.status != 201 {
        artifact.set_failure(format!(
            "cheetah local WHIP responded {} instead of 201",
            local_resp.status
        ));
        engine.stop().await;
        panic!("cheetah local WHIP failed");
    }
    artifact
        .write(
            "cheetah-answer.sdp",
            String::from_utf8_lossy(&local_resp.body).as_bytes(),
        )
        .expect("write cheetah answer");

    // Step 3: forward the same offer to ZLM.
    let timeout = interop_harness::timeout();
    match http_post_sdp(&url, &offer, timeout).await {
        Ok((status, answer)) => {
            artifact
                .write("zlm-answer.sdp", answer.as_bytes())
                .expect("write zlm answer");
            if !(200..300).contains(&status) {
                artifact.set_failure(format!(
                    "ZLM WHIP responded {status} (expected 2xx); see zlm-answer.sdp"
                ));
                engine.stop().await;
                panic!("ZLM WHIP non-2xx: {status}");
            }
            // Step 4: assertion helpers against ZLM's answer.
            if let Err(err) = assert_answer_well_formed(&answer) {
                artifact.set_failure(format!("ZLM answer not well-formed: {err}"));
                engine.stop().await;
                panic!("{err}");
            }
            artifact
                .append(
                    "module-events.log",
                    &format!("zlm whip OK status={status} url={url}"),
                )
                .expect("append log");
        }
        Err(err) => {
            artifact.set_failure(format!("ZLM WHIP transport failed: {err}"));
            engine.stop().await;
            panic!("transport: {err}");
        }
    }

    engine.stop().await;
}
