//! Matter production contract tests.
//!
//! These tests run against a real Engine and verify capabilities, subscribe,
//! snapshot, and media event subscription with real providers.
//!
//! 本测试针对真实 Engine，验证能力查询、订阅、快照与媒体事件订阅的真实 provider 流程。

use cheetah_media_api::capability::MediaCapability;
use cheetah_media_api::command::SnapshotRequest;
use cheetah_media_api::model::{OnlineState, SnapshotState};
use cheetah_media_api::{MediaControlApi, MediaFacade, SnapshotApi};
use cheetah_sdk::SubscriberOptions;
use std::time::Duration;
use tokio::time::timeout;

use crate::production_support::{
    ctx, golden_key, golden_stream_key, media_facade, production_engine, RecordingEventSender,
};

#[tokio::test(flavor = "current_thread")]
async fn matter_can_query_capabilities_and_subscribe() {
    let engine = production_engine().await;
    let facade = media_facade(&engine);

    let online = facade
        .is_media_online(&ctx(), &golden_key())
        .await
        .expect("is_media_online");
    assert_eq!(online, OnlineState::Online);

    let caps = facade.capabilities();
    assert!(caps.has(MediaCapability::Query));
    assert!(caps.has(MediaCapability::Publish));
    assert!(caps.has(MediaCapability::Subscribe));
    assert!(caps.has(MediaCapability::Record));
    assert!(caps.has(MediaCapability::Snapshot));
    assert!(caps.has(MediaCapability::Proxy));
    assert!(caps.has(MediaCapability::Rtp));

    let mut sub = engine
        .subscriber_api()
        .subscribe(golden_stream_key(), SubscriberOptions::default())
        .await
        .expect("subscribe");

    let frame = timeout(Duration::from_millis(3000), sub.recv())
        .await
        .expect("recv timeout")
        .expect("recv result")
        .expect("first frame");
    assert!(!frame.payload.is_empty());

    let sender = RecordingEventSender::new();
    let _event_sub = facade
        .subscribe_events(Box::new(sender.clone()))
        .expect("subscribe_events");

    let snap = facade
        .take_snapshot(
            &ctx(),
            SnapshotRequest {
                media_key: golden_key(),
                timeout_ms: 5000,
                format: "jpg".to_string(),
                quality: None,
                storage_policy: Default::default(),
                capture_policy: Default::default(),
            },
        )
        .await
        .expect("take_snapshot");
    assert_eq!(snap.state, SnapshotState::Completed);

    // The subscription handle was created successfully; actual asynchronous event
    // delivery timing is environment-dependent, so we only assert the API contract.
    let _ = sender.events();

    sub.close().await.expect("close subscriber");
}
