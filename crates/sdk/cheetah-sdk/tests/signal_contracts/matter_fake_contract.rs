use std::collections::HashMap;
use std::sync::Arc;

use cheetah_media_api::command::*;
use cheetah_media_api::ids::*;
use cheetah_media_api::model::*;
use cheetah_media_api::MediaFacade;
use cheetah_sdk::MediaServices;

use crate::fake_support::{ctx, FakeEventSender, FakeMediaProvider};

/// Simulate a Matter project that queries capabilities, creates/destroys a playback
/// subscription, takes a snapshot, starts a recording, and subscribes to events.
///
/// 模拟一个 Matter 项目：查询能力、创建/销毁播放订阅、抓拍、启动录制、订阅事件。
async fn matter_flow(services: MediaServices, facade: Arc<dyn MediaFacade>) {
    let ctx = ctx();
    let key = MediaKey::with_default_vhost("matter", "endpoint_1", Some(MediaSchema::Hls))
        .expect("valid Matter key");

    let caps = facade.capabilities();
    assert!(caps.has(cheetah_media_api::MediaCapability::Subscribe));
    assert!(caps.has(cheetah_media_api::MediaCapability::Snapshot));
    assert!(caps.has(cheetah_media_api::MediaCapability::Record));

    facade
        .subscribe_events(Box::new(FakeEventSender))
        .expect("subscribe events");

    let sub = services
        .publish_subscribe()
        .expect("publish/subscribe provider available")
        .open_subscriber(
            &ctx,
            SubscribeRequest {
                media_key: key.clone(),
                output_schema: MediaSchema::Hls,
                subscriber_kind: "matter".to_string(),
                start_policy: "immediate".to_string(),
                auth_context: HashMap::new(),
            },
        )
        .await
        .expect("open subscriber");
    assert_eq!(sub.media_key, key);

    let snapshot = services
        .snapshot()
        .expect("snapshot provider available")
        .take_snapshot(
            &ctx,
            SnapshotRequest {
                media_key: key.clone(),
                timeout_ms: 10_000,
                format: "jpg".to_string(),
                quality: None,
                storage_policy: StoragePolicy::default(),
                capture_policy: HashMap::new(),
            },
        )
        .await
        .expect("take snapshot");
    assert_eq!(snapshot.media_key, key);

    let record = services
        .record()
        .expect("record provider available")
        .start_record(
            &ctx,
            StartRecordRequest {
                media_key: key.clone(),
                format: "mp4".to_string(),
                template: RecordTemplate::default(),
                segment_duration_ms: None,
                max_segments: None,
                storage_policy: StoragePolicy::default(),
                idempotency_key: None,
            },
        )
        .await
        .expect("start record");
    assert_eq!(record.media_key, key);

    services
        .publish_subscribe()
        .expect("publish/subscribe provider available")
        .close_handle(&ctx, &sub.session_id, CloseReason::Normal)
        .await
        .expect("close subscriber");

    services
        .record()
        .expect("record provider available")
        .stop_record(
            &ctx,
            StopRecordRequest {
                task_id: record.task_id,
            },
        )
        .await
        .expect("stop record");
}

#[tokio::test]
async fn matter_can_complete_media_call_flow() {
    let services = MediaServices::unavailable();
    let provider = Arc::new(FakeMediaProvider::new());
    services.register_publish_subscribe(provider.clone());
    services.register_snapshot(provider.clone());
    services.register_record(provider.clone());

    matter_flow(services, provider).await;
}
