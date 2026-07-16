//! Apple HomeKit production contract tests.
//!
//! These tests run against a real Engine and verify subscribe, keyframe request,
//! and snapshot flows with real providers.
//!
//! 本测试针对真实 Engine，验证订阅、关键帧请求与快照的真实 provider 流程。

use cheetah_codec::CodecId;
use cheetah_media_api::command::SnapshotRequest;
use cheetah_media_api::model::{OnlineState, SnapshotState};
use cheetah_media_api::{MediaControlApi, SnapshotApi};
use cheetah_sdk::SubscriberOptions;
use std::time::Duration;
use tokio::time::timeout;

use crate::production_support::{
    ctx, golden_key, golden_stream_key, media_facade, production_engine,
};

#[tokio::test(flavor = "current_thread")]
async fn homekit_can_subscribe_and_snapshot() {
    let engine = production_engine().await;
    let facade = media_facade(&engine);

    let online = facade
        .is_media_online(&ctx(), &golden_key())
        .await
        .expect("is_media_online");
    assert_eq!(online, OnlineState::Online);

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
    assert_eq!(frame.codec, CodecId::MJPEG);
    assert!(!frame.payload.is_empty());

    facade
        .request_keyframe(&ctx(), &golden_key())
        .await
        .expect("request_keyframe");

    let snap = facade
        .take_snapshot(
            &ctx(),
            SnapshotRequest {
                media_key: golden_key(),
                timeout_ms: 5000,
                format: "jpg".to_string(),
                quality: None,
                max_width: None,
                max_height: None,
                storage_policy: Default::default(),
                capture_policy: Default::default(),
            },
        )
        .await
        .expect("take_snapshot");
    assert_eq!(snap.state, SnapshotState::Completed);

    sub.close().await.expect("close subscriber");
}
