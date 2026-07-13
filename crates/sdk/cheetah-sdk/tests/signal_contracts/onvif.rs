use std::collections::HashMap;
use std::sync::Arc;

use cheetah_media_api::command::*;
use cheetah_media_api::ids::*;
use cheetah_media_api::model::*;
use cheetah_sdk::MediaServices;

use crate::support::{ctx, FakeMediaProvider};

/// Simulate an ONVIF project that discovers a media resource, creates a pull proxy,
/// takes a snapshot, starts/stops a recording, and requests a keyframe.
///
/// 模拟一个 ONVIF 项目：发现媒体资源、创建拉流代理、抓拍、启动/停止录制、请求关键帧。
async fn onvif_flow(services: MediaServices) {
    let ctx = ctx();
    let key = MediaKey::with_default_vhost("onvif", "profile_1", None).expect("valid ONVIF key");

    let info = services
        .control()
        .expect("control provider available")
        .get_media(&ctx, &key)
        .await
        .expect("get media info");
    assert_eq!(info.key, key);

    let proxy = services
        .proxy()
        .expect("proxy provider available")
        .create_pull_proxy(
            &ctx,
            PullProxyRequest {
                source_url: "rtsp://192.0.2.2/stream1".to_string(),
                destination: key.clone(),
                retry_policy: RetryPolicy::default(),
                heartbeat_ms: Some(10_000),
                timeout_ms: 30_000,
                transcode_policy: TranscodePolicy::default(),
                output_policy: OutputPolicy::default(),
                record_policy: None,
            },
        )
        .await
        .expect("create pull proxy");
    assert_eq!(proxy.kind, ProxyKind::Pull);
    assert_eq!(proxy.destination, key);

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
    assert_eq!(snapshot.state, SnapshotState::Completed);

    services
        .control()
        .expect("control provider available")
        .request_keyframe(&ctx, &key)
        .await
        .expect("request keyframe");

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

    services
        .proxy()
        .expect("proxy provider available")
        .delete_pull_proxy(&ctx, &proxy.proxy_id)
        .await
        .expect("delete pull proxy");

    services
        .control()
        .expect("control provider available")
        .kick_stream(&ctx, &key, CloseReason::Normal)
        .await
        .expect("close stream");
}

#[tokio::test]
async fn onvif_can_complete_media_call_flow() {
    let services = MediaServices::unavailable();
    let provider = Arc::new(FakeMediaProvider::new());
    services.register_control(provider.clone());
    services.register_proxy(provider.clone());
    services.register_snapshot(provider.clone());
    services.register_record(provider);

    onvif_flow(services).await;
}
