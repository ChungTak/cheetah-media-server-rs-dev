//! Self-loopback interop tests.
//!
//! Phase 06 (`plans-27-webrtc-zlm2/phase-06-external-interop-infra.md`):
//! the ignored interop suite needs an external peer (Pion / ZLM /
//! Janus / browser). Until the docker-compose lab is wired all the
//! way in, we still want a fast, deterministic test that exercises
//! the assertion helpers against a *real* cheetah-generated SDP.
//!
//! These tests:
//!
//! 1. Start a `WebRtcModule` in-process the same way
//!    `module_lifecycle.rs` does.
//! 2. POST a WHIP offer through the registered `HttpService`.
//! 3. Run the harness `assertion` helpers against the answer SDP
//!    cheetah produced.
//!
//! They are not `#[ignore]` because everything runs in-process; they
//! complement `zlm_sdp_fixtures.rs` (which tests against static ZLM
//! samples) by validating cheetah's *own* SDP shape stays compliant
//! with the harness contract.

mod interop_harness;

use std::sync::Arc;

use bytes::Bytes;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{HttpHeader, HttpMethod, HttpRequest};
use cheetah_webrtc_module::WebRtcModuleFactory;
use interop_harness::assertions::{
    assert_answer_well_formed, assert_offer_well_formed, count_candidates,
};

fn fixture_offer() -> String {
    include_str!("fixtures/minimal_offer.sdp").to_string()
}

async fn build_engine() -> cheetah_engine::Engine {
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
    engine
}

async fn whip_publish(svc: &Arc<dyn cheetah_sdk::ModuleHttpService>, offer: &str) -> (u16, String) {
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("app=live&stream=loopback".into()),
            headers: vec![HttpHeader {
                name: "Content-Type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::copy_from_slice(offer.as_bytes()),
        })
        .await
        .expect("whip handler");
    let body = String::from_utf8_lossy(resp.body.as_ref()).to_string();
    (resp.status, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cheetah_self_whip_answer_passes_assertion_helpers() {
    let engine = build_engine().await;
    let mounts = engine.module_manager_api().http_mounts();
    let svc = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount")
        .service
        .clone();

    let offer = fixture_offer();
    // Sanity: the input fixture itself is well-formed (regression
    // guard for `tests/fixtures/minimal_offer.sdp`).
    assert_offer_well_formed(&offer).expect("input offer well-formed");

    let (status, answer) = whip_publish(&svc, &offer).await;
    assert_eq!(status, 201, "WHIP must return 201 Created with answer SDP");

    // The whole point of this test: cheetah's own answer must
    // satisfy the harness `assert_answer_well_formed` helper.
    assert_answer_well_formed(&answer).expect("cheetah answer well-formed");

    // Counted candidates from cheetah's answer can legitimately be
    // zero on the initial 201 response — cheetah trickles via the
    // WHIP PATCH endpoint after the answer. The point of the count
    // here is to verify the harness parser doesn't blow up on
    // candidate-free SDPs and reports zero relay candidates (we
    // never advertise relays without a TURN config).
    let counts = count_candidates(&answer);
    assert_eq!(
        counts.relay, 0,
        "cheetah WHIP answer must not advertise relay without TURN, saw {counts:?}"
    );

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cheetah_self_whip_answer_carries_required_attributes() {
    // Belt-and-suspenders: confirm cheetah's answers include the
    // attributes the harness expects (BUNDLE, fingerprint, mid).
    // If str0m's defaults change, this test fires before any of
    // the ignored interop tests would.
    let engine = build_engine().await;
    let mounts = engine.module_manager_api().http_mounts();
    let svc = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount")
        .service
        .clone();

    let (status, answer) = whip_publish(&svc, &fixture_offer()).await;
    assert_eq!(status, 201);
    assert!(answer.contains("v=0"), "answer must start with `v=0`");
    assert!(
        answer.contains("a=group:BUNDLE"),
        "answer must include BUNDLE"
    );
    assert!(
        answer.contains("a=fingerprint:"),
        "answer must include DTLS fingerprint"
    );
    assert!(
        answer.contains("a=mid:"),
        "answer must include at least one m=section mid"
    );
    assert!(
        answer.contains("a=ice-ufrag:") && answer.contains("a=ice-pwd:"),
        "answer must include ICE credentials"
    );

    engine.stop().await;
}
